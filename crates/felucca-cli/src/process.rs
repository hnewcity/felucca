//! Process-wide settings for a long-running server or agent.

/// Records panics in the log and raises the open-file limit.
///
/// Call once, after logging is installed.
pub fn prepare() {
    log_panics();
    #[cfg(unix)]
    raise_descriptor_limit();
}

/// Sends panics to the configured log as well as stderr.
///
/// A service manager often discards stderr, and a panic inside one connection
/// task is otherwise caught by Tokio and dropped without a trace. The default
/// hook still runs, so foreground output is unchanged.
fn log_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!(
            thread = std::thread::current().name().unwrap_or("unnamed"),
            panic = %info,
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "panic"
        );
        default(info);
    }));
}

/// Most systems start a process with a soft limit of 256 (launchd) or 1024
/// (systemd and login shells), which a busy server or agent reaches at a few
/// hundred connections. The hard limit is the administrator's ceiling, so take
/// as much of it as the kernel accepts, up to `CEILING`.
#[cfg(unix)]
fn raise_descriptor_limit() {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    const CEILING: u64 = 1 << 20;

    let limit = getrlimit(Resource::Nofile);
    let Some(current) = limit.current else {
        return;
    };
    let mut target = limit.maximum.unwrap_or(u64::MAX).min(CEILING);
    // macOS rejects a soft limit above kern.maxfilesperproc even when the hard
    // limit is unlimited; step down until the kernel accepts.
    while target > current {
        let raised = Rlimit {
            current: Some(target),
            maximum: limit.maximum,
        };
        if setrlimit(Resource::Nofile, raised).is_ok() {
            tracing::debug!(from = current, to = target, "raised open file limit");
            return;
        }
        target /= 2;
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn the_soft_descriptor_limit_is_never_lowered() {
        use rustix::process::{Resource, getrlimit};
        let before = getrlimit(Resource::Nofile).current;
        super::raise_descriptor_limit();
        match (before, getrlimit(Resource::Nofile).current) {
            (Some(before), Some(after)) => assert!(after >= before),
            (_, None) => {}
            (None, Some(after)) => panic!("unlimited soft limit lowered to {after}"),
        }
    }
}
