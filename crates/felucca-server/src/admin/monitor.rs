//! HTTP query adaptation for bounded host metric history.
use super::{Result, bad};
use std::time::Duration;

pub(super) fn window(query: Option<&str>) -> Result<Duration> {
    let seconds = match query {
        None | Some("") => 900,
        Some("window=300") => 300,
        Some("window=900") => 900,
        Some("window=3600") => 3600,
        _ => return Err(bad("window must be 300, 900 or 3600 seconds")),
    };
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_query_cannot_request_unbounded_or_ambiguous_ranges() {
        assert_eq!(window(None).unwrap(), Duration::from_secs(900));
        assert_eq!(
            window(Some("window=3600")).unwrap(),
            Duration::from_secs(3600)
        );
        for query in [
            "window=0",
            "window=99999999",
            "window=300&window=3600",
            "unknown=1",
        ] {
            assert!(window(Some(query)).is_err());
        }
    }
}
