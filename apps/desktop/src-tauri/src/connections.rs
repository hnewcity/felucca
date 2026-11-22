//! The connections this desktop client runs, each an in-process agent.
//!
//! An agent reconnects by itself and never returns on its own, so the only
//! states worth showing are: stopped, connecting, online, and refused before
//! it started (a launch URI that no longer validates, say).

use std::collections::HashMap;
use std::sync::Arc;

use felucca_client::Agent;
use felucca_i18n::Language;
use serde::Serialize;

use crate::store::{Saved, Store};

/// Text the window translates, or text already rendered in its language.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    Key(&'static str),
    Text(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Connecting,
    Online,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct View {
    pub id: String,
    pub name: String,
    pub server: String,
    pub device: String,
    pub enabled: bool,
    pub status: Status,
    pub error: Option<Reply>,
}

struct Running {
    agent: Arc<Agent>,
    task: tauri::async_runtime::JoinHandle<()>,
}

/// The launch URI inside pasted text: a bare URI or the console's whole command.
pub fn find_uri(text: &str) -> Option<&str> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find("felucca://")?;
    let rest = &text[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | '`'))
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// What a launch URI names, for a confirmation that must not show the key.
pub fn describe(uri: &str, language: Language) -> Result<(String, String), Reply> {
    let config = felucca_core::config::launch::parse(uri)
        .map_err(|error| Reply::Text(error.message().render(language)))?;
    Ok((config.server, config.key_id.unwrap_or_default()))
}

pub struct Connections {
    store: Store,
    language: Language,
    saved: Vec<Saved>,
    running: HashMap<String, Running>,
    refused: HashMap<String, Reply>,
}

impl Connections {
    pub fn open(store: Store, language: Language) -> std::io::Result<Self> {
        let saved = store.load()?;
        let mut connections = Self {
            store,
            language,
            saved,
            running: HashMap::new(),
            refused: HashMap::new(),
        };
        for id in connections.enabled_ids() {
            connections.start(&id);
        }
        Ok(connections)
    }

    pub fn language(&self) -> Language {
        self.language
    }

    fn enabled_ids(&self) -> Vec<String> {
        self.saved
            .iter()
            .filter(|saved| saved.enabled)
            .map(|saved| saved.id.clone())
            .collect()
    }

    pub fn views(&self) -> Vec<View> {
        self.saved
            .iter()
            .map(|saved| {
                let (server, device) = describe(&saved.uri, self.language).unwrap_or_default();
                let error = self.refused.get(&saved.id).cloned();
                let status = match self.running.get(&saved.id) {
                    _ if error.is_some() => Status::Failed,
                    Some(running) if running.task.inner().is_finished() => Status::Failed,
                    Some(running) if running.agent.is_ready() => Status::Online,
                    Some(_) => Status::Connecting,
                    None => Status::Stopped,
                };
                View {
                    id: saved.id.clone(),
                    name: saved.name.clone(),
                    server,
                    device,
                    enabled: saved.enabled,
                    status,
                    error,
                }
            })
            .collect()
    }

    /// Validates, saves and starts a connection taken from pasted text or a link.
    pub fn add(&mut self, text: &str, name: &str) -> Result<(), Reply> {
        let uri = find_uri(text).ok_or(Reply::Key("error.noLink"))?;
        let (server, device) = describe(uri, self.language)?;
        // Two agents with one identity keep replacing each other's session.
        if self.saved.iter().any(|saved| {
            describe(&saved.uri, self.language)
                .is_ok_and(|(s, d)| s.eq_ignore_ascii_case(&server) && d == device)
        }) {
            return Err(Reply::Key("error.duplicate"));
        }
        if self.saved.len() >= 32 {
            return Err(Reply::Key("error.tooMany"));
        }
        let name = name.trim();
        let saved = Saved {
            id: uuid::Uuid::new_v4().to_string(),
            name: if name.is_empty() {
                server
            } else {
                name.chars().take(80).collect()
            },
            uri: uri.to_owned(),
            enabled: true,
        };
        let id = saved.id.clone();
        self.saved.push(saved);
        if let Err(error) = self.store.save(&self.saved) {
            self.saved.pop();
            return Err(Reply::Text(error.to_string()));
        }
        self.start(&id);
        Ok(())
    }

    pub fn remove(&mut self, id: &str) -> Result<(), Reply> {
        let position = self
            .saved
            .iter()
            .position(|saved| saved.id == id)
            .ok_or(Reply::Key("error.missing"))?;
        let removed = self.saved.remove(position);
        if let Err(error) = self.store.save(&self.saved) {
            self.saved.insert(position, removed);
            return Err(Reply::Text(error.to_string()));
        }
        self.stop(id);
        Ok(())
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), Reply> {
        let saved = self
            .saved
            .iter_mut()
            .find(|saved| saved.id == id)
            .ok_or(Reply::Key("error.missing"))?;
        let previous = std::mem::replace(&mut saved.enabled, enabled);
        if let Err(error) = self.store.save(&self.saved) {
            if let Some(saved) = self.saved.iter_mut().find(|saved| saved.id == id) {
                saved.enabled = previous;
            }
            return Err(Reply::Text(error.to_string()));
        }
        self.stop(id);
        if enabled {
            self.start(id);
        }
        Ok(())
    }

    fn start(&mut self, id: &str) {
        let Some(saved) = self.saved.iter().find(|saved| saved.id == id) else {
            return;
        };
        self.refused.remove(id);
        let prepared = felucca_core::config::launch::parse(&saved.uri).and_then(|config| {
            // Visitors bind sockets while preparing, which needs the runtime.
            let _runtime = tauri::async_runtime::handle().inner().enter();
            tracing::info!(server = %config.server, "starting connection");
            Agent::new(config)
        });
        match prepared {
            Ok(agent) => {
                let agent = Arc::new(agent);
                let task = tauri::async_runtime::spawn(agent.clone().run());
                self.running.insert(id.to_owned(), Running { agent, task });
            }
            Err(error) => {
                tracing::warn!(%error, "connection refused before it started");
                self.refused.insert(
                    id.to_owned(),
                    Reply::Text(error.message().render(self.language)),
                );
            }
        }
    }

    fn stop(&mut self, id: &str) {
        self.refused.remove(id);
        if let Some(running) = self.running.remove(id) {
            running.agent.shutdown();
            running.task.abort();
        }
    }

    pub fn stop_all(&mut self) {
        for id in self.running.keys().cloned().collect::<Vec<_>>() {
            self.stop(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_link_in_a_pasted_command() {
        let uri = "felucca://example.com:14028?key_id=a&vkey=b&remote=1";
        assert_eq!(find_uri(uri), Some(uri));
        assert_eq!(
            find_uri(&format!("./felucca-agent --config '{uri}'")),
            Some(uri)
        );
        assert_eq!(
            find_uri(&format!("felucca-agent.exe --config \"{uri}\"\n")),
            Some(uri)
        );
        assert_eq!(find_uri("curl https://example.com | sh"), None);
    }

    #[test]
    fn describes_a_link_without_its_key() {
        let (server, device) = describe(
            "felucca://example.com:14028?key_id=edge&vkey=secret",
            Language::English,
        )
        .unwrap();
        assert_eq!(
            (server.as_str(), device.as_str()),
            ("example.com:14028", "edge")
        );
        assert!(describe("felucca://example.com?vkey=x", Language::English).is_err());
    }
}
