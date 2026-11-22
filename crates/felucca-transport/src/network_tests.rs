use super::*;
use crate::test_dns as dns;
use felucca_core::config::network::ProxyCredentials;
use tokio::net::TcpListener;

#[tokio::test]
async fn configured_dns_caches_both_families_and_falls_back_from_udp_to_tcp() {
    for truncate in [false, true] {
        let dns = dns::Dns::start(truncate).await;
        let network = Network::new(
            &NetworkConfig {
                dns: vec![dns.address],
                local_ip: Some("127.0.0.1".parse().unwrap()),
                ..Default::default()
            },
            Duration::from_secs(2),
        )
        .unwrap();
        let addresses = network.resolve("custom.test:9001").await.unwrap();
        assert!(addresses.contains(&"127.0.0.1:9001".parse().unwrap()));
        assert!(addresses.contains(&"[::1]:9001".parse().unwrap()));
        let count = dns.hits.lock().unwrap().len();
        assert_eq!(network.resolve("custom.test:9001").await.unwrap().len(), 2);
        assert_eq!(dns.hits.lock().unwrap().len(), count);
        assert_eq!(
            dns.tcp_hits.load(std::sync::atomic::Ordering::SeqCst) > 0,
            truncate
        );
    }
}

#[tokio::test]
async fn http_connect_keeps_prefetched_bytes_and_resolves_locally_only_when_requested() {
    let dns = dns::Dns::start(false).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        for remote in [true, false] {
            let (mut io, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip(), "127.0.0.1".parse::<IpAddr>().unwrap());
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(io.read_u8().await.unwrap());
            }
            let head = String::from_utf8(head).unwrap();
            if remote {
                assert!(head.starts_with("CONNECT target.test:9001 "));
            } else {
                assert!(!head.contains("target.test"));
            }
            assert!(head.contains("Proxy-Authorization: Basic dXNlcjpwYXNzd29yZA=="));
            io.write_all(b"HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 Connected\r\n\r\nprefetched")
                .await
                .unwrap();
        }
    });
    for remote_dns in [true, false] {
        let network = Network::new(
            &NetworkConfig {
                local_ip: Some("127.0.0.1".parse().unwrap()),
                dns: vec![dns.address],
                proxy: Some(UpstreamProxy {
                    kind: ProxyKind::Http,
                    server: address.to_string(),
                    auth: Some(ProxyCredentials {
                        username: "user".into(),
                        password: "password".into(),
                    }),
                    remote_dns,
                }),
                ..Default::default()
            },
            Duration::from_secs(2),
        )
        .unwrap();
        let mut io = network.bridge_tcp("target.test:9001").await.unwrap();
        let mut bytes = Vec::new();
        io.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, b"prefetched");
        assert_eq!(dns.hits.lock().unwrap().is_empty(), remote_dns);
    }
    task.await.unwrap();
    assert_eq!(dns.tcp_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn socks_proxy_authentication_cannot_downgrade_to_anonymous() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut offered = [0; 3];
        io.read_exact(&mut offered).await.unwrap();
        assert_eq!(offered, [5, 1, 2]);
        io.write_all(&[5, 0]).await.unwrap();
        let mut bytes = Vec::new();
        io.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
    });
    let network = Network::new(
        &NetworkConfig {
            proxy: Some(UpstreamProxy {
                kind: ProxyKind::Socks5,
                server: address.to_string(),
                remote_dns: true,
                auth: Some(ProxyCredentials {
                    username: "user".into(),
                    password: "secret".into(),
                }),
            }),
            ..Default::default()
        },
        Duration::from_secs(2),
    )
    .unwrap();
    let error = network.bridge_tcp("target.test:9001").await.err().unwrap();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert!(!format!("{network:?}").contains("secret"));
    task.await.unwrap();
    assert!(
        Network::connect_to(address, Some("::1".parse().unwrap()))
            .await
            .is_err()
    );
}
