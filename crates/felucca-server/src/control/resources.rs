use super::{Principal, Resource, Result, Write, bad, forbidden, missing};
use felucca_core::{
    ServerConfig,
    config::{AdminRole, TunnelConfig},
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

pub const REDACTED: &str = "<redacted>";

pub fn visible(principal: &Principal, user: Option<&str>) -> bool {
    principal.role == AdminRole::Admin
        || user.is_some_and(|user| principal.user.as_deref() == Some(user))
}
pub fn admin(principal: &Principal) -> Result<()> {
    if principal.role != AdminRole::Admin {
        return Err(forbidden());
    }
    Ok(())
}
fn client(config: &ServerConfig, principal: &Principal, key: &str) -> Result<()> {
    let client = config.clients.get(key).ok_or_else(missing)?;
    if !visible(principal, client.user.as_deref()) {
        return Err(missing());
    }
    Ok(())
}
pub fn value(value: impl Serialize) -> Value {
    serde_json::to_value(value).expect("config serialization")
}
pub fn redacted(value: impl Serialize) -> Value {
    let mut value = self::value(value);
    redact(&mut value, false);
    value
}
fn redact(value: &mut Value, credentials: bool) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if (credentials
                    || matches!(
                        key.as_str(),
                        "vkey"
                            | "password"
                            | "password_hash"
                            | "token_hash"
                            | "token"
                            | "signing_key"
                            | "hmac_key"
                            | "zerossl_api_key"
                            | "secret"
                    ))
                    && value.is_string()
                {
                    *value = Value::String(REDACTED.into());
                } else {
                    // Authentication.users is a map of usernames to passwords;
                    // ServerConfig.user has object values and remains visible.
                    redact(value, key == "users");
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(|v| redact(v, false)),
        _ => {}
    }
}
fn restore(value: &mut Value, previous: Option<&Value>) -> Result<()> {
    match value {
        Value::String(text) if text == REDACTED => {
            *value = previous
                .filter(|v| v.is_string())
                .cloned()
                .ok_or_else(|| bad("new credentials cannot use the redaction marker"))?;
        }
        Value::Object(map) => {
            for (key, child) in map {
                restore(child, previous.and_then(|v| v.get(key)))?;
            }
        }
        Value::Array(values) => {
            for (index, child) in values.iter_mut().enumerate() {
                let old = if let Some(id) = child.get("id").filter(|id| !id.is_null()) {
                    previous
                        .and_then(Value::as_array)
                        .and_then(|values| values.iter().find(|v| v.get("id") == Some(id)))
                } else {
                    previous.and_then(|v| v.get(index))
                };
                restore(child, old)?;
            }
        }
        _ => {}
    }
    Ok(())
}
pub fn parse<T: DeserializeOwned>(mut input: Value, previous: Option<Value>) -> Result<T> {
    restore(&mut input, previous.as_ref())?;
    serde_json::from_value(input).map_err(|error| bad(format!("invalid resource: {error}")))
}
pub fn read(config: &ServerConfig, principal: &Principal, resource: &Resource) -> Result<Value> {
    match resource {
        Resource::Platforms => {
            admin(principal)?;
            Ok(Value::Array(config.node.as_ref().ok_or_else(missing)?.platforms.iter()
                .map(|(id, platform)| json!({"id":id,"config":redacted(platform)})).collect()))
        }
        Resource::Platform(id) => {
            admin(principal)?;
            Ok(redacted(config.node.as_ref().and_then(|n|n.platforms.get(id)).ok_or_else(missing)?))
        }
        Resource::Webhooks => {
            Ok(Value::Array(config.node.as_ref().ok_or_else(missing)?.webhooks.iter()
                .filter(|(_, w)|visible(principal,w.user.as_deref()))
                .map(|(id, webhook)|json!({"id":id,"config":redacted(webhook)})).collect()))
        }
        Resource::Webhook(id) => {
            let webhook=config.node.as_ref().and_then(|n|n.webhooks.get(id)).filter(|w|visible(principal,w.user.as_deref())).ok_or_else(missing)?;
            Ok(redacted(webhook))
        }
        Resource::Shares => Ok(Value::Array(config.shares.iter()
            .filter(|(_, share)| share_visible(config, principal, &share.client))
            .map(|(id, share)| json!({"id": id, "config": redacted(share)})).collect())),
        Resource::Share(id) => {
            let share = config.shares.get(id).filter(|s| share_visible(config, principal, &s.client)).ok_or_else(missing)?;
            Ok(redacted(share))
        }
        Resource::Users => Ok(json!(config.users.iter().filter(|(key, _)| visible(principal, Some(key)))
            .map(|(id, config)| json!({"id": id, "config": config})).collect::<Vec<_>>())),
        Resource::User(id) if visible(principal, Some(id)) => Ok(value(config.users.get(id).ok_or_else(missing)?)),
        Resource::Clients => Ok(Value::Array(config.clients.iter().filter(|(_, c)| visible(principal, c.user.as_deref()))
            .map(|(id, c)| json!({"id":id,"managed":config.managed.contains_key(id),"config":redacted(c)})).collect())),
        Resource::Client(id) => { client(config, principal, id)?; Ok(redacted(&config.clients[id])) }
        Resource::Managed(id) => {
            client(config, principal, id)?;
            Ok(redacted(config.managed.get(id).ok_or_else(missing)?))
        }
        Resource::ManagedTunnel(id, tunnel) => {
            client(config, principal, id)?;
            let tunnel_id = *tunnel;
            Ok(redacted(config.managed.get(id).ok_or_else(missing)?.tunnels
                .iter().find(|t| t.id == Some(tunnel_id)).ok_or_else(missing)?))
        }
        Resource::Locals => Ok(Value::Array(config.locals.iter().filter(|(_, c)| visible(principal, c.user.as_deref()))
            .map(|(id, c)| json!({"id":id, "config":redacted(c)})).collect())),
        Resource::Local(id) => {
            let local = config.locals.get(id).filter(|c| visible(principal, c.user.as_deref())).ok_or_else(missing)?;
            Ok(redacted(local))
        }
        Resource::Accounts => { admin(principal)?; Ok(redacted(&config.admin.as_ref().ok_or_else(missing)?.accounts)) }
        Resource::Config => { admin(principal)?; Ok(redacted(config)) }
        _ => Err(missing()),
    }
}

pub fn mutate(
    config: &mut ServerConfig,
    principal: &Principal,
    method: Write,
    resource: &Resource,
    input: Value,
) -> Result<()> {
    if principal.role == AdminRole::Viewer {
        return Err(forbidden());
    }
    let delete = method == Write::Delete;
    match resource {
        Resource::Platform(id) => {
            admin(principal)?;
            let node = config.node.as_mut().ok_or_else(missing)?;
            if delete {
                node.platforms.remove(id).ok_or_else(missing)?;
            } else {
                let next = parse(input, node.platforms.get(id).map(value))?;
                node.platforms.insert(id.clone(), next);
            }
        }
        Resource::Webhook(id) => {
            let node = config.node.as_mut().ok_or_else(missing)?;
            admin(principal)?;
            if node
                .webhooks
                .get(id)
                .is_some_and(|w| !visible(principal, w.user.as_deref()))
            {
                return Err(missing());
            }
            if delete {
                node.webhooks.remove(id).ok_or_else(missing)?;
            } else {
                let next: felucca_core::config::node::WebhookConfig =
                    parse(input, node.webhooks.get(id).map(value))?;
                if !visible(principal, next.user.as_deref()) {
                    return Err(forbidden());
                }
                node.webhooks.insert(id.clone(), next);
            }
        }
        Resource::Share(id) => {
            if let Some(old) = config.shares.get(id)
                && !share_visible(config, principal, &old.client)
            {
                return Err(missing());
            }
            if delete {
                config.shares.remove(id).ok_or_else(missing)?;
            } else {
                let next: felucca_core::config::ShareConfig =
                    parse(input, config.shares.get(id).map(value))?;
                if !share_visible(config, principal, &next.client) {
                    return Err(forbidden());
                }
                config.shares.insert(id.clone(), next);
            }
        }
        Resource::Config if !delete => {
            admin(principal)?;
            *config = parse(input, Some(value(&*config)))?;
        }
        Resource::User(id) => {
            admin(principal)?;
            if delete {
                config.users.remove(id).ok_or_else(missing)?;
            } else {
                let next = parse(input, config.users.get(id).map(value))?;
                config.users.insert(id.clone(), next);
            }
        }
        Resource::Client(id) => {
            if let Some(old) = config.clients.get(id)
                && !visible(principal, old.user.as_deref())
            {
                return Err(missing());
            }
            if delete {
                client(config, principal, id)?;
                config.clients.remove(id);
                config.managed.remove(id);
            } else {
                let next: felucca_core::config::ClientEntry =
                    parse(input, config.clients.get(id).map(value))?;
                if !visible(principal, next.user.as_deref()) {
                    return Err(forbidden());
                }
                config.clients.insert(id.clone(), next);
            }
        }
        Resource::Managed(id) => {
            client(config, principal, id)?;
            if delete {
                config.managed.remove(id).ok_or_else(missing)?;
            } else {
                let next = parse(input, config.managed.get(id).map(value))?;
                config.managed.insert(id.clone(), next);
            }
        }
        Resource::ManagedTunnel(id, tunnel) => {
            client(config, principal, id)?;
            let managed = config
                .managed
                .get_mut(id)
                .ok_or_else(|| bad("create managed configuration before editing its tunnels"))?;
            update_tunnel(&mut managed.tunnels, *tunnel, delete, input)?;
        }
        Resource::Local(id) => {
            // Local definitions can open ambient directories on the server.
            admin(principal)?;
            if delete {
                config.locals.remove(id).ok_or_else(missing)?;
            } else {
                let next = parse(input, config.locals.get(id).map(value))?;
                config.locals.insert(id.clone(), next);
            }
        }
        Resource::LocalTunnel(id, tunnel) => {
            admin(principal)?;
            let local = config.locals.get_mut(id).ok_or_else(missing)?;
            update_tunnel(&mut local.tunnels, *tunnel, delete, input)?;
        }
        Resource::Account(id) => {
            admin(principal)?;
            let accounts = &mut config.admin.as_mut().ok_or_else(missing)?.accounts;
            if delete {
                accounts.remove(id).ok_or_else(missing)?;
            } else {
                let next = parse(input, accounts.get(id).map(value))?;
                accounts.insert(id.clone(), next);
            }
        }
        _ => return Err(missing()),
    }
    Ok(())
}
fn share_visible(config: &ServerConfig, principal: &Principal, key: &str) -> bool {
    // Shares deliberately grant access to a provider's services. Only that
    // provider's owner (or an administrator) can read or alter the grant.
    let owner = config
        .clients
        .get(key)
        .map(|c| c.user.as_deref())
        .or_else(|| {
            key.strip_prefix("local.")
                .and_then(|key| config.locals.get(key))
                .map(|c| c.user.as_deref())
        });
    owner.is_some_and(|owner| visible(principal, owner))
}
fn update_tunnel(
    tunnels: &mut Vec<TunnelConfig>,
    id: u32,
    delete: bool,
    input: Value,
) -> Result<()> {
    if id == 0 {
        return Err(bad("tunnel ids are nonzero integers"));
    }
    let position = tunnels.iter().position(|t| t.id == Some(id));
    if delete {
        tunnels.remove(position.ok_or_else(missing)?);
    } else {
        let mut next: TunnelConfig = parse(input, position.map(|i| value(&tunnels[i])))?;
        if next.id.is_some_and(|candidate| candidate != id) {
            return Err(bad("tunnel id does not match URL"));
        }
        next.id = Some(id);
        if let Some(i) = position {
            tunnels[i] = next;
        } else {
            tunnels.push(next);
        }
    }
    Ok(())
}
