use felucca_core::TrustPolicy;
use felucca_core::access::TargetAcl;
use felucca_transport::{BoxedConn, TransportError, Verification, network::Network, tls};
use std::{io, net::SocketAddr};

pub fn verification(policy: &TrustPolicy, address: &str) -> Verification {
    match policy {
        TrustPolicy::Webpki => Verification::WebPki {
            server_name: address
                .rsplit_once(':')
                .map_or(address, |(h, _)| h)
                .trim_matches(['[', ']'])
                .to_owned(),
        },
        TrustPolicy::Pinned { fingerprint } => Verification::Pinned {
            fingerprint: fingerprint.clone(),
        },
        TrustPolicy::Insecure => Verification::Insecure,
    }
}

pub async fn resolve(
    network: &Network,
    address: &str,
    policy: &TargetAcl,
) -> io::Result<Vec<SocketAddr>> {
    felucca_core::config::validate_endpoint(address).map_err(io::Error::other)?;
    let host = address
        .rsplit_once(':')
        .ok_or_else(|| io::Error::other("missing port"))?
        .0;
    let addresses: Vec<_> = network
        .resolve(address)
        .await?
        .into_iter()
        .filter(|address| policy.permits(host, *address))
        .filter(|address| {
            network
                .backend_local_ip()
                .is_none_or(|ip| ip.is_ipv4() == address.is_ipv4())
        })
        .collect();
    if addresses.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "destination denied or has no permitted address",
        ));
    }
    Ok(addresses)
}

pub async fn connect_protocol(
    network: &Network,
    address: &str,
    encrypted: bool,
    trust: &TrustPolicy,
    policy: &TargetAcl,
    http2: bool,
) -> Result<BoxedConn, TransportError> {
    let addresses = resolve(network, address, policy).await?;
    let host = address
        .rsplit_once(':')
        .expect("validated endpoint")
        .0
        .trim_matches(['[', ']']);
    let verification = verification(trust, address);
    let mut last = TransportError::Io(io::Error::other("no destination address"));
    let count = addresses.len();
    for (index, address) in addresses.into_iter().enumerate() {
        let io = match Network::connect_candidate(
            address,
            network.backend_local_ip(),
            index + 1 == count,
        )
        .await
        {
            Ok(io) => io,
            Err(error) => {
                last = error.into();
                continue;
            }
        };
        let attempt: Result<BoxedConn, TransportError> = if encrypted {
            tls::handshake(io, host, &verification, if http2 { &[b"h2"] } else { &[] })
                .await
                .map(|io| Box::new(io) as BoxedConn)
        } else {
            Ok(Box::new(io) as BoxedConn)
        };
        match attempt {
            Ok(io) => return Ok(io),
            Err(error) => last = error,
        }
    }
    Err(last)
}
