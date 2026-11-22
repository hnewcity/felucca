//! Mutual-challenge authentication for a tunnel connection.
//!
//! The verify key is a shared secret, and it never crosses the wire. The server
//! sends a random nonce; the agent answers with an HMAC over a *transcript* of
//! the handshake, and the server answers in turn with a proof of its own.
//! Because the transcript covers the negotiated version, the nonce, the key id,
//! the timestamp and the connection's role, a captured proof cannot be replayed
//! on another connection, nor reused to negotiate a weaker protocol version than
//! the one it was made for.
//!
//! Authentication runs both ways on purpose. If only the agent proved itself,
//! anything in the network path could accept the agent's connection, answer with
//! a success it is in no position to grant, and then read the whole tunnel — the
//! agent would have authenticated to an impostor. Because both proofs are keyed
//! on the same secret, an endpoint that cannot produce one cannot fake either.
//!
//! The Go original hashes a timestamp and the key together and leans on a clock
//! window to bound replay (`bridge/handshake.go`). A per-connection nonce is
//! strictly stronger: it makes replay impossible rather than merely brief, which
//! also means a client with a skewed clock is a nuisance instead of a hole.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use crate::message::{
    Auth, ConnectionRole, MAC_LEN, MIN_PROTOCOL_VERSION, NONCE_LEN, PROTOCOL_VERSION,
};

type HmacSha256 = Hmac<Sha256>;

/// Domain-separation label, so a MAC computed here can never be confused with
/// one computed for another purpose under the same key.
const TRANSCRIPT_LABEL: &[u8] = b"felucca-handshake-v1";

/// Which side produced a proof.
///
/// Mixed into the MAC so the two directions are not interchangeable. Without it
/// an impostor could reflect the agent's own proof back as its answer and pass
/// for a server that knows the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The agent proving it holds the key.
    ClientProof,
    /// The server proving the same.
    ServerProof,
}

impl Direction {
    fn tag(self) -> u8 {
        match self {
            Self::ClientProof => 0x01,
            Self::ServerProof => 0x02,
        }
    }
}

/// How far an agent's clock may differ from the server's, when the server
/// enforces the check at all.
///
/// The nonce already prevents replay, so this window exists only to catch
/// grossly misconfigured clocks; it can be generous.
pub const MAX_CLOCK_SKEW_SECS: i64 = 300;

/// Failure during the handshake.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HandshakeError {
    #[error(
        "no protocol version in common: peer offers {peer_min}..={peer_max}, we speak {ours_min}..={ours_max}"
    )]
    VersionMismatch {
        peer_min: u16,
        peer_max: u16,
        ours_min: u16,
        ours_max: u16,
    },

    #[error("authentication failed")]
    BadMac,

    #[error("unknown key id")]
    UnknownKey,

    #[error("peer clock is off by {skew_secs}s, more than the permitted {MAX_CLOCK_SKEW_SECS}s")]
    ClockSkew { skew_secs: i64 },
}

/// Everything both sides must agree on for a MAC to verify.
///
/// Constructed identically on each side; any disagreement — a different
/// negotiated version, a tampered nonce, a changed role — yields a different MAC
/// and fails the handshake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript<'a> {
    /// The version the server selected, not the one either side offered.
    pub protocol: u16,
    pub nonce: &'a [u8; NONCE_LEN],
    pub key_id: &'a str,
    pub timestamp: i64,
    pub role: ConnectionRole,
}

impl Transcript<'_> {
    /// Serializes the transcript unambiguously.
    ///
    /// The variable-length field carries an explicit length, so no two distinct
    /// transcripts can encode to the same bytes — without that, a key id ending
    /// in digits could collide with a different id and timestamp.
    fn encode(&self, direction: Direction) -> Vec<u8> {
        let key = self.key_id.as_bytes();
        let mut buf = Vec::with_capacity(TRANSCRIPT_LABEL.len() + NONCE_LEN + key.len() + 25);
        buf.extend_from_slice(TRANSCRIPT_LABEL);
        buf.push(direction.tag());
        buf.extend_from_slice(&self.protocol.to_be_bytes());
        buf.extend_from_slice(self.nonce);
        buf.extend_from_slice(&(key.len() as u64).to_be_bytes());
        buf.extend_from_slice(key);
        buf.extend_from_slice(&self.timestamp.to_be_bytes());
        buf.push(match self.role {
            ConnectionRole::Tunnel => 1,
            ConnectionRole::ConfigPush => 2,
        });
        buf
    }
}

/// Computes one side's proof for `transcript` under `vkey`.
pub fn compute_mac(
    vkey: &[u8],
    transcript: &Transcript<'_>,
    direction: Direction,
) -> [u8; MAC_LEN] {
    // HMAC accepts a key of any length, so this cannot fail.
    let mut mac = HmacSha256::new_from_slice(vkey).expect("HMAC accepts any key length");
    mac.update(&transcript.encode(direction));
    mac.finalize().into_bytes().into()
}

/// Checks an agent's proof.
///
/// Comparison is constant-time: a timing-variable check would leak the expected
/// MAC one byte at a time to a peer that can retry.
pub fn verify_mac(
    vkey: &[u8],
    transcript: &Transcript<'_>,
    direction: Direction,
    presented: &[u8; MAC_LEN],
) -> Result<(), HandshakeError> {
    let mut mac = HmacSha256::new_from_slice(vkey).expect("HMAC accepts any key length");
    mac.update(&transcript.encode(direction));
    mac.verify_slice(presented)
        .map_err(|_| HandshakeError::BadMac)
}

/// Picks the highest version both sides speak.
///
/// Taking the minimum of the two maxima is what lets a newer server keep
/// serving older agents: it steps down to their ceiling rather than refusing.
pub fn negotiate_version(peer_max: u16, peer_min: u16) -> Result<u16, HandshakeError> {
    let chosen = peer_max.min(PROTOCOL_VERSION);
    if chosen < peer_min || chosen < MIN_PROTOCOL_VERSION {
        return Err(HandshakeError::VersionMismatch {
            peer_min,
            peer_max,
            ours_min: MIN_PROTOCOL_VERSION,
            ours_max: PROTOCOL_VERSION,
        });
    }
    Ok(chosen)
}

/// Rejects a timestamp too far from the server's clock, in either direction.
pub fn validate_timestamp(peer: i64, now: i64) -> Result<(), HandshakeError> {
    let skew = now.saturating_sub(peer);
    if skew.abs() > MAX_CLOCK_SKEW_SECS {
        return Err(HandshakeError::ClockSkew { skew_secs: skew });
    }
    Ok(())
}

/// Generates a fresh challenge nonce.
pub fn generate_nonce() -> [u8; NONCE_LEN] {
    use rand::Rng;
    let mut nonce = [0u8; NONCE_LEN];
    rand::rng().fill_bytes(&mut nonce);
    nonce
}

/// Builds the [`Auth`] message proving possession of `vkey`.
pub fn build_auth(
    vkey: &[u8],
    key_id: &str,
    protocol: u16,
    nonce: &[u8; NONCE_LEN],
    role: ConnectionRole,
    timestamp: i64,
) -> Auth {
    let transcript = Transcript {
        protocol,
        nonce,
        key_id,
        timestamp,
        role,
    };
    Auth {
        key_id: key_id.to_owned(),
        timestamp,
        mac: compute_mac(vkey, &transcript, Direction::ClientProof),
    }
}

/// The proof a server returns once it has accepted an agent.
pub fn build_server_proof(vkey: &[u8], transcript: &Transcript<'_>) -> [u8; MAC_LEN] {
    compute_mac(vkey, transcript, Direction::ServerProof)
}

/// Checks the server's proof. An agent must call this before sending traffic.
pub fn verify_server_proof(
    vkey: &[u8],
    transcript: &Transcript<'_>,
    presented: &[u8; MAC_LEN],
) -> Result<(), HandshakeError> {
    verify_mac(vkey, transcript, Direction::ServerProof, presented)
}

fn result_mac(
    vkey: &[u8],
    transcript: &Transcript<'_>,
    client_id: u32,
    keepalive: u32,
) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(vkey).expect("HMAC accepts any key length");
    mac.update(&transcript.encode(Direction::ServerProof));
    mac.update(b"accepted-v3");
    mac.update(&client_id.to_be_bytes());
    mac.update(&keepalive.to_be_bytes());
    mac
}

pub fn build_server_result_proof(
    vkey: &[u8],
    transcript: &Transcript<'_>,
    client_id: u32,
    keepalive: u32,
) -> [u8; MAC_LEN] {
    result_mac(vkey, transcript, client_id, keepalive)
        .finalize()
        .into_bytes()
        .into()
}

pub fn verify_server_result_proof(
    vkey: &[u8],
    transcript: &Transcript<'_>,
    client_id: u32,
    keepalive: u32,
    presented: &[u8; MAC_LEN],
) -> Result<(), HandshakeError> {
    result_mac(vkey, transcript, client_id, keepalive)
        .verify_slice(presented)
        .map_err(|_| HandshakeError::BadMac)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VKEY: &[u8] = b"correct horse battery staple";
    const NONCE: [u8; NONCE_LEN] = [0x5a; NONCE_LEN];

    #[test]
    fn negotiated_server_metadata_cannot_be_changed_in_transit() {
        let transcript = transcript(&NONCE, "agent");
        let proof = build_server_result_proof(VKEY, &transcript, 42, 15);
        assert!(verify_server_result_proof(VKEY, &transcript, 42, 15, &proof).is_ok());
        assert!(verify_server_result_proof(VKEY, &transcript, 42, 1, &proof).is_err());
        assert!(verify_server_result_proof(VKEY, &transcript, 99, 15, &proof).is_err());
        let old = build_server_proof(VKEY, &transcript);
        assert!(verify_server_result_proof(VKEY, &transcript, 42, 15, &old).is_err());
    }

    fn transcript<'a>(nonce: &'a [u8; NONCE_LEN], key_id: &'a str) -> Transcript<'a> {
        Transcript {
            protocol: PROTOCOL_VERSION,
            nonce,
            key_id,
            timestamp: 1_700_000_000,
            role: ConnectionRole::Tunnel,
        }
    }

    #[test]
    fn a_matching_key_verifies() {
        let t = transcript(&NONCE, "client-1");
        let mac = compute_mac(VKEY, &t, Direction::ClientProof);
        assert!(verify_mac(VKEY, &t, Direction::ClientProof, &mac).is_ok());
    }

    #[test]
    fn a_wrong_key_is_rejected() {
        let t = transcript(&NONCE, "client-1");
        let mac = compute_mac(VKEY, &t, Direction::ClientProof);
        assert_eq!(
            verify_mac(b"wrong key", &t, Direction::ClientProof, &mac),
            Err(HandshakeError::BadMac)
        );
    }

    #[test]
    fn a_proof_does_not_transfer_to_another_nonce() {
        // The replay defence: a MAC captured on one connection is useless on the
        // next, because the server issues a fresh nonce each time.
        let t = transcript(&NONCE, "client-1");
        let mac = compute_mac(VKEY, &t, Direction::ClientProof);

        let other = [0xa5; NONCE_LEN];
        let t2 = transcript(&other, "client-1");
        assert_eq!(
            verify_mac(VKEY, &t2, Direction::ClientProof, &mac),
            Err(HandshakeError::BadMac)
        );
    }

    #[test]
    fn a_proof_is_bound_to_the_negotiated_version() {
        // Without this, a peer could authenticate at v2 and replay onto v1.
        let t = transcript(&NONCE, "client-1");
        let mac = compute_mac(VKEY, &t, Direction::ClientProof);

        let downgraded = Transcript {
            protocol: t.protocol.wrapping_add(1),
            ..t.clone()
        };
        assert_eq!(
            verify_mac(VKEY, &downgraded, Direction::ClientProof, &mac),
            Err(HandshakeError::BadMac)
        );
    }

    #[test]
    fn a_proof_is_bound_to_the_role_and_timestamp() {
        let t = transcript(&NONCE, "client-1");
        let mac = compute_mac(VKEY, &t, Direction::ClientProof);

        let other_role = Transcript {
            role: ConnectionRole::ConfigPush,
            ..t.clone()
        };
        assert_eq!(
            verify_mac(VKEY, &other_role, Direction::ClientProof, &mac),
            Err(HandshakeError::BadMac)
        );

        let other_ts = Transcript {
            timestamp: t.timestamp + 1,
            ..t.clone()
        };
        assert_eq!(
            verify_mac(VKEY, &other_ts, Direction::ClientProof, &mac),
            Err(HandshakeError::BadMac)
        );
    }

    #[test]
    fn a_proof_is_bound_to_the_key_id() {
        let t = transcript(&NONCE, "client-1");
        let mac = compute_mac(VKEY, &t, Direction::ClientProof);
        let t2 = transcript(&NONCE, "client-2");
        assert_eq!(
            verify_mac(VKEY, &t2, Direction::ClientProof, &mac),
            Err(HandshakeError::BadMac)
        );
    }

    #[test]
    fn the_key_id_length_prefix_prevents_field_confusion() {
        // "ab" + ts and "a" + shifted bytes must not encode alike.
        let a = Transcript {
            key_id: "ab",
            ..transcript(&NONCE, "ab")
        };
        let b = Transcript {
            key_id: "a",
            ..transcript(&NONCE, "a")
        };
        assert_ne!(
            compute_mac(VKEY, &a, Direction::ClientProof),
            compute_mac(VKEY, &b, Direction::ClientProof)
        );
    }

    #[test]
    fn build_auth_produces_a_verifiable_proof() {
        let auth = build_auth(
            VKEY,
            "client-1",
            PROTOCOL_VERSION,
            &NONCE,
            ConnectionRole::Tunnel,
            42,
        );
        assert_eq!(auth.key_id, "client-1");
        assert_eq!(auth.timestamp, 42);

        let t = Transcript {
            protocol: PROTOCOL_VERSION,
            nonce: &NONCE,
            key_id: &auth.key_id,
            timestamp: auth.timestamp,
            role: ConnectionRole::Tunnel,
        };
        assert!(verify_mac(VKEY, &t, Direction::ClientProof, &auth.mac).is_ok());
    }

    #[test]
    fn the_server_proves_itself_too() {
        let t = transcript(&NONCE, "client-1");
        let proof = build_server_proof(VKEY, &t);
        assert!(verify_server_proof(VKEY, &t, &proof).is_ok());
        assert!(verify_server_proof(b"wrong key", &t, &proof).is_err());
    }

    #[test]
    fn the_two_directions_are_not_interchangeable() {
        // The reflection attack: an impostor that cannot compute a server proof
        // echoes the agent's own proof back instead. Domain separation is what
        // stops that from verifying.
        let t = transcript(&NONCE, "client-1");
        let client_proof = compute_mac(VKEY, &t, Direction::ClientProof);
        let server_proof = build_server_proof(VKEY, &t);

        assert_ne!(client_proof, server_proof);
        assert!(
            verify_server_proof(VKEY, &t, &client_proof).is_err(),
            "an agent must reject its own proof reflected back at it"
        );
        assert!(verify_mac(VKEY, &t, Direction::ClientProof, &server_proof).is_err());
    }

    #[test]
    fn the_server_proof_is_bound_to_the_same_connection() {
        let t = transcript(&NONCE, "client-1");
        let proof = build_server_proof(VKEY, &t);

        let other = [0x11; NONCE_LEN];
        let t2 = transcript(&other, "client-1");
        assert!(
            verify_server_proof(VKEY, &t2, &proof).is_err(),
            "a captured server proof must not work on the next connection"
        );
    }

    #[test]
    fn nonces_differ_between_connections() {
        let a = generate_nonce();
        let b = generate_nonce();
        assert_ne!(a, b, "a repeated nonce would reopen the replay window");
        assert_ne!(a, [0u8; NONCE_LEN], "nonce must not be all zeroes");
    }

    #[test]
    fn version_negotiation_steps_down_to_the_peer() {
        assert_eq!(
            negotiate_version(PROTOCOL_VERSION, MIN_PROTOCOL_VERSION),
            Ok(PROTOCOL_VERSION)
        );
        // A newer peer: we settle on our own ceiling.
        assert_eq!(
            negotiate_version(PROTOCOL_VERSION + 5, MIN_PROTOCOL_VERSION),
            Ok(PROTOCOL_VERSION)
        );
    }

    #[test]
    fn version_negotiation_fails_with_no_overlap() {
        // A peer that requires something newer than we speak.
        let err = negotiate_version(PROTOCOL_VERSION + 9, PROTOCOL_VERSION + 5).unwrap_err();
        assert!(
            matches!(err, HandshakeError::VersionMismatch { .. }),
            "{err}"
        );

        // A peer too old for our floor.
        if MIN_PROTOCOL_VERSION > 0 {
            let err = negotiate_version(MIN_PROTOCOL_VERSION - 1, 0).unwrap_err();
            assert!(
                matches!(err, HandshakeError::VersionMismatch { .. }),
                "{err}"
            );
        }
    }

    #[test]
    fn clock_skew_is_bounded_in_both_directions() {
        let now = 1_700_000_000;
        assert!(validate_timestamp(now, now).is_ok());
        assert!(validate_timestamp(now - MAX_CLOCK_SKEW_SECS, now).is_ok());
        assert!(validate_timestamp(now + MAX_CLOCK_SKEW_SECS, now).is_ok());

        assert!(validate_timestamp(now - MAX_CLOCK_SKEW_SECS - 1, now).is_err());
        // A clock in the future is just as wrong as one in the past.
        assert!(validate_timestamp(now + MAX_CLOCK_SKEW_SECS + 1, now).is_err());
    }

    #[test]
    fn an_extreme_timestamp_does_not_overflow() {
        // saturating_sub keeps a hostile i64::MIN from panicking in debug.
        assert!(validate_timestamp(i64::MIN, 0).is_err());
        assert!(validate_timestamp(i64::MAX, 0).is_err());
    }
}
