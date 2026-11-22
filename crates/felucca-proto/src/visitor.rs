//! Share proofs bind the stream, authenticated visitor identity and both nonces.
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitRequest {
    pub connection: felucca_core::config::peer::VisitorConnection,
    pub share: String,
    pub nonce: [u8; 32],
    pub datagram: bool,
    pub target: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitChallenge {
    pub nonce: [u8; 32],
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisitResult {
    Ok {
        proof: [u8; 32],
    },
    Direct {
        offer: Box<crate::peer::VisitorOffer>,
        proof: [u8; 32],
    },
    Err(crate::StreamReject),
}
#[derive(Debug, Clone, Copy)]
pub enum Side {
    Visitor,
    Server,
}
fn mac(
    secret: &[u8],
    request: &VisitRequest,
    challenge: &VisitChallenge,
    identity: &str,
    side: Side,
) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(b"felucca-visitor-v10");
    mac.update(&[
        match side {
            Side::Visitor => 0,
            Side::Server => 1,
        },
        u8::from(request.datagram),
        match request.connection {
            felucca_core::config::peer::VisitorConnection::Relay => 0,
            felucca_core::config::peer::VisitorConnection::PreferDirect => 1,
            felucca_core::config::peer::VisitorConnection::Direct => 2,
        },
    ]);
    for text in [request.share.as_bytes(), identity.as_bytes()] {
        mac.update(&(text.len() as u64).to_be_bytes());
        mac.update(text);
    }
    mac.update(&[u8::from(request.target.is_some())]);
    if let Some(target) = &request.target {
        mac.update(&(target.len() as u64).to_be_bytes());
        mac.update(target.as_bytes());
    }
    mac.update(&request.nonce);
    mac.update(&challenge.nonce);
    mac
}
pub fn prove(
    secret: &[u8],
    request: &VisitRequest,
    challenge: &VisitChallenge,
    identity: &str,
    side: Side,
) -> [u8; 32] {
    mac(secret, request, challenge, identity, side)
        .finalize()
        .into_bytes()
        .into()
}
pub fn verify(
    secret: &[u8],
    request: &VisitRequest,
    challenge: &VisitChallenge,
    identity: &str,
    side: Side,
    proof: &[u8; 32],
) -> bool {
    mac(secret, request, challenge, identity, side)
        .verify_slice(proof)
        .is_ok()
}

fn offer_mac(
    secret: &[u8],
    request: &VisitRequest,
    challenge: &VisitChallenge,
    identity: &str,
    offer: &crate::peer::VisitorOffer,
) -> Hmac<Sha256> {
    let mut mac = mac(secret, request, challenge, identity, Side::Server);
    mac.update(b"direct-offer");
    let data = postcard::to_allocvec(offer).expect("bounded peer offer");
    mac.update(&(data.len() as u64).to_be_bytes());
    mac.update(&data);
    mac
}
pub fn prove_offer(
    secret: &[u8],
    request: &VisitRequest,
    challenge: &VisitChallenge,
    identity: &str,
    offer: &crate::peer::VisitorOffer,
) -> [u8; 32] {
    offer_mac(secret, request, challenge, identity, offer)
        .finalize()
        .into_bytes()
        .into()
}
pub fn verify_offer(
    secret: &[u8],
    request: &VisitRequest,
    challenge: &VisitChallenge,
    identity: &str,
    offer: &crate::peer::VisitorOffer,
    proof: &[u8; 32],
) -> bool {
    offer_mac(secret, request, challenge, identity, offer)
        .verify_slice(proof)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_offer_proof_binds_certificate_ticket_keys_and_exact_route() {
        let request = VisitRequest {
            connection: felucca_core::config::peer::VisitorConnection::PreferDirect,
            share: "web".into(),
            nonce: [7; 32],
            datagram: false,
            target: None,
        };
        let challenge = VisitChallenge { nonce: [8; 32] };
        let offer = crate::peer::VisitorOffer {
            id: [1; 16],
            key: "one-use-direct-ticket".into(),
            probe_key: "visitor-probe-secret".into(),
            broker: "localhost:8025".into(),
            timeout_ms: 1000,
            provider: "127.0.0.1:9000".parse().unwrap(),
            fingerprint: "01".repeat(32),
            route: crate::peer::PeerRoute {
                tunnel_id: 1,
                source: "127.0.0.1:2000".parse().unwrap(),
                destination: "127.0.0.1:3000".parse().unwrap(),
                target: None,
                udp: false,
                association: false,
                payload: Default::default(),
                proxy_header: Vec::new(),
            },
        };
        let proof = prove_offer(b"share-secret", &request, &challenge, "guest", &offer);
        assert!(verify_offer(
            b"share-secret",
            &request,
            &challenge,
            "guest",
            &offer,
            &proof
        ));
        for change in 0..8 {
            let mut changed = offer.clone();
            match change {
                0 => changed.key = "substituted-ticket-key".into(),
                1 => changed.probe_key = "substituted-probe-key".into(),
                2 => changed.fingerprint = "02".repeat(32),
                3 => changed.provider.set_port(9001),
                4 => changed.id[0] ^= 1,
                5 => changed.route.tunnel_id = 2,
                6 => changed.route.payload.encrypt = true,
                _ => changed.route.proxy_header = b"forged-source".to_vec(),
            }
            assert!(!verify_offer(
                b"share-secret",
                &request,
                &challenge,
                "guest",
                &changed,
                &proof
            ));
        }
        assert!(!verify_offer(
            b"share-secret",
            &request,
            &challenge,
            "another-guest",
            &offer,
            &proof
        ));
        assert!(!verify(
            b"share-secret",
            &request,
            &challenge,
            "guest",
            Side::Server,
            &proof
        ));
    }
    #[test]
    fn share_proofs_cannot_be_reflected_replayed_or_retargeted() {
        let request = VisitRequest {
            connection: Default::default(),
            share: "ssh".into(),
            nonce: [1; 32],
            datagram: false,
            target: None,
        };
        let challenge = VisitChallenge { nonce: [2; 32] };
        let proof = prove(b"share-key", &request, &challenge, "visitor", Side::Visitor);
        assert!(verify(
            b"share-key",
            &request,
            &challenge,
            "visitor",
            Side::Visitor,
            &proof
        ));
        assert!(!verify(
            b"share-key",
            &request,
            &challenge,
            "visitor",
            Side::Server,
            &proof
        ));
        assert!(!verify(
            b"share-key",
            &request,
            &challenge,
            "other",
            Side::Visitor,
            &proof
        ));
        assert!(!verify(
            b"other-key",
            &request,
            &challenge,
            "visitor",
            Side::Visitor,
            &proof
        ));
        assert!(!verify(
            b"share-key",
            &request,
            &VisitChallenge { nonce: [3; 32] },
            "visitor",
            Side::Visitor,
            &proof
        ));
        for changed in [
            VisitRequest {
                share: "different".into(),
                ..request.clone()
            },
            VisitRequest {
                target: Some("localhost:80".into()),
                ..request.clone()
            },
            VisitRequest {
                datagram: true,
                ..request.clone()
            },
            VisitRequest {
                connection: felucca_core::config::peer::VisitorConnection::Direct,
                ..request.clone()
            },
            VisitRequest {
                nonce: [4; 32],
                ..request
            },
        ] {
            assert!(!verify(
                b"share-key",
                &changed,
                &challenge,
                "visitor",
                Side::Visitor,
                &proof
            ));
        }
        let response = VisitResult::Ok { proof };
        let bytes = postcard::to_allocvec(&response).unwrap();
        assert_eq!(
            postcard::from_bytes::<VisitResult>(&bytes).unwrap(),
            response
        );
    }
}
