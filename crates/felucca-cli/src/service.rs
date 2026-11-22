//! Service definitions and explicit manager operations. Rendering never mutates
//! the system; installation refuses to overwrite an existing definition.
use super::messages::say;
use anyhow::{Context, bail, ensure};
use clap::{Args, Subcommand, ValueEnum};
use felucca_i18n::{Message, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

#[cfg(windows)]
#[path = "service_windows.rs"]
mod windows;

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Platform {
    Systemd,
    Launchd,
    Windows,
}
impl Platform {
    fn native() -> anyhow::Result<Self> {
        if cfg!(target_os = "linux") {
            Ok(Self::Systemd)
        } else if cfg!(target_os = "macos") {
            Ok(Self::Launchd)
        } else if cfg!(windows) {
            Ok(Self::Windows)
        } else {
            bail!(
                "{}",
                say("service management is unsupported on this platform")
            )
        }
    }
}
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq)]
pub enum Scope {
    User,
    System,
}
#[derive(Debug, Args)]
pub struct ServiceArgs {
    /// Name of this service instance; defaults to the program name.
    #[arg(long, global = true)]
    name: Option<String>,
    /// Install per-user or machine-wide (Windows requires system).
    #[arg(long, global=true, default_value=if cfg!(windows) {"system"} else {"user"})]
    scope: Scope,
    /// Print definitions and manager commands without running them.
    #[arg(long, global = true)]
    dry_run: bool,
    #[command(subcommand)]
    action: Action,
}
#[derive(Debug, Subcommand)]
enum Action {
    /// Generate a definition on stdout; no installation or startup.
    Render {
        #[arg(long)]
        platform: Option<Platform>,
        #[command(flatten)]
        options: Install,
    },
    /// Install and enable startup; use `service start` to run it now.
    Install(Install),
    Start,
    Stop,
    Restart,
    Status,
    /// Stop and remove a definition installed by felucca; keeps config and logs.
    Uninstall,
}
#[derive(Debug, Args)]
struct Install {
    /// Local configuration file. Relative resources resolve under --workdir.
    #[arg(long)]
    config: PathBuf,
    #[arg(long)]
    workdir: Option<PathBuf>,
    /// Executable to install; defaults to this program.
    #[arg(long)]
    binary: Option<PathBuf>,
    /// Additional daemon arguments after `--` (do not include service/update).
    #[arg(last = true)]
    arguments: Vec<String>,
}
#[derive(Debug)]
struct Definition {
    name: String,
    binary: String,
    workdir: String,
    args: Vec<String>,
}
const MARKER: &str = "Managed by felucca; service definition v1";
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
}
fn text(path: &Path) -> anyhow::Result<String> {
    let value = path.to_str().context(say("service paths must be UTF-8"))?;
    ensure!(
        !value.chars().any(char::is_control),
        "service paths cannot contain control characters"
    );
    Ok(value.to_owned())
}
impl Definition {
    fn new(name: String, options: Install) -> anyhow::Result<Self> {
        let config = options
            .config
            .canonicalize()
            .context(say("service configuration file does not exist"))?;
        ensure!(
            config.is_file(),
            "service configuration must be a regular file"
        );
        let binary = options
            .binary
            .unwrap_or(std::env::current_exe()?)
            .canonicalize()?;
        ensure!(binary.is_file(), "service binary must be a regular file");
        let workdir = options
            .workdir
            .unwrap_or_else(|| config.parent().expect("absolute path").to_owned())
            .canonicalize()?;
        ensure!(workdir.is_dir(), "service workdir must be a directory");
        ensure!(
            options
                .arguments
                .iter()
                .all(|arg| !arg.chars().any(char::is_control)
                    && !matches!(
                        arg.as_str(),
                        "service" | "update" | "--service-name" | "--service-workdir"
                    )
                    && !arg.starts_with("--service-name=")
                    && !arg.starts_with("--service-workdir=")),
            "invalid extra daemon argument"
        );
        let mut args = vec!["--config".into(), text(&config)?];
        args.extend(options.arguments);
        Ok(Self {
            name,
            binary: text(&binary)?,
            workdir: text(&workdir)?,
            args,
        })
    }
    fn render(&self, platform: Platform) -> anyhow::Result<String> {
        let rendered = match platform {
            Platform::Systemd => {
                let command = std::iter::once(&self.binary)
                    .chain(&self.args)
                    .map(|s| systemd(s))
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    "# {MARKER}\n[Unit]\nDescription=Felucca {}\nAfter=network-online.target\nWants=network-online.target\nStartLimitIntervalSec=0\n\n[Service]\nType=simple\nWorkingDirectory={}\nExecStart=:{command}\nRestart=always\nRestartSec=5\nLimitNOFILE=1048576\nTimeoutStopSec=30\nKillSignal=SIGTERM\nUMask=0077\n\n[Install]\nWantedBy=default.target\n",
                    self.name,
                    systemd(&self.workdir)
                )
            }
            Platform::Launchd => {
                let args = std::iter::once(&self.binary)
                    .chain(&self.args)
                    .map(|arg| format!("<string>{}</string>", xml(arg)))
                    .collect::<String>();
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<!-- {MARKER} -->\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>com.felucca.{}</string>\n<key>ProgramArguments</key><array>{args}</array>\n<key>WorkingDirectory</key><string>{}</string>\n<key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n<key>ThrottleInterval</key><integer>5</integer>\n<key>ExitTimeOut</key><integer>30</integer>\n<key>Umask</key><integer>63</integer>\n</dict></plist>\n",
                    xml(&self.name),
                    xml(&self.workdir)
                )
            }
            Platform::Windows => {
                let args = self.windows_args();
                serde_json::to_string_pretty(
                    &serde_json::json!({"marker":MARKER,"service":self.name,"binary":self.binary,"arguments":args,"working_directory":self.workdir,"command_line":std::iter::once(&self.binary).chain(&args).map(|s|windows_quote(s)).collect::<Vec<_>>().join(" ")}),
                )?
            }
        };
        Ok(rendered)
    }
    fn windows_args(&self) -> Vec<String> {
        let mut args = vec![
            "--service-name".into(),
            self.name.clone(),
            "--service-workdir".into(),
            self.workdir.clone(),
        ];
        args.extend(self.args.clone());
        args
    }
}
// Specifiers expand in unit paths too. ExecStart uses the colon prefix to
// suppress environment expansion; WorkingDirectory never expands dollars.
fn systemd(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
    )
}
fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn windows_quote(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            result.extend(std::iter::repeat_n('\\', slashes * 2 + 1));
        } else {
            result.extend(std::iter::repeat_n('\\', slashes));
        }
        slashes = 0;
        result.push(ch);
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}
fn directory(platform: Platform, scope: Scope) -> anyhow::Result<PathBuf> {
    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .context(say("HOME unavailable for user service"))
    };
    Ok(match (platform, scope) {
        (Platform::Systemd, Scope::System) => PathBuf::from("/etc/systemd/system"),
        (Platform::Systemd, Scope::User) => std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or(home()?.join(".config"))
            .join("systemd/user"),
        (Platform::Launchd, Scope::System) => PathBuf::from("/Library/LaunchDaemons"),
        (Platform::Launchd, Scope::User) => home()?.join("Library/LaunchAgents"),
        (Platform::Windows, _) => bail!(
            "{}",
            say("Windows definitions are stored by the service manager")
        ),
    })
}
fn filename(platform: Platform, name: &str) -> String {
    match platform {
        Platform::Launchd => format!("com.felucca.{name}.plist"),
        _ => format!("{name}.service"),
    }
}
async fn command(program: &str, args: &[String], dry: bool) -> anyhow::Result<()> {
    if dry {
        println!(
            "{}",
            serde_json::to_string(&serde_json::json!({"program":program,"args":args}))?
        );
        return Ok(());
    }
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| {
            say(Message::with(
                "cannot start {program}",
                [Value::new("program", program)],
            ))
        })?;
    let status = tokio::time::timeout(Duration::from_secs(40), child.wait())
        .await
        .context(say("service manager timed out"))??;
    ensure!(
        status.success(),
        "{}",
        say(Message::with(
            "{program} failed with {status}",
            [Value::new("program", program), Value::new("status", status),],
        ))
    );
    Ok(())
}
async fn launch_domain(scope: Scope) -> anyhow::Result<String> {
    if scope == Scope::System {
        return Ok("system".into());
    }
    let output = tokio::process::Command::new("id")
        .arg("-u")
        .output()
        .await?;
    ensure!(output.status.success(), "cannot determine user id");
    let uid = String::from_utf8(output.stdout)?.trim().parse::<u32>()?;
    Ok(format!("gui/{uid}"))
}
impl ServiceArgs {
    pub async fn run(self, role: &str) -> anyhow::Result<()> {
        let name = self.name.unwrap_or_else(|| role.to_owned());
        ensure!(
            valid_name(&name),
            "service name requires 1..64 ASCII letters, digits, dots, hyphens or underscores, starting with a letter or digit"
        );
        if let Action::Render { platform, options } = self.action {
            let platform = match platform {
                Some(p) => p,
                None => Platform::native()?,
            };
            let definition = Definition::new(name, options)?;
            println!("{}", definition.render(platform)?);
            return Ok(());
        }
        let platform = Platform::native()?;
        if matches!(platform, Platform::Windows) {
            ensure!(
                self.scope == Scope::System,
                "Windows services require --scope system"
            );
            #[cfg(windows)]
            return windows::manage(&name, self.action, self.dry_run);
            #[cfg(not(windows))]
            bail!("{}", say("Windows service manager unavailable"));
        }
        let file = directory(platform, self.scope)?.join(filename(platform, &name));
        let mut prefix = Vec::new();
        if self.scope == Scope::User {
            prefix.push("--user".into());
        }
        let systemctl = |action: &str| {
            let mut args = prefix.clone();
            args.extend([action.into(), format!("{name}.service")]);
            args
        };
        if let Action::Install(options) = self.action {
            let definition = Definition::new(name.clone(), options)?;
            let mut content = definition.render(platform)?;
            if matches!(platform, Platform::Systemd) && self.scope == Scope::System {
                content = content.replace("WantedBy=default.target", "WantedBy=multi-user.target");
            }
            if self.dry_run {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &serde_json::json!({"path":file,"contents":content})
                    )?
                );
                return Ok(());
            }
            let parent = file.parent().expect("service directory");
            std::fs::create_dir_all(parent)?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            use std::io::Write;
            temp.write_all(content.as_bytes())?;
            temp.as_file().sync_all()?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                temp.as_file()
                    .set_permissions(std::fs::Permissions::from_mode(0o644))?;
            }
            temp.persist_noclobber(&file)
                .map_err(|e| e.error)
                .context(say(
                    "service definition already exists or cannot be installed",
                ))?;
            if matches!(platform, Platform::Systemd) {
                let mut reload = prefix.clone();
                reload.push("daemon-reload".into());
                let result = async {
                    command("systemctl", &reload, false).await?;
                    command("systemctl", &systemctl("enable"), false).await
                }
                .await;
                if let Err(error) = result {
                    let _ = std::fs::remove_file(&file);
                    let _ = command("systemctl", &reload, false).await;
                    return Err(error);
                }
            }
            println!(
                "{}",
                say(Message::with(
                    "installed {file}; use service start to run it",
                    [Value::new("file", file.display())],
                ))
            );
            return Ok(());
        }
        if matches!(self.action, Action::Uninstall) && !self.dry_run {
            let metadata = std::fs::symlink_metadata(&file)
                .context(say("service definition is not installed"))?;
            ensure!(
                metadata.is_file() && metadata.len() < 1024 * 1024,
                "unexpected service definition type or size"
            );
            let content = std::fs::read_to_string(&file)?;
            ensure!(
                content.contains(MARKER),
                "refusing to remove a service not installed by felucca"
            );
        }
        match platform {
            Platform::Systemd => {
                if matches!(self.action, Action::Uninstall) {
                    let mut args = prefix.clone();
                    args.extend(["disable".into(), "--now".into(), format!("{name}.service")]);
                    command("systemctl", &args, self.dry_run).await?;
                } else {
                    let verb = match self.action {
                        Action::Start => "start",
                        Action::Stop => "stop",
                        Action::Restart => "restart",
                        Action::Status => "status",
                        _ => unreachable!(),
                    };
                    command("systemctl", &systemctl(verb), self.dry_run).await?;
                }
            }
            Platform::Launchd => {
                let domain = launch_domain(self.scope).await?;
                let label = format!("{domain}/com.felucca.{name}");
                match self.action {
                    Action::Start => {
                        command(
                            "launchctl",
                            &["bootstrap".into(), domain, text(&file)?],
                            self.dry_run,
                        )
                        .await?
                    }
                    Action::Status => {
                        command("launchctl", &["print".into(), label], self.dry_run).await?
                    }
                    Action::Stop => {
                        command("launchctl", &["bootout".into(), label], self.dry_run).await?
                    }
                    Action::Restart | Action::Uninstall => {
                        if self.dry_run {
                            command("launchctl", &["bootout".into(), label], true).await?;
                        } else {
                            // An unloaded service has nothing to stop. Do not ignore a
                            // stop failure for a service that is actually registered.
                            let loaded = tokio::process::Command::new("launchctl")
                                .args(["print", &label])
                                .stdout(Stdio::null())
                                .stderr(Stdio::null())
                                .status()
                                .await?
                                .success();
                            if loaded {
                                command("launchctl", &["bootout".into(), label], false).await?;
                            }
                        }
                        if matches!(self.action, Action::Restart) {
                            command(
                                "launchctl",
                                &["bootstrap".into(), domain, text(&file)?],
                                self.dry_run,
                            )
                            .await?;
                        }
                    }
                    _ => unreachable!(),
                }
            }
            Platform::Windows => unreachable!(),
        }
        if matches!(self.action, Action::Uninstall) {
            if self.dry_run {
                println!(
                    "{}",
                    say(Message::with(
                        "remove {file}",
                        [Value::new("file", file.display())],
                    ))
                );
            } else {
                std::fs::remove_file(&file)?;
            }
            if matches!(platform, Platform::Systemd) {
                prefix.push("daemon-reload".into());
                command("systemctl", &prefix, self.dry_run).await?;
            }
        }
        Ok(())
    }
}

/// Called before a Tokio runtime is built; Windows SCM invokes the runner on
/// its service thread. Normal console invocations use the same runner directly.
pub fn entrypoint(runner: fn() -> anyhow::Result<()>) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        windows::entrypoint(runner)
    }
    #[cfg(not(windows))]
    {
        ensure!(
            !std::env::args_os().any(
                |v| v == "--service-name" || v.to_string_lossy().starts_with("--service-name=")
            ),
            "--service-name is reserved for Windows SCM"
        );
        finish(runner)
    }
}

/// Runs a binary's main body and reports a failure in the process language.
///
/// Returning the error would let the runtime print it with `Debug`, which is the
/// English source text for a configuration failure. Exiting here is what the
/// runtime would have done anyway, minus the wrong language.
fn finish(runner: fn() -> anyhow::Result<()>) -> anyhow::Result<()> {
    match runner() {
        Ok(()) => Ok(()),
        Err(error) => {
            super::messages::report(&error);
            std::process::exit(1);
        }
    }
}
pub fn ready() -> anyhow::Result<()> {
    #[cfg(windows)]
    windows::ready()?;
    Ok(())
}
#[cfg(windows)]
pub async fn cancelled() {
    windows::cancelled().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_definitions_escape_manager_syntax_without_a_shell() {
        let def = Definition {
            name: "office".into(),
            binary: "/tmp/a b/$prog%\\\"&".into(),
            workdir: "/tmp/dir&<%$".into(),
            args: vec!["--config".into(), "/tmp/config<&.toml".into()],
        };
        let unit = def.render(Platform::Systemd).unwrap();
        assert!(unit.contains("$prog%%"));
        assert!(unit.contains("WorkingDirectory=\"/tmp/dir&<%%$\""));
        let plist = def.render(Platform::Launchd).unwrap();
        assert!(plist.contains("config&lt;&amp;.toml"));
        // Restart after any exit the manager did not request, a stray SIGTERM
        // included; `service stop` uses systemctl stop or launchctl bootout.
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
        assert!(unit.contains("\nRestart=always\n"));
        assert!(unit.contains("\nStartLimitIntervalSec=0\n"));
        assert!(unit.contains("\nLimitNOFILE=1048576\n"));
        let windows: serde_json::Value =
            serde_json::from_str(&def.render(Platform::Windows).unwrap()).unwrap();
        assert_eq!(windows["arguments"][0], "--service-name");
        assert_eq!(windows["arguments"][2], "--service-workdir");
        assert_eq!(
            windows_quote("C:\\dir with space\\"),
            "\"C:\\dir with space\\\\\""
        );
        assert_eq!(windows_quote("a\\\"b"), "\"a\\\\\\\"b\"");
        for name in ["../x", "-switch", "a\nb", "a/b", ""] {
            assert!(!valid_name(name));
        }
        assert!(valid_name("felucca.office-1"));
    }
}
