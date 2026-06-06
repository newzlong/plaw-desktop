//! Registry of connected MCP clients, keyed by server name.
//!
//! Owns one [`crate::tools::mcp::client::McpClient`] per configured
//! `[[mcp.servers]]` entry. Built eagerly at agent startup (mirrors
//! the eager-spawn lifecycle decision from the
//! `mcp-client-discovery` workflow — failures surface in startup
//! logs, not mid-conversation), passed to the proxy `McpTool` via an
//! [`std::sync::Arc`].
//!
//! Phase 0 lifecycle: register-once-at-startup. Restart-on-crash and
//! hot-reload are explicit follow-ups.

use super::client::McpClient;
use crate::config::McpServerConfig;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// Outcome of [`McpRegistry::connect_all`] for one server.
#[derive(Debug, Clone)]
pub enum ServerStatus {
    /// Subprocess spawned, handshake completed, ready to serve calls.
    Connected,
    /// Subprocess failed to spawn or handshake; excluded from the
    /// registry. `error` is the user-facing reason.
    Failed { error: String },
}

/// Per-server connection state held by the registry.
struct ServerEntry {
    client: Arc<McpClient>,
    /// Effective allow-list for this server. `None` means "wildcard
    /// — admit every tool the server advertises".
    allowed_tools: Option<HashSet<String>>,
}

/// Registry of active MCP server connections. Cheap to clone via
/// `Arc<McpRegistry>` from caller code; internally borrow-immutable.
pub struct McpRegistry {
    servers: HashMap<String, ServerEntry>,
    statuses: HashMap<String, ServerStatus>,
}

impl McpRegistry {
    /// Spawn every configured server in parallel, return a registry
    /// populated with the ones whose handshake succeeded. Failed
    /// servers are recorded in [`Self::statuses`] with their error
    /// message; they're queryable via [`Self::status`] but not callable.
    ///
    /// Per-server failure is non-fatal — plaw starts up even if one
    /// MCP server has a typo in its `command`. The proxy tool's
    /// description surfaces the failed status so the LLM doesn't
    /// blindly retry.
    pub async fn connect_all(
        configs: &[McpServerConfig],
        secret_store: Arc<crate::security::SecretStore>,
    ) -> Self {
        let mut futures = Vec::with_capacity(configs.len());
        for cfg in configs {
            let cfg = cfg.clone();
            let secret_store = secret_store.clone();
            futures.push(tokio::spawn(async move {
                let allowed = compute_allow_set(&cfg.allowed_tools);
                let startup_timeout = Duration::from_millis(cfg.startup_timeout_ms);
                let request_timeout = Duration::from_millis(cfg.request_timeout_ms);
                let result = match &cfg.transport {
                    crate::config::McpTransport::Stdio => {
                        McpClient::connect(
                            &cfg.name,
                            &cfg.command,
                            &cfg.args,
                            &cfg.env,
                            startup_timeout,
                            request_timeout,
                        )
                        .await
                    }
                    crate::config::McpTransport::Http {
                        url,
                        bearer_token,
                        headers,
                    } => {
                        McpClient::connect_http(
                            &cfg.name,
                            url,
                            bearer_token.as_ref(),
                            headers,
                            startup_timeout,
                            request_timeout,
                            secret_store.as_ref(),
                        )
                        .await
                    }
                };
                (cfg.name, result, allowed)
            }));
        }

        let mut servers = HashMap::new();
        let mut statuses = HashMap::new();
        for f in futures {
            match f.await {
                Ok((name, Ok(client), allowed)) => {
                    tracing::info!(server = %name, "MCP server connected");
                    servers.insert(
                        name.clone(),
                        ServerEntry {
                            client: Arc::new(client),
                            allowed_tools: allowed,
                        },
                    );
                    statuses.insert(name, ServerStatus::Connected);
                }
                Ok((name, Err(e), _)) => {
                    let msg = format!("{e:#}");
                    tracing::warn!(
                        server = %name,
                        error = %msg,
                        "MCP server failed to connect; excluding from registry"
                    );
                    statuses.insert(name, ServerStatus::Failed { error: msg });
                }
                Err(join_err) => {
                    tracing::warn!(error = %join_err, "MCP connect task panicked");
                }
            }
        }

        Self { servers, statuses }
    }

    /// Names of currently-connected servers, sorted for stable
    /// iteration (used in the proxy tool's description string).
    pub fn connected_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.servers.keys().cloned().collect();
        names.sort();
        names
    }

    /// Per-server status for every configured server (connected and
    /// failed alike). Lets the proxy tool annotate its description
    /// with `[unavailable]` markers.
    pub fn statuses(&self) -> &HashMap<String, ServerStatus> {
        &self.statuses
    }

    pub fn status(&self, server_name: &str) -> Option<&ServerStatus> {
        self.statuses.get(server_name)
    }

    /// Look up a server by name. Returns `None` for unknown / failed
    /// servers — caller surfaces a helpful error.
    pub fn get(&self, server_name: &str) -> Option<&Arc<McpClient>> {
        self.servers.get(server_name).map(|e| &e.client)
    }

    /// Whether `tool_name` is admitted by this server's allow-list.
    /// `None` (no allow-list configured) means "wildcard / admit all".
    pub fn is_tool_allowed(&self, server_name: &str, tool_name: &str) -> bool {
        match self.servers.get(server_name) {
            Some(entry) => match &entry.allowed_tools {
                None => true,
                Some(set) => set.contains(tool_name),
            },
            None => false,
        }
    }

    /// Total number of connected servers (excludes failed ones).
    pub fn connected_count(&self) -> usize {
        self.servers.len()
    }

    /// Total number of configured servers (connected + failed).
    pub fn configured_count(&self) -> usize {
        self.statuses.len()
    }
}

/// Compute the per-server allow-set. `["*"]` (or any list containing
/// `"*"`) yields `None` (= wildcard). Empty list yields `Some(empty)`
/// — admit nothing, a deliberate choice (an empty allow-list is
/// reasonable for "configured but locked down").
fn compute_allow_set(configured: &[String]) -> Option<HashSet<String>> {
    if configured.iter().any(|s| s == "*") {
        None
    } else {
        Some(configured.iter().cloned().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_set_wildcard_yields_none() {
        assert!(compute_allow_set(&["*".into()]).is_none());
    }

    #[test]
    fn allow_set_wildcard_among_others_still_wildcards() {
        // Documented behavior: any `*` in the list trumps explicit names.
        let result = compute_allow_set(&["create_issue".into(), "*".into()]);
        assert!(result.is_none());
    }

    #[test]
    fn allow_set_explicit_names_yields_some() {
        let result = compute_allow_set(&["create_issue".into(), "list_prs".into()]);
        let set = result.unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.contains("create_issue"));
        assert!(set.contains("list_prs"));
    }

    #[test]
    fn allow_set_empty_yields_empty_some() {
        let result = compute_allow_set(&[]);
        let set = result.unwrap();
        assert!(set.is_empty());
    }

    fn test_secret_store() -> Arc<crate::security::SecretStore> {
        Arc::new(crate::security::SecretStore::new(
            std::path::Path::new(""),
            false,
        ))
    }

    #[tokio::test]
    async fn connect_all_with_no_configs_yields_empty_registry() {
        let reg = McpRegistry::connect_all(&[], test_secret_store()).await;
        assert_eq!(reg.connected_count(), 0);
        assert_eq!(reg.configured_count(), 0);
        assert!(reg.connected_names().is_empty());
    }

    #[tokio::test]
    async fn connect_all_with_invalid_command_records_failed_status() {
        // Spawn a known-bad command — should land in Failed bucket
        // without poisoning the rest of startup.
        let cfg = McpServerConfig {
            name: "ghost".into(),
            transport: crate::config::McpTransport::Stdio,
            command: "this-binary-does-not-exist-xyz123".into(),
            args: vec![],
            env: HashMap::new(),
            allowed_tools: vec!["*".into()],
            startup_timeout_ms: 200,
            request_timeout_ms: 1000,
        };
        let reg = McpRegistry::connect_all(&[cfg], test_secret_store()).await;
        assert_eq!(reg.connected_count(), 0);
        assert_eq!(reg.configured_count(), 1);
        match reg.status("ghost").expect("status present") {
            ServerStatus::Failed { error } => {
                assert!(!error.is_empty());
            }
            ServerStatus::Connected => panic!("nonexistent command should fail"),
        }
        assert!(reg.get("ghost").is_none());
    }

    #[tokio::test]
    async fn connect_all_with_http_unreachable_url_records_failed_status() {
        // PR #76: HTTP transport — point at a guaranteed-unreachable
        // URL; entry must bucket as Failed without poisoning startup
        // and without crashing the registry.
        let cfg = McpServerConfig {
            name: "remote-ghost".into(),
            transport: crate::config::McpTransport::Http {
                url: "http://127.0.0.1:1/this-port-should-never-listen".into(),
                bearer_token: None,
                headers: HashMap::new(),
            },
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            allowed_tools: vec!["*".into()],
            startup_timeout_ms: 300,
            request_timeout_ms: 500,
        };
        let reg = McpRegistry::connect_all(&[cfg], test_secret_store()).await;
        assert_eq!(reg.connected_count(), 0);
        assert_eq!(reg.configured_count(), 1);
        assert!(matches!(
            reg.status("remote-ghost").expect("status present"),
            ServerStatus::Failed { .. }
        ));
    }

    #[tokio::test]
    async fn is_tool_allowed_returns_false_for_unknown_server() {
        let reg = McpRegistry::connect_all(&[], test_secret_store()).await;
        assert!(!reg.is_tool_allowed("ghost", "any_tool"));
    }
}
