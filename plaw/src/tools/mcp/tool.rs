//! `mcp_call` proxy tool — single LLM-facing entry to every connected
//! MCP server.
//!
//! Args: `{server: String, tool: String, arguments: Object}`.
//! Looks up the server in [`McpRegistry`], checks the per-server
//! allowed-tools list, and forwards the call. Errors at any layer
//! (unknown server, disallowed tool, JSON-RPC protocol error, MCP
//! `isError: true`) are returned as `ToolResult { success: false }`
//! with a clear message so the LLM can retry intelligently.

use super::registry::{McpRegistry, ServerStatus};
use crate::tools::traits::{
    SideEffectClass, Tool, ToolResult, ToolResultValue, TypedToolResult,
};
use async_trait::async_trait;
use std::sync::Arc;

pub struct McpTool {
    registry: Arc<McpRegistry>,
}

impl McpTool {
    pub fn new(registry: Arc<McpRegistry>) -> Self {
        Self { registry }
    }

    /// Build a description string that lists the connected servers
    /// and (when discoverable) the tools each one advertises. This
    /// is what the LLM sees in its tool catalog — the description
    /// IS the API documentation here.
    fn build_description(&self) -> String {
        let mut out = String::from(
            "Call a tool exposed by a configured Model Context Protocol \
             (MCP) server. Pass `server` (the configured server name), \
             `tool` (the tool advertised by that server), and \
             `arguments` (a JSON object matching the tool's input schema).",
        );
        let connected = self.registry.connected_names();
        if connected.is_empty() {
            out.push_str("\n\nNo MCP servers are currently connected.");
            // Surface failed ones too so user sees the problem in logs.
            let failed: Vec<&String> = self
                .registry
                .statuses()
                .iter()
                .filter_map(|(name, status)| match status {
                    ServerStatus::Failed { .. } => Some(name),
                    _ => None,
                })
                .collect();
            if !failed.is_empty() {
                out.push_str(" Configured servers that failed to start: ");
                let mut names: Vec<&str> = failed.iter().map(|s| s.as_str()).collect();
                names.sort();
                out.push_str(&names.join(", "));
                out.push('.');
            }
            return out;
        }

        out.push_str("\n\nConnected servers:");
        for name in &connected {
            if let Some(client) = self.registry.get(name) {
                let info = client.initialize_result();
                let title = if info.server_info.name.is_empty() {
                    name.as_str()
                } else {
                    info.server_info.name.as_str()
                };
                let version = if info.server_info.version.is_empty() {
                    "?"
                } else {
                    info.server_info.version.as_str()
                };
                out.push_str(&format!("\n  - {name}: {title} v{version}"));
                if let Some(instr) = &info.instructions {
                    if !instr.is_empty() {
                        out.push_str(" — ");
                        out.push_str(instr);
                    }
                }
            }
        }
        out
    }
}

#[async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        "mcp_call"
    }

    fn description(&self) -> &str {
        // The description is dynamic (depends on which servers
        // connected at startup); since the Tool trait requires a
        // `&str`, we leak the string once at first call. Acceptable
        // — the description is computed once at startup and stable
        // for the agent loop's lifetime. Avoids holding a Mutex<String>
        // for every Tool::description() call.
        use std::sync::OnceLock;
        static DESC_CELL: OnceLock<String> = OnceLock::new();
        DESC_CELL.get_or_init(|| self.build_description()).as_str()
    }

    fn parameters_schema(&self) -> serde_json::Value {
        let connected = self.registry.connected_names();
        let server_param = if connected.is_empty() {
            serde_json::json!({
                "type": "string",
                "description": "Name of the MCP server. No servers are \
                    currently connected."
            })
        } else {
            serde_json::json!({
                "type": "string",
                "description": format!(
                    "Name of the MCP server. Connected: {}.",
                    connected.join(", ")
                ),
                "enum": connected,
            })
        };
        serde_json::json!({
            "type": "object",
            "properties": {
                "server": server_param,
                "tool": {
                    "type": "string",
                    "description": "Name of the tool advertised by the server. \
                        Inspect available tools via the server's documentation \
                        or by calling its `tools/list` (not exposed in Phase 0)."
                },
                "arguments": {
                    "type": "object",
                    "description": "JSON object matching the tool's input \
                        schema. Pass `{}` if the tool takes no arguments."
                }
            },
            "required": ["server", "tool", "arguments"]
        })
    }

    fn side_effects(&self) -> SideEffectClass {
        // MCP tools fan out to subprocesses that can do anything —
        // most conservative classification.
        SideEffectClass::Spawn
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let Some(server) = args.get("server").and_then(|v| v.as_str()) else {
            return Ok(error_result("missing required `server` parameter"));
        };
        let Some(tool_name) = args.get("tool").and_then(|v| v.as_str()) else {
            return Ok(error_result("missing required `tool` parameter"));
        };
        let tool_args = args
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        let Some(client) = self.registry.get(server) else {
            return Ok(error_result(&format!(
                "MCP server '{server}' is not connected. Available: {}",
                self.registry.connected_names().join(", ")
            )));
        };

        if !self.registry.is_tool_allowed(server, tool_name) {
            return Ok(error_result(&format!(
                "tool '{tool_name}' is not in server '{server}' allow-list. \
                 Edit `[[mcp.servers]] allowed_tools` to permit it."
            )));
        }

        match client.call_tool(tool_name, tool_args).await {
            Ok(result) => {
                let body = if result.content.is_empty() {
                    "[empty response]".to_string()
                } else {
                    result
                        .content
                        .iter()
                        .map(|c| c.render())
                        .collect::<Vec<_>>()
                        .join("\n")
                };
                if result.is_error {
                    Ok(ToolResult {
                        success: false,
                        output: String::new(),
                        error: Some(format!("MCP tool '{server}.{tool_name}' returned error: {body}")),
                    })
                } else {
                    Ok(ToolResult {
                        success: true,
                        output: body,
                        error: None,
                    })
                }
            }
            Err(e) => Ok(error_result(&format!(
                "MCP call '{server}.{tool_name}' failed: {e:#}"
            ))),
        }
    }

    async fn execute_typed(
        &self,
        args: serde_json::Value,
    ) -> anyhow::Result<TypedToolResult> {
        let server = args
            .get("server")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let tool_name = args
            .get("tool")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if server.is_empty() || tool_name.is_empty() {
            let result = self.execute(args).await?;
            return Ok(TypedToolResult::untyped(result));
        }

        let Some(client) = self.registry.get(&server) else {
            let result = self.execute(args).await?;
            return Ok(TypedToolResult::untyped(result));
        };
        if !self.registry.is_tool_allowed(&server, &tool_name) {
            let result = self.execute(args).await?;
            return Ok(TypedToolResult::untyped(result));
        }

        let tool_args = args
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        match client.call_tool(&tool_name, tool_args).await {
            Ok(call_result) => {
                let body = call_result
                    .content
                    .iter()
                    .map(|c| c.render())
                    .collect::<Vec<_>>()
                    .join("\n");
                let value = ToolResultValue::Json {
                    data: serde_json::json!({
                        "server": server,
                        "tool": tool_name,
                        "is_error": call_result.is_error,
                        "content_blocks": call_result.content.len(),
                    }),
                };
                if call_result.is_error {
                    Ok(TypedToolResult {
                        result: ToolResult {
                            success: false,
                            output: String::new(),
                            error: Some(format!(
                                "MCP tool '{server}.{tool_name}' returned error: {body}"
                            )),
                        },
                        value: Some(value),
                    })
                } else {
                    Ok(TypedToolResult {
                        result: ToolResult {
                            success: true,
                            output: body,
                            error: None,
                        },
                        value: Some(value),
                    })
                }
            }
            Err(e) => Ok(TypedToolResult::untyped(error_result(&format!(
                "MCP call '{server}.{tool_name}' failed: {e:#}"
            )))),
        }
    }
}

fn error_result(msg: &str) -> ToolResult {
    ToolResult {
        success: false,
        output: String::new(),
        error: Some(msg.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::McpServerConfig;
    use std::collections::HashMap;

    fn test_secret_store() -> Arc<crate::security::SecretStore> {
        Arc::new(crate::security::SecretStore::new(
            std::path::Path::new(""),
            false,
        ))
    }

    async fn empty_registry() -> Arc<McpRegistry> {
        Arc::new(McpRegistry::connect_all(&[], test_secret_store()).await)
    }

    #[tokio::test]
    async fn tool_metadata_when_no_servers_connected() {
        let tool = McpTool::new(empty_registry().await);
        assert_eq!(tool.name(), "mcp_call");
        assert_eq!(tool.side_effects(), SideEffectClass::Spawn);
        let schema = tool.parameters_schema();
        // No enum constraint when registry is empty.
        assert!(schema["properties"]["server"].get("enum").is_none());
    }

    #[tokio::test]
    async fn execute_errors_when_server_unknown() {
        let tool = McpTool::new(empty_registry().await);
        let result = tool
            .execute(serde_json::json!({
                "server": "ghost",
                "tool": "anything",
                "arguments": {}
            }))
            .await
            .unwrap();
        assert!(!result.success);
        let err = result.error.unwrap();
        assert!(err.contains("'ghost' is not connected"));
    }

    #[tokio::test]
    async fn execute_errors_when_args_missing() {
        let tool = McpTool::new(empty_registry().await);
        let r = tool
            .execute(serde_json::json!({"server": "x"}))
            .await
            .unwrap();
        assert!(!r.success);
        assert!(r.error.unwrap().contains("`tool`"));
    }

    #[tokio::test]
    async fn description_lists_failed_servers_when_no_connections_succeed() {
        let cfg = McpServerConfig {
            name: "bad-server".into(),
            transport: crate::config::McpTransport::Stdio,
            command: "this-command-does-not-exist-zzz".into(),
            args: vec![],
            env: HashMap::new(),
            allowed_tools: vec!["*".into()],
            startup_timeout_ms: 100,
            request_timeout_ms: 500,
        };
        let registry = Arc::new(McpRegistry::connect_all(&[cfg], test_secret_store()).await);
        let tool = McpTool::new(registry);
        let desc = tool.build_description();
        assert!(desc.contains("No MCP servers are currently connected"));
        assert!(desc.contains("bad-server"));
    }
}
