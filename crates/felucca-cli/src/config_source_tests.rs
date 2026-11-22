use super::*;
use felucca_core::{AgentConfig, TransportKind};
use tokio::{io::AsyncWriteExt, net::TcpListener};

#[test]
fn json_and_toml_preserve_configuration_types_and_reject_duplicate_fields() {
    for text in [
        r#"{"server":"127.0.0.1:9001","vkey":"key","dial_timeout":"2s","max_connections":7}"#,
        "server='127.0.0.1:9001'\nvkey='key'\ndial_timeout='2s'\nmax_connections=7",
    ] {
        let mut config: AgentConfig =
            serde_json::from_value(parse(text.as_bytes()).unwrap()).unwrap();
        config.validate().unwrap();
        assert_eq!(config.dial_timeout, Duration::from_secs(2));
        assert_eq!(config.max_connections, Some(7));
    }
    for text in [
        r#"{"vkey":"first","vkey":"second"}"#,
        r#"{"client":{"edge":{"vkey":"a"},"edge":{"vkey":"b"}}}"#,
        "vkey='first'\nvkey='second'",
        r#"{"secret":"never-print-this",malformed}"#,
    ] {
        let error = parse(text.as_bytes()).unwrap_err().to_string();
        assert!(!error.contains("never-print-this"));
    }
    assert!(parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    assert!(parse(&[255]).is_err());
}

#[test]
fn environment_values_cannot_inject_config_and_are_expanded_only_once() {
    let injected = "'\nserver='evil:1234'\n${NEXT}";
    let mut value =
        parse(b"server='127.0.0.1:9001'\nvkey='${KEY}'\nws_path='${PATH:-/ws}'").unwrap();
    let mut budget = MAX_BYTES;
    expand(
        &mut value,
        &|name| Ok((name == "KEY").then(|| injected.to_owned())),
        &mut budget,
    )
    .unwrap();
    assert_eq!(value["server"], "127.0.0.1:9001");
    assert_eq!(value["vkey"], injected);
    assert_eq!(value["ws_path"], "/ws");
    assert_eq!(
        expand_string("$${KEY} / ${MISSING:-你好}", &|_| Ok(None), 100).unwrap(),
        "${KEY} / 你好"
    );
    for invalid in ["${MISSING}", "${KEY", "${1INVALID}", "${A:-${B}}"] {
        assert!(expand_string(invalid, &|_| Ok(None), 100).is_err());
    }
    assert!(expand_string("${LARGE}", &|_| Ok(Some("large".into())), 4).is_err());
    let mut values = serde_json::json!(["123", "456"]);
    assert!(expand(&mut values, &|_| Ok(None), &mut 5).is_err());
}

#[tokio::test]
async fn launch_uri_can_hand_tunnels_and_scope_to_the_server() {
    let source = format!(
        "felucca://localhost:14028?key_id=edge&vkey=k&fingerprint={}&remote=1",
        "ab".repeat(32)
    );
    let mut config: AgentConfig = load(&source, false, None).await.unwrap();
    config.validate().unwrap();
    assert!(config.remote.enabled);
    assert!(config.remote.console_scope);
    assert_eq!(config.remote.target_policy, Default::default());
    assert!(config.remote.file_roots.is_empty());
    let config: AgentConfig = load("felucca://localhost:14028?vkey=k&remote=0", false, None)
        .await
        .unwrap();
    assert!(!config.remote.enabled);
}

#[tokio::test]
async fn launch_uri_and_base64_roundtrip_without_echoing_credentials() {
    let source = "felucca://[::1]:14028?key_id=edge&vkey=a%2Bb%26c&transport=quic&ws_path=%2Fagent";
    let mut config: AgentConfig = load(source, false, None).await.unwrap();
    config.validate().unwrap();
    assert_eq!(config.server, "[::1]:14028");
    assert_eq!(config.transport, TransportKind::Quic);
    assert_eq!(serde_json::to_value(&config).unwrap()["vkey"], "a+b&c");
    assert_eq!(label(source), "launch URI");
    for source in [
        "felucca://secret@localhost:14028?vkey=x",
        "felucca://localhost:0?vkey=x",
        "felucca://localhost:14028?vkey=a&vkey=b",
        "felucca://localhost:14028?vkey=a&typo=b",
        "felucca://localhost:14028/unsupported?vkey=a",
        "felucca://localhost:14028?vkey=a&remote=yes",
    ] {
        assert!(load::<AgentConfig>(source, false, None).await.is_err());
    }
    let json = r#"{"server":"localhost:14028","vkey":"inline-secret"}"#;
    for encoded in [STANDARD.encode(json), URL_SAFE_NO_PAD.encode(json)] {
        let config: AgentConfig = load(&format!("base64:{encoded}"), false, None)
            .await
            .unwrap();
        assert_eq!(config.server, "localhost:14028");
    }
    assert_eq!(label(json), "inline configuration");
    assert_eq!(
        label("https://example.test/config?token=secret"),
        "remote configuration"
    );
    assert_eq!(
        label("HTTPS://example.test/config?token=secret"),
        "remote configuration"
    );
    assert_eq!(label("unknown://secret"), "configuration URI");
}

async fn serve_once(response: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut io, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(io.read_u8().await.unwrap());
        }
        let _ = io.write_all(&response).await;
    });
    (format!("http://{address}/config?private-token"), task)
}

#[tokio::test]
async fn downloads_are_bounded_streaming_and_never_follow_redirects() {
    let json = br#"{"server":"localhost:14028","vkey":"remote-secret"}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n0\r\n\r\n",
        json.len(),
        std::str::from_utf8(json).unwrap()
    );
    let (source, task) = serve_once(response.into_bytes()).await;
    let config: AgentConfig = load(&source, false, None).await.unwrap();
    assert_eq!(config.server, "localhost:14028");
    task.await.unwrap();
    for response in [
        format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            MAX_BYTES + 1
        ),
        "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n".into(),
        "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".into(),
    ] {
        let (source, task) = serve_once(response.into_bytes()).await;
        let error = load::<AgentConfig>(&source, false, None)
            .await
            .unwrap_err()
            .to_string();
        assert!(!error.contains("private-token"));
        task.await.unwrap();
    }
    let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let response = format!(
        "HTTP/1.1 302 Found\r\nLocation: http://{}/leak\r\nContent-Length: 0\r\n\r\n",
        destination.local_addr().unwrap()
    );
    let (source, task) = serve_once(response.into_bytes()).await;
    assert!(load::<AgentConfig>(&source, false, None).await.is_err());
    task.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(50), destination.accept())
            .await
            .is_err()
    );
}
