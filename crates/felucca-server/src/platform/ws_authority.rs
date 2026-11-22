use super::super::{Node, ReverseIdentity, principal};
use crate::control::Principal;
use felucca_core::config::node::PlatformConfig;
use std::{io, time::Duration};
#[derive(Clone)]
pub(crate) enum Authority {
    Reverse {
        identity: ReverseIdentity,
        config: std::sync::Arc<PlatformConfig>,
    },
    Direct {
        headers: http::HeaderMap,
        principal: Principal,
    },
}
impl Authority {
    pub(super) fn current(&self, node: &Node) -> io::Result<Principal> {
        let admin = node.admin.upgrade().ok_or(io::ErrorKind::NotConnected)?;
        let daemon = admin.daemon.upgrade().ok_or(io::ErrorKind::NotConnected)?;
        let config = daemon.config().1;
        match self {
            Self::Reverse { identity, .. } => {
                let platform = config
                    .node
                    .as_ref()
                    .and_then(|n| n.platforms.get(&identity.id))
                    .filter(|p| {
                        p.enabled
                            && p.mode.reverse()
                            && super::super::generation(p) == identity.generation
                    })
                    .ok_or(io::ErrorKind::PermissionDenied)?;
                Ok(principal(&identity.id, platform))
            }
            Self::Direct { headers, principal } => {
                let settings = config.admin.as_ref().ok_or(io::ErrorKind::NotConnected)?;
                let found = admin
                    .auth
                    .authenticate(settings, headers)
                    .or_else(|| super::super::authenticate(config.node.as_ref(), headers, None))
                    .ok_or(io::ErrorKind::PermissionDenied)?;
                if &found.principal != principal {
                    return Err(io::ErrorKind::PermissionDenied.into());
                }
                Ok(found.principal)
            }
        }
    }
    pub(super) fn headers(&self) -> http::HeaderMap {
        match self {
            Self::Direct { headers, .. } => headers.clone(),
            Self::Reverse { config, .. } => {
                let mut headers = http::HeaderMap::new();
                let mut value =
                    http::HeaderValue::from_str(&format!("Bearer {}", config.token.expose()))
                        .expect("validated token");
                value.set_sensitive(true);
                headers.insert(http::header::AUTHORIZATION, value);
                headers
            }
        }
    }
    pub(super) fn heartbeat(&self) -> Duration {
        match self {
            Self::Reverse { config, .. } => config.heartbeat,
            Self::Direct { .. } => Duration::from_secs(15),
        }
    }
    pub(super) fn note(&self, node: &Node, edit: impl FnOnce(&mut super::super::ReverseStatus)) {
        if let Self::Reverse { identity, .. } = self {
            node.note(&identity.id, identity.generation, edit);
        }
    }
}
