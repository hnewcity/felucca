//! Agent-owned capabilities. Remote configuration cannot replace these handles.
use cap_std::fs::Dir;
use felucca_core::{
    ConfigError,
    access::{TargetAcl, TargetPolicy},
    config::{FileConfig, RemotePolicy, managed::file_capability},
    geodata::GeoResolver,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Debug, Clone)]
pub struct RemoteAuthority {
    pub(crate) target_acl: Arc<TargetAcl>,
    console_scope: bool,
    roots: Arc<BTreeMap<String, (Dir, bool)>>,
}

impl RemoteAuthority {
    pub fn new(policy: &RemotePolicy, geo: &GeoResolver) -> Result<Self, ConfigError> {
        let target_acl = Arc::new(TargetAcl::compile_with_geo(&policy.target_policy, geo)?);
        let mut roots = BTreeMap::new();
        for (name, root) in &policy.file_roots {
            let dir = Dir::open_ambient_dir(&root.path, cap_std::ambient_authority())
                .map_err(|e| ConfigError::invalid(format!("remote file root {name:?}: {e}")))?;
            roots.insert(name.clone(), (dir, root.read_only));
        }
        Ok(Self {
            target_acl,
            console_scope: policy.console_scope,
            roots: Arc::new(roots),
        })
    }

    /// The authority a managed configuration runs under. A console-supplied
    /// destination scope replaces the local one only when this agent opted in;
    /// directory capabilities always stay local.
    pub fn scoped(
        &self,
        policy: Option<&TargetPolicy>,
        geo: &GeoResolver,
    ) -> Result<Self, ConfigError> {
        let Some(policy) = policy else {
            return Ok(self.clone());
        };
        if !self.console_scope {
            return Err(ConfigError::invalid(
                "this agent does not accept a destination scope from the console; set remote.console_scope = true",
            ));
        }
        Ok(Self {
            target_acl: Arc::new(TargetAcl::compile_with_geo(policy, geo)?),
            console_scope: true,
            roots: self.roots.clone(),
        })
    }

    pub(crate) fn file(
        &self,
        config: &FileConfig,
    ) -> Result<crate::file_server::FileServer, ConfigError> {
        let (name, relative) = file_capability(&config.root)?;
        let (root, read_only) = self.roots.get(name).ok_or_else(|| {
            ConfigError::invalid("remote file root was not granted by this agent")
        })?;
        if *read_only && !config.read_only {
            return Err(ConfigError::invalid(
                "remote configuration cannot grant writes to a read-only root",
            ));
        }
        // Resolve relative to the retained directory handle: symlink or rename
        // races cannot escape the operator's root between checking and opening.
        let dir = root
            .open_dir(relative)
            .map_err(|e| ConfigError::invalid(format!("remote file directory: {e}")))?;
        crate::file_server::FileServer::from_fs(
            config,
            crate::file_fs::ConfinedFs::from_dir(dir, config.read_only),
        )
        .map_err(|e| ConfigError::invalid(format!("remote file service: {e}")))
    }
}
