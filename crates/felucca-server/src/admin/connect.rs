//! HTTP adaptation for the one-command device launcher.
use super::{Result, forbidden, missing};
use crate::http_support::{Body, response};
use felucca_core::ServerConfig;
use felucca_i18n::{Language, Message};
use http::{HeaderMap, Response, StatusCode, header};

pub(super) fn script(windows: bool, language: Language) -> Response<Body> {
    let mut script = if windows {
        include_str!("connect.ps1")
    } else {
        include_str!("connect.sh")
    }
    .to_owned();
    for (key, source) in [
        (
            "unsupported",
            Message::text("This operating system or processor is not supported."),
        ),
        (
            "unavailable",
            Message::text(
                "Cannot download this client. Check the server address, device permission and available client packages.",
            ),
        ),
        (
            "checksum",
            Message::text("Client verification failed. Run the connection command again."),
        ),
        (
            "starting",
            Message::text(
                "Connecting this device. Keep this window open; interrupted connections recover automatically.",
            ),
        ),
        (
            "invalid",
            Message::text("The connection command is incomplete. Copy it again from the console."),
        ),
    ] {
        let value = source.render(language);
        let quoted = if windows {
            format!("'{}'", value.replace('\'', "''"))
        } else {
            format!("'{}'", value.replace('\'', "'\\''"))
        };
        script = script.replace(&format!("{{{{{key}}}}}"), &quoted);
    }
    let mut result = response(StatusCode::OK, script);
    result.headers_mut().insert(
        header::CONTENT_TYPE,
        "text/plain; charset=utf-8".parse().unwrap(),
    );
    result
}

pub(super) async fn download(
    config: &ServerConfig,
    path: &str,
    headers: &HeaderMap,
) -> Result<Response<Body>> {
    let parts: Vec<_> = path
        .trim_start_matches("/api/connect/")
        .split('/')
        .collect();
    let [id, target, action] = parts.as_slice() else {
        return Err(missing());
    };
    let id = percent_encoding::percent_decode_str(id)
        .decode_utf8()
        .map_err(|_| missing())?;
    if headers.get_all(header::AUTHORIZATION).iter().count() != 1 {
        return Err(forbidden());
    }
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(forbidden)?;
    crate::control::connection::authorize(config, &id, token)?;
    match *action {
        "sha256" => {
            let target = target.to_string();
            let package = tokio::task::spawn_blocking(move || {
                super::downloads::catalog()
                    .into_iter()
                    .find(|package| package.target == target)
            })
            .await
            .map_err(|_| missing())?
            .ok_or_else(missing)?;
            let mut result = response(StatusCode::OK, package.sha256);
            result.headers_mut().insert(
                header::CONTENT_TYPE,
                "text/plain; charset=utf-8".parse().unwrap(),
            );
            Ok(result)
        }
        "binary" => super::downloads::download(target).await,
        _ => Err(missing()),
    }
}
