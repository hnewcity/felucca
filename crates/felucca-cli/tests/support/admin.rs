//! Shared admin fixtures; test cases live in the feature suites.
use super::*;
use felucca_core::config::{AdminAccount, AdminConfig, AdminRole, UserEntry};
use felucca_server::{Daemon, admin::credentials};
use serde_json::{Value, json};
pub(crate) const PASSWORD: &str = "management-test-password";
pub(crate) fn password_hash() -> String {
    static HASH: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    HASH.get_or_init(|| credentials::hash_password(PASSWORD.as_bytes()).unwrap())
        .clone()
}
pub(crate) struct Fixture {
    pub(crate) files: TestIdentity,
    pub(crate) config: ServerConfig,
    pub(crate) port: u16,
    pub(crate) root_token: String,
    pub(crate) operator_token: String,
    pub(crate) viewer_token: String,
}
impl Fixture {
    pub(crate) async fn new() -> Self {
        let files = identity("admin.test").await;
        let port = free_port().await;
        let root_token = credentials::token();
        let operator_token = credentials::token();
        let viewer_token = credentials::token();
        let account = |role, user: Option<&str>, token: &str| AdminAccount {
            role,
            user: user.map(str::to_owned),
            password_hash: password_hash().into(),
            token_hash: Some(credentials::token_hash(token).into()),
            disabled: false,
        };
        let mut config = server_config();
        config.users.insert("team-a".into(), UserEntry::default());
        config.users.insert("team-b".into(), UserEntry::default());
        config.clients.get_mut("features").unwrap().user = Some("team-a".into());
        let mut other = config.clients["features"].clone();
        other.user = Some("team-b".into());
        other.vkey = "different-tenant-secret".into();
        config.clients.insert("other".into(), other);
        config.admin = Some(AdminConfig {
            profiling: false,
            bind: format!("127.0.0.1:{port}"),
            state_path: files.root.join("runtime.json"),
            tls: None,
            origin: None,
            session_ttl: Duration::from_secs(3600),
            max_sessions: 128,
            accounts: [
                ("root".into(), account(AdminRole::Admin, None, &root_token)),
                (
                    "operator".into(),
                    account(AdminRole::Operator, Some("team-a"), &operator_token),
                ),
                (
                    "viewer".into(),
                    account(AdminRole::Viewer, Some("team-a"), &viewer_token),
                ),
            ]
            .into(),
        });
        Self {
            files,
            config,
            port,
            root_token,
            operator_token,
            viewer_token,
        }
    }
    pub(crate) async fn start(&self) -> Arc<Daemon> {
        Daemon::start(self.config.clone(), None).await.unwrap()
    }
    pub(crate) fn api(&self, token: &str) -> Api {
        Api {
            port: self.port,
            headers: vec![("Authorization".into(), format!("Bearer {token}"))],
        }
    }
}
#[derive(Clone)]
pub(crate) struct Api {
    pub(crate) port: u16,
    pub(crate) headers: Vec<(String, String)>,
}
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) headers: std::collections::HashMap<String, String>,
    pub(crate) body: Value,
    pub(crate) raw: String,
}
impl Api {
    pub(crate) async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        revision: u64,
        key: &str,
    ) -> Reply {
        self.send_within(Duration::from_secs(10), method, path, body, revision, key)
            .await
    }
    /// Like `send`, for endpoints whose work legitimately outlasts the default
    /// wait, such as CPU sampling plus flamegraph rendering on a loaded runner.
    pub(crate) async fn send_within(
        &self,
        wait: Duration,
        method: &str,
        path: &str,
        body: Option<Value>,
        revision: u64,
        key: &str,
    ) -> Reply {
        let mut io = TcpStream::connect(("127.0.0.1", self.port)).await.unwrap();
        let bytes = body.map_or_else(Vec::new, |v| serde_json::to_vec(&v).unwrap());
        let mut request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\nIf-Match: \"{revision}\"\r\nIdempotency-Key: {key}\r\n",
            self.port,
            self.port,
            bytes.len()
        );
        for (key, value) in &self.headers {
            request.push_str(&format!("{key}: {value}\r\n"));
        }
        request.push_str("\r\n");
        let mut request = request.into_bytes();
        request.extend_from_slice(&bytes);
        io.write_all(&request).await.unwrap();
        // These non-streaming endpoints return Content-Length. Read that frame,
        // not TCP EOF: rejecting a request before consuming its body may reset
        // the socket after an otherwise complete HTTP response has arrived.
        let raw = tokio::time::timeout(wait, async {
            use tokio::io::AsyncBufReadExt;
            let mut io = tokio::io::BufReader::new(io);
            let mut head = String::new();
            while !head.ends_with("\r\n\r\n") {
                let count = io.read_line(&mut head).await?;
                assert!(
                    count > 0 && head.len() <= 65536,
                    "{method} {path}: incomplete headers"
                );
            }
            let length: usize = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .expect("fixture endpoint must return a framed response")
                .1
                .trim()
                .parse()
                .unwrap();
            assert!(length <= 16 * 1024 * 1024);
            let mut body = vec![0; length];
            io.read_exact(&mut body).await?;
            head.push_str(std::str::from_utf8(&body).unwrap());
            Ok::<_, std::io::Error>(head)
        })
        .await
        .expect("management response timed out")
        .unwrap_or_else(|error| panic!("{method} {path}: {error}"));
        let (head, text) = raw.split_once("\r\n\r\n").unwrap();
        let status = head.split_whitespace().nth(1).unwrap().parse().unwrap();
        let headers = head
            .lines()
            .skip(1)
            .filter_map(|line| line.split_once(':'))
            .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
            .collect();
        let body = serde_json::from_str(text).unwrap_or(Value::Null);
        Reply {
            status,
            headers,
            body,
            raw,
        }
    }
    pub(crate) async fn get(&self, path: &str) -> Reply {
        self.send("GET", path, None, 0, "read").await
    }
    pub(crate) async fn login(&self) -> (Self, String) {
        let response = self
            .send(
                "POST",
                "/api/login",
                Some(json!({"account":"root","password":PASSWORD})),
                0,
                "login",
            )
            .await;
        assert_eq!(response.status, 200, "{}", response.raw);
        let cookie = response.headers["set-cookie"]
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let csrf = response.body["csrf"].as_str().unwrap().to_owned();
        (
            Self {
                port: self.port,
                headers: vec![
                    ("Cookie".into(), cookie),
                    ("X-Felucca-CSRF".into(), csrf.clone()),
                ],
            },
            csrf,
        )
    }
}
