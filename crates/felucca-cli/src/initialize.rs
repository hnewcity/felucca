//! First-run server setup using the same configuration model as normal startup.
use crate::messages::say;
use anyhow::Context;
use felucca_core::ServerConfig;
use felucca_i18n::{Message, Value};
use std::{io::Write, path::Path};

pub fn server(path: &Path, address: Option<&str>) -> anyhow::Result<()> {
    let address = address.unwrap_or("http://127.0.0.1:8990");
    let url = url::Url::parse(address).context(say(
        "Use a complete management address, such as http://127.0.0.1:8990.",
    ))?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "{}",
        say("Use a complete management address, such as http://127.0.0.1:8990.")
    );
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    let data = path.with_extension("data");
    anyhow::ensure!(
        !data.join("runtime.json").try_exists()?,
        "{}",
        say(
            "Existing server data found. Restore the original configuration file or choose a new configuration path."
        )
    );
    let port = url.port_or_known_default().expect("validated HTTP URL");
    let bind = if url.scheme() == "https" {
        // HTTPS origins are served through an existing TLS reverse proxy.
        "127.0.0.1:8990".to_owned()
    } else {
        match url.host() {
            Some(url::Host::Ipv6(ip)) => {
                format!("[{}]:{port}", if ip.is_loopback() { "::1" } else { "::" })
            }
            Some(url::Host::Ipv4(ip)) if ip.is_loopback() => format!("{ip}:{port}"),
            Some(url::Host::Domain("localhost")) => format!("127.0.0.1:{port}"),
            _ => format!("0.0.0.0:{port}"),
        }
    };
    let password = felucca_server::admin::credentials::token();
    let hash = felucca_server::admin::credentials::hash_password(password.as_bytes())?;
    let mut config: ServerConfig = serde_json::from_value(serde_json::json!({
        "admin": { "bind": bind, "origin": url.origin().ascii_serialization(),
            "state_path": data.join("runtime.json"),
            "account": { "root": { "role": "admin", "password_hash": hash } } },
        "state": { "path": data.join("traffic.json") },
    }))?;
    config.validate()?;
    let bytes = toml::to_string_pretty(&config)?;
    let mut file =
        tempfile::NamedTempFile::new_in(path.parent().expect("absolute configuration path"))?;
    file.write_all(bytes.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist_noclobber(&path).context(say("The configuration already exists or cannot be created. Start with the existing file, or choose a new path."))?;
    println!(
        "{}",
        say(Message::with(
            "Management address: {address}",
            [Value::new("address", url.origin().ascii_serialization())]
        ))
    );
    println!(
        "{}",
        say(Message::with(
            "Initial login: root / {password}",
            [Value::new("password", password)]
        ))
    );
    println!(
        "{}",
        say(Message::with(
            "Keep {directory} when restarting, upgrading or backing up this server.",
            [Value::new("directory", data.display())]
        ))
    );
    Ok(())
}
