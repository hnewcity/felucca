//! Authenticated one-use route tickets and bounded direct-transfer accounting.
use felucca_core::{Secret, config::payload::PayloadOptions};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::net::SocketAddr;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerRoute {
    pub tunnel_id: u32,
    pub source: SocketAddr,
    pub destination: SocketAddr,
    pub target: Option<String>,
    /// True for either a fixed UDP route or dynamic proxy UDP target.
    pub udp: bool,
    /// Routed.datagram: only dynamic proxy UDP, never a fixed UDP route.
    pub association: bool,
    pub payload: PayloadOptions,
    /// Emitted by the provider, outside metered application data.
    pub proxy_header: Vec<u8>,
}
impl PeerRoute {
    pub fn request(&self) -> crate::StreamRequest {
        crate::StreamRequest::Routed {
            tunnel_id: self.tunnel_id,
            source: self.source.ip(),
            target: self.target.clone(),
            datagram: self.association,
        }
    }
    pub fn valid(&self) -> bool {
        self.tunnel_id != 0
            && self.source.port() != 0
            && self.proxy_header.len() <= 256
            && (!self.association || self.udp && self.target.is_some())
            && (!self.udp || self.proxy_header.is_empty())
            && self.target.as_ref().is_none_or(|target| {
                target.len() <= 4096 && felucca_core::config::validate_endpoint(target).is_ok()
            })
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderOffer {
    pub id: [u8; 16],
    pub key: Secret,
    pub probe_key: Secret,
    pub broker: String,
    pub route: PeerRoute,
    pub timeout_ms: u16,
}
impl ProviderOffer {
    pub fn valid(&self) -> bool {
        self.id != [0; 16]
            && (16..=128).contains(&self.key.len())
            && (16..=128).contains(&self.probe_key.len())
            && self.broker.len() <= 4096
            && felucca_core::config::validate_endpoint(&self.broker).is_ok()
            && (100..=15000).contains(&self.timeout_ms)
            && self.route.valid()
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitorOffer {
    pub id: [u8; 16],
    pub key: Secret,
    pub probe_key: Secret,
    pub broker: String,
    pub route: PeerRoute,
    pub timeout_ms: u16,
    pub provider: SocketAddr,
    pub fingerprint: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Inlet,
    Export,
}
impl Direction {
    pub fn index(self) -> usize {
        match self {
            Self::Inlet => 0,
            Self::Export => 1,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProviderMessage {
    Prepared {
        fingerprint: Option<String>,
    },
    Connected,
    Grant {
        sequence: u64,
        direction: Direction,
        bytes: u32,
    },
    Commit {
        sequence: u64,
        direction: Direction,
        bytes: u32,
    },
    Drained,
    Finished,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BrokerMessage {
    Start { peer: SocketAddr },
    Commit,
    Granted { sequence: u64 },
    Recorded { sequence: u64 },
    Finished,
    Rejected,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisitorMessage {
    Observed,
    Connected,
    Finished,
    Abort,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofSide {
    Visitor,
    Provider,
}
fn mac(
    key: &Secret,
    id: &[u8; 16],
    route: &PeerRoute,
    hello: &[u8; 32],
    challenge: &[u8; 32],
    side: ProofSide,
) -> Hmac<Sha256> {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(b"felucca-peer-v10");
    mac.update(&[match side {
        ProofSide::Visitor => 0,
        ProofSide::Provider => 1,
    }]);
    mac.update(id);
    mac.update(hello);
    mac.update(challenge);
    let route = postcard::to_allocvec(route).expect("bounded route serializes");
    mac.update(&(route.len() as u64).to_be_bytes());
    mac.update(&route);
    mac
}
pub fn prove(
    key: &Secret,
    id: &[u8; 16],
    route: &PeerRoute,
    hello: &[u8; 32],
    challenge: &[u8; 32],
    side: ProofSide,
) -> [u8; 32] {
    mac(key, id, route, hello, challenge, side)
        .finalize()
        .into_bytes()
        .into()
}
pub fn verify(
    key: &Secret,
    id: &[u8; 16],
    route: &PeerRoute,
    hello: &[u8; 32],
    challenge: &[u8; 32],
    side: ProofSide,
    proof: &[u8; 32],
) -> bool {
    mac(key, id, route, hello, challenge, side)
        .verify_slice(proof)
        .is_ok()
}

/// The ticket is authenticated even when optional payload encryption is disabled.
pub async fn authenticate<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin>(
    io: &mut T,
    key: &Secret,
    id: &[u8; 16],
    route: &PeerRoute,
    side: ProofSide,
) -> std::io::Result<()> {
    use crate::{read_frame, write_frame};
    let bad = || std::io::Error::from(std::io::ErrorKind::PermissionDenied);
    match side {
        ProofSide::Visitor => {
            let hello = crate::handshake::generate_nonce();
            write_frame(io, &hello)
                .await
                .map_err(std::io::Error::other)?;
            let challenge = read_frame(io).await.map_err(std::io::Error::other)?;
            let proof = prove(key, id, route, &hello, &challenge, ProofSide::Visitor);
            write_frame(io, &proof)
                .await
                .map_err(std::io::Error::other)?;
            let proof = read_frame(io).await.map_err(std::io::Error::other)?;
            if !verify(
                key,
                id,
                route,
                &hello,
                &challenge,
                ProofSide::Provider,
                &proof,
            ) {
                return Err(bad());
            }
        }
        ProofSide::Provider => {
            let hello = read_frame(io).await.map_err(std::io::Error::other)?;
            let challenge = crate::handshake::generate_nonce();
            write_frame(io, &challenge)
                .await
                .map_err(std::io::Error::other)?;
            let proof = read_frame(io).await.map_err(std::io::Error::other)?;
            if !verify(
                key,
                id,
                route,
                &hello,
                &challenge,
                ProofSide::Visitor,
                &proof,
            ) {
                return Err(bad());
            }
            let proof = prove(key, id, route, &hello, &challenge, ProofSide::Provider);
            write_frame(io, &proof)
                .await
                .map_err(std::io::Error::other)?;
        }
    }
    Ok(())
}
/// One granted chunk per direction. A grant is permission, not traffic: only a
/// valid, non-replayed commit is recorded by the server's MeterGroups.
#[derive(Debug)]
pub struct CreditLedger {
    next: u64,
    pending: [Option<(u64, u32)>; 2],
}
impl Default for CreditLedger {
    fn default() -> Self {
        Self {
            next: 1,
            pending: [None, None],
        }
    }
}
impl CreditLedger {
    pub fn grant(
        &mut self,
        sequence: u64,
        direction: Direction,
        bytes: u32,
        maximum: u32,
    ) -> Result<(), &'static str> {
        if sequence != self.next || bytes > maximum || self.pending[direction.index()].is_some() {
            return Err("invalid or overlapping direct credit request");
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or("direct sequence exhausted")?;
        self.pending[direction.index()] = Some((sequence, bytes));
        Ok(())
    }
    pub fn commit(
        &mut self,
        sequence: u64,
        direction: Direction,
        bytes: u32,
    ) -> Result<(), &'static str> {
        let Some((expected, grant)) = self.pending[direction.index()] else {
            return Err("direct commit has no grant");
        };
        if sequence != expected || bytes > grant {
            return Err("direct commit exceeds its grant");
        }
        self.pending[direction.index()] = None;
        Ok(())
    }
    pub fn idle(&self) -> bool {
        self.pending.iter().all(Option::is_none)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn route() -> PeerRoute {
        PeerRoute {
            tunnel_id: 1,
            source: "127.0.0.1:1234".parse().unwrap(),
            destination: "127.0.0.1:8080".parse().unwrap(),
            target: None,
            udp: false,
            association: false,
            payload: PayloadOptions {
                compress: true,
                encrypt: true,
            },
            proxy_header: vec![],
        }
    }
    #[test]
    fn ticket_proofs_bind_direction_route_and_every_challenge() {
        let key: Secret = "one-use-peer-ticket-key".into();
        let original = route();
        let id = [1; 16];
        let hello = [2; 32];
        let challenge = [3; 32];
        let proof = prove(&key, &id, &original, &hello, &challenge, ProofSide::Visitor);
        assert!(verify(
            &key,
            &id,
            &original,
            &hello,
            &challenge,
            ProofSide::Visitor,
            &proof
        ));
        assert!(!verify(
            &key,
            &id,
            &original,
            &hello,
            &challenge,
            ProofSide::Provider,
            &proof
        ));
        assert!(!verify(
            &key,
            &[9; 16],
            &original,
            &hello,
            &challenge,
            ProofSide::Visitor,
            &proof
        ));
        assert!(!verify(
            &key,
            &id,
            &original,
            &[9; 32],
            &challenge,
            ProofSide::Visitor,
            &proof
        ));
        assert!(!verify(
            &key,
            &id,
            &original,
            &hello,
            &[9; 32],
            ProofSide::Visitor,
            &proof
        ));
        let mut changed = original.clone();
        changed.target = Some("localhost:22".into());
        assert!(!verify(
            &key,
            &id,
            &changed,
            &hello,
            &challenge,
            ProofSide::Visitor,
            &proof
        ));
        changed = original;
        changed.proxy_header = b"forged-source".to_vec();
        assert!(!verify(
            &key,
            &id,
            &changed,
            &hello,
            &challenge,
            ProofSide::Visitor,
            &proof
        ));
    }
    #[test]
    fn accounting_rejects_replay_overlapping_grants_and_overclaim() {
        let mut ledger = CreditLedger::default();
        assert!(ledger.idle());
        ledger.grant(1, Direction::Inlet, 16, 16).unwrap();
        ledger.grant(2, Direction::Export, 10, 16).unwrap();
        assert!(ledger.grant(3, Direction::Inlet, 1, 16).is_err());
        assert!(ledger.commit(1, Direction::Inlet, 17).is_err());
        assert!(ledger.commit(1, Direction::Export, 10).is_err());
        ledger.commit(1, Direction::Inlet, 12).unwrap();
        assert!(ledger.commit(1, Direction::Inlet, 12).is_err());
        assert!(ledger.grant(2, Direction::Inlet, 1, 16).is_err());
        assert!(ledger.grant(3, Direction::Inlet, 17, 16).is_err());
        ledger.grant(3, Direction::Inlet, 0, 16).unwrap();
        ledger.commit(2, Direction::Export, 10).unwrap();
        ledger.commit(3, Direction::Inlet, 0).unwrap();
        assert!(ledger.idle());
    }
}
