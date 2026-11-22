//! Device credentials grant package retrieval, never management access.
use super::{Error, Result, credentials};
use felucca_core::ServerConfig;
use subtle::ConstantTimeEq;

pub(crate) fn authorize(config: &ServerConfig, id: &str, token: &str) -> Result<()> {
    let client = config.clients.get(id).ok_or(Error::Forbidden)?;
    if !client.enabled
        || token.is_empty()
        || token.len() > 4096
        || !bool::from(
            credentials::digest(client.vkey.as_bytes())
                .ct_eq(&credentials::digest(token.as_bytes())),
        )
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
