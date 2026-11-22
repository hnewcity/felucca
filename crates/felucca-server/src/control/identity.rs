use felucca_core::config::AdminRole;

/// Authorization identity shared by browser sessions and platform connectors.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Principal {
    pub account: String,
    pub role: AdminRole,
    pub user: Option<String>,
}
