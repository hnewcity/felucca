use super::{Error, Principal, Resource, Result, Write, bad, credentials, forbidden, resources};
use crate::{
    Daemon,
    daemon::{Applied, OperationRecord},
};
use felucca_core::{Secret, config::AdminRole};
use felucca_i18n::{Message, Value as MessageValue};
use serde_json::Value;
use std::sync::{Arc, Weak};

pub(crate) struct Change {
    pub resource: Resource,
    pub method: Write,
    pub value: Value,
}

pub(crate) enum Command {
    Change(Change),
    Batch(Vec<Change>),
    Kick(Vec<String>),
    Sync,
}

/// The transport supplies the original request digest so retries remain
/// recognizable across upgrades and after successful deletion of a resource.
pub(crate) struct Operation {
    pub revision: u64,
    pub key: String,
    pub digest: [u8; 32],
}

pub(crate) struct Management {
    daemon: Weak<Daemon>,
    password_workers: Arc<tokio::sync::Semaphore>,
}
impl Management {
    pub fn new(daemon: Weak<Daemon>, password_workers: Arc<tokio::sync::Semaphore>) -> Self {
        Self {
            daemon,
            password_workers,
        }
    }

    /// Check retries before preparing commands: a successful DELETE no longer
    /// has a target. Daemon repeats this check under the transaction lock.
    pub fn replay(&self, principal: &Principal, operation: &Operation) -> Result<Option<Applied>> {
        if principal.role == AdminRole::Viewer {
            return Err(forbidden());
        }
        let daemon = self
            .daemon
            .upgrade()
            .ok_or_else(|| Error::Unavailable(Message::text("daemon unavailable")))?;
        if let Some(old) = daemon.operation(&principal.account, &operation.key) {
            if old.digest != operation.digest {
                return Err(bad("idempotency key already names a different request"));
            }
            return Ok(Some(Applied {
                revision: old.revision,
                replayed: true,
                durable: true,
            }));
        }
        if daemon.config().0 != operation.revision {
            return Err(Error::Conflict(
                "configuration revision changed; reload and retry".into(),
            ));
        }
        Ok(None)
    }

    pub async fn apply(
        &self,
        principal: &Principal,
        operation: Operation,
        command: Command,
    ) -> Result<Applied> {
        if matches!(command, Command::Sync) {
            resources::admin(principal)?;
        }
        if let Some(applied) = self.replay(principal, &operation)? {
            return Ok(applied);
        }
        let daemon = self
            .daemon
            .upgrade()
            .ok_or_else(|| Error::Unavailable("daemon unavailable".into()))?;
        let (revision, mut next) = daemon.config();
        if revision != operation.revision {
            return Err(Error::Conflict(
                "configuration revision changed; reload and retry".into(),
            ));
        }
        match command {
            Command::Change(mut change) => {
                if matches!(change.resource, Resource::Account(_)) && change.method == Write::Put {
                    resources::admin(principal)?;
                    self.encode_password(&mut change.value).await?;
                }
                resources::mutate(
                    &mut next,
                    principal,
                    change.method,
                    &change.resource,
                    change.value,
                )?;
            }
            Command::Batch(changes) => {
                if changes.is_empty() || changes.len() > 128 {
                    return Err(bad("batches require 1..128 operations"));
                }
                for change in changes {
                    resources::mutate(
                        &mut next,
                        principal,
                        change.method,
                        &change.resource,
                        change.value,
                    )?;
                }
            }
            Command::Kick(ids) => {
                if ids.is_empty()
                    || ids.len() > 128
                    || ids.iter().collect::<std::collections::HashSet<_>>().len() != ids.len()
                {
                    return Err(bad("kick requires 1..128 unique client ids"));
                }
                for id in ids {
                    let client = next.clients.get_mut(&id).ok_or(super::Error::Missing)?;
                    if !resources::visible(principal, client.user.as_deref()) {
                        return Err(forbidden());
                    }
                    client.enabled = false;
                }
            }
            Command::Sync => resources::admin(principal)?,
        }
        next.validate().map_err(|error| bad(error.message()))?;
        let record = OperationRecord {
            key: operation.key,
            account: principal.account.clone(),
            user: principal.user.clone(),
            digest: operation.digest,
            revision: 0,
        };
        daemon
            .apply_operation(next, operation.revision, record)
            .await
            .map_err(|error| {
                // The message describes the store failure, which no catalog covers;
                // the reason is the part an operator can act on.
                let reason = error.to_string();
                match error.kind() {
                    std::io::ErrorKind::AlreadyExists => Error::Conflict(reason.into()),
                    std::io::ErrorKind::InvalidInput => Error::Invalid(reason.into()),
                    _ => Error::Unavailable(reason.into()),
                }
            })
    }

    async fn encode_password(&self, input: &mut Value) -> Result<()> {
        let Some(password) = input.as_object_mut().and_then(|map| map.remove("password")) else {
            return Ok(());
        };
        let password = Secret::from(
            password
                .as_str()
                .ok_or_else(|| bad("password must be a string"))?,
        );
        let slot = self
            .password_workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let encoded = tokio::task::spawn_blocking(move || {
            let _slot = slot;
            credentials::hash_password(password.as_bytes())
        })
        .await
        .map_err(|_| Error::Unavailable(Message::text("password worker failed")))?
        .map_err(|error| {
            bad(Message::with(
                "cannot hash a password: {reason}",
                [MessageValue::new("reason", error)],
            ))
        })?;
        input
            .as_object_mut()
            .expect("password came from object")
            .insert("password_hash".into(), Value::String(encoded));
        Ok(())
    }
}
