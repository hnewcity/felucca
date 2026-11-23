use super::*;

struct LocalServer(Arc<Server>);
impl Drop for LocalServer {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}
async fn local_config(text: &str) -> LocalServer {
    let config = toml::from_str(&format!("[bridge]\nbind='127.0.0.1:0'\n{text}")).unwrap();
    let server = Arc::new(Server::new(config).unwrap());
    server.clone().run().await.unwrap();
    LocalServer(server)
}
fn route(id: u32, port: u16, target: &str) -> String {
    format!(
        "\n[[local.office.tunnel]]\nid={id}\nmode='tcp'\nport={port}\nbind='127.0.0.1'\nbackend=[{{addr='{target}'}}]\n"
    )
}
async fn refused(port: u16) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let _ = stream.write_all(b"must not pass").await;
    let mut data = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), stream.read_to_end(&mut data))
        .await
        .expect("rejection timed out");
    assert!(data.is_empty(), "rejected route returned data");
}

#[tokio::test]
async fn local_services_run_without_an_agent_and_preserve_half_close_udp_and_meters() {
    let tcp = free_port().await;
    let udp = free_port().await;
    let backend = echo().await;
    let udp_backend = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let address = udp_backend.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut bytes = vec![0; 65536];
        while let Ok((n, peer)) = udp_backend.recv_from(&mut bytes).await {
            udp_backend.send_to(&bytes[..n], peer).await.unwrap();
        }
    });
    let server = local_config(&format!(
        r#"
[user.team]
[local.office]
user="team"
network={{local_ip="127.0.0.1", forward_local_ip=true}}
{}
[local.office.tunnel.options.payload]
compress=true
encrypt=true
[[local.office.tunnel]]
id=2
mode="udp"
port={udp}
bind="127.0.0.1"
backend=[{{addr="{address}"}}]
[local.office.tunnel.options.payload]
compress=true
encrypt=true
"#,
        route(1, tcp, &format!("127.0.0.1:{backend}"))
    ))
    .await;
    assert_eq!(server.0.registry().len(), 1);
    let mut stream = TcpStream::connect(("127.0.0.1", tcp)).await.unwrap();
    let payload = vec![42; 2 * 1024 * 1024];
    let received = tokio::time::timeout(Duration::from_secs(10), async {
        let (mut read, mut write) = stream.split();
        let send = async {
            write.write_all(&payload).await.unwrap();
            write.shutdown().await.unwrap();
        };
        let read = async {
            let mut data = Vec::new();
            read.read_to_end(&mut data).await.unwrap();
            data
        };
        tokio::join!(send, read).1
    })
    .await
    .unwrap();
    assert_eq!(received, payload);
    let socket = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    socket.connect(("127.0.0.1", udp)).await.unwrap();
    let mut buffer = vec![0; 65536];
    for len in [0, 1, 50000] {
        socket.send(&vec![7; len]).await.unwrap();
        let n = tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buffer[..n], vec![7; len]);
    }
    server.0.shutdown_and_wait().await.unwrap();
    let client = server
        .0
        .registry()
        .store
        .stats(&felucca_server::resources::ResourceKey::Client {
            name: "local.office".into(),
        })
        .unwrap();
    assert_eq!(client.inlet, (payload.len() + 50001) as u64);
    assert_eq!(client.inlet, client.export);
    task.abort();
    TcpListener::bind(("127.0.0.1", tcp)).await.unwrap();
}

#[tokio::test]
async fn nested_local_routes_preserve_proxy_source_and_reject_cycles_and_cross_tenant_access() {
    let ports = [
        free_port().await,
        free_port().await,
        free_port().await,
        free_port().await,
    ];
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n") {
            header.push(io.read_u8().await.unwrap());
        }
        assert!(
            String::from_utf8(header)
                .unwrap()
                .starts_with("PROXY TCP4 198.51.100.7 127.0.0.1 4321 ")
        );
        let (mut r, mut w) = io.split();
        tokio::io::copy(&mut r, &mut w).await.unwrap();
    });
    let server = local_config(&format!(
        r#"
[user.alice]
[user.bob]
[local.office]
user="alice"
{}
[local.office.tunnel.options.proxy_accept]
mode="required"
trusted=["127.0.0.1"]
{}
acl_mode="allow"
acl_rules="198.51.100.7"
[local.office.tunnel.options]
proxy_protocol="v1"
{}
[[local.other.tunnel]]
id=1
mode="tcp"
port={}
bind="127.0.0.1"
backend=[{{addr="{backend}"}}]
[local.other]
user="bob"
"#,
        route(1, ports[0], "tunnel://local.office/2"),
        route(2, ports[1], &backend.to_string()),
        route(3, ports[2], "tunnel://local.other/1"),
        ports[3]
    ))
    .await;
    let mut stream = TcpStream::connect(("127.0.0.1", ports[0])).await.unwrap();
    stream
        .write_all(format!("PROXY TCP4 198.51.100.7 127.0.0.1 4321 {}\r\n", ports[0]).as_bytes())
        .await
        .unwrap();
    roundtrip(stream).await;
    task.await.unwrap();
    refused(ports[2]).await;
    server.0.shutdown_and_wait().await.unwrap();
    let cycle = local_config(
        &(route(1, ports[0], "tunnel://local.office/2")
            + &route(2, ports[1], "tunnel://local.office/1")),
    )
    .await;
    refused(ports[0]).await;
    tokio::time::timeout(Duration::from_secs(2), async {
        while cycle
            .0
            .registry()
            .get("local.office")
            .unwrap()
            .live_connections()
            != 0
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn nested_file_routes_retain_authentication_and_cannot_impersonate_another_site() {
    let files = identity("local-file.test").await;
    std::fs::write(files.root.join("index.html"), "confined").unwrap();
    let http = free_port().await;
    let proxy = free_port().await;
    let server = local_config(&format!(
        r#"
[http]
bind="127.0.0.1:{http}"
tls_bind=""
{}
[[local.office.tunnel]]
id=2
mode="file"
port={http}
hosts=["files.test"]
[local.office.tunnel.file]
root='{}'
read_only=true
[local.office.tunnel.file.auth.users]
alice="password"
"#,
        route(1, proxy, "file://local.office/2"),
        files.root.display()
    ))
    .await;
    for (host, auth, expected) in [
        ("files.test", "", 401),
        (
            "files.test",
            "Authorization: Basic YWxpY2U6cGFzc3dvcmQ=\r\n",
            200,
        ),
        (
            "other.test",
            "Authorization: Basic YWxpY2U6cGFzc3dvcmQ=\r\n",
            404,
        ),
    ] {
        let response = http_request(
            proxy,
            &format!("GET /index.html HTTP/1.1\r\nHost: {host}\r\n{auth}Connection: close\r\n\r\n"),
        )
        .await;
        assert!(
            response.starts_with(&format!("HTTP/1.1 {expected}")),
            "{response}"
        );
        if expected == 200 {
            assert!(response.ends_with("confined"));
        }
    }
    server.0.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn nested_routes_reach_remote_agent_and_disconnect_releases_resources() {
    let public = free_port().await;
    let remote = free_port().await;
    let mut config = server_config();
    config.locals = toml::from_str::<ServerConfig>(&route(1, public, "tunnel://features/1"))
        .unwrap()
        .locals;
    let agent = agent(&format!(
        "[[tunnel]]\nid=1\nmode='tcp'\nport={remote}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{}'}}]",
        echo().await
    ));
    let running = start(config, agent, TransportKind::Quic).await;
    roundtrip(TcpStream::connect(("127.0.0.1", public)).await.unwrap()).await;
    for key in ["local.office", "features"] {
        tokio::time::timeout(Duration::from_secs(2), async {
            while running
                .server
                .registry()
                .get(key)
                .unwrap()
                .live_connections()
                != 0
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            running.server.registry().get(key).unwrap().traffic.total(),
            10
        );
    }
    running.agent.shutdown();
    running.agent_task.abort();
    running.server.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn bridge_aliases_authenticate_all_four_stream_transports_and_retire_cancelled_sessions() {
    for transport in [
        TransportKind::Tcp,
        TransportKind::Tls,
        TransportKind::Ws,
        TransportKind::Wss,
    ] {
        let bridge = free_port().await;
        let public = free_port().await;
        let mut config = server_config();
        config.locals = toml::from_str::<ServerConfig>(&route(
            1,
            bridge,
            &format!("bridge://{}", transport.as_str()),
        ))
        .unwrap()
        .locals;
        let server = LocalServer(Arc::new(Server::new(config).unwrap()));
        server.0.clone().run().await.unwrap();
        let mut config = agent(&format!(
            "[[tunnel]]\nid=1\nmode='tcp'\nport={public}\nbind='127.0.0.1'\nbackend=[{{addr='127.0.0.1:{}'}}]",
            echo().await
        ));
        config.server = format!("127.0.0.1:{bridge}");
        config.transport = transport;
        config.trust = TrustPolicy::Pinned {
            fingerprint: server.0.fingerprint(),
        };
        let agent = Arc::new(Agent::new(config).unwrap());
        let task = tokio::spawn(agent.clone().run());
        tokio::time::timeout(Duration::from_secs(5), async {
            while !agent.is_ready() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("nested {transport:?} bridge did not authenticate"));
        roundtrip(TcpStream::connect(("127.0.0.1", public)).await.unwrap()).await;
        // Cancels the inner authenticated session future when the outer local
        // route disappears; its listener and registry entry must still retire.
        server
            .0
            .registry()
            .get("local.office")
            .unwrap()
            .cancel
            .cancel();
        agent.shutdown();
        task.abort();
        tokio::time::timeout(Duration::from_secs(5), async {
            while server.0.registry().get("features").is_some() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("nested bridge cancellation leaked its authenticated client");
        TcpListener::bind(("127.0.0.1", public)).await.unwrap();
        server.0.shutdown_and_wait().await.unwrap();
    }
}

#[tokio::test]
async fn nested_udp_preserves_packets_and_counts_payload_without_framing() {
    let first = free_port().await;
    let second = free_port().await;
    let backend = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    let address = backend.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut buf = vec![0; 65536];
        while let Ok((n, peer)) = backend.recv_from(&mut buf).await {
            backend.send_to(&buf[..n], peer).await.unwrap();
        }
    });
    let text = (route(1, first, "tunnel://local.office/2")
        + &route(2, second, &address.to_string()))
        .replace("mode='tcp'", "mode='udp'");
    let server = local_config(&text).await;
    let socket = felucca_transport::udp::bind("127.0.0.1:0").await.unwrap();
    socket.connect(("127.0.0.1", first)).await.unwrap();
    for n in [0, 33000, 1] {
        socket.send(&vec![8; n]).await.unwrap();
        let mut buf = vec![0; 65536];
        let got = tokio::time::timeout(Duration::from_secs(3), socket.recv(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..got], vec![8; n]);
    }
    server.0.shutdown_and_wait().await.unwrap();
    for id in [1, 2] {
        let stats = server
            .0
            .registry()
            .store
            .stats(&felucca_server::resources::ResourceKey::Tunnel {
                client: "local.office".into(),
                id,
            })
            .unwrap();
        assert_eq!(stats.inlet, 33001);
        assert_eq!(stats.export, 33001);
    }
    task.abort();
}

#[tokio::test]
async fn nested_mixed_proxy_runs_authentication_instead_of_exposing_dynamic_dialing() {
    let entry = free_port().await;
    let proxy = free_port().await;
    let backend = echo().await;
    let server = local_config(&format!(
        r#"
{}
[[local.office.tunnel]]
id=2
mode="mixedproxy"
port={proxy}
bind="127.0.0.1"
[local.office.tunnel.options.auth.users]
alice="password"
"#,
        route(1, entry, "tunnel://local.office/2")
    ))
    .await;
    let anonymous=http_request(entry,&format!("CONNECT 127.0.0.1:{backend} HTTP/1.1\r\nHost: 127.0.0.1:{backend}\r\nConnection: close\r\n\r\n")).await;
    assert!(anonymous.starts_with("HTTP/1.1 407"), "{anonymous}");
    let mut http = TcpStream::connect(("127.0.0.1", entry)).await.unwrap();
    http.write_all(format!("CONNECT 127.0.0.1:{backend} HTTP/1.1\r\nHost: 127.0.0.1:{backend}\r\nProxy-Authorization: Basic YWxpY2U6cGFzc3dvcmQ=\r\n\r\n").as_bytes()).await.unwrap();
    assert!(head(&mut http).await.starts_with("HTTP/1.1 200"));
    roundtrip(http).await;
    let mut socks = TcpStream::connect(("127.0.0.1", entry)).await.unwrap();
    socks.write_all(&[5, 1, 0]).await.unwrap();
    let mut answer = [0; 2];
    socks.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, [5, 255]);
    let mut socks = TcpStream::connect(("127.0.0.1", entry)).await.unwrap();
    socks.write_all(&[5, 1, 2]).await.unwrap();
    socks.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, [5, 2]);
    socks.write_all(b"\x01\x05alice\x08password").await.unwrap();
    socks.read_exact(&mut answer).await.unwrap();
    assert_eq!(answer, [1, 0]);
    let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
    request.extend(backend.to_be_bytes());
    socks.write_all(&request).await.unwrap();
    let mut reply = [0; 10];
    socks.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0);
    roundtrip(socks).await;
    server.0.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn nested_sni_routes_support_offload_and_passthrough_without_crossing_route_scope() {
    let frontend = identity("offload.test").await;
    let backend_identity = identity("pass.test").await;
    let backend = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_port = backend.local_addr().unwrap().port();
    let acceptor = backend_identity.identity.acceptor().unwrap();
    let task = tokio::spawn(async move {
        let (io, _) = backend.accept().await.unwrap();
        let io = acceptor.accept(io).await.unwrap();
        let (mut r, mut w) = tokio::io::split(io);
        tokio::io::copy(&mut r, &mut w).await.unwrap();
        w.shutdown().await.unwrap();
    });
    let port = free_port().await;
    let pass = free_port().await;
    let offload = free_port().await;
    let mut config = server_config();
    config.clients.clear();
    config.http = Some(HttpProxyConfig {
        bind: None,
        tls_bind: Some(format!("127.0.0.1:{port}")),
        default_tls: Some(frontend.config.clone()),
        ..Default::default()
    });
    let text = format!(
        r#"
{}{}
[[local.office.tunnel]]
id=3
mode="http"
port={port}
hosts=["pass.test"]
backend=[{{addr="127.0.0.1:{backend_port}"}}]
[local.office.tunnel.options.http]
tls_mode="passthrough"
[[local.office.tunnel]]
id=4
mode="http"
port={port}
hosts=["offload.test"]
backend=[{{addr="127.0.0.1:{}"}}]
[local.office.tunnel.options.http]
tls_mode="offload"
"#,
        route(1, pass, "tunnel://local.office/3"),
        route(2, offload, "tunnel://local.office/4"),
        echo().await
    );
    config.locals = toml::from_str::<ServerConfig>(&text).unwrap().locals;
    let server = LocalServer(Arc::new(Server::new(config).unwrap()));
    server.0.clone().run().await.unwrap();
    for (port, host, pin) in [
        (pass, "pass.test", backend_identity.identity.fingerprint()),
        (offload, "offload.test", frontend.identity.fingerprint()),
    ] {
        let tls = felucca_transport::tls::connect_to(
            ([127, 0, 0, 1], port).into(),
            host,
            &felucca_transport::Verification::Pinned { fingerprint: pin },
        )
        .await
        .unwrap();
        roundtrip(tls).await;
    }
    task.await.unwrap();
    // SNI from another registered route must not select that route through an alias.
    let mut tls = felucca_transport::tls::connect_to(
        ([127, 0, 0, 1], pass).into(),
        "offload.test",
        &felucca_transport::Verification::Pinned {
            fingerprint: frontend.identity.fingerprint(),
        },
    )
    .await
    .unwrap();
    tls.write_all(b"GET / HTTP/1.1\r\nHost: offload.test\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut response = String::new();
    tokio::time::timeout(Duration::from_secs(3), tls.read_to_string(&mut response))
        .await
        .unwrap()
        .unwrap();
    assert!(response.starts_with("HTTP/1.1 404"), "{response}");
}

#[tokio::test]
async fn file_alias_reaches_a_remote_confined_service_and_retains_readonly_auth() {
    let files = identity("remote-file.test").await;
    std::fs::write(files.root.join("index.html"), "remote file").unwrap();
    let entry = free_port().await;
    let remote = free_port().await;
    let mut config = server_config();
    config.locals = toml::from_str::<ServerConfig>(&route(1, entry, "file://features/9"))
        .unwrap()
        .locals;
    let running = start(
        config,
        agent(&format!(
            r#"
[[tunnel]]
id=9
mode="file"
port={remote}
bind="127.0.0.1"
[tunnel.file]
root='{}'
read_only=true
[tunnel.file.auth.users]
alice="password"
"#,
            files.root.display()
        )),
        TransportKind::Tls,
    )
    .await;
    for (method, auth, status) in [
        ("GET", "", 401),
        ("GET", "Authorization: Basic YWxpY2U6cGFzc3dvcmQ=\r\n", 200),
        (
            "DELETE",
            "Authorization: Basic YWxpY2U6cGFzc3dvcmQ=\r\n",
            405,
        ),
    ] {
        let response=http_request(entry,&format!("{method} /index.html HTTP/1.1\r\nHost: file.test\r\n{auth}Connection: close\r\n\r\n")).await;
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{response}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(files.root.join("index.html")).unwrap(),
        "remote file"
    );
    running.server.shutdown_and_wait().await.unwrap();
}

#[tokio::test]
async fn web_bridge_preserves_host_routing_and_site_authentication() {
    let web = free_port().await;
    let entry = free_port().await;
    let private = free_port().await;
    let server = local_config(&format!(
        r#"
[http]
bind="127.0.0.1:{web}"
tls_bind=""
{}
[[local.office.tunnel]]
id=2
mode="http"
port={web}
hosts=["web.test"]
backend=[{{addr="127.0.0.1:{}"}}]
[local.office.tunnel.options.auth.users]
alice="password"
[user.tenant]
[local.tenant]
user="tenant"
[[local.tenant.tunnel]]
mode="tcp"
port={private}
bind="127.0.0.1"
backend=[{{addr="bridge://http"}}]
"#,
        route(1, entry, "bridge://http"),
        web_backend("nested web").await
    ))
    .await;
    for (host, auth, status) in [
        ("web.test", "", 401),
        (
            "web.test",
            "Authorization: Basic YWxpY2U6cGFzc3dvcmQ=\r\n",
            200,
        ),
        ("unknown.test", "", 404),
    ] {
        let response = http_request(
            entry,
            &format!("GET / HTTP/1.1\r\nHost: {host}\r\n{auth}Connection: close\r\n\r\n"),
        )
        .await;
        assert!(
            response.starts_with(&format!("HTTP/1.1 {status}")),
            "{response}"
        );
    }
    refused(private).await;
    server.0.shutdown_and_wait().await.unwrap();
}
