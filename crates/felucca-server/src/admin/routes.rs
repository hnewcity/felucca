use super::{Result, bad, missing};
use crate::control::{Resource, Write};
fn path(path: &str) -> Result<Vec<String>> {
    if !path.starts_with("/api/") || path.len() > 4096 {
        return Err(missing());
    }
    path.trim_start_matches('/')
        .split('/')
        .map(|part| {
            let value = percent_encoding::percent_decode_str(part)
                .decode_utf8()
                .map_err(|_| bad("invalid URL encoding"))?;
            if value.is_empty() || value.chars().any(char::is_control) {
                return Err(bad("invalid resource path"));
            }
            Ok(value.into_owned())
        })
        .collect()
}

pub(super) fn write_method(method: &str) -> Result<Write> {
    match method {
        "PUT" => Ok(Write::Put),
        "DELETE" => Ok(Write::Delete),
        _ => Err(bad("resource writes use PUT or DELETE")),
    }
}
pub(super) fn resource(url: &str) -> Result<Resource> {
    let parts = path(url)?;
    let parts: Vec<&str> = parts.iter().map(String::as_str).collect();
    Ok(match parts.as_slice() {
        ["api", "platforms"] => Resource::Platforms,
        ["api", "platforms", id] => Resource::Platform((*id).into()),
        ["api", "webhooks"] => Resource::Webhooks,
        ["api", "webhooks", id] => Resource::Webhook((*id).into()),
        ["api", "shares"] => Resource::Shares,
        ["api", "shares", id] => Resource::Share((*id).into()),
        ["api", "users"] => Resource::Users,
        ["api", "users", id] => Resource::User((*id).into()),
        ["api", "clients"] => Resource::Clients,
        ["api", "clients", id] => Resource::Client((*id).into()),
        ["api", "managed", id] => Resource::Managed((*id).into()),
        ["api", "managed", id, "tunnels", tunnel] => Resource::ManagedTunnel(
            (*id).into(),
            tunnel.parse().map_err(|_| bad("invalid tunnel id"))?,
        ),
        ["api", "locals"] => Resource::Locals,
        ["api", "locals", id] => Resource::Local((*id).into()),
        ["api", "locals", id, "tunnels", tunnel] => Resource::LocalTunnel(
            (*id).into(),
            tunnel.parse().map_err(|_| bad("invalid tunnel id"))?,
        ),
        ["api", "accounts"] => Resource::Accounts,
        ["api", "accounts", id] => Resource::Account((*id).into()),
        ["api", "config"] => Resource::Config,
        _ => return Err(missing()),
    })
}
