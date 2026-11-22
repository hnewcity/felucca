//! Real subprocess checks confined to copies in a temporary directory.
use ring::digest::{Context as DigestContext, SHA256};
use std::{
    path::Path,
    process::{Command, Output},
};
const SERVER: &str = env!("CARGO_BIN_EXE_felucca-server");
const AGENT: &str = env!("CARGO_BIN_EXE_felucca-agent");
fn run(exe: &Path, args: &[&str]) -> Output {
    Command::new(exe)
        .args(args)
        .env_remove("FELUCCA_CONFIG")
        .output()
        .unwrap()
}
fn digest(path: &Path) -> String {
    use std::io::Read;
    let mut file = std::fs::File::open(path).unwrap();
    let mut hash = DigestContext::new(&SHA256);
    let mut bytes = [0; 65536];
    loop {
        let n = file.read(&mut bytes).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    hash.finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
#[test]
fn service_render_uses_absolute_escaped_paths_without_installing() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config space &%.toml");
    std::fs::write(&config, "invalid TOML deliberately: render only").unwrap();
    for executable in [SERVER, AGENT] {
        for platform in ["systemd", "launchd", "windows"] {
            let output = run(
                Path::new(executable),
                &[
                    "service",
                    "--name",
                    "test.office",
                    "render",
                    "--platform",
                    platform,
                    "--config",
                    config.to_str().unwrap(),
                ],
            );
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8(output.stdout).unwrap();
            assert!(text.contains("test.office"));
            assert!(text.contains("Managed by felucca"));
            if platform == "launchd" && cfg!(target_os = "macos") {
                let file = dir.path().join("service.plist");
                std::fs::write(&file, &text).unwrap();
                assert!(
                    Command::new("plutil")
                        .args(["-lint", "--"])
                        .arg(&file)
                        .output()
                        .unwrap()
                        .status
                        .success()
                );
            }
        }
        let output = run(
            Path::new(executable),
            &[
                "service",
                "--name",
                "../bad",
                "render",
                "--config",
                config.to_str().unwrap(),
            ],
        );
        assert!(!output.status.success());
    }
}
#[test]
fn updater_preserves_binary_on_errors_and_replaces_only_a_verified_copy() {
    let dir = tempfile::tempdir().unwrap();
    let installed = dir
        .path()
        .join(format!("installed{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(SERVER, &installed).unwrap();
    let original = digest(&installed);
    let source = digest(Path::new(SERVER));
    let update = |artifact: &str, hash: &str, extra: &[&str]| {
        let mut args = vec!["update", "--source", artifact, "--sha256", hash];
        args.extend_from_slice(extra);
        run(&installed, &args)
    };
    let wrong = update(SERVER, &"0".repeat(64), &[]);
    assert!(!wrong.status.success());
    assert_eq!(digest(&installed), original);
    let role = update(AGENT, &digest(Path::new(AGENT)), &[]);
    assert!(!role.status.success());
    assert!(String::from_utf8_lossy(&role.stderr).contains("not a felucca-server"));
    assert_eq!(digest(&installed), original);
    let version = update(SERVER, &source, &["--expect-version", "999.0.0"]);
    assert!(!version.status.success());
    assert_eq!(digest(&installed), original);
    let check = update(SERVER, &source, &["--check-only"]);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert_eq!(digest(&installed), original);
    let mut lock_name = installed.file_name().unwrap().to_os_string();
    lock_name.push(".update.lock");
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.path().join(lock_name))
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    assert!(!update(SERVER, &source, &[]).status.success());
    fs2::FileExt::unlock(&lock).unwrap();
    #[cfg(unix)]
    let before = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(&installed).unwrap().ino()
    };
    let success = update(SERVER, &source, &[]);
    assert!(
        success.status.success(),
        "{}",
        String::from_utf8_lossy(&success.stderr)
    );
    assert_eq!(digest(&installed), source);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_ne!(
            std::fs::metadata(&installed).unwrap().ino(),
            before,
            "atomic replacement must use a new inode"
        );
    }
    assert!(run(&installed, &["--version"]).status.success());
    assert_eq!(
        digest(Path::new(SERVER)),
        source,
        "project binary must never be replaced"
    );
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".felucca-update-")
    }));
}
