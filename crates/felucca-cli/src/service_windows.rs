//! Safe SCM wrapper. Only the upstream generated FFI entry point permits unsafe.
use super::{Action, Definition, MARKER, Platform};
use crate::messages::say;
use anyhow::{Context, ensure};
use std::{
    ffi::{OsStr, OsString},
    sync::OnceLock,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use windows_service::{
    service::{
        Service, ServiceAccess, ServiceAction, ServiceActionType, ServiceControl,
        ServiceControlAccept, ServiceErrorControl, ServiceExitCode, ServiceFailureActions,
        ServiceFailureResetPeriod, ServiceInfo, ServiceStartType, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult, ServiceStatusHandle},
    service_manager::{ServiceManager, ServiceManagerAccess},
};
static RUNNER: OnceLock<fn() -> anyhow::Result<()>> = OnceLock::new();
static NAME: OnceLock<String> = OnceLock::new();
static STOP: OnceLock<CancellationToken> = OnceLock::new();
static STATUS: OnceLock<ServiceStatusHandle> = OnceLock::new();
static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
// windows-service owns argument decoding at the SCM ABI boundary. No handwritten
// raw-pointer operations are used by felucca.
#[allow(unsafe_code)]
mod ffi {
    use super::service_main;
    windows_service::define_windows_service!(entry, service_main);
    pub(super) fn dispatch(name: &str) -> windows_service::Result<()> {
        windows_service::service_dispatcher::start(name, entry)
    }
}
fn value(flag: &str) -> anyhow::Result<Option<OsString>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let mut result = None;
    for (i, arg) in args.iter().enumerate() {
        let candidate = if arg == flag {
            Some(
                args.get(i + 1)
                    .context(say("missing service option value"))?
                    .clone(),
            )
        } else {
            arg.to_str()
                .and_then(|s| s.strip_prefix(&format!("{flag}=")))
                .map(OsString::from)
        };
        if let Some(candidate) = candidate {
            ensure!(result.is_none(), "duplicate service option");
            result = Some(candidate);
        }
    }
    Ok(result)
}
pub(super) fn entrypoint(runner: fn() -> anyhow::Result<()>) -> anyhow::Result<()> {
    let Some(name) = value("--service-name")? else {
        return super::finish(runner);
    };
    let name = name
        .into_string()
        .map_err(|_| anyhow::anyhow!("invalid service name"))?;
    ensure!(super::valid_name(&name), "invalid service name");
    let workdir = value("--service-workdir")?.context(say("SCM requires --service-workdir"))?;
    std::env::set_current_dir(workdir).context(say("cannot enter service working directory"))?;
    NAME.set(name.clone())
        .map_err(|_| anyhow::anyhow!("service already initialized"))?;
    RUNNER
        .set(runner)
        .map_err(|_| anyhow::anyhow!("runner already initialized"))?;
    STOP.get_or_init(CancellationToken::new);
    ffi::dispatch(&name).context(say("cannot connect to Windows SCM (run service start)"))?;
    match RESULT.get() {
        Some(Ok(())) => Ok(()),
        Some(Err(error)) => Err(anyhow::anyhow!(error.clone())),
        None => Err(anyhow::anyhow!("SCM did not invoke the service runner")),
    }
}
/// Without failure actions the SCM leaves a crashed or failed service stopped
/// until the next boot. Restart after 5, 10, then every 30 seconds; the count
/// resets after a day without failures. A non-zero exit, such as a start before
/// the network is up, counts as a failure as well as a crash does.
fn restart_on_failure(service: &Service) -> windows_service::Result<()> {
    let restart = |seconds| ServiceAction {
        action_type: ServiceActionType::Restart,
        delay: Duration::from_secs(seconds),
    };
    service.update_failure_actions(ServiceFailureActions {
        reset_period: ServiceFailureResetPeriod::After(Duration::from_secs(24 * 60 * 60)),
        reboot_msg: None,
        command: None,
        actions: Some(vec![restart(5), restart(10), restart(30)]),
    })?;
    service.set_failure_actions_on_non_crash_failures(true)
}

fn status(state: ServiceState, exit: ServiceExitCode) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: exit,
        checkpoint: if matches!(
            state,
            ServiceState::StartPending | ServiceState::StopPending
        ) {
            1
        } else {
            0
        },
        wait_hint: if state == ServiceState::StartPending {
            Duration::from_secs(60)
        } else if state == ServiceState::StopPending {
            Duration::from_secs(30)
        } else {
            Duration::ZERO
        },
        process_id: None,
    }
}
fn service_main(_: Vec<OsString>) {
    let result = (|| -> anyhow::Result<()> {
        let handle = service_control_handler::register(
            NAME.get().context(say("missing service name"))?,
            |event| match event {
                ServiceControl::Stop | ServiceControl::Shutdown => {
                    STOP.get_or_init(CancellationToken::new).cancel();
                    if let Some(handle) = STATUS.get() {
                        let _ = handle.set_service_status(status(
                            ServiceState::StopPending,
                            ServiceExitCode::Win32(0),
                        ));
                    }
                    ServiceControlHandlerResult::NoError
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            },
        )?;
        STATUS
            .set(handle)
            .map_err(|_| anyhow::anyhow!("status handle already initialized"))?;
        handle.set_service_status(status(
            ServiceState::StartPending,
            ServiceExitCode::Win32(0),
        ))?;
        let runner = RUNNER.get().context(say("missing service runner"))?;
        // Catch a main-thread panic so SCM receives Stopped instead of leaving
        // a stale Running status. Normal Tokio connection tasks remain isolated.
        std::panic::catch_unwind(*runner).map_err(|_| anyhow::anyhow!("service runner panicked"))?
    })();
    if let Some(handle) = STATUS.get() {
        let _ = handle.set_service_status(status(
            ServiceState::Stopped,
            if result.is_ok() {
                ServiceExitCode::Win32(0)
            } else {
                ServiceExitCode::ServiceSpecific(1)
            },
        ));
    }
    let _ = RESULT.set(result.map_err(|e| format!("{e:#}")));
}
pub(super) fn ready() -> anyhow::Result<()> {
    if let Some(handle) = STATUS.get() {
        let state = if STOP.get().is_some_and(CancellationToken::is_cancelled) {
            ServiceState::StopPending
        } else {
            ServiceState::Running
        };
        handle.set_service_status(status(state, ServiceExitCode::Win32(0)))?;
    }
    Ok(())
}
pub(super) async fn cancelled() {
    STOP.get_or_init(CancellationToken::new).cancelled().await;
}
fn wait(service: &Service, target: ServiceState) -> anyhow::Result<()> {
    let end = Instant::now() + Duration::from_secs(35);
    loop {
        let status = service.query_status()?;
        if status.current_state == target {
            return Ok(());
        }
        ensure!(
            Instant::now() < end,
            "service state transition timed out: {:?}",
            status.current_state
        );
        if target == ServiceState::Running && status.current_state == ServiceState::Stopped {
            anyhow::bail!("service stopped during startup: {:?}", status.exit_code);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
pub(super) fn manage(name: &str, action: Action, dry: bool) -> anyhow::Result<()> {
    if let Action::Install(options) = action {
        let def = Definition::new(name.into(), options)?;
        if dry {
            println!("{}", def.render(Platform::Windows)?);
            return Ok(());
        }
        let manager = ServiceManager::local_computer(
            None::<&str>,
            ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
        )?;
        let info = ServiceInfo {
            name: name.into(),
            display_name: format!("Felucca {name}").into(),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: def.binary.clone().into(),
            launch_arguments: def.windows_args().into_iter().map(OsString::from).collect(),
            dependencies: vec![],
            account_name: Some(OsString::from("NT AUTHORITY\\LocalService")),
            account_password: None,
        };
        // SC_ACTION_RESTART needs SERVICE_START on the handle that sets it.
        let service = manager.create_service(
            &info,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::DELETE,
        )?;
        if let Err(error) = service
            .set_description(MARKER)
            .and_then(|()| restart_on_failure(&service))
        {
            let _ = service.delete();
            return Err(error.into());
        }
        println!(
            "installed {name} as LocalService; grant it access to config, logs and backend files, then service start"
        );
        return Ok(());
    }
    if dry {
        println!("SCM {action:?} {name}");
        return Ok(());
    }
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let mut access = ServiceAccess::QUERY_STATUS | ServiceAccess::QUERY_CONFIG;
    if matches!(action, Action::Start | Action::Restart) {
        access |= ServiceAccess::START;
    }
    if matches!(action, Action::Stop | Action::Restart | Action::Uninstall) {
        access |= ServiceAccess::STOP;
    }
    if matches!(action, Action::Uninstall) {
        access |= ServiceAccess::DELETE;
    }
    let service = manager.open_service(name, access)?;
    ensure!(
        service.query_config()?.display_name == OsString::from(format!("Felucca {name}")),
        "service is not a felucca installation"
    );
    if matches!(action, Action::Status) {
        println!("{:#?}", service.query_status()?);
        return Ok(());
    }
    if matches!(action, Action::Stop | Action::Restart | Action::Uninstall)
        && service.query_status()?.current_state != ServiceState::Stopped
    {
        service.stop()?;
        wait(&service, ServiceState::Stopped)?;
    }
    if matches!(action, Action::Start | Action::Restart)
        && service.query_status()?.current_state != ServiceState::Running
    {
        service.start::<&OsStr>(&[])?;
        wait(&service, ServiceState::Running)?;
    }
    if matches!(action, Action::Uninstall) {
        service.delete()?;
    }
    Ok(())
}
