//! Opaque sessions and bounded password verification.
use super::credentials;
use felucca_core::{Secret, config::AdminConfig};
use parking_lot::Mutex;
use std::{collections::HashMap, net::IpAddr, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::time::Instant;

const COOKIE: &str = "felucca_session";
const DUMMY: &str = "pbkdf2-sha256$600000$00000000000000000000000000000000$0000000000000000000000000000000000000000000000000000000000000000";

pub(crate) use crate::control::Principal;

const MAX_ATTEMPT_SOURCES: usize = 4096;
const MAX_ACCOUNT_SESSIONS: usize = 32;

/// Rate-limits IPv6 by /64, the block a single host or site normally holds,
/// so rotating addresses inside it does not reset the login allowance.
fn attempt_key(peer: IpAddr) -> IpAddr {
    match peer.to_canonical() {
        IpAddr::V6(address) => {
            let mut segments = address.segments();
            segments[4..].fill(0);
            IpAddr::V6(segments.into())
        }
        address => address,
    }
}
#[derive(Clone)]
struct Session {
    account: String,
    credential: [u8; 32],
    csrf: String,
    expires: Instant,
}
struct Attempts {
    started: Instant,
    count: u32,
}
pub struct AuthState {
    sessions: Mutex<HashMap<[u8; 32], Session>>,
    attempts: Mutex<HashMap<IpAddr, Attempts>>,
    workers: Arc<tokio::sync::Semaphore>,
}
pub struct Authenticated {
    pub principal: Principal,
    pub csrf: Option<String>,
}
pub struct Issued {
    pub principal: Principal,
    pub csrf: String,
    pub cookie: String,
}
#[derive(Debug, PartialEq, Eq)]
pub enum LoginError {
    BadCredentials,
    RateLimited,
    Unavailable,
}

impl Default for AuthState {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthState {
    pub fn new() -> Self {
        Self {
            sessions: Default::default(),
            attempts: Default::default(),
            workers: Arc::new(tokio::sync::Semaphore::new(8)),
        }
    }
    pub(crate) fn password_workers(&self) -> Arc<tokio::sync::Semaphore> {
        self.workers.clone()
    }
    fn admit(&self, peer: IpAddr) -> bool {
        let now = Instant::now();
        let peer = attempt_key(peer);
        let mut attempts = self.attempts.lock();
        attempts.retain(|_, a| now.duration_since(a.started) < Duration::from_secs(60));
        // When full, evict the oldest window instead of refusing every new
        // address: refusing let anyone with enough addresses lock the real
        // administrator out of the console.
        if !attempts.contains_key(&peer)
            && attempts.len() >= MAX_ATTEMPT_SOURCES
            && let Some(oldest) = attempts
                .iter()
                .min_by_key(|(_, a)| a.started)
                .map(|(source, _)| *source)
        {
            attempts.remove(&oldest);
        }
        let attempts = attempts.entry(peer).or_insert(Attempts {
            started: now,
            count: 0,
        });
        attempts.count = attempts.count.saturating_add(1);
        attempts.count <= 10
    }
    pub async fn login(
        &self,
        config: &AdminConfig,
        name: String,
        password: Secret,
        peer: IpAddr,
    ) -> Result<Issued, LoginError> {
        if !self.admit(peer) {
            return Err(LoginError::RateLimited);
        }
        if name.len() > 128 || password.len() > 4096 {
            return Err(LoginError::BadCredentials);
        }
        let slot = self
            .workers
            .clone()
            .try_acquire_owned()
            .map_err(|_| LoginError::RateLimited)?;
        let account = config.accounts.get(&name).filter(|a| !a.disabled).cloned();
        let encoded = account
            .as_ref()
            .map_or(DUMMY, |a| a.password_hash.expose())
            .to_owned();
        let credential = credentials::digest(encoded.as_bytes());
        let valid = tokio::task::spawn_blocking(move || {
            // Cancellation of the HTTP request must not release capacity while
            // its CPU-bound password verification is still running.
            let _slot = slot;
            credentials::verify_password(&encoded, password.as_bytes())
        })
        .await
        .map_err(|_| LoginError::Unavailable)?;
        let account = account
            .filter(|_| valid)
            .ok_or(LoginError::BadCredentials)?;
        let principal = Principal {
            account: name.clone(),
            role: account.role,
            user: account.user,
        };
        let token = credentials::token();
        let csrf = credentials::token();
        let now = Instant::now();
        let mut sessions = self.sessions.lock();
        sessions.retain(|_, session| session.expires > now);
        // One account replaces its own oldest session at its share of the
        // table. Otherwise a script that logs in and never logs out, or any
        // viewer account, fills the table and blocks every login for a TTL.
        let per_account = MAX_ACCOUNT_SESSIONS.min(config.max_sessions);
        let own: Vec<_> = sessions
            .iter()
            .filter(|(_, session)| session.account == name)
            .map(|(key, session)| (*key, session.expires))
            .collect();
        if own.len() >= per_account
            && let Some((oldest, _)) = own.iter().min_by_key(|(_, expires)| *expires)
        {
            sessions.remove(oldest);
        }
        if sessions.len() >= config.max_sessions {
            return Err(LoginError::RateLimited);
        }
        sessions.insert(
            credentials::digest(token.as_bytes()),
            Session {
                account: name,
                credential,
                csrf: csrf.clone(),
                expires: now + config.session_ttl,
            },
        );
        let secure = if config.secure() { "; Secure" } else { "" };
        Ok(Issued {
            principal,
            csrf,
            cookie: format!(
                "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
                config.session_ttl.as_secs(),
                secure
            ),
        })
    }
    pub fn authenticate(
        &self,
        config: &AdminConfig,
        headers: &http::HeaderMap,
    ) -> Option<Authenticated> {
        if headers.contains_key(http::header::AUTHORIZATION) {
            if headers.get_all(http::header::AUTHORIZATION).iter().count() != 1 {
                return None;
            }
            let token = headers
                .get(http::header::AUTHORIZATION)?
                .to_str()
                .ok()?
                .strip_prefix("Bearer ")?;
            if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            let supplied = credentials::digest(token.as_bytes());
            let (name, account) = config.accounts.iter().find(|(_, account)| {
                !account.disabled
                    && account
                        .token_hash
                        .as_ref()
                        .and_then(|hash| credentials::unhex::<32>(hash.expose()))
                        .is_some_and(|expected| bool::from(supplied.ct_eq(&expected)))
            })?;
            return Some(Authenticated {
                principal: Principal {
                    account: name.clone(),
                    role: account.role,
                    user: account.user.clone(),
                },
                csrf: None,
            });
        }
        let token = cookie(headers)?;
        let digest = credentials::digest(token.as_bytes());
        let mut sessions = self.sessions.lock();
        let session = sessions.get(&digest)?;
        let account = config
            .accounts
            .get(&session.account)
            .filter(|a| !a.disabled);
        let account = account.filter(|account| {
            session.expires > Instant::now()
                && bool::from(
                    session
                        .credential
                        .ct_eq(&credentials::digest(account.password_hash.as_bytes())),
                )
        });
        let Some(account) = account else {
            sessions.remove(&digest);
            return None;
        };
        Some(Authenticated {
            principal: Principal {
                account: session.account.clone(),
                role: account.role,
                user: account.user.clone(),
            },
            csrf: Some(session.csrf.clone()),
        })
    }
    pub fn logout(&self, headers: &http::HeaderMap) {
        if let Some(token) = cookie(headers) {
            self.sessions
                .lock()
                .remove(&credentials::digest(token.as_bytes()));
        }
    }
}
impl Authenticated {
    pub fn csrf_valid(&self, headers: &http::HeaderMap) -> bool {
        let Some(expected) = &self.csrf else {
            return true;
        };
        if headers.get_all("x-felucca-csrf").iter().count() != 1 {
            return false;
        }
        headers
            .get("x-felucca-csrf")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|supplied| {
                bool::from(
                    credentials::digest(supplied.as_bytes())
                        .ct_eq(&credentials::digest(expected.as_bytes())),
                )
            })
    }
}
fn cookie(headers: &http::HeaderMap) -> Option<String> {
    let mut token = None;
    for header in headers.get_all(http::header::COOKIE) {
        for item in header.to_str().ok()?.split(';') {
            let Some((name, value)) = item.trim().split_once('=') else {
                continue;
            };
            if name == COOKIE {
                if token.is_some()
                    || value.len() != 64
                    || !value.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return None;
                }
                token = Some(value.to_owned());
            }
        }
    }
    token
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv6_sources_share_a_slash_64_and_ipv4_stays_per_address() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::2".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(attempt_key(a), attempt_key(b));
        assert_ne!(attempt_key(a), attempt_key(c));
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(attempt_key(mapped), "192.0.2.7".parse::<IpAddr>().unwrap());
    }

    #[tokio::test]
    async fn a_full_attempt_table_still_admits_a_new_source() {
        let auth = AuthState::new();
        for index in 0..MAX_ATTEMPT_SOURCES as u32 {
            assert!(auth.admit(IpAddr::from(index.to_be_bytes())));
        }
        assert!(auth.admit("198.51.100.1".parse().unwrap()));
        assert_eq!(auth.attempts.lock().len(), MAX_ATTEMPT_SOURCES);
    }

    #[tokio::test]
    async fn rotating_inside_one_slash_64_keeps_one_allowance() {
        let auth = AuthState::new();
        let admitted = (0..20u16)
            .filter(|i| auth.admit(IpAddr::from([0x2001, 0xdb8, 0, 0, *i, 0, 0, 1])))
            .count();
        assert_eq!(admitted, 10);
    }
}
