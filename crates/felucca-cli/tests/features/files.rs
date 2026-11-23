use super::*;

const AUTH: &str = "Authorization: Basic YWxpY2U6cGFzc3dvcmQ=\r\n";
fn status(response: &str) -> u16 {
    response.split_whitespace().nth(1).unwrap().parse().unwrap()
}
async fn dav(port: u16, method: &str, path: &str, headers: &str, body: &str) -> String {
    http_request(port, &format!("{method} {path} HTTP/1.1\r\nHost: localhost:{port}\r\n{AUTH}{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())).await
}

#[tokio::test]
async fn file_service_supports_auth_ranges_webdav_locks_and_read_only_mode() {
    let files = identity("files.test").await;
    let root = files.root.join("public");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("index.html"), b"file index").unwrap();
    let port = free_port().await;
    for readonly in [false, true] {
        let config = agent(&format!(
            r#"
[[tunnel]]
mode = "file"
port = {port}
bind = "127.0.0.1"
[tunnel.file]
root = '{}'
prefix = "/dav"
read_only = {readonly}
[tunnel.options.payload]
compress = true
encrypt = true
[tunnel.file.auth.users]
alice = "password"
bob = "password2"
"#,
            root.display()
        ));
        let running = start(server_config(), config, TransportKind::Tls).await;
        let anonymous = http_request(
            port,
            "GET /dav/ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert_eq!(status(&anonymous), 401);
        let index = dav(port, "GET", "/dav/", "", "").await;
        assert_eq!(status(&index), 200, "{index}");
        assert!(index.ends_with("file index"));
        let bob = http_request(port, "GET /dav/index.html HTTP/1.1\r\nHost: localhost\r\nAuthorization: Basic Ym9iOnBhc3N3b3JkMg==\r\nConnection: close\r\n\r\n").await;
        assert_eq!(status(&bob), 200);
        let ranged = dav(port, "GET", "/dav/index.html", "Range: bytes=5-9\r\n", "").await;
        assert_eq!(status(&ranged), 206, "{ranged}");
        assert!(ranged.ends_with("index"));
        let head = dav(port, "HEAD", "/dav/index.html", "", "").await;
        assert_eq!(status(&head), 200);
        assert!(head.ends_with("\r\n\r\n"));
        let listing = dav(port, "PROPFIND", "/dav/", "Depth: 1\r\n", "").await;
        assert_eq!(status(&listing), 207, "{listing}");
        assert!(listing.contains("index.html"));
        if readonly {
            for (method, path, headers) in [
                ("PUT", "/dav/index.html", ""),
                ("DELETE", "/dav/index.html", ""),
                ("MKCOL", "/dav/forbidden", ""),
                ("COPY", "/dav/index.html", "Destination: /dav/copy\r\n"),
                ("MOVE", "/dav/index.html", "Destination: /dav/move\r\n"),
            ] {
                assert_eq!(
                    status(&dav(port, method, path, headers, "").await),
                    405,
                    "{method}"
                );
            }
            assert_eq!(
                std::fs::read(root.join("index.html")).unwrap(),
                b"file index"
            );
        } else {
            assert_eq!(
                status(&dav(port, "MKCOL", "/dav/folder", "", "").await),
                201
            );
            let payload = "x".repeat(2 * 1024 * 1024);
            let put = dav(port, "PUT", "/dav/folder/large", "", &payload).await;
            assert_eq!(status(&put), 201, "{put}");
            assert_eq!(
                std::fs::read(root.join("folder/large")).unwrap(),
                payload.as_bytes()
            );
            let get = dav(port, "GET", "/dav/folder/large", "", "").await;
            assert_eq!(status(&get), 200);
            assert!(get.ends_with(&payload));
            let copy = dav(
                port,
                "COPY",
                "/dav/folder/large",
                &format!("Destination: http://localhost:{port}/dav/copied\r\n"),
                "",
            )
            .await;
            assert_eq!(status(&copy), 201, "{copy}");
            let moved = dav(
                port,
                "MOVE",
                "/dav/copied",
                &format!("Destination: http://localhost:{port}/dav/moved\r\n"),
                "",
            )
            .await;
            assert_eq!(status(&moved), 201, "{moved}");
            let lock_body = r#"<?xml version="1.0"?><D:lockinfo xmlns:D="DAV:"><D:lockscope><D:exclusive/></D:lockscope><D:locktype><D:write/></D:locktype></D:lockinfo>"#;
            let lock = dav(
                port,
                "LOCK",
                "/dav/moved",
                "Content-Type: application/xml\r\nTimeout: Second-60\r\n",
                lock_body,
            )
            .await;
            assert_eq!(status(&lock), 200, "{lock}");
            let token = lock
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("lock-token:"))
                .unwrap()
                .split_once(':')
                .unwrap()
                .1
                .trim();
            assert_eq!(
                status(&dav(port, "PUT", "/dav/moved", "", "locked").await),
                423
            );
            assert_eq!(
                status(
                    &dav(
                        port,
                        "UNLOCK",
                        "/dav/moved",
                        &format!("Lock-Token: {token}\r\n"),
                        ""
                    )
                    .await
                ),
                204
            );
            assert_eq!(
                status(&dav(port, "PUT", "/dav/moved", "", "unlocked").await),
                204
            );
            assert_eq!(std::fs::read(root.join("moved")).unwrap(), b"unlocked");
            assert_eq!(
                status(&dav(port, "DELETE", "/dav/moved", "", "").await),
                204
            );
            assert!(!root.join("moved").exists());
        }
        running.server.shutdown_and_wait().await.unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn file_routes_reject_traversal_and_symlinks_through_the_http_reverse_proxy() {
    let files = identity("file-host.test").await;
    let root = files.root.join("public");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("safe.txt"), b"allowed file").unwrap();
    std::fs::create_dir(files.root.join("private")).unwrap();
    std::fs::write(
        files.root.join("private/secret.txt"),
        b"outside secret marker",
    )
    .unwrap();
    std::os::unix::fs::symlink("../private", root.join("escape")).unwrap();
    let port = free_port().await;
    let mut server = server_config();
    server.http = Some(HttpProxyConfig {
        bind: Some(format!("127.0.0.1:{port}")),
        tls_bind: None,
        ..Default::default()
    });
    let config = agent(&format!(
        r#"
[[tunnel]]
mode = "file"
port = {port}
hosts = ["files.test"]
[tunnel.file]
root = '{}'
read_only = false
[tunnel.options.http]
backend_http2 = true
"#,
        root.display()
    ));
    let _running = start(server, config, TransportKind::Quic).await;
    let safe = http_request(
        port,
        "GET /safe.txt HTTP/1.1\r\nHost: files.test\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert_eq!(status(&safe), 200, "{safe}");
    assert!(safe.ends_with("allowed file"));
    for path in [
        "/escape/secret.txt",
        "/../private/secret.txt",
        "/%2e%2e/private/secret.txt",
        "/%2e%2e%2fprivate/secret.txt",
    ] {
        let response = http_request(
            port,
            &format!("GET {path} HTTP/1.1\r\nHost: files.test\r\nConnection: close\r\n\r\n"),
        )
        .await;
        assert!(
            !response.contains("outside secret marker"),
            "{path}: {response}"
        );
        assert_ne!(status(&response), 200, "{path}: {response}");
    }
    let put = http_request(port, "PUT /escape/owned HTTP/1.1\r\nHost: files.test\r\nContent-Length: 4\r\nConnection: close\r\n\r\nevil").await;
    assert!(status(&put) >= 400, "{put}");
    let copy = http_request(port, &format!("COPY /safe.txt HTTP/1.1\r\nHost: files.test\r\nDestination: http://files.test:{port}/escape/copied\r\nConnection: close\r\n\r\n")).await;
    assert!(status(&copy) >= 400, "{copy}");
    assert!(!files.root.join("private/owned").exists());
    assert!(!files.root.join("private/copied").exists());
}
