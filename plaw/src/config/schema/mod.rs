// Sub-module splits of the historical 10K-LoC `schema.rs` mega-file
// (see [[project-2026-05-23-four-lens-synthesis]] Top-4 #3b). Each split
// re-exports its public surface so `crate::config::ProxyConfig` etc.
// keep working without consumer churn. Add new splits here as the
// remaining sections (channels, providers, tools, runtime, memory) get
// peeled off in follow-up PRs.
mod proxy;
pub use proxy::{
    apply_runtime_proxy_to_builder, build_runtime_proxy_client,
    build_runtime_proxy_client_with_timeouts, runtime_proxy_config, set_runtime_proxy_config,
};
pub use proxy::{ProxyConfig, ProxyScope};
mod chain_of_verification;
mod research;
pub use chain_of_verification::ChainOfVerificationConfig;
pub use research::{ResearchPhaseConfig, ResearchTrigger};
mod runtime;
pub use runtime::{
    DockerRuntimeConfig, RuntimeConfig, WasmCapabilityEscalationMode, WasmModuleHashPolicy,
    WasmRuntimeConfig, WasmSecurityConfig,
};
// Internal helpers used by Config::sync_proxy_runtime + the in-file
// tests — kept pub(super) in proxy.rs so they don't leak to non-schema
// consumers.
use proxy::{
    clear_proxy_env_pair, normalize_no_proxy_list, normalize_proxy_url_option,
    normalize_service_list, parse_proxy_enabled, parse_proxy_scope,
};
#[cfg(test)]
use proxy::{
    clear_runtime_proxy_client_cache, runtime_proxy_cache_key, runtime_proxy_client_cache,
};

use crate::config::traits::ChannelConfig;
use crate::providers::{is_glm_alias, is_zai_alias};
use crate::security::{AutonomyLevel, DomainMatcher};
use anyhow::{Context, Result};
use directories::UserDirs;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
#[cfg(unix)]
use tokio::fs::File;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

/// Default provider written to `Config::default()` when no `config.toml`
/// exists. Aligned with plaw/CLAUDE.md §"AI 模型配置" — DeepSeek V4 Pro
/// is the current China-direct recommended default. Per
/// [[project-model-agnostic-invariant]] this is a value not a code
/// branch: users override via config file with zero code changes.
///
/// Also used by `apply_env_overrides` as the "untouched fallback"
/// marker — the legacy `PROVIDER` env var only overrides when
/// `default_provider` still matches this fallback (i.e. user hasn't
/// explicitly chosen a different provider in their config).
pub const DEFAULT_PROVIDER_FALLBACK: &str = "deepseek";

/// Default model paired with `DEFAULT_PROVIDER_FALLBACK`. Override
/// independently via `default_model` in config.toml.
pub const DEFAULT_MODEL_FALLBACK: &str = "deepseek-v4-pro";

// ── Top-level config ──────────────────────────────────────────────

/// Protocol mode for `custom:` OpenAI-compatible providers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderApiMode {
    /// Default behavior: `/chat/completions` first, optional `/responses`
    /// fallback when supported.
    OpenAiChatCompletions,
    /// Responses-first behavior: call `/responses` directly.
    OpenAiResponses,
}

impl ProviderApiMode {
    pub fn as_compatible_mode(self) -> crate::providers::compatible::CompatibleApiMode {
        match self {
            Self::OpenAiChatCompletions => {
                crate::providers::compatible::CompatibleApiMode::OpenAiChatCompletions
            }
            Self::OpenAiResponses => {
                crate::providers::compatible::CompatibleApiMode::OpenAiResponses
            }
        }
    }
}

/// Top-level Plaw configuration, loaded from `config.toml`.
///
/// Resolution order: `PLAW_WORKSPACE` env → `active_workspace.toml` marker → `~/.plaw/config.toml`.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct Config {
    /// Workspace directory - computed from home, not serialized
    #[serde(skip)]
    pub workspace_dir: PathBuf,
    /// Path to config.toml - computed from home, not serialized
    #[serde(skip)]
    pub config_path: PathBuf,
    /// API key for the selected provider. Overridden by `PLAW_API_KEY` or `API_KEY` env vars.
    pub api_key: Option<String>,
    /// Base URL override for provider API (e.g. "http://10.0.0.1:11434" for remote Ollama)
    pub api_url: Option<String>,
    /// Default provider ID or alias (e.g. `"openrouter"`, `"ollama"`, `"anthropic"`). Default: `"openrouter"`.
    #[serde(alias = "model_provider")]
    pub default_provider: Option<String>,
    /// Optional API protocol mode for `custom:` providers.
    #[serde(default)]
    pub provider_api: Option<ProviderApiMode>,
    /// Default model routed through the selected provider (e.g. `"anthropic/claude-sonnet-4-6"`).
    #[serde(alias = "model")]
    pub default_model: Option<String>,
    /// Optional named provider profiles keyed by id (Codex app-server compatible layout).
    #[serde(default)]
    pub model_providers: HashMap<String, ModelProviderConfig>,
    /// Provider-specific behavior overrides (`[provider]`).
    #[serde(default)]
    pub provider: ProviderConfig,
    /// Default model temperature (0.0–2.0). Default: `0.7`.
    pub default_temperature: f64,

    /// Observability backend configuration (`[observability]`).
    #[serde(default)]
    pub observability: ObservabilityConfig,

    /// Autonomy and security policy configuration (`[autonomy]`).
    #[serde(default)]
    pub autonomy: AutonomyConfig,

    /// Security subsystem configuration (`[security]`).
    #[serde(default)]
    pub security: SecurityConfig,

    /// Runtime adapter configuration (`[runtime]`). Controls native vs Docker execution.
    #[serde(default)]
    pub runtime: RuntimeConfig,

    /// Research phase configuration (`[research]`). Proactive information gathering.
    #[serde(default)]
    pub research: ResearchPhaseConfig,

    /// Reliability settings: retries, fallback providers, backoff (`[reliability]`).
    #[serde(default)]
    pub reliability: ReliabilityConfig,

    /// Scheduler configuration for periodic task execution (`[scheduler]`).
    #[serde(default)]
    pub scheduler: SchedulerConfig,

    /// Agent orchestration settings (`[agent]`).
    #[serde(default)]
    pub agent: AgentConfig,

    /// Skills loading and community repository behavior (`[skills]`).
    #[serde(default)]
    pub skills: SkillsConfig,

    /// Model routing rules — route `hint:<name>` to specific provider+model combos.
    #[serde(default)]
    pub model_routes: Vec<ModelRouteConfig>,

    /// Embedding routing rules — route `hint:<name>` to specific provider+model combos.
    #[serde(default)]
    pub embedding_routes: Vec<EmbeddingRouteConfig>,

    /// Automatic query classification — maps user messages to model hints.
    #[serde(default)]
    pub query_classification: QueryClassificationConfig,

    /// Heartbeat configuration for periodic health pings (`[heartbeat]`).
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,

    /// Cron job configuration (`[cron]`).
    #[serde(default)]
    pub cron: CronConfig,

    /// Goal loop configuration for autonomous long-term goal execution (`[goal_loop]`).
    #[serde(default)]
    pub goal_loop: GoalLoopConfig,

    /// Channel configurations: Telegram, Discord, Slack, etc. (`[channels_config]`).
    #[serde(default)]
    pub channels_config: ChannelsConfig,

    /// Memory backend configuration: sqlite, markdown, embeddings (`[memory]`).
    #[serde(default)]
    pub memory: MemoryConfig,

    /// Persistent storage provider configuration (`[storage]`).
    #[serde(default)]
    pub storage: StorageConfig,

    /// Tunnel configuration for exposing the gateway publicly (`[tunnel]`).
    #[serde(default)]
    pub tunnel: TunnelConfig,

    /// Gateway server configuration: host, port, pairing, rate limits (`[gateway]`).
    #[serde(default)]
    pub gateway: GatewayConfig,

    /// Composio managed OAuth tools integration (`[composio]`).
    #[serde(default)]
    pub composio: ComposioConfig,

    /// Secrets encryption configuration (`[secrets]`).
    #[serde(default)]
    pub secrets: SecretsConfig,

    /// Browser automation configuration (`[browser]`).
    #[serde(default)]
    pub browser: BrowserConfig,

    /// HTTP request tool configuration (`[http_request]`).
    #[serde(default)]
    pub http_request: HttpRequestConfig,

    /// Multimodal (image) handling configuration (`[multimodal]`).
    #[serde(default)]
    pub multimodal: MultimodalConfig,

    /// Web fetch tool configuration (`[web_fetch]`).
    #[serde(default)]
    pub web_fetch: WebFetchConfig,

    /// Web search tool configuration (`[web_search]`).
    #[serde(default)]
    pub web_search: WebSearchConfig,

    /// Proxy configuration for outbound HTTP/HTTPS/SOCKS5 traffic (`[proxy]`).
    #[serde(default)]
    pub proxy: ProxyConfig,

    /// Identity format configuration: OpenClaw or AIEOS (`[identity]`).
    #[serde(default)]
    pub identity: IdentityConfig,

    /// Cost tracking and budget enforcement configuration (`[cost]`).
    #[serde(default)]
    pub cost: CostConfig,

    /// Peripheral board configuration for hardware integration (`[peripherals]`).
    #[serde(default)]
    pub peripherals: PeripheralsConfig,

    /// Delegate agent configurations for multi-agent workflows.
    #[serde(default)]
    pub agents: HashMap<String, DelegateAgentConfig>,

    /// Pre-defined deterministic multi-stage workflows (planner →
    /// researcher → coder → reporter style) that the main LLM can
    /// invoke as a single `run_pipeline` tool call. Each entry is keyed
    /// by pipeline name and references the same `[agents.*]` registry.
    /// See [`crate::agent::pipeline`] for runtime semantics.
    #[serde(default)]
    pub pipelines: HashMap<String, PipelineConfig>,

    /// Model Context Protocol (MCP) client configuration. When enabled,
    /// plaw spawns the configured MCP servers at startup and exposes
    /// them via a single `mcp_call` proxy tool to the agent loop. See
    /// [`crate::tools::mcp`] for the on-the-wire protocol details.
    #[serde(default)]
    pub mcp: McpConfig,

    /// Delegate coordination runtime configuration (`[coordination]`).
    #[serde(default)]
    pub coordination: CoordinationConfig,

    /// Hooks configuration (lifecycle hooks and built-in hook toggles).
    #[serde(default)]
    pub hooks: HooksConfig,

    /// Hardware configuration (wizard-driven physical world setup).
    #[serde(default)]
    pub hardware: HardwareConfig,

    /// Voice transcription configuration (Whisper API via Groq).
    #[serde(default)]
    pub transcription: TranscriptionConfig,

    /// Inter-process agent communication (`[agents_ipc]`).
    #[serde(default)]
    pub agents_ipc: AgentsIpcConfig,

    /// Repository map configuration (`[repo_map]`). Aider-style code context
    /// injected once per WS session when enabled.
    #[serde(default)]
    pub repo_map: RepoMapConfig,

    /// Edit-with-linter configuration (`[edit_linter]`). Tree-sitter parse
    /// check around `file_write` / `file_edit`. Default mode `warn`.
    #[serde(default)]
    pub edit_linter: EditLinterConfig,

    /// Chain-of-Verification configuration (`[chain_of_verification]`).
    /// Post-response verifier for factual-lookup turns. Default off.
    #[serde(default)]
    pub chain_of_verification: ChainOfVerificationConfig,

    /// Vision support override for the active provider/model.
    /// - `None` (default): use provider's built-in default
    /// - `Some(true)`: force vision support on (e.g. Ollama running llava)
    /// - `Some(false)`: force vision support off
    #[serde(default)]
    pub model_support_vision: Option<bool>,
}

/// Named provider profile definition compatible with Codex app-server style config.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ModelProviderConfig {
    /// Optional provider type/name override (e.g. "openai", "openai-codex", or custom profile id).
    #[serde(default)]
    pub name: Option<String>,
    /// Optional base URL for OpenAI-compatible endpoints.
    #[serde(default)]
    pub base_url: Option<String>,
    /// Provider protocol variant ("responses" or "chat_completions").
    #[serde(default)]
    pub wire_api: Option<String>,
    /// If true, load OpenAI auth material (OPENAI_API_KEY or ~/.codex/auth.json).
    #[serde(default)]
    pub requires_openai_auth: bool,
}

/// Provider behavior overrides (`[provider]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct ProviderConfig {
    /// Optional reasoning level override for providers that support explicit levels
    /// (e.g. OpenAI Codex `/responses` reasoning effort).
    #[serde(default)]
    pub reasoning_level: Option<String>,
}

// ── Delegate Agents ──────────────────────────────────────────────

/// Configuration for a delegate sub-agent used by the `delegate` tool.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub struct DelegateAgentConfig {
    /// Provider name (e.g. "ollama", "openrouter", "anthropic").
    /// Empty string means "inherit from main config default_provider".
    #[serde(default)]
    pub provider: String,
    /// Model name. Empty string means "inherit from main config default_model".
    #[serde(default)]
    pub model: String,
    /// Optional system prompt for the sub-agent
    #[serde(default)]
    pub system_prompt: Option<String>,
    /// Optional API key override
    #[serde(default)]
    pub api_key: Option<String>,
    /// Temperature override
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Max recursion depth for nested delegation
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    /// Enable agentic sub-agent mode (multi-turn tool-call loop).
    #[serde(default)]
    pub agentic: bool,
    /// Allowlist of tool names available to the sub-agent in agentic mode.
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// Maximum tool-call iterations in agentic mode.
    #[serde(default = "default_max_tool_iterations")]
    pub max_iterations: usize,
}

fn default_max_depth() -> u32 {
    3
}

fn default_max_tool_iterations() -> usize {
    10
}

impl std::fmt::Debug for DelegateAgentConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegateAgentConfig")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("system_prompt", &self.system_prompt)
            .field("api_key_configured", &self.api_key.is_some())
            .field("temperature", &self.temperature)
            .field("max_depth", &self.max_depth)
            .field("agentic", &self.agentic)
            .field("allowed_tools", &self.allowed_tools)
            .field("max_iterations", &self.max_iterations)
            .finish()
    }
}

// ── Pipelines ────────────────────────────────────────────────────

/// Deterministic multi-stage workflow over the `[agents.*]` registry.
///
/// Each stage dispatches to a named delegate agent with a prompt
/// template that may reference `{user_message}` (the initial input
/// passed to the pipeline) and `{prior.<output_key>}` (outputs of
/// earlier stages in the same pipeline). Stage outputs accumulate in
/// a `HashMap<String, String>` blackboard; the final stage's output is
/// returned as the pipeline's result.
///
/// Design DNA borrowed from DeerFlow v1's Plan-then-execute pattern
/// (per workflow `deerflow-pattern-discovery` lens B): structured
/// inter-stage handoff via accumulated observations, role-specialized
/// prompts, anti-hallucination through compressed context. Diverges
/// from DeerFlow by being **config-driven freeform** (no fixed
/// Planner/Researcher/Coder/Reporter enum) so users define their own
/// taxonomies — matching plaw's model-agnostic + provider-agnostic
/// invariants. See [[framework-adoption-decision-2026-05-30]].
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PipelineConfig {
    /// Ordered list of stages. Each stage runs once, in declaration
    /// order, with template substitution against the blackboard.
    #[serde(default)]
    pub stages: Vec<PipelineStage>,
    /// What to do when a stage fails. Default: `abort` — first
    /// failure short-circuits the pipeline and surfaces the error.
    #[serde(default)]
    pub on_error: PipelineErrorPolicy,
}

/// One stage in a [`PipelineConfig`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PipelineStage {
    /// Name of a delegate agent registered under `[agents.<name>]`.
    /// Errors at execution time if the name is not in the registry.
    pub agent: String,
    /// Prompt template. Supports two placeholder forms:
    /// - `{user_message}` — the initial input to the pipeline.
    /// - `{prior.<output_key>}` — output of an earlier stage; the
    ///   key must match a previous stage's `output_key`.
    pub prompt: String,
    /// Key under which this stage's output is stored on the blackboard.
    /// Later stages reference it via `{prior.<output_key>}`. Must be
    /// unique within the pipeline; collisions are detected at runtime.
    pub output_key: String,
    /// Optional `context` field passed to the delegate tool. Same
    /// template-substitution rules as `prompt`.
    #[serde(default)]
    pub context: Option<String>,
}

/// Failure policy for [`PipelineConfig`].
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PipelineErrorPolicy {
    /// First stage failure aborts the pipeline (default — safe).
    #[default]
    Abort,
    /// Failed stages contribute an error message under their
    /// `output_key` but the pipeline continues. Useful for
    /// best-effort gather-and-synthesize workflows.
    Continue,
}

// ── MCP ──────────────────────────────────────────────────────────

/// Model Context Protocol (MCP) client configuration (`[mcp]` section).
///
/// MCP is an open standard published by Anthropic that lets external
/// processes expose tools to LLM applications via JSON-RPC over stdio
/// (or HTTP/SSE — plaw Phase 0 only implements stdio). Each configured
/// server is spawned as a subprocess at agent startup; its advertised
/// tools become callable from the LLM via a single proxy tool
/// `mcp_call(server, tool, arguments)`.
///
/// Phase 0 scope: stdio transport only, `tools/list` + `tools/call`,
/// no `resources` / `prompts` / `sampling`. See spec at
/// <https://modelcontextprotocol.io/specification/2025-06-18>.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct McpConfig {
    /// Global enable flag. When `false`, no MCP servers are spawned
    /// and the `mcp_call` proxy tool is not registered (zero overhead).
    #[serde(default)]
    pub enabled: bool,
    /// List of MCP servers to spawn. Order is not significant; servers
    /// are addressed by `name` in tool calls.
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

/// One MCP server entry (`[[mcp.servers]]` array element).
///
/// Defines the subprocess to spawn and the policy applied to its
/// advertised tools. Each server must have a unique `name`; the name
/// becomes the `server` argument of the proxy tool's `mcp_call`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct McpServerConfig {
    /// Stable identifier used to address this server in `mcp_call`.
    /// Conventional shape: lowercase alphanumeric + hyphen, e.g.
    /// `"github"`, `"filesystem"`, `"sqlite-local"`.
    pub name: String,
    /// Transport selector. Defaults to [`McpTransport::Stdio`] so PR #63
    /// configs deserialise byte-identically. Set the nested
    /// `[mcp.servers.transport]` table with `kind = "http"` to switch
    /// to the Phase-0 Streamable HTTP transport (PR #76).
    #[serde(default)]
    pub transport: McpTransport,
    /// Command to execute when `transport.kind = "stdio"`. Same semantics
    /// as `tokio::process::Command::new`. Typical: `"npx"` for npm-
    /// published servers, `"uvx"` for Python. Ignored when
    /// `transport.kind = "http"` — kept defaultable so HTTP entries do
    /// not need to spell out an unused field.
    #[serde(default)]
    pub command: String,
    /// Arguments passed to `command`. Often holds the server's
    /// package id, e.g. `["-y", "@modelcontextprotocol/server-github"]`.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables visible to the spawned subprocess.
    /// Inherits plaw's environment by default; entries here override
    /// or add to it. Use for tokens / API keys / paths the MCP server
    /// reads at startup (e.g. `GITHUB_TOKEN`, `OPENAI_API_KEY`).
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Whitelist of tool names this server is allowed to expose. The
    /// special value `"*"` admits every tool the server advertises.
    /// Tools advertised by the server but not in this list are dropped
    /// from `tools/list` and reject `tools/call` with a clear error.
    /// Default: `["*"]` (trust the server).
    #[serde(default = "default_mcp_allowed_tools")]
    pub allowed_tools: Vec<String>,
    /// Maximum time the initial `initialize` handshake may take before
    /// the server is marked as failed and excluded from the registry.
    /// Default: 10 seconds.
    #[serde(default = "default_mcp_startup_timeout_ms")]
    pub startup_timeout_ms: u64,
    /// Maximum time for a single `tools/call` request before plaw
    /// sends `notifications/cancelled` and surfaces a timeout error.
    /// Default: 60 seconds (MCP servers vary widely; some shell out
    /// to slow external APIs).
    #[serde(default = "default_mcp_request_timeout_ms")]
    pub request_timeout_ms: u64,
}

/// Transport selector for a single `[[mcp.servers]]` entry.
///
/// Default = [`Self::Stdio`] which preserves PR #63 wire format
/// byte-identically. The Phase-0 HTTP variant (PR #76) supports
/// sync request/response only — `text/event-stream` response bodies
/// are rejected and OAuth is unsupported. Streamable bidirectional
/// (SSE response body, standalone GET, OAuth 2.1 + PKCE) is deferred
/// to PR #77.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpTransport {
    /// stdio JSON-RPC subprocess. Connection params come from the
    /// sibling `command` / `args` / `env` fields on
    /// [`McpServerConfig`] (kept flat so legacy TOML works unchanged).
    Stdio,
    /// Streamable HTTP transport per MCP spec 2025-06-18, Phase 0
    /// subset. POSTs JSON-RPC envelopes to a single URL; rejects
    /// `text/event-stream` response bodies. Static `bearer_token`
    /// reaches self-hosted servers; full OAuth 2.1 is Phase 1.
    Http {
        /// Server endpoint URL. Required for the HTTP transport.
        url: String,
        /// Optional bearer token. Stored as
        /// [`crate::security::Secret`] so it never lands in
        /// `tracing` events in plaintext.
        #[serde(default)]
        bearer_token: Option<crate::security::Secret>,
        /// Optional custom headers (e.g. `X-Api-Key`). Values are
        /// treated as opaque — do NOT log. (Asymmetry vs
        /// `bearer_token` documented; Phase 1 may upgrade to
        /// `HashMap<String, Secret>` if header leaks become a real
        /// concern.)
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}

impl Default for McpTransport {
    fn default() -> Self {
        Self::Stdio
    }
}

fn default_mcp_allowed_tools() -> Vec<String> {
    vec!["*".into()]
}

fn default_mcp_startup_timeout_ms() -> u64 {
    10_000
}

fn default_mcp_request_timeout_ms() -> u64 {
    60_000
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let model_provider_ids: Vec<&str> =
            self.model_providers.keys().map(String::as_str).collect();
        let delegate_agent_ids: Vec<&str> = self.agents.keys().map(String::as_str).collect();
        let enabled_channel_count = [
            self.channels_config.telegram.is_some(),
            self.channels_config.discord.is_some(),
            self.channels_config.slack.is_some(),
            self.channels_config.mattermost.is_some(),
            self.channels_config.webhook.is_some(),
            self.channels_config.imessage.is_some(),
            self.channels_config.matrix.is_some(),
            self.channels_config.signal.is_some(),
            self.channels_config.whatsapp.is_some(),
            self.channels_config.linq.is_some(),
            self.channels_config.wati.is_some(),
            self.channels_config.nextcloud_talk.is_some(),
            self.channels_config.email.is_some(),
            self.channels_config.irc.is_some(),
            self.channels_config.lark.is_some(),
            self.channels_config.feishu.is_some(),
            self.channels_config.dingtalk.is_some(),
            self.channels_config.qq.is_some(),
            self.channels_config.nostr.is_some(),
            self.channels_config.clawdtalk.is_some(),
        ]
        .into_iter()
        .filter(|enabled| *enabled)
        .count();

        f.debug_struct("Config")
            .field("workspace_dir", &self.workspace_dir)
            .field("config_path", &self.config_path)
            .field("api_key_configured", &self.api_key.is_some())
            .field("api_url_configured", &self.api_url.is_some())
            .field("default_provider", &self.default_provider)
            .field("provider_api", &self.provider_api)
            .field("default_model", &self.default_model)
            .field("model_providers", &model_provider_ids)
            .field("default_temperature", &self.default_temperature)
            .field("model_routes_count", &self.model_routes.len())
            .field("embedding_routes_count", &self.embedding_routes.len())
            .field("delegate_agents", &delegate_agent_ids)
            .field("cli_channel_enabled", &self.channels_config.cli)
            .field("enabled_channels_count", &enabled_channel_count)
            .field("sensitive_sections", &"***REDACTED***")
            .finish_non_exhaustive()
    }
}

// ── Hardware Config (wizard-driven) ─────────────────────────────

/// Hardware transport mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
pub enum HardwareTransport {
    #[default]
    None,
    Native,
    Serial,
    Probe,
}

impl std::fmt::Display for HardwareTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Native => write!(f, "native"),
            Self::Serial => write!(f, "serial"),
            Self::Probe => write!(f, "probe"),
        }
    }
}

/// Wizard-driven hardware configuration for physical world interaction.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HardwareConfig {
    /// Whether hardware access is enabled
    #[serde(default)]
    pub enabled: bool,
    /// Transport mode
    #[serde(default)]
    pub transport: HardwareTransport,
    /// Serial port path (e.g. "/dev/ttyACM0")
    #[serde(default)]
    pub serial_port: Option<String>,
    /// Serial baud rate
    #[serde(default = "default_baud_rate")]
    pub baud_rate: u32,
    /// Probe target chip (e.g. "STM32F401RE")
    #[serde(default)]
    pub probe_target: Option<String>,
    /// Enable workspace datasheet RAG (index PDF schematics for AI pin lookups)
    #[serde(default)]
    pub workspace_datasheets: bool,
}

fn default_baud_rate() -> u32 {
    115_200
}

impl HardwareConfig {
    /// Return the active transport mode.
    pub fn transport_mode(&self) -> HardwareTransport {
        self.transport.clone()
    }
}

impl Default for HardwareConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            transport: HardwareTransport::None,
            serial_port: None,
            baud_rate: default_baud_rate(),
            probe_target: None,
            workspace_datasheets: false,
        }
    }
}

// ── Transcription ────────────────────────────────────────────────

fn default_transcription_api_url() -> String {
    "https://api.groq.com/openai/v1/audio/transcriptions".into()
}

fn default_transcription_model() -> String {
    "whisper-large-v3-turbo".into()
}

fn default_transcription_max_duration_secs() -> u64 {
    120
}

/// Voice transcription configuration (Whisper API via Groq).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptionConfig {
    /// Enable voice transcription for channels that support it.
    #[serde(default)]
    pub enabled: bool,
    /// Whisper API endpoint URL.
    #[serde(default = "default_transcription_api_url")]
    pub api_url: String,
    /// Whisper model name.
    #[serde(default = "default_transcription_model")]
    pub model: String,
    /// Optional language hint (ISO-639-1, e.g. "en", "ru").
    #[serde(default)]
    pub language: Option<String>,
    /// Maximum voice duration in seconds (messages longer than this are skipped).
    #[serde(default = "default_transcription_max_duration_secs")]
    pub max_duration_secs: u64,
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_url: default_transcription_api_url(),
            model: default_transcription_model(),
            language: None,
            max_duration_secs: default_transcription_max_duration_secs(),
        }
    }
}

// ── Agents IPC ──────────────────────────────────────────────────

fn default_agents_ipc_db_path() -> String {
    "~/.plaw/agents.db".into()
}

fn default_agents_ipc_staleness_secs() -> u64 {
    300
}

/// Inter-process agent communication configuration (`[agents_ipc]` section).
///
/// When enabled, registers IPC tools that let independent Plaw processes
/// on the same host discover each other and exchange messages via a shared
/// SQLite database. Disabled by default (zero overhead when off).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentsIpcConfig {
    /// Enable inter-process agent communication tools.
    #[serde(default)]
    pub enabled: bool,
    /// Path to shared SQLite database (all agents on this host share one file).
    #[serde(default = "default_agents_ipc_db_path")]
    pub db_path: String,
    /// Agents not seen within this window are considered offline (seconds).
    #[serde(default = "default_agents_ipc_staleness_secs")]
    pub staleness_secs: u64,
}

impl Default for AgentsIpcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            db_path: default_agents_ipc_db_path(),
            staleness_secs: default_agents_ipc_staleness_secs(),
        }
    }
}

fn default_coordination_enabled() -> bool {
    true
}

fn default_coordination_lead_agent() -> String {
    "delegate-lead".into()
}

fn default_coordination_max_inbox_messages_per_agent() -> usize {
    256
}

fn default_coordination_max_dead_letters() -> usize {
    256
}

fn default_coordination_max_context_entries() -> usize {
    512
}

fn default_coordination_max_seen_message_ids() -> usize {
    4096
}

/// Delegate coordination runtime configuration (`[coordination]` section).
///
/// Controls typed delegate message-bus integration used by `delegate` and
/// `delegate_coordination_status` tools.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CoordinationConfig {
    /// Enable delegate coordination tracing/runtime bus integration.
    #[serde(default = "default_coordination_enabled")]
    pub enabled: bool,
    /// Logical lead-agent identity used as coordinator sender/recipient.
    #[serde(default = "default_coordination_lead_agent")]
    pub lead_agent: String,
    /// Maximum retained inbox messages per registered agent.
    #[serde(default = "default_coordination_max_inbox_messages_per_agent")]
    pub max_inbox_messages_per_agent: usize,
    /// Maximum retained dead-letter entries.
    #[serde(default = "default_coordination_max_dead_letters")]
    pub max_dead_letters: usize,
    /// Maximum retained shared-context entries (`ContextPatch` state keys).
    #[serde(default = "default_coordination_max_context_entries")]
    pub max_context_entries: usize,
    /// Maximum retained dedupe window size for processed message IDs.
    #[serde(default = "default_coordination_max_seen_message_ids")]
    pub max_seen_message_ids: usize,
}

impl Default for CoordinationConfig {
    fn default() -> Self {
        Self {
            enabled: default_coordination_enabled(),
            lead_agent: default_coordination_lead_agent(),
            max_inbox_messages_per_agent: default_coordination_max_inbox_messages_per_agent(),
            max_dead_letters: default_coordination_max_dead_letters(),
            max_context_entries: default_coordination_max_context_entries(),
            max_seen_message_ids: default_coordination_max_seen_message_ids(),
        }
    }
}

/// Agent orchestration configuration (`[agent]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentConfig {
    /// When true: bootstrap_max_chars=6000, rag_chunk_limit=2. Use for 13B or smaller models.
    #[serde(default)]
    pub compact_context: bool,
    /// Maximum tool-call loop turns per user message. Default: effectively
    /// unlimited (`i64::MAX as usize`) — long autonomous chains are guarded
    /// by per-tool anti-loop caps inside the agent loop, not by this field.
    /// Setting to `0` falls back to the runtime default. Set an explicit
    /// finite cap when you want strict bounds (e.g. test runs, billing
    /// limits).
    #[serde(default = "default_agent_max_tool_iterations")]
    pub max_tool_iterations: usize,
    /// Maximum conversation history messages retained per session. Default: `50`.
    #[serde(default = "default_agent_max_history_messages")]
    pub max_history_messages: usize,
    /// Enable parallel tool execution within a single iteration. Default: `false`.
    #[serde(default)]
    pub parallel_tools: bool,
    /// Tool dispatch strategy (e.g. `"auto"`). Default: `"auto"`.
    #[serde(default = "default_agent_tool_dispatcher")]
    pub tool_dispatcher: String,
    /// Approximate context window size in tokens. Used to trigger auto-compaction
    /// when input tokens approach 70% of this value. Default: `200000`.
    /// Set to `0` to disable token-based compaction (message-count compaction still applies).
    #[serde(default = "default_agent_max_context_tokens")]
    pub max_context_tokens: usize,
    /// Phase 3 L1: per-turn intent classification + prompt scaffold injection.
    /// When `true`, every user message is run through the rule-based
    /// `HybridRouter` before the main loop. Detected intents (WrongPremise,
    /// Ambiguous, AdversarialInjection, ConflictingConstraints, BorderlineSafety)
    /// prepend a short instruction block to the user message. FactualLookup /
    /// TaskRequest (the common ~95%) leave the message byte-identical so
    /// behavior on the default path is unchanged.
    /// Default: `false` while the eval suite validates the change against the
    /// post-Phase-2 baseline.
    #[serde(default)]
    pub intent_routing_enabled: bool,
    /// Per-iteration durable snapshots of the agent loop. See
    /// [`crate::agent::checkpoint`] for the on-disk format. Default: disabled.
    #[serde(default)]
    pub checkpoint: CheckpointConfig,
}

/// Configuration for per-iteration agent loop snapshots
/// (`[agent.checkpoint]` section).
///
/// Phase 0: writer-only. The on-disk format is documented at
/// [`crate::agent::checkpoint::Snapshot`]; resume / CLI inspection land in
/// follow-up PRs. Default `enabled = false` — opt-in until users explicitly
/// want the disk usage (~1 KB / iteration / turn).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CheckpointConfig {
    /// When `true`, the agent loop writes a snapshot to disk after every
    /// iteration via [`crate::agent::checkpoint::FsCheckpointWriter`].
    /// When `false`, the loop emits no snapshots (zero disk I/O).
    #[serde(default)]
    pub enabled: bool,
    /// Directory (relative to the plaw data dir) where snapshots are
    /// persisted. Default: `state/checkpoints`. Files land at
    /// `<data_dir>/<dir>/<turn_id>/<iteration:06>.json`.
    #[serde(default = "default_checkpoint_dir")]
    pub dir: String,
}

fn default_checkpoint_dir() -> String {
    "state/checkpoints".into()
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            dir: default_checkpoint_dir(),
        }
    }
}

/// Repository map configuration (`[repo_map]`).
///
/// When `enabled = true`, the WebSocket gateway builds an Aider-style
/// repo-map (tree-sitter + weighted PageRank + token-budget render) once per
/// session and injects the rendered text as a `[Repository map]` system
/// message above the user turns. Phase 0 (PR #70): build-once per WS session,
/// no mtime polling, no sqlite persistence. Refresh strategies and library/CLI
/// parity defer to follow-up PRs. Default `enabled = false`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RepoMapConfig {
    /// When `true`, build + inject a repository map at the start of each WS
    /// session. The build runs once on the first user message (via
    /// `tokio::task::spawn_blocking`) and the rendered text is cached for the
    /// rest of the session.
    #[serde(default)]
    pub enabled: bool,
    /// Token budget for the rendered map. Aider's empirical default is 1024.
    /// `0` effectively disables injection while leaving the feature enabled
    /// (useful for staged rollout).
    #[serde(default = "default_repo_map_max_tokens")]
    pub max_tokens: usize,
    /// Optional override for the repo root walked by the parser. When `None`,
    /// the session uses `Config::workspace_dir`. For the desktop product,
    /// `workspace_dir` points at `plaw-data/` which is rarely the right repo;
    /// set this to the user's project directory.
    #[serde(default)]
    pub root: Option<PathBuf>,
}

fn default_repo_map_max_tokens() -> usize {
    1024
}

impl Default for RepoMapConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_tokens: default_repo_map_max_tokens(),
            root: None,
        }
    }
}

/// Edit-with-linter configuration (`[edit_linter]`).
///
/// Wraps `file_write` / `file_edit` with a tree-sitter parse pass over the
/// proposed file content. In `warn` mode (the default) the write always
/// proceeds and a `[lint]` note is appended to the tool's `output` so the
/// LLM sees the diagnostic in its next turn. In `block` mode the write is
/// rejected when the proposed content has STRICTLY MORE parse errors than
/// the pre-edit content — so refactors that don't make parse worse still
/// proceed. Mode `off` disables the linter entirely.
///
/// Default mode is `warn` because plaw routinely edits non-source files
/// (Markdown, TOML, JSON, templates), partial-file refactors, and
/// proc-macro DSLs that current tree-sitter grammars cannot always parse
/// cleanly. `block` is one config line away for disciplined Rust/Go-only
/// repos that want SWE-agent-style strictness.
///
/// Per-call escape: a tool call may pass `"skip_lint": true` in its
/// arguments to bypass the linter for that single write.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditLinterConfig {
    /// Master switch. `false` disables the linter regardless of `mode`.
    #[serde(default = "default_edit_linter_enabled")]
    pub enabled: bool,
    /// Behaviour when parse errors are detected.
    #[serde(default)]
    pub mode: EditLinterMode,
    /// Glob patterns of paths to skip (matched against the raw `path` arg).
    /// Example: `["target/**", "**/*_pb.rs", "**/*.generated.rs"]`.
    #[serde(default)]
    pub skip_paths: Vec<String>,
    /// File extensions (with leading dot) to skip regardless of language
    /// detection. Useful for templates: `.tpl`, `.tmpl`, `.j2`, `.hbs`, etc.
    #[serde(default = "default_edit_linter_skip_extensions")]
    pub skip_extensions: Vec<String>,
    /// Skip parse when proposed content exceeds this byte size. Default
    /// 512 KiB.
    #[serde(default = "default_edit_linter_max_file_bytes")]
    pub max_file_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum EditLinterMode {
    /// Linter skipped — write proceeds with no parse.
    Off,
    /// Linter runs; on parse errors the write proceeds and a `[lint]` note
    /// is appended to the tool output. Default.
    #[default]
    Warn,
    /// Linter runs; on parse errors that strictly worsen the file (new
    /// error count > pre-edit error count), the write is rejected.
    Block,
}

fn default_edit_linter_enabled() -> bool {
    true
}

fn default_edit_linter_skip_extensions() -> Vec<String> {
    vec![
        ".tpl".into(),
        ".tmpl".into(),
        ".j2".into(),
        ".hbs".into(),
        ".mustache".into(),
        ".in".into(),
        ".erb".into(),
    ]
}

fn default_edit_linter_max_file_bytes() -> usize {
    524_288
}

impl Default for EditLinterConfig {
    fn default() -> Self {
        Self {
            enabled: default_edit_linter_enabled(),
            mode: EditLinterMode::default(),
            skip_paths: Vec::new(),
            skip_extensions: default_edit_linter_skip_extensions(),
            max_file_bytes: default_edit_linter_max_file_bytes(),
        }
    }
}

fn default_agent_max_tool_iterations() -> usize {
    // i64::MAX (not usize::MAX) so the value round-trips through TOML
    // serialization. TOML integers are i64, and usize::MAX overflows on
    // 64-bit. i64::MAX is still effectively "no built-in cap" — see
    // agent::loop_::DEFAULT_MAX_TOOL_ITERATIONS for the same reasoning.
    i64::MAX as usize
}

fn default_agent_max_history_messages() -> usize {
    50
}

fn default_agent_tool_dispatcher() -> String {
    "auto".into()
}

fn default_agent_max_context_tokens() -> usize {
    200_000
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            compact_context: false,
            max_tool_iterations: default_agent_max_tool_iterations(),
            max_history_messages: default_agent_max_history_messages(),
            parallel_tools: false,
            tool_dispatcher: default_agent_tool_dispatcher(),
            max_context_tokens: default_agent_max_context_tokens(),
            intent_routing_enabled: false,
            checkpoint: CheckpointConfig::default(),
        }
    }
}

/// Skills loading configuration (`[skills]` section).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum SkillsPromptInjectionMode {
    /// Inline full skill instructions and tool metadata into the system prompt.
    #[default]
    Full,
    /// Inline only compact skill metadata (name/description/location) and load details on demand.
    Compact,
}

fn parse_skills_prompt_injection_mode(raw: &str) -> Option<SkillsPromptInjectionMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "full" => Some(SkillsPromptInjectionMode::Full),
        "compact" => Some(SkillsPromptInjectionMode::Compact),
        _ => None,
    }
}

/// Skills loading configuration (`[skills]` section).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SkillsConfig {
    /// Enable loading and syncing the community open-skills repository.
    /// Default: `false` (opt-in).
    #[serde(default)]
    pub open_skills_enabled: bool,
    /// Optional path to a local open-skills repository.
    /// If unset, defaults to `$HOME/open-skills` when enabled.
    #[serde(default)]
    pub open_skills_dir: Option<String>,
    /// Controls how skills are injected into the system prompt.
    /// `full` preserves legacy behavior. `compact` keeps context small and loads skills on demand.
    #[serde(default)]
    pub prompt_injection_mode: SkillsPromptInjectionMode,
}

/// Multimodal (image) handling configuration (`[multimodal]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MultimodalConfig {
    /// Maximum number of image attachments accepted per request.
    #[serde(default = "default_multimodal_max_images")]
    pub max_images: usize,
    /// Maximum image payload size in MiB before base64 encoding.
    #[serde(default = "default_multimodal_max_image_size_mb")]
    pub max_image_size_mb: usize,
    /// Allow fetching remote image URLs (http/https). Disabled by default.
    #[serde(default)]
    pub allow_remote_fetch: bool,
}

fn default_multimodal_max_images() -> usize {
    4
}

fn default_multimodal_max_image_size_mb() -> usize {
    5
}

impl MultimodalConfig {
    /// Clamp configured values to safe runtime bounds.
    pub fn effective_limits(&self) -> (usize, usize) {
        let max_images = self.max_images.clamp(1, 16);
        let max_image_size_mb = self.max_image_size_mb.clamp(1, 20);
        (max_images, max_image_size_mb)
    }
}

impl Default for MultimodalConfig {
    fn default() -> Self {
        Self {
            max_images: default_multimodal_max_images(),
            max_image_size_mb: default_multimodal_max_image_size_mb(),
            allow_remote_fetch: false,
        }
    }
}

// ── Identity (AIEOS / OpenClaw format) ──────────────────────────

/// Identity format configuration (`[identity]` section).
///
/// Supports `"openclaw"` (default) or `"aieos"` identity documents.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IdentityConfig {
    /// Identity format: "openclaw" (default) or "aieos"
    #[serde(default = "default_identity_format")]
    pub format: String,
    /// Path to AIEOS JSON file (relative to workspace)
    #[serde(default)]
    pub aieos_path: Option<String>,
    /// Inline AIEOS JSON (alternative to file path)
    #[serde(default)]
    pub aieos_inline: Option<String>,
}

fn default_identity_format() -> String {
    "openclaw".into()
}

impl Default for IdentityConfig {
    fn default() -> Self {
        Self {
            format: default_identity_format(),
            aieos_path: None,
            aieos_inline: None,
        }
    }
}

// ── Cost tracking and budget enforcement ───────────────────────────

/// Cost tracking and budget enforcement configuration (`[cost]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CostConfig {
    /// Enable cost tracking (default: false)
    #[serde(default)]
    pub enabled: bool,

    /// Daily spending limit in USD (default: 10.00)
    #[serde(default = "default_daily_limit")]
    pub daily_limit_usd: f64,

    /// Monthly spending limit in USD (default: 100.00)
    #[serde(default = "default_monthly_limit")]
    pub monthly_limit_usd: f64,

    /// Warn when spending reaches this percentage of limit (default: 80)
    #[serde(default = "default_warn_percent")]
    pub warn_at_percent: u8,

    /// Allow requests to exceed budget with --override flag (default: false)
    #[serde(default)]
    pub allow_override: bool,

    /// Per-model pricing (USD per 1M tokens)
    #[serde(default)]
    pub prices: std::collections::HashMap<String, ModelPricing>,
}

/// Per-model pricing entry (USD per 1M tokens).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelPricing {
    /// Input price per 1M tokens
    #[serde(default)]
    pub input: f64,

    /// Output price per 1M tokens
    #[serde(default)]
    pub output: f64,
}

fn default_daily_limit() -> f64 {
    10.0
}

fn default_monthly_limit() -> f64 {
    100.0
}

fn default_warn_percent() -> u8 {
    80
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            daily_limit_usd: default_daily_limit(),
            monthly_limit_usd: default_monthly_limit(),
            warn_at_percent: default_warn_percent(),
            allow_override: false,
            prices: get_default_pricing(),
        }
    }
}

/// Default pricing for popular models (USD per 1M tokens)
fn get_default_pricing() -> std::collections::HashMap<String, ModelPricing> {
    let mut prices = std::collections::HashMap::new();

    // Anthropic models
    prices.insert(
        "anthropic/claude-sonnet-4-20250514".into(),
        ModelPricing {
            input: 3.0,
            output: 15.0,
        },
    );
    prices.insert(
        "anthropic/claude-opus-4-20250514".into(),
        ModelPricing {
            input: 15.0,
            output: 75.0,
        },
    );
    prices.insert(
        "anthropic/claude-3.5-sonnet".into(),
        ModelPricing {
            input: 3.0,
            output: 15.0,
        },
    );
    prices.insert(
        "anthropic/claude-3-haiku".into(),
        ModelPricing {
            input: 0.25,
            output: 1.25,
        },
    );

    // OpenAI models
    prices.insert(
        "openai/gpt-4o".into(),
        ModelPricing {
            input: 5.0,
            output: 15.0,
        },
    );
    prices.insert(
        "openai/gpt-4o-mini".into(),
        ModelPricing {
            input: 0.15,
            output: 0.60,
        },
    );
    prices.insert(
        "openai/o1-preview".into(),
        ModelPricing {
            input: 15.0,
            output: 60.0,
        },
    );

    // Google models
    prices.insert(
        "google/gemini-2.0-flash".into(),
        ModelPricing {
            input: 0.10,
            output: 0.40,
        },
    );
    prices.insert(
        "google/gemini-1.5-pro".into(),
        ModelPricing {
            input: 1.25,
            output: 5.0,
        },
    );

    prices
}

// ── Peripherals (hardware: STM32, RPi GPIO, etc.) ────────────────────────

/// Peripheral board integration configuration (`[peripherals]` section).
///
/// Boards become agent tools when enabled.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct PeripheralsConfig {
    /// Enable peripheral support (boards become agent tools)
    #[serde(default)]
    pub enabled: bool,
    /// Board configurations (nucleo-f401re, rpi-gpio, etc.)
    #[serde(default)]
    pub boards: Vec<PeripheralBoardConfig>,
    /// Path to datasheet docs (relative to workspace) for RAG retrieval.
    /// Place .md/.txt files named by board (e.g. nucleo-f401re.md, rpi-gpio.md).
    #[serde(default)]
    pub datasheet_dir: Option<String>,
}

/// Configuration for a single peripheral board (e.g. STM32, RPi GPIO).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PeripheralBoardConfig {
    /// Board type: "nucleo-f401re", "rpi-gpio", "esp32", etc.
    pub board: String,
    /// Transport: "serial", "native", "websocket"
    #[serde(default = "default_peripheral_transport")]
    pub transport: String,
    /// Path for serial: "/dev/ttyACM0", "/dev/ttyUSB0"
    #[serde(default)]
    pub path: Option<String>,
    /// Baud rate for serial (default: 115200)
    #[serde(default = "default_peripheral_baud")]
    pub baud: u32,
}

fn default_peripheral_transport() -> String {
    "serial".into()
}

fn default_peripheral_baud() -> u32 {
    115_200
}

impl Default for PeripheralBoardConfig {
    fn default() -> Self {
        Self {
            board: String::new(),
            transport: default_peripheral_transport(),
            path: None,
            baud: default_peripheral_baud(),
        }
    }
}

// ── Gateway security ─────────────────────────────────────────────

/// Gateway server configuration (`[gateway]` section).
///
/// Controls the HTTP gateway for webhook and pairing endpoints.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GatewayConfig {
    /// Gateway port (default: 42617)
    #[serde(default = "default_gateway_port")]
    pub port: u16,
    /// Gateway host (default: 127.0.0.1)
    #[serde(default = "default_gateway_host")]
    pub host: String,
    /// Require pairing before accepting requests (default: true)
    #[serde(default = "default_true")]
    pub require_pairing: bool,
    /// Allow binding to non-localhost without a tunnel (default: false)
    #[serde(default)]
    pub allow_public_bind: bool,
    /// Paired bearer tokens (managed automatically, not user-edited)
    #[serde(default)]
    pub paired_tokens: Vec<String>,

    /// Max `/pair` requests per minute per client key.
    #[serde(default = "default_pair_rate_limit")]
    pub pair_rate_limit_per_minute: u32,

    /// Max `/webhook` requests per minute per client key.
    #[serde(default = "default_webhook_rate_limit")]
    pub webhook_rate_limit_per_minute: u32,

    /// Trust proxy-forwarded client IP headers (`X-Forwarded-For`, `X-Real-IP`).
    /// Disabled by default; enable only behind a trusted reverse proxy.
    #[serde(default)]
    pub trust_forwarded_headers: bool,

    /// Maximum distinct client keys tracked by gateway rate limiter maps.
    #[serde(default = "default_gateway_rate_limit_max_keys")]
    pub rate_limit_max_keys: usize,

    /// TTL for webhook idempotency keys.
    #[serde(default = "default_idempotency_ttl_secs")]
    pub idempotency_ttl_secs: u64,

    /// Maximum distinct idempotency keys retained in memory.
    #[serde(default = "default_gateway_idempotency_max_keys")]
    pub idempotency_max_keys: usize,

    /// Node-control protocol scaffold (`[gateway.node_control]`).
    #[serde(default)]
    pub node_control: NodeControlConfig,
}

/// Node-control scaffold settings under `[gateway.node_control]`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct NodeControlConfig {
    /// Enable experimental node-control API endpoints.
    #[serde(default)]
    pub enabled: bool,

    /// Optional extra shared token for node-control API calls.
    /// When set, clients must send this value in `X-Node-Control-Token`.
    #[serde(default)]
    pub auth_token: Option<String>,

    /// Allowlist of remote node IDs for `node.describe`/`node.invoke`.
    /// Empty means "no explicit allowlist" (accept all IDs).
    #[serde(default)]
    pub allowed_node_ids: Vec<String>,
}

fn default_gateway_port() -> u16 {
    42617
}

fn default_gateway_host() -> String {
    "127.0.0.1".into()
}

fn default_pair_rate_limit() -> u32 {
    10
}

fn default_webhook_rate_limit() -> u32 {
    60
}

fn default_idempotency_ttl_secs() -> u64 {
    300
}

fn default_gateway_rate_limit_max_keys() -> usize {
    10_000
}

fn default_gateway_idempotency_max_keys() -> usize {
    10_000
}

pub(super) fn default_true() -> bool {
    true
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            port: default_gateway_port(),
            host: default_gateway_host(),
            require_pairing: true,
            allow_public_bind: false,
            paired_tokens: Vec::new(),
            pair_rate_limit_per_minute: default_pair_rate_limit(),
            webhook_rate_limit_per_minute: default_webhook_rate_limit(),
            trust_forwarded_headers: false,
            rate_limit_max_keys: default_gateway_rate_limit_max_keys(),
            idempotency_ttl_secs: default_idempotency_ttl_secs(),
            idempotency_max_keys: default_gateway_idempotency_max_keys(),
            node_control: NodeControlConfig::default(),
        }
    }
}

// ── Composio (managed tool surface) ─────────────────────────────

/// Composio managed OAuth tools integration (`[composio]` section).
///
/// Provides access to 1000+ OAuth-connected tools via the Composio platform.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ComposioConfig {
    /// Enable Composio integration for 1000+ OAuth tools
    #[serde(default, alias = "enable")]
    pub enabled: bool,
    /// Composio API key (stored encrypted when secrets.encrypt = true)
    #[serde(default)]
    pub api_key: Option<String>,
    /// Default entity ID for multi-user setups
    #[serde(default = "default_entity_id")]
    pub entity_id: String,
}

fn default_entity_id() -> String {
    "default".into()
}

impl Default for ComposioConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: None,
            entity_id: default_entity_id(),
        }
    }
}

// ── Secrets (encrypted credential store) ────────────────────────

/// Secrets encryption configuration (`[secrets]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SecretsConfig {
    /// Enable encryption for API keys and tokens in config.toml
    #[serde(default = "default_true")]
    pub encrypt: bool,
}

impl Default for SecretsConfig {
    fn default() -> Self {
        Self { encrypt: true }
    }
}

// ── Browser (friendly-service browsing only) ───────────────────

/// Computer-use sidecar configuration (`[browser.computer_use]` section).
///
/// Delegates OS-level mouse, keyboard, and screenshot actions to a local sidecar.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserComputerUseConfig {
    /// Sidecar endpoint for computer-use actions (OS-level mouse/keyboard/screenshot)
    #[serde(default = "default_browser_computer_use_endpoint")]
    pub endpoint: String,
    /// Optional bearer token for computer-use sidecar
    #[serde(default)]
    pub api_key: Option<String>,
    /// Per-action request timeout in milliseconds
    #[serde(default = "default_browser_computer_use_timeout_ms")]
    pub timeout_ms: u64,
    /// Allow remote/public endpoint for computer-use sidecar (default: false)
    #[serde(default)]
    pub allow_remote_endpoint: bool,
    /// Optional window title/process allowlist forwarded to sidecar policy
    #[serde(default)]
    pub window_allowlist: Vec<String>,
    /// Optional X-axis boundary for coordinate-based actions
    #[serde(default)]
    pub max_coordinate_x: Option<i64>,
    /// Optional Y-axis boundary for coordinate-based actions
    #[serde(default)]
    pub max_coordinate_y: Option<i64>,
}

fn default_browser_computer_use_endpoint() -> String {
    "http://127.0.0.1:8787/v1/actions".into()
}

fn default_browser_computer_use_timeout_ms() -> u64 {
    15_000
}

impl Default for BrowserComputerUseConfig {
    fn default() -> Self {
        Self {
            endpoint: default_browser_computer_use_endpoint(),
            api_key: None,
            timeout_ms: default_browser_computer_use_timeout_ms(),
            allow_remote_endpoint: false,
            window_allowlist: Vec::new(),
            max_coordinate_x: None,
            max_coordinate_y: None,
        }
    }
}

/// Browser automation configuration (`[browser]` section).
///
/// Controls the `browser_open` tool and browser automation backends.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BrowserConfig {
    /// Enable `browser_open` tool (opens URLs in the system browser without scraping)
    #[serde(default)]
    pub enabled: bool,
    /// Allowed domains for `browser_open` (exact or subdomain match)
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Browser for `browser_open` tool: "disable" | "brave" | "chrome" | "firefox" | "default"
    #[serde(default = "default_browser_open")]
    pub browser_open: String,
    /// Browser session name (for agent-browser automation)
    #[serde(default)]
    pub session_name: Option<String>,
    /// Browser automation backend: "agent_browser" | "rust_native" | "computer_use" | "auto"
    #[serde(default = "default_browser_backend")]
    pub backend: String,
    /// Headless mode for rust-native backend
    #[serde(default = "default_true")]
    pub native_headless: bool,
    /// WebDriver endpoint URL for rust-native backend (e.g. http://127.0.0.1:9515)
    #[serde(default = "default_browser_webdriver_url")]
    pub native_webdriver_url: String,
    /// Optional Chrome/Chromium executable path for rust-native backend
    #[serde(default)]
    pub native_chrome_path: Option<String>,
    /// Computer-use sidecar configuration
    #[serde(default)]
    pub computer_use: BrowserComputerUseConfig,
}

fn default_browser_backend() -> String {
    "agent_browser".into()
}

fn default_browser_open() -> String {
    "default".into()
}

fn default_browser_webdriver_url() -> String {
    "http://127.0.0.1:9515".into()
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_domains: Vec::new(),
            browser_open: default_browser_open(),
            session_name: None,
            backend: default_browser_backend(),
            native_headless: default_true(),
            native_webdriver_url: default_browser_webdriver_url(),
            native_chrome_path: None,
            computer_use: BrowserComputerUseConfig::default(),
        }
    }
}

// ── HTTP request tool ───────────────────────────────────────────

/// HTTP request tool configuration (`[http_request]` section).
///
/// Deny-by-default: if `allowed_domains` is empty, all HTTP requests are rejected.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HttpRequestConfig {
    /// Enable `http_request` tool for API interactions
    #[serde(default)]
    pub enabled: bool,
    /// Allowed domains for HTTP requests (exact or subdomain match)
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Maximum response size in bytes (default: 1MB, 0 = unlimited)
    #[serde(default = "default_http_max_response_size")]
    pub max_response_size: usize,
    /// Request timeout in seconds (default: 30)
    #[serde(default = "default_http_timeout_secs")]
    pub timeout_secs: u64,
    /// User-Agent string sent with HTTP requests (env: PLAW_HTTP_REQUEST_USER_AGENT)
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
    /// 允许访问 localhost/127.x/::1 等本地地址（用于调用本机服务，默认 false）
    #[serde(default)]
    pub allow_local: bool,
}

impl Default for HttpRequestConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_domains: vec![],
            max_response_size: default_http_max_response_size(),
            timeout_secs: default_http_timeout_secs(),
            user_agent: default_user_agent(),
            allow_local: false,
        }
    }
}

fn default_http_max_response_size() -> usize {
    1_000_000 // 1MB
}

fn default_http_timeout_secs() -> u64 {
    30
}

// ── Web fetch ────────────────────────────────────────────────────

/// Web fetch tool configuration (`[web_fetch]` section).
///
/// Fetches web pages and converts HTML to plain text for LLM consumption.
/// Domain filtering: `allowed_domains` controls which hosts are reachable (use `["*"]`
/// for all public hosts). `blocked_domains` takes priority over `allowed_domains`.
/// If `allowed_domains` is empty, all requests are rejected (deny-by-default).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebFetchConfig {
    /// Enable `web_fetch` tool for fetching web page content
    #[serde(default)]
    pub enabled: bool,
    /// Provider: "fast_html2md", "nanohtml2text", "firecrawl", or "tavily"
    #[serde(default = "default_web_fetch_provider")]
    pub provider: String,
    /// Optional provider API key (required for provider = "firecrawl" or "tavily").
    /// Multiple keys can be comma-separated for round-robin load balancing.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Optional provider API URL override (for self-hosted providers)
    #[serde(default)]
    pub api_url: Option<String>,
    /// Allowed domains for web fetch (exact or subdomain match; `["*"]` = all public hosts)
    #[serde(default)]
    pub allowed_domains: Vec<String>,
    /// Blocked domains (exact or subdomain match; always takes priority over allowed_domains)
    #[serde(default)]
    pub blocked_domains: Vec<String>,
    /// Maximum response size in bytes (default: 500KB, plain text is much smaller than raw HTML)
    #[serde(default = "default_web_fetch_max_response_size")]
    pub max_response_size: usize,
    /// Request timeout in seconds (default: 30)
    #[serde(default = "default_web_fetch_timeout_secs")]
    pub timeout_secs: u64,
    /// User-Agent string sent with fetch requests (env: PLAW_WEB_FETCH_USER_AGENT)
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
}

fn default_web_fetch_max_response_size() -> usize {
    500_000 // 500KB
}

fn default_web_fetch_provider() -> String {
    "fast_html2md".into()
}

fn default_web_fetch_timeout_secs() -> u64 {
    30
}

impl Default for WebFetchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_web_fetch_provider(),
            api_key: None,
            api_url: None,
            allowed_domains: vec!["*".into()],
            blocked_domains: vec![],
            max_response_size: default_web_fetch_max_response_size(),
            timeout_secs: default_web_fetch_timeout_secs(),
            user_agent: default_user_agent(),
        }
    }
}

// ── Web search ───────────────────────────────────────────────────

/// Web search tool configuration (`[web_search]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebSearchConfig {
    /// Enable `web_search_tool` for web searches
    #[serde(default)]
    pub enabled: bool,
    /// Search provider: "duckduckgo" (free, no API key), "brave", "firecrawl", or "tavily"
    #[serde(default = "default_web_search_provider")]
    pub provider: String,
    /// Generic provider API key (used by firecrawl, tavily, and as fallback for brave).
    /// Multiple keys can be comma-separated for round-robin load balancing.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Optional provider API URL override (for self-hosted providers)
    #[serde(default)]
    pub api_url: Option<String>,
    /// Brave Search API key (required if provider is "brave")
    #[serde(default)]
    pub brave_api_key: Option<String>,
    /// Maximum results per search (1-10)
    #[serde(default = "default_web_search_max_results")]
    pub max_results: usize,
    /// Request timeout in seconds
    #[serde(default = "default_web_search_timeout_secs")]
    pub timeout_secs: u64,
    /// User-Agent string sent with search requests (env: PLAW_WEB_SEARCH_USER_AGENT)
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
}

fn default_web_search_provider() -> String {
    "duckduckgo".into()
}

fn default_web_search_max_results() -> usize {
    5
}

fn default_web_search_timeout_secs() -> u64 {
    15
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: default_web_search_provider(),
            api_key: None,
            api_url: None,
            brave_api_key: None,
            max_results: default_web_search_max_results(),
            timeout_secs: default_web_search_timeout_secs(),
            user_agent: default_user_agent(),
        }
    }
}

fn default_user_agent() -> String {
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".into()
}

// ── Memory ───────────────────────────────────────────────────

/// Persistent storage configuration (`[storage]` section).
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct StorageConfig {
    /// Storage provider settings (e.g. sqlite, postgres).
    #[serde(default)]
    pub provider: StorageProviderSection,
}

/// Wrapper for the storage provider configuration section.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct StorageProviderSection {
    /// Storage provider backend settings.
    #[serde(default)]
    pub config: StorageProviderConfig,
}

/// Storage provider backend configuration (e.g. postgres connection details).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StorageProviderConfig {
    /// Storage engine key (e.g. "postgres", "sqlite").
    #[serde(default)]
    pub provider: String,

    /// Connection URL for remote providers.
    /// Accepts legacy aliases: dbURL, database_url, databaseUrl.
    #[serde(
        default,
        alias = "dbURL",
        alias = "database_url",
        alias = "databaseUrl"
    )]
    pub db_url: Option<String>,

    /// Database schema for SQL backends.
    #[serde(default = "default_storage_schema")]
    pub schema: String,

    /// Table name for memory entries.
    #[serde(default = "default_storage_table")]
    pub table: String,

    /// Optional connection timeout in seconds for remote providers.
    #[serde(default)]
    pub connect_timeout_secs: Option<u64>,

    /// Enable TLS for the PostgreSQL connection.
    ///
    /// `true` — require TLS (skips certificate verification; suitable for
    /// self-signed certs and most managed databases).
    /// `false` (default) — plain TCP, backward-compatible.
    #[serde(default)]
    pub tls: bool,
}

fn default_storage_schema() -> String {
    "public".into()
}

fn default_storage_table() -> String {
    "memories".into()
}

impl Default for StorageProviderConfig {
    fn default() -> Self {
        Self {
            provider: String::new(),
            db_url: None,
            schema: default_storage_schema(),
            table: default_storage_table(),
            connect_timeout_secs: None,
            tls: false,
        }
    }
}

/// Memory backend configuration (`[memory]` section).
///
/// Controls conversation memory storage, embeddings, hybrid search, response caching,
/// and memory snapshot/hydration.
/// Configuration for Qdrant vector database backend (`[memory.qdrant]`).
/// Used when `[memory].backend = "qdrant"`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QdrantConfig {
    /// Qdrant server URL (e.g. "http://localhost:6333").
    /// Falls back to `QDRANT_URL` env var if not set.
    #[serde(default)]
    pub url: Option<String>,
    /// Qdrant collection name for storing memories.
    /// Falls back to `QDRANT_COLLECTION` env var, or default "plaw_memories".
    #[serde(default = "default_qdrant_collection")]
    pub collection: String,
    /// Optional API key for Qdrant Cloud or secured instances.
    /// Falls back to `QDRANT_API_KEY` env var if not set.
    #[serde(default)]
    pub api_key: Option<String>,
}

fn default_qdrant_collection() -> String {
    "plaw_memories".into()
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            url: None,
            collection: default_qdrant_collection(),
            api_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[allow(clippy::struct_excessive_bools)]
pub struct MemoryConfig {
    /// "sqlite" | "lucid" | "postgres" | "qdrant" | "markdown" | "none" (`none` = explicit no-op memory)
    ///
    /// `postgres` requires `[storage.provider.config]` with `db_url` (`dbURL` alias supported).
    /// `qdrant` uses `[memory.qdrant]` config or `QDRANT_URL` env var.
    #[serde(default = "default_memory_backend")]
    pub backend: String,
    /// Auto-save user-stated conversation input to memory (assistant output is excluded)
    #[serde(default = "default_true")]
    pub auto_save: bool,
    /// Run memory/session hygiene (archiving + retention cleanup)
    #[serde(default = "default_hygiene_enabled")]
    pub hygiene_enabled: bool,
    /// Archive daily/session files older than this many days
    #[serde(default = "default_archive_after_days")]
    pub archive_after_days: u32,
    /// Purge archived files older than this many days
    #[serde(default = "default_purge_after_days")]
    pub purge_after_days: u32,
    /// For sqlite backend: prune conversation rows older than this many days
    #[serde(default = "default_conversation_retention_days")]
    pub conversation_retention_days: u32,
    /// Embedding provider: "none" | "openai" | "custom:URL"
    #[serde(default = "default_embedding_provider")]
    pub embedding_provider: String,
    /// Embedding model name (e.g. "text-embedding-3-small")
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
    /// Embedding vector dimensions
    #[serde(default = "default_embedding_dims")]
    pub embedding_dimensions: usize,
    /// Weight for vector similarity in hybrid search (0.0–1.0)
    #[serde(default = "default_vector_weight")]
    pub vector_weight: f64,
    /// Weight for keyword BM25 in hybrid search (0.0–1.0)
    #[serde(default = "default_keyword_weight")]
    pub keyword_weight: f64,
    /// Minimum hybrid score (0.0–1.0) for a memory to be included in context.
    /// Memories scoring below this threshold are dropped to prevent irrelevant
    /// context from bleeding into conversations. Default: 0.4
    #[serde(default = "default_min_relevance_score")]
    pub min_relevance_score: f64,
    /// Max embedding cache entries before LRU eviction
    #[serde(default = "default_cache_size")]
    pub embedding_cache_size: usize,
    /// Max tokens per chunk for document splitting
    #[serde(default = "default_chunk_size")]
    pub chunk_max_tokens: usize,

    // ── Response Cache (saves tokens on repeated prompts) ──────
    /// Enable LLM response caching to avoid paying for duplicate prompts
    #[serde(default)]
    pub response_cache_enabled: bool,
    /// TTL in minutes for cached responses (default: 60)
    #[serde(default = "default_response_cache_ttl")]
    pub response_cache_ttl_minutes: u32,
    /// Max number of cached responses before LRU eviction (default: 5000)
    #[serde(default = "default_response_cache_max")]
    pub response_cache_max_entries: usize,

    // ── Memory Snapshot (soul backup to Markdown) ─────────────
    /// Enable periodic export of core memories to MEMORY_SNAPSHOT.md
    #[serde(default)]
    pub snapshot_enabled: bool,
    /// Run snapshot during hygiene passes (heartbeat-driven)
    #[serde(default)]
    pub snapshot_on_hygiene: bool,
    /// Auto-hydrate from MEMORY_SNAPSHOT.md when brain.db is missing
    #[serde(default = "default_true")]
    pub auto_hydrate: bool,

    // ── SQLite backend options ─────────────────────────────────
    /// For sqlite backend: max seconds to wait when opening the DB (e.g. file locked).
    /// None = wait indefinitely (default). Recommended max: 300.
    #[serde(default)]
    pub sqlite_open_timeout_secs: Option<u64>,

    // ── Qdrant backend options ─────────────────────────────────
    /// Configuration for Qdrant vector database backend.
    /// Only used when `backend = "qdrant"`.
    #[serde(default)]
    pub qdrant: QdrantConfig,
}

fn default_memory_backend() -> String {
    "sqlite".into()
}
fn default_embedding_provider() -> String {
    "none".into()
}
fn default_hygiene_enabled() -> bool {
    true
}
fn default_archive_after_days() -> u32 {
    7
}
fn default_purge_after_days() -> u32 {
    30
}
fn default_conversation_retention_days() -> u32 {
    30
}
fn default_embedding_model() -> String {
    "text-embedding-3-small".into()
}
fn default_embedding_dims() -> usize {
    1536
}
fn default_vector_weight() -> f64 {
    0.7
}
fn default_keyword_weight() -> f64 {
    0.3
}
fn default_min_relevance_score() -> f64 {
    0.4
}
fn default_cache_size() -> usize {
    10_000
}
fn default_chunk_size() -> usize {
    512
}
fn default_response_cache_ttl() -> u32 {
    60
}
fn default_response_cache_max() -> usize {
    5_000
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            backend: "sqlite".into(),
            auto_save: true,
            hygiene_enabled: default_hygiene_enabled(),
            archive_after_days: default_archive_after_days(),
            purge_after_days: default_purge_after_days(),
            conversation_retention_days: default_conversation_retention_days(),
            embedding_provider: default_embedding_provider(),
            embedding_model: default_embedding_model(),
            embedding_dimensions: default_embedding_dims(),
            vector_weight: default_vector_weight(),
            keyword_weight: default_keyword_weight(),
            min_relevance_score: default_min_relevance_score(),
            embedding_cache_size: default_cache_size(),
            chunk_max_tokens: default_chunk_size(),
            response_cache_enabled: false,
            response_cache_ttl_minutes: default_response_cache_ttl(),
            response_cache_max_entries: default_response_cache_max(),
            snapshot_enabled: false,
            snapshot_on_hygiene: false,
            auto_hydrate: true,
            sqlite_open_timeout_secs: None,
            qdrant: QdrantConfig::default(),
        }
    }
}

// ── Observability ─────────────────────────────────────────────────

/// Observability backend configuration (`[observability]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ObservabilityConfig {
    /// "none" | "log" | "prometheus" | "otel"
    pub backend: String,

    /// OTLP endpoint (e.g. "http://localhost:4318"). Only used when backend = "otel".
    #[serde(default)]
    pub otel_endpoint: Option<String>,

    /// Service name reported to the OTel collector. Defaults to "plaw".
    #[serde(default)]
    pub otel_service_name: Option<String>,

    /// Runtime trace storage mode: "none" | "rolling" | "full".
    /// Controls whether model replies and tool-call diagnostics are persisted.
    #[serde(default = "default_runtime_trace_mode")]
    pub runtime_trace_mode: String,

    /// Runtime trace file path. Relative paths are resolved under workspace_dir.
    #[serde(default = "default_runtime_trace_path")]
    pub runtime_trace_path: String,

    /// Maximum entries retained when runtime_trace_mode = "rolling".
    #[serde(default = "default_runtime_trace_max_entries")]
    pub runtime_trace_max_entries: usize,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            backend: "none".into(),
            otel_endpoint: None,
            otel_service_name: None,
            runtime_trace_mode: default_runtime_trace_mode(),
            runtime_trace_path: default_runtime_trace_path(),
            runtime_trace_max_entries: default_runtime_trace_max_entries(),
        }
    }
}

fn default_runtime_trace_mode() -> String {
    "none".to_string()
}

fn default_runtime_trace_path() -> String {
    "state/runtime-trace.jsonl".to_string()
}

fn default_runtime_trace_max_entries() -> usize {
    200
}

// ── Hooks ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HooksConfig {
    /// Enable lifecycle hook execution.
    ///
    /// Hooks run in-process with the same privileges as the main runtime.
    /// Keep enabled hook handlers narrowly scoped and auditable.
    pub enabled: bool,
    #[serde(default)]
    pub builtin: BuiltinHooksConfig,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            builtin: BuiltinHooksConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct BuiltinHooksConfig {
    /// Enable the command-logger hook (logs tool calls for auditing).
    pub command_logger: bool,
}

// ── Autonomy / Security ──────────────────────────────────────────

/// Natural-language behavior for non-CLI approval-management commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NonCliNaturalLanguageApprovalMode {
    /// Do not treat natural-language text as approval-management commands.
    /// Operators must use explicit slash commands.
    Disabled,
    /// Natural-language approval phrases create a pending request that must be
    /// confirmed with a request ID.
    RequestConfirm,
    /// Natural-language approval phrases directly approve the named tool.
    ///
    /// This keeps private-chat workflows simple while still requiring a human
    /// sender and passing the same approver allowlist checks as slash commands.
    #[default]
    Direct,
}

/// Autonomy and security policy configuration (`[autonomy]` section).
///
/// Controls what the agent is allowed to do: shell commands, filesystem access,
/// risk approval gates, and per-policy budgets.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AutonomyConfig {
    /// Autonomy level: `read_only`, `supervised` (default), or `full`.
    pub level: AutonomyLevel,
    /// Restrict absolute filesystem paths to workspace-relative references. Default: `true`.
    /// Resolved paths outside the workspace still require `allowed_roots`.
    pub workspace_only: bool,
    /// Allowlist of executable names permitted for shell execution.
    pub allowed_commands: Vec<String>,
    /// Explicit path denylist. Default includes system-critical paths and sensitive dotdirs.
    pub forbidden_paths: Vec<String>,
    /// Maximum actions allowed per hour per policy. Default: `100`.
    pub max_actions_per_hour: u32,
    /// Maximum cost per day in cents per policy. Default: `1000`.
    pub max_cost_per_day_cents: u32,

    /// Require explicit approval for medium-risk shell commands.
    #[serde(default = "default_true")]
    pub require_approval_for_medium_risk: bool,

    /// Block high-risk shell commands even if allowlisted.
    #[serde(default = "default_true")]
    pub block_high_risk_commands: bool,

    /// Additional environment variables allowed for shell tool subprocesses.
    ///
    /// These names are explicitly allowlisted and merged with the built-in safe
    /// baseline (`PATH`, `HOME`, etc.) after `env_clear()`.
    #[serde(default)]
    pub shell_env_passthrough: Vec<String>,

    /// Tools that never require approval (e.g. read-only tools).
    #[serde(default = "default_auto_approve")]
    pub auto_approve: Vec<String>,

    /// Tools that always require interactive approval, even after "Always".
    #[serde(default = "default_always_ask")]
    pub always_ask: Vec<String>,

    /// Extra directory roots the agent may read/write outside the workspace.
    /// Supports absolute, `~/...`, and workspace-relative entries.
    /// Resolved paths under any of these roots pass `is_resolved_path_allowed`.
    #[serde(default)]
    pub allowed_roots: Vec<String>,

    /// Tools to exclude from non-CLI channels (e.g. Telegram, Discord).
    ///
    /// When a tool is listed here, non-CLI channels will not expose it to the
    /// model in tool specs.
    #[serde(default = "default_non_cli_excluded_tools")]
    pub non_cli_excluded_tools: Vec<String>,

    /// Optional allowlist for who can manage non-CLI approval commands.
    ///
    /// When empty, any sender already admitted by the channel allowlist can
    /// use approval-management commands.
    ///
    /// Supported entry formats:
    /// - `"*"`: allow any sender on any channel
    /// - `"alice"`: allow sender `alice` on any channel
    /// - `"telegram:alice"`: allow sender `alice` only on `telegram`
    /// - `"telegram:*"`: allow any sender on `telegram`
    /// - `"*:alice"`: allow sender `alice` on any channel
    #[serde(default)]
    pub non_cli_approval_approvers: Vec<String>,

    /// Natural-language handling mode for non-CLI approval-management commands.
    ///
    /// Values:
    /// - `direct` (default): phrases like `授权工具 shell` immediately approve.
    /// - `request_confirm`: phrases create pending requests requiring confirm.
    /// - `disabled`: ignore natural-language approval commands (slash only).
    #[serde(default)]
    pub non_cli_natural_language_approval_mode: NonCliNaturalLanguageApprovalMode,

    /// Optional per-channel override for natural-language approval mode.
    ///
    /// Keys are channel names (for example: `telegram`, `discord`, `slack`).
    /// Values use the same enum as `non_cli_natural_language_approval_mode`.
    ///
    /// Example:
    /// - `telegram = "direct"` for private-chat ergonomics
    /// - `discord = "request_confirm"` for stricter team channels
    #[serde(default)]
    pub non_cli_natural_language_approval_mode_by_channel:
        HashMap<String, NonCliNaturalLanguageApprovalMode>,
}

fn default_auto_approve() -> Vec<String> {
    vec!["file_read".into(), "memory_recall".into()]
}

fn default_always_ask() -> Vec<String> {
    vec![]
}

fn default_non_cli_excluded_tools() -> Vec<String> {
    [
        "shell",
        "file_write",
        "file_edit",
        "git_operations",
        "browser",
        "browser_open",
        "http_request",
        "schedule",
        "cron_add",
        "cron_remove",
        "cron_update",
        "cron_run",
        "memory_store",
        "memory_forget",
        "proxy_config",
        "model_routing_config",
        "pushover",
        "composio",
        "delegate",
        "screenshot",
        "image_info",
    ]
    .into_iter()
    .map(std::string::ToString::to_string)
    .collect()
}

fn is_valid_env_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphabetic() || first == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

impl Default for AutonomyConfig {
    fn default() -> Self {
        Self {
            level: AutonomyLevel::Supervised,
            workspace_only: true,
            allowed_commands: vec![
                "git".into(),
                "npm".into(),
                "cargo".into(),
                "ls".into(),
                "cat".into(),
                "grep".into(),
                "find".into(),
                "echo".into(),
                "pwd".into(),
                "wc".into(),
                "head".into(),
                "tail".into(),
                "date".into(),
            ],
            forbidden_paths: vec![
                "/etc".into(),
                "/root".into(),
                "/home".into(),
                "/usr".into(),
                "/bin".into(),
                "/sbin".into(),
                "/lib".into(),
                "/opt".into(),
                "/boot".into(),
                "/dev".into(),
                "/proc".into(),
                "/sys".into(),
                "/var".into(),
                "/tmp".into(),
                "~/.ssh".into(),
                "~/.gnupg".into(),
                "~/.aws".into(),
                "~/.config".into(),
            ],
            max_actions_per_hour: 20,
            max_cost_per_day_cents: 500,
            require_approval_for_medium_risk: true,
            block_high_risk_commands: true,
            shell_env_passthrough: vec![],
            auto_approve: default_auto_approve(),
            always_ask: default_always_ask(),
            allowed_roots: Vec::new(),
            non_cli_excluded_tools: default_non_cli_excluded_tools(),
            non_cli_approval_approvers: Vec::new(),
            non_cli_natural_language_approval_mode: NonCliNaturalLanguageApprovalMode::default(),
            non_cli_natural_language_approval_mode_by_channel: HashMap::new(),
        }
    }
}

// ── Reliability / supervision ────────────────────────────────────

/// Reliability and supervision configuration (`[reliability]` section).
///
/// Controls provider retries, fallback chains, API key rotation, and channel restart backoff.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ReliabilityConfig {
    /// Retries per provider before failing over.
    #[serde(default = "default_provider_retries")]
    pub provider_retries: u32,
    /// Base backoff (ms) for provider retry delay.
    #[serde(default = "default_provider_backoff_ms")]
    pub provider_backoff_ms: u64,
    /// Fallback provider chain (e.g. `["anthropic", "openai"]`).
    #[serde(default)]
    pub fallback_providers: Vec<String>,
    /// Additional API keys for round-robin rotation on rate-limit (429) errors.
    /// The primary `api_key` is always tried first; these are extras.
    #[serde(default)]
    pub api_keys: Vec<String>,
    /// Per-model fallback chains. When a model fails, try these alternatives in order.
    /// Example: `{ "claude-opus-4-20250514" = ["claude-sonnet-4-20250514", "gpt-4o"] }`
    ///
    /// Compatibility behavior: keys matching configured provider names are treated
    /// as provider-scoped remap chains during provider fallback.
    #[serde(default)]
    pub model_fallbacks: std::collections::HashMap<String, Vec<String>>,
    /// Initial backoff for channel/daemon restarts.
    #[serde(default = "default_channel_backoff_secs")]
    pub channel_initial_backoff_secs: u64,
    /// Max backoff for channel/daemon restarts.
    #[serde(default = "default_channel_backoff_max_secs")]
    pub channel_max_backoff_secs: u64,
    /// Scheduler polling cadence in seconds.
    #[serde(default = "default_scheduler_poll_secs")]
    pub scheduler_poll_secs: u64,
    /// Max retries for cron job execution attempts.
    #[serde(default = "default_scheduler_retries")]
    pub scheduler_retries: u32,
}

fn default_provider_retries() -> u32 {
    2
}

fn default_provider_backoff_ms() -> u64 {
    500
}

fn default_channel_backoff_secs() -> u64 {
    2
}

fn default_channel_backoff_max_secs() -> u64 {
    60
}

fn default_scheduler_poll_secs() -> u64 {
    15
}

fn default_scheduler_retries() -> u32 {
    2
}

impl Default for ReliabilityConfig {
    fn default() -> Self {
        Self {
            provider_retries: default_provider_retries(),
            provider_backoff_ms: default_provider_backoff_ms(),
            fallback_providers: Vec::new(),
            api_keys: Vec::new(),
            model_fallbacks: std::collections::HashMap::new(),
            channel_initial_backoff_secs: default_channel_backoff_secs(),
            channel_max_backoff_secs: default_channel_backoff_max_secs(),
            scheduler_poll_secs: default_scheduler_poll_secs(),
            scheduler_retries: default_scheduler_retries(),
        }
    }
}

// ── Scheduler ────────────────────────────────────────────────────

/// Scheduler configuration for periodic task execution (`[scheduler]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SchedulerConfig {
    /// Enable the built-in scheduler loop.
    #[serde(default = "default_scheduler_enabled")]
    pub enabled: bool,
    /// Maximum number of persisted scheduled tasks.
    #[serde(default = "default_scheduler_max_tasks")]
    pub max_tasks: usize,
    /// Maximum tasks executed per scheduler polling cycle.
    #[serde(default = "default_scheduler_max_concurrent")]
    pub max_concurrent: usize,
}

fn default_scheduler_enabled() -> bool {
    true
}

fn default_scheduler_max_tasks() -> usize {
    64
}

fn default_scheduler_max_concurrent() -> usize {
    4
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: default_scheduler_enabled(),
            max_tasks: default_scheduler_max_tasks(),
            max_concurrent: default_scheduler_max_concurrent(),
        }
    }
}

// ── Model routing ────────────────────────────────────────────────

/// Route a task hint to a specific provider + model.
///
/// ```toml
/// [[model_routes]]
/// hint = "reasoning"
/// provider = "openrouter"
/// model = "anthropic/claude-opus-4-20250514"
///
/// [[model_routes]]
/// hint = "fast"
/// provider = "groq"
/// model = "llama-3.3-70b-versatile"
/// ```
///
/// Usage: pass `hint:reasoning` as the model parameter to route the request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ModelRouteConfig {
    /// Task hint name (e.g. "reasoning", "fast", "code", "summarize")
    pub hint: String,
    /// Provider to route to (must match a known provider name)
    pub provider: String,
    /// Model to use with that provider
    pub model: String,
    /// Optional max_tokens override for this route.
    /// When set, provider requests cap output tokens to this value.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// Optional API key override for this route's provider
    #[serde(default)]
    pub api_key: Option<String>,
}

// ── Embedding routing ───────────────────────────────────────────

/// Route an embedding hint to a specific provider + model.
///
/// ```toml
/// [[embedding_routes]]
/// hint = "semantic"
/// provider = "openai"
/// model = "text-embedding-3-small"
/// dimensions = 1536
///
/// [memory]
/// embedding_model = "hint:semantic"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EmbeddingRouteConfig {
    /// Route hint name (e.g. "semantic", "archive", "faq")
    pub hint: String,
    /// Embedding provider (`none`, `openai`, or `custom:<url>`)
    pub provider: String,
    /// Embedding model to use with that provider
    pub model: String,
    /// Optional embedding dimension override for this route
    #[serde(default)]
    pub dimensions: Option<usize>,
    /// Optional API key override for this route's provider
    #[serde(default)]
    pub api_key: Option<String>,
}

// ── Query Classification ─────────────────────────────────────────

/// Automatic query classification — classifies user messages by keyword/pattern
/// and routes to the appropriate model hint. Disabled by default.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct QueryClassificationConfig {
    /// Enable automatic query classification. Default: `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Classification rules evaluated in priority order.
    #[serde(default)]
    pub rules: Vec<ClassificationRule>,
}

/// A single classification rule mapping message patterns to a model hint.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct ClassificationRule {
    /// Must match a `[[model_routes]]` hint value.
    pub hint: String,
    /// Case-insensitive substring matches.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Case-sensitive literal matches (for "```", "fn ", etc.).
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Only match if message length >= N chars.
    #[serde(default)]
    pub min_length: Option<usize>,
    /// Only match if message length <= N chars.
    #[serde(default)]
    pub max_length: Option<usize>,
    /// Higher priority rules are checked first.
    #[serde(default)]
    pub priority: i32,
}

// ── Heartbeat ────────────────────────────────────────────────────

/// Heartbeat configuration for periodic health pings (`[heartbeat]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct HeartbeatConfig {
    /// Enable periodic heartbeat pings. Default: `false`.
    pub enabled: bool,
    /// Interval in minutes between heartbeat pings. Default: `30`.
    pub interval_minutes: u32,
    /// Optional fallback task text when `HEARTBEAT.md` has no task entries.
    #[serde(default)]
    pub message: Option<String>,
    /// Optional delivery channel for heartbeat output (for example: `telegram`).
    #[serde(default, alias = "channel")]
    pub target: Option<String>,
    /// Optional delivery recipient/chat identifier (required when `target` is set).
    #[serde(default, alias = "recipient")]
    pub to: Option<String>,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: 30,
            message: None,
            target: None,
            to: None,
        }
    }
}

// ── Goal Loop Config ────────────────────────────────────────────

/// Configuration for the autonomous goal loop engine (`[goal_loop]`).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GoalLoopConfig {
    /// Enable autonomous goal execution. Default: `false`.
    pub enabled: bool,
    /// Interval in minutes between goal loop cycles. Default: `10`.
    pub interval_minutes: u32,
    /// Timeout in seconds for a single step execution. Default: `120`.
    pub step_timeout_secs: u64,
    /// Maximum steps to execute per cycle. Default: `3`.
    pub max_steps_per_cycle: u32,
    /// Optional channel to deliver goal events to (e.g. "lark", "telegram").
    #[serde(default)]
    pub channel: Option<String>,
    /// Optional recipient/chat_id for goal event delivery.
    #[serde(default)]
    pub target: Option<String>,
}

impl Default for GoalLoopConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_minutes: 10,
            step_timeout_secs: 120,
            max_steps_per_cycle: 3,
            channel: None,
            target: None,
        }
    }
}

// ── Cron ────────────────────────────────────────────────────────

/// Cron job configuration (`[cron]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CronConfig {
    /// Enable the cron subsystem. Default: `true`.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Maximum number of historical cron run records to retain. Default: `50`.
    #[serde(default = "default_max_run_history")]
    pub max_run_history: u32,
}

fn default_max_run_history() -> u32 {
    50
}

impl Default for CronConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_run_history: default_max_run_history(),
        }
    }
}

// ── Tunnel ──────────────────────────────────────────────────────

/// Tunnel configuration for exposing the gateway publicly (`[tunnel]` section).
///
/// Supported providers: `"none"` (default), `"cloudflare"`, `"tailscale"`, `"ngrok"`, `"custom"`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TunnelConfig {
    /// Tunnel provider: `"none"`, `"cloudflare"`, `"tailscale"`, `"ngrok"`, or `"custom"`. Default: `"none"`.
    pub provider: String,

    /// Cloudflare Tunnel configuration (used when `provider = "cloudflare"`).
    #[serde(default)]
    pub cloudflare: Option<CloudflareTunnelConfig>,

    /// Tailscale Funnel/Serve configuration (used when `provider = "tailscale"`).
    #[serde(default)]
    pub tailscale: Option<TailscaleTunnelConfig>,

    /// ngrok tunnel configuration (used when `provider = "ngrok"`).
    #[serde(default)]
    pub ngrok: Option<NgrokTunnelConfig>,

    /// Custom tunnel command configuration (used when `provider = "custom"`).
    #[serde(default)]
    pub custom: Option<CustomTunnelConfig>,
}

impl Default for TunnelConfig {
    fn default() -> Self {
        Self {
            provider: "none".into(),
            cloudflare: None,
            tailscale: None,
            ngrok: None,
            custom: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CloudflareTunnelConfig {
    /// Cloudflare Tunnel token (from Zero Trust dashboard)
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TailscaleTunnelConfig {
    /// Use Tailscale Funnel (public internet) vs Serve (tailnet only)
    #[serde(default)]
    pub funnel: bool,
    /// Optional hostname override
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NgrokTunnelConfig {
    /// ngrok auth token
    pub auth_token: String,
    /// Optional custom domain
    pub domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CustomTunnelConfig {
    /// Command template to start the tunnel. Use {port} and {host} placeholders.
    /// Example: "bore local {port} --to bore.pub"
    pub start_command: String,
    /// Optional URL to check tunnel health
    pub health_url: Option<String>,
    /// Optional regex to extract public URL from command stdout
    pub url_pattern: Option<String>,
}

// ── Channels ─────────────────────────────────────────────────────

struct ConfigWrapper<T: ChannelConfig>(std::marker::PhantomData<T>);

impl<T: ChannelConfig> ConfigWrapper<T> {
    fn new(_: Option<&T>) -> Self {
        Self(std::marker::PhantomData)
    }
}

impl<T: ChannelConfig> crate::config::traits::ConfigHandle for ConfigWrapper<T> {
    fn name(&self) -> &'static str {
        T::name()
    }
    fn desc(&self) -> &'static str {
        T::desc()
    }
}

/// Top-level channel configurations (`[channels_config]` section).
///
/// Each channel sub-section (e.g. `telegram`, `discord`) is optional;
/// setting it to `Some(...)` enables that channel.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ChannelsConfig {
    /// Enable the CLI interactive channel. Default: `true`.
    pub cli: bool,
    /// Telegram bot channel configuration.
    pub telegram: Option<TelegramConfig>,
    /// Discord bot channel configuration.
    pub discord: Option<DiscordConfig>,
    /// Slack bot channel configuration.
    pub slack: Option<SlackConfig>,
    /// Mattermost bot channel configuration.
    pub mattermost: Option<MattermostConfig>,
    /// Webhook channel configuration.
    pub webhook: Option<WebhookConfig>,
    /// iMessage channel configuration (macOS only).
    pub imessage: Option<IMessageConfig>,
    /// Matrix channel configuration.
    pub matrix: Option<MatrixConfig>,
    /// Signal channel configuration.
    pub signal: Option<SignalConfig>,
    /// WhatsApp channel configuration (Cloud API or Web mode).
    pub whatsapp: Option<WhatsAppConfig>,
    /// Linq Partner API channel configuration.
    pub linq: Option<LinqConfig>,
    /// WATI WhatsApp Business API channel configuration.
    pub wati: Option<WatiConfig>,
    /// Nextcloud Talk bot channel configuration.
    pub nextcloud_talk: Option<NextcloudTalkConfig>,
    /// Email channel configuration.
    pub email: Option<crate::channels::email_channel::EmailConfig>,
    /// IRC channel configuration.
    pub irc: Option<IrcConfig>,
    /// Lark channel configuration.
    pub lark: Option<LarkConfig>,
    /// Feishu channel configuration.
    pub feishu: Option<FeishuConfig>,
    /// DingTalk channel configuration.
    pub dingtalk: Option<DingTalkConfig>,
    /// QQ Official Bot channel configuration.
    pub qq: Option<QQConfig>,
    pub nostr: Option<NostrConfig>,
    /// ClawdTalk voice channel configuration.
    pub clawdtalk: Option<crate::channels::clawdtalk::ClawdTalkConfig>,
    /// Base timeout in seconds for processing a single channel message (LLM + tools).
    /// Runtime uses this as a per-turn budget that scales with tool-loop depth
    /// (up to 4x, capped) so one slow/retried model call does not consume the
    /// entire conversation budget.
    /// Default: 300s for on-device LLMs (Ollama) which are slower than cloud APIs.
    #[serde(default = "default_channel_message_timeout_secs")]
    pub message_timeout_secs: u64,
}

impl ChannelsConfig {
    /// get channels' metadata and `.is_some()`, except webhook
    #[rustfmt::skip]
    pub fn channels_except_webhook(&self) -> Vec<(Box<dyn super::traits::ConfigHandle>, bool)> {
        vec![
            (
                Box::new(ConfigWrapper::new(self.telegram.as_ref())),
                self.telegram.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.discord.as_ref())),
                self.discord.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.slack.as_ref())),
                self.slack.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.mattermost.as_ref())),
                self.mattermost.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.imessage.as_ref())),
                self.imessage.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.matrix.as_ref())),
                self.matrix.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.signal.as_ref())),
                self.signal.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.whatsapp.as_ref())),
                self.whatsapp.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.linq.as_ref())),
                self.linq.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.wati.as_ref())),
                self.wati.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.nextcloud_talk.as_ref())),
                self.nextcloud_talk.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.email.as_ref())),
                self.email.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.irc.as_ref())),
                self.irc.is_some()
            ),
            (
                Box::new(ConfigWrapper::new(self.lark.as_ref())),
                self.lark.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.feishu.as_ref())),
                self.feishu.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.dingtalk.as_ref())),
                self.dingtalk.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.qq.as_ref())),
                self.qq
                    .as_ref()
                    .is_some_and(|qq| qq.receive_mode == QQReceiveMode::Websocket)
            ),
            (
                Box::new(ConfigWrapper::new(self.nostr.as_ref())),
                self.nostr.is_some(),
            ),
            (
                Box::new(ConfigWrapper::new(self.clawdtalk.as_ref())),
                self.clawdtalk.is_some(),
            ),
        ]
    }

    pub fn channels(&self) -> Vec<(Box<dyn super::traits::ConfigHandle>, bool)> {
        let mut ret = self.channels_except_webhook();
        ret.push((
            Box::new(ConfigWrapper::new(self.webhook.as_ref())),
            self.webhook.is_some(),
        ));
        ret
    }
}

fn default_channel_message_timeout_secs() -> u64 {
    300
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            cli: true,
            telegram: None,
            discord: None,
            slack: None,
            mattermost: None,
            webhook: None,
            imessage: None,
            matrix: None,
            signal: None,
            whatsapp: None,
            linq: None,
            wati: None,
            nextcloud_talk: None,
            email: None,
            irc: None,
            lark: None,
            feishu: None,
            dingtalk: None,
            qq: None,
            nostr: None,
            clawdtalk: None,
            message_timeout_secs: default_channel_message_timeout_secs(),
        }
    }
}

/// Streaming mode for channels that support progressive message updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum StreamMode {
    /// No streaming -- send the complete response as a single message (default).
    #[default]
    Off,
    /// Update a draft message with every flush interval.
    Partial,
}

fn default_draft_update_interval_ms() -> u64 {
    1000
}

/// Group-chat reply trigger mode for channels that support mention gating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupReplyMode {
    /// Reply only when the bot is explicitly @-mentioned in group chats.
    MentionOnly,
    /// Reply to every message in group chats.
    AllMessages,
}

impl GroupReplyMode {
    #[must_use]
    pub fn requires_mention(self) -> bool {
        matches!(self, Self::MentionOnly)
    }
}

/// Advanced group-chat trigger controls.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct GroupReplyConfig {
    /// Optional explicit trigger mode.
    ///
    /// If omitted, channel-specific legacy behavior is used for compatibility.
    #[serde(default)]
    pub mode: Option<GroupReplyMode>,
    /// Sender IDs that always trigger group replies.
    ///
    /// These IDs bypass mention gating in group chats, but do not bypass the
    /// channel-level inbound allowlist (`allowed_users` / equivalents).
    #[serde(default)]
    pub allowed_sender_ids: Vec<String>,
}

fn resolve_group_reply_mode(
    group_reply: Option<&GroupReplyConfig>,
    legacy_mention_only: Option<bool>,
    default_mode: GroupReplyMode,
) -> GroupReplyMode {
    if let Some(mode) = group_reply.and_then(|cfg| cfg.mode) {
        return mode;
    }
    if let Some(mention_only) = legacy_mention_only {
        return if mention_only {
            GroupReplyMode::MentionOnly
        } else {
            GroupReplyMode::AllMessages
        };
    }
    default_mode
}

fn clone_group_reply_allowed_sender_ids(group_reply: Option<&GroupReplyConfig>) -> Vec<String> {
    group_reply
        .map(|cfg| cfg.allowed_sender_ids.clone())
        .unwrap_or_default()
}

/// Telegram bot channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TelegramConfig {
    /// Telegram Bot API token (from @BotFather).
    ///
    /// Stored as [`crate::security::Secret`] so on-disk form is
    /// `enc2:...` ciphertext when encryption is enabled. Plaintext
    /// only lives inside `.reveal(&store)` return; readers in
    /// `channels::mod` + `cron::scheduler` reveal once at channel
    /// construction. See [[project-secret-newtype-lazy-reveal]].
    pub bot_token: crate::security::Secret,
    /// Allowed Telegram user IDs or usernames. Empty = deny all.
    pub allowed_users: Vec<String>,
    /// Streaming mode for progressive response delivery via message edits.
    #[serde(default)]
    pub stream_mode: StreamMode,
    /// Minimum interval (ms) between draft message edits to avoid rate limits.
    #[serde(default = "default_draft_update_interval_ms")]
    pub draft_update_interval_ms: u64,
    /// When true, a newer Telegram message from the same sender in the same chat
    /// cancels the in-flight request and starts a fresh response with preserved history.
    #[serde(default)]
    pub interrupt_on_new_message: bool,
    /// When true, only respond to messages that @-mention the bot in groups.
    /// Direct messages are always processed.
    #[serde(default)]
    pub mention_only: bool,
    /// Group-chat trigger controls.
    #[serde(default)]
    pub group_reply: Option<GroupReplyConfig>,
    /// Optional custom base URL for Telegram-compatible APIs.
    /// Defaults to "https://api.telegram.org" when omitted.
    /// Example for Bale messenger: "https://tapi.bale.ai"
    #[serde(default)]
    pub base_url: Option<String>,
}

impl ChannelConfig for TelegramConfig {
    fn name() -> &'static str {
        "Telegram"
    }
    fn desc() -> &'static str {
        "connect your bot"
    }
}

impl TelegramConfig {
    #[must_use]
    pub fn effective_group_reply_mode(&self) -> GroupReplyMode {
        resolve_group_reply_mode(
            self.group_reply.as_ref(),
            Some(self.mention_only),
            GroupReplyMode::AllMessages,
        )
    }

    #[must_use]
    pub fn group_reply_allowed_sender_ids(&self) -> Vec<String> {
        clone_group_reply_allowed_sender_ids(self.group_reply.as_ref())
    }
}

/// Discord bot channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiscordConfig {
    /// Discord bot token (from Discord Developer Portal).
    ///
    /// Stored as [`crate::security::Secret`] (PR #N — follows the
    /// Telegram pattern). Readers reveal via SecretStore at channel
    /// construction.
    pub bot_token: crate::security::Secret,
    /// Optional guild (server) ID to restrict the bot to a single guild.
    pub guild_id: Option<String>,
    /// Allowed Discord user IDs. Empty = deny all.
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// When true, process messages from other bots (not just humans).
    /// The bot still ignores its own messages to prevent feedback loops.
    #[serde(default)]
    pub listen_to_bots: bool,
    /// When true, only respond to messages that @-mention the bot.
    /// Other messages in the guild are silently ignored.
    #[serde(default)]
    pub mention_only: bool,
    /// Group-chat trigger controls.
    #[serde(default)]
    pub group_reply: Option<GroupReplyConfig>,
}

impl ChannelConfig for DiscordConfig {
    fn name() -> &'static str {
        "Discord"
    }
    fn desc() -> &'static str {
        "connect your bot"
    }
}

impl DiscordConfig {
    #[must_use]
    pub fn effective_group_reply_mode(&self) -> GroupReplyMode {
        resolve_group_reply_mode(
            self.group_reply.as_ref(),
            Some(self.mention_only),
            GroupReplyMode::AllMessages,
        )
    }

    #[must_use]
    pub fn group_reply_allowed_sender_ids(&self) -> Vec<String> {
        clone_group_reply_allowed_sender_ids(self.group_reply.as_ref())
    }
}

/// Slack bot channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SlackConfig {
    /// Slack bot OAuth token (xoxb-...).
    pub bot_token: crate::security::Secret,
    /// Slack app-level token for Socket Mode (xapp-...).
    pub app_token: Option<crate::security::Secret>,
    // ── below this line: unchanged Slack fields preserved by PR #30 ──
    /// Optional channel ID to restrict the bot to a single channel.
    /// Omit (or set `"*"`) to listen across all accessible channels.
    pub channel_id: Option<String>,
    /// Allowed Slack user IDs. Empty = deny all.
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Group-chat trigger controls.
    #[serde(default)]
    pub group_reply: Option<GroupReplyConfig>,
}

impl ChannelConfig for SlackConfig {
    fn name() -> &'static str {
        "Slack"
    }
    fn desc() -> &'static str {
        "connect your bot"
    }
}

impl SlackConfig {
    #[must_use]
    pub fn effective_group_reply_mode(&self) -> GroupReplyMode {
        resolve_group_reply_mode(self.group_reply.as_ref(), None, GroupReplyMode::AllMessages)
    }

    #[must_use]
    pub fn group_reply_allowed_sender_ids(&self) -> Vec<String> {
        clone_group_reply_allowed_sender_ids(self.group_reply.as_ref())
    }
}

/// Mattermost bot channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MattermostConfig {
    /// Mattermost server URL (e.g. `"https://mattermost.example.com"`).
    pub url: String,
    /// Mattermost bot access token.
    pub bot_token: crate::security::Secret,
    /// Optional channel ID to restrict the bot to a single channel.
    pub channel_id: Option<String>,
    /// Allowed Mattermost user IDs. Empty = deny all.
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// When true (default), replies thread on the original post.
    /// When false, replies go to the channel root.
    #[serde(default)]
    pub thread_replies: Option<bool>,
    /// When true, only respond to messages that @-mention the bot.
    /// Other messages in the channel are silently ignored.
    #[serde(default)]
    pub mention_only: Option<bool>,
    /// Group-chat trigger controls.
    #[serde(default)]
    pub group_reply: Option<GroupReplyConfig>,
}

impl ChannelConfig for MattermostConfig {
    fn name() -> &'static str {
        "Mattermost"
    }
    fn desc() -> &'static str {
        "connect to your bot"
    }
}

impl MattermostConfig {
    #[must_use]
    pub fn effective_group_reply_mode(&self) -> GroupReplyMode {
        resolve_group_reply_mode(
            self.group_reply.as_ref(),
            Some(self.mention_only.unwrap_or(false)),
            GroupReplyMode::AllMessages,
        )
    }

    #[must_use]
    pub fn group_reply_allowed_sender_ids(&self) -> Vec<String> {
        clone_group_reply_allowed_sender_ids(self.group_reply.as_ref())
    }
}

/// Webhook channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WebhookConfig {
    /// Port to listen on for incoming webhooks.
    pub port: u16,
    /// Optional shared secret for webhook signature verification.
    ///
    /// Stored as a [`crate::security::Secret`] (encrypted at rest);
    /// revealed + hashed once at gateway startup. When `None` AND the
    /// gateway is bound to a non-loopback address, the `/webhook`
    /// handler rejects all requests (secure-by-default).
    pub secret: Option<crate::security::Secret>,
}

impl ChannelConfig for WebhookConfig {
    fn name() -> &'static str {
        "Webhook"
    }
    fn desc() -> &'static str {
        "HTTP endpoint"
    }
}

/// iMessage channel configuration (macOS only).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IMessageConfig {
    /// Allowed iMessage contacts (phone numbers or email addresses). Empty = deny all.
    pub allowed_contacts: Vec<String>,
}

impl ChannelConfig for IMessageConfig {
    fn name() -> &'static str {
        "iMessage"
    }
    fn desc() -> &'static str {
        "macOS only"
    }
}

/// Matrix channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MatrixConfig {
    /// Matrix homeserver URL (e.g. `"https://matrix.org"`).
    pub homeserver: String,
    /// Matrix access token for the bot account.
    pub access_token: crate::security::Secret,
    /// Optional Matrix user ID (e.g. `"@bot:matrix.org"`).
    #[serde(default)]
    pub user_id: Option<String>,
    /// Optional Matrix device ID.
    #[serde(default)]
    pub device_id: Option<String>,
    /// Matrix room ID to listen in (e.g. `"!abc123:matrix.org"`).
    pub room_id: String,
    /// Allowed Matrix user IDs. Empty = deny all.
    pub allowed_users: Vec<String>,
    /// When true, only respond to direct rooms, explicit @-mentions, or replies to bot messages.
    #[serde(default)]
    pub mention_only: bool,
}

impl ChannelConfig for MatrixConfig {
    fn name() -> &'static str {
        "Matrix"
    }
    fn desc() -> &'static str {
        "self-hosted chat"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SignalConfig {
    /// Base URL for the signal-cli HTTP daemon (e.g. "http://127.0.0.1:8686").
    pub http_url: String,
    /// E.164 phone number of the signal-cli account (e.g. "+1234567890").
    pub account: String,
    /// Optional group ID to filter messages.
    /// - `None` or omitted: accept all messages (DMs and groups)
    /// - `"dm"`: only accept direct messages
    /// - Specific group ID: only accept messages from that group
    #[serde(default)]
    pub group_id: Option<String>,
    /// Allowed sender phone numbers (E.164) or "*" for all.
    #[serde(default)]
    pub allowed_from: Vec<String>,
    /// Skip messages that are attachment-only (no text body).
    #[serde(default)]
    pub ignore_attachments: bool,
    /// Skip incoming story messages.
    #[serde(default)]
    pub ignore_stories: bool,
}

impl ChannelConfig for SignalConfig {
    fn name() -> &'static str {
        "Signal"
    }
    fn desc() -> &'static str {
        "An open-source, encrypted messaging service"
    }
}

/// WhatsApp channel configuration (Cloud API or Web mode).
///
/// Set `phone_number_id` for Cloud API mode, or `session_path` for Web mode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WhatsAppConfig {
    /// Access token from Meta Business Suite (Cloud API mode)
    #[serde(default)]
    pub access_token: Option<crate::security::Secret>,
    /// Phone number ID from Meta Business API (Cloud API mode)
    #[serde(default)]
    pub phone_number_id: Option<String>,
    /// Webhook verify token (you define this, Meta sends it back for verification)
    /// Only used in Cloud API mode
    #[serde(default)]
    pub verify_token: Option<crate::security::Secret>,
    /// App secret from Meta Business Suite (for webhook signature verification)
    /// Can also be set via `PLAW_WHATSAPP_APP_SECRET` environment variable
    /// Only used in Cloud API mode
    #[serde(default)]
    pub app_secret: Option<crate::security::Secret>,
    /// Session database path for WhatsApp Web client (Web mode)
    /// When set, enables native WhatsApp Web mode with wa-rs
    #[serde(default)]
    pub session_path: Option<String>,
    /// Phone number for pair code linking (Web mode, optional)
    /// Format: country code + number (e.g., "15551234567")
    /// If not set, QR code pairing will be used
    #[serde(default)]
    pub pair_phone: Option<String>,
    /// Custom pair code for linking (Web mode, optional)
    /// Leave empty to let WhatsApp generate one
    #[serde(default)]
    pub pair_code: Option<String>,
    /// Allowed phone numbers (E.164 format: +1234567890) or "*" for all
    #[serde(default)]
    pub allowed_numbers: Vec<String>,
}

impl ChannelConfig for WhatsAppConfig {
    fn name() -> &'static str {
        "WhatsApp"
    }
    fn desc() -> &'static str {
        "Business Cloud API"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LinqConfig {
    /// Linq Partner API token (Bearer auth) used for OUTBOUND calls.
    ///
    /// Stored as [`crate::security::Secret`] so the on-disk form is
    /// `enc2:` ciphertext; plaintext lives only inside the
    /// `reveal(&store)` return at channel construction.
    pub api_token: crate::security::Secret,
    /// Phone number to send from (E.164 format)
    pub from_phone: String,
    /// Webhook signing secret for signature verification.
    ///
    /// Can also be set via `PLAW_LINQ_SIGNING_SECRET`. Stored as a
    /// [`crate::security::Secret`] (encrypted at rest).
    #[serde(default)]
    pub signing_secret: Option<crate::security::Secret>,
    /// Allowed sender handles (phone numbers) or "*" for all
    #[serde(default)]
    pub allowed_senders: Vec<String>,
}

impl ChannelConfig for LinqConfig {
    fn name() -> &'static str {
        "Linq"
    }
    fn desc() -> &'static str {
        "iMessage/RCS/SMS via Linq API"
    }
}

/// WATI WhatsApp Business API channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WatiConfig {
    /// WATI API token (Bearer auth) used for OUTBOUND calls to WATI's REST API.
    ///
    /// Stored as [`crate::security::Secret`] so on-disk form is `enc2:`
    /// ciphertext. Plaintext lives only inside `reveal(&store)` return.
    pub api_token: crate::security::Secret,
    /// WATI API base URL (default: https://live-mt-server.wati.io).
    #[serde(default = "default_wati_api_url")]
    pub api_url: String,
    /// Tenant ID for multi-channel setups (optional).
    #[serde(default)]
    pub tenant_id: Option<String>,
    /// Allowed phone numbers (E.164 format) or "*" for all.
    #[serde(default)]
    pub allowed_numbers: Vec<String>,
    /// INBOUND shared secret for the `/wati` webhook endpoint. WATI does
    /// not sign webhook callbacks, so plaw cannot verify origin
    /// cryptographically; instead, configure this secret here AND on the
    /// WATI dashboard's webhook URL as a query string or header (e.g.
    /// append `?secret=<value>` to the URL, or configure an
    /// `X-Webhook-Secret` header if WATI's UI allows). The handler then
    /// requires the same value on every incoming POST.
    ///
    /// Stored as a [`crate::security::Secret`] so the on-disk form is
    /// the encrypted `enc2:` ciphertext blob; the plaintext is only
    /// reconstituted via `.reveal(&SecretStore)` at the secret-hash
    /// computation site. This is the first field in the codebase to
    /// adopt the `Secret` newtype — see [`crate::security::secret`] for
    /// the lazy-reveal contract.
    ///
    /// When `None` AND the gateway is bound to a non-loopback address,
    /// the handler rejects all requests with 401 — that's the
    /// secure-by-default invariant for this endpoint. Loopback-only
    /// deployments (typical desktop use) work without a secret.
    #[serde(default)]
    pub webhook_secret: Option<crate::security::Secret>,
}

fn default_wati_api_url() -> String {
    "https://live-mt-server.wati.io".to_string()
}

impl ChannelConfig for WatiConfig {
    fn name() -> &'static str {
        "WATI"
    }
    fn desc() -> &'static str {
        "WhatsApp via WATI Business API"
    }
}

/// Nextcloud Talk bot configuration (webhook receive + OCS send API).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NextcloudTalkConfig {
    /// Nextcloud base URL (e.g. "https://cloud.example.com").
    pub base_url: String,
    /// Bot app token used for OCS API bearer auth.
    ///
    /// Stored as [`crate::security::Secret`] so the on-disk form is
    /// `enc2:` ciphertext; plaintext lives only inside the
    /// `reveal(&store)` return at channel construction.
    pub app_token: crate::security::Secret,
    /// Shared secret for webhook signature verification.
    ///
    /// Can also be set via `PLAW_NEXTCLOUD_TALK_WEBHOOK_SECRET`. Stored
    /// as a [`crate::security::Secret`] (encrypted at rest).
    #[serde(default)]
    pub webhook_secret: Option<crate::security::Secret>,
    /// Allowed Nextcloud actor IDs (`[]` = deny all, `"*"` = allow all).
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

impl ChannelConfig for NextcloudTalkConfig {
    fn name() -> &'static str {
        "NextCloud Talk"
    }
    fn desc() -> &'static str {
        "NextCloud Talk platform"
    }
}

impl WhatsAppConfig {
    /// Detect which backend to use based on config fields.
    /// Returns "cloud" if phone_number_id is set, "web" if session_path is set.
    pub fn backend_type(&self) -> &'static str {
        if self.phone_number_id.is_some() {
            "cloud"
        } else if self.session_path.is_some() {
            "web"
        } else {
            // Default to Cloud API for backward compatibility
            "cloud"
        }
    }

    /// Check if this is a valid Cloud API config
    pub fn is_cloud_config(&self) -> bool {
        self.phone_number_id.is_some() && self.access_token.is_some() && self.verify_token.is_some()
    }

    /// Check if this is a valid Web config
    pub fn is_web_config(&self) -> bool {
        self.session_path.is_some()
    }

    /// Returns true when both Cloud and Web selectors are present.
    ///
    /// Runtime currently prefers Cloud mode in this case for backward compatibility.
    pub fn is_ambiguous_config(&self) -> bool {
        self.phone_number_id.is_some() && self.session_path.is_some()
    }
}

/// IRC channel configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IrcConfig {
    /// IRC server hostname
    pub server: String,
    /// IRC server port (default: 6697 for TLS)
    #[serde(default = "default_irc_port")]
    pub port: u16,
    /// Bot nickname
    pub nickname: String,
    /// Username (defaults to nickname if not set)
    pub username: Option<String>,
    /// Channels to join on connect
    #[serde(default)]
    pub channels: Vec<String>,
    /// Allowed nicknames (case-insensitive) or "*" for all
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Server password (for bouncers like ZNC). Encrypted at rest via [`crate::security::Secret`].
    pub server_password: Option<crate::security::Secret>,
    /// NickServ IDENTIFY password. Encrypted at rest via [`crate::security::Secret`].
    pub nickserv_password: Option<crate::security::Secret>,
    /// SASL PLAIN password (IRCv3). Encrypted at rest via [`crate::security::Secret`].
    pub sasl_password: Option<crate::security::Secret>,
    /// Verify TLS certificate (default: true)
    pub verify_tls: Option<bool>,
}

impl ChannelConfig for IrcConfig {
    fn name() -> &'static str {
        "IRC"
    }
    fn desc() -> &'static str {
        "IRC over TLS"
    }
}

fn default_irc_port() -> u16 {
    6697
}

/// How Plaw receives events from Feishu / Lark.
///
/// - `websocket` (default) — persistent WSS long-connection; no public URL required.
/// - `webhook`             — HTTP callback server; requires a public HTTPS endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LarkReceiveMode {
    #[default]
    Websocket,
    Webhook,
}

pub fn default_lark_draft_update_interval_ms() -> u64 {
    3000
}

pub fn default_lark_max_draft_edits() -> u32 {
    20
}

/// Lark/Feishu configuration for messaging integration.
/// Lark is the international version; Feishu is the Chinese version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LarkConfig {
    /// App ID from Lark/Feishu developer console
    pub app_id: String,
    /// App Secret from Lark/Feishu developer console
    pub app_secret: crate::security::Secret,
    /// Encrypt key for webhook message decryption (optional)
    #[serde(default)]
    pub encrypt_key: Option<crate::security::Secret>,
    /// Verification token for webhook validation (optional)
    #[serde(default)]
    pub verification_token: Option<crate::security::Secret>,
    /// Allowed user IDs or union IDs (empty = deny all, "*" = allow all)
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// When true, only respond to messages that @-mention the bot in groups.
    /// Direct messages are always processed.
    #[serde(default)]
    pub mention_only: bool,
    /// Group-chat trigger controls.
    #[serde(default)]
    pub group_reply: Option<GroupReplyConfig>,
    /// Whether to use the Feishu (Chinese) endpoint instead of Lark (International)
    #[serde(default)]
    pub use_feishu: bool,
    /// Event receive mode: "websocket" (default) or "webhook"
    #[serde(default)]
    pub receive_mode: LarkReceiveMode,
    /// HTTP port for webhook mode only. Must be set when receive_mode = "webhook".
    /// Not required (and ignored) for websocket mode.
    #[serde(default)]
    pub port: Option<u16>,
    /// Minimum interval (ms) between draft message edits. Default: 3000.
    #[serde(default = "default_lark_draft_update_interval_ms")]
    pub draft_update_interval_ms: u64,
    /// Maximum number of edits per draft message before stopping updates.
    #[serde(default = "default_lark_max_draft_edits")]
    pub max_draft_edits: u32,
}

impl ChannelConfig for LarkConfig {
    fn name() -> &'static str {
        "Lark"
    }
    fn desc() -> &'static str {
        "Lark Bot"
    }
}

impl LarkConfig {
    #[must_use]
    pub fn effective_group_reply_mode(&self) -> GroupReplyMode {
        resolve_group_reply_mode(
            self.group_reply.as_ref(),
            Some(self.mention_only),
            GroupReplyMode::AllMessages,
        )
    }

    #[must_use]
    pub fn group_reply_allowed_sender_ids(&self) -> Vec<String> {
        clone_group_reply_allowed_sender_ids(self.group_reply.as_ref())
    }
}

/// Feishu configuration for messaging integration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FeishuConfig {
    /// App ID from Feishu developer console
    pub app_id: String,
    /// App Secret from Feishu developer console
    pub app_secret: crate::security::Secret,
    /// Encrypt key for webhook message decryption (optional)
    #[serde(default)]
    pub encrypt_key: Option<crate::security::Secret>,
    /// Verification token for webhook validation (optional)
    #[serde(default)]
    pub verification_token: Option<crate::security::Secret>,
    /// Allowed user IDs or union IDs (empty = deny all, "*" = allow all)
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Group-chat trigger controls.
    #[serde(default)]
    pub group_reply: Option<GroupReplyConfig>,
    /// Event receive mode: "websocket" (default) or "webhook"
    #[serde(default)]
    pub receive_mode: LarkReceiveMode,
    /// HTTP port for webhook mode only. Must be set when receive_mode = "webhook".
    /// Not required (and ignored) for websocket mode.
    #[serde(default)]
    pub port: Option<u16>,
    /// Minimum interval between streaming draft edits (milliseconds).
    #[serde(default = "default_lark_draft_update_interval_ms")]
    pub draft_update_interval_ms: u64,
    /// Maximum number of draft edits per message before finalizing.
    #[serde(default = "default_lark_max_draft_edits")]
    pub max_draft_edits: u32,
}

impl ChannelConfig for FeishuConfig {
    fn name() -> &'static str {
        "Feishu"
    }
    fn desc() -> &'static str {
        "Feishu Bot"
    }
}

impl FeishuConfig {
    #[must_use]
    pub fn effective_group_reply_mode(&self) -> GroupReplyMode {
        resolve_group_reply_mode(self.group_reply.as_ref(), None, GroupReplyMode::AllMessages)
    }

    #[must_use]
    pub fn group_reply_allowed_sender_ids(&self) -> Vec<String> {
        clone_group_reply_allowed_sender_ids(self.group_reply.as_ref())
    }
}

// ── Security Config ─────────────────────────────────────────────────

/// Security configuration for sandboxing, resource limits, and audit logging
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
pub struct SecurityConfig {
    /// Sandbox configuration
    #[serde(default)]
    pub sandbox: SandboxConfig,

    /// Resource limits
    #[serde(default)]
    pub resources: ResourceLimitsConfig,

    /// Audit logging configuration
    #[serde(default)]
    pub audit: AuditConfig,

    /// OTP gating configuration for sensitive actions/domains.
    #[serde(default)]
    pub otp: OtpConfig,

    /// Emergency-stop state machine configuration.
    #[serde(default)]
    pub estop: EstopConfig,

    /// Syscall anomaly detection profile for daemon shell/process execution.
    #[serde(default)]
    pub syscall_anomaly: SyscallAnomalyConfig,
}

/// OTP validation strategy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OtpMethod {
    /// Time-based one-time password (RFC 6238).
    #[default]
    Totp,
    /// Future method for paired-device confirmations.
    Pairing,
    /// Future method for local CLI challenge prompts.
    CliPrompt,
}

/// Security OTP configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OtpConfig {
    /// Enable OTP gating. Defaults to disabled for backward compatibility.
    #[serde(default)]
    pub enabled: bool,

    /// OTP method.
    #[serde(default)]
    pub method: OtpMethod,

    /// TOTP time-step in seconds.
    #[serde(default = "default_otp_token_ttl_secs")]
    pub token_ttl_secs: u64,

    /// Reuse window for recently validated OTP codes.
    #[serde(default = "default_otp_cache_valid_secs")]
    pub cache_valid_secs: u64,

    /// Tool/action names gated by OTP.
    #[serde(default = "default_otp_gated_actions")]
    pub gated_actions: Vec<String>,

    /// Explicit domain patterns gated by OTP.
    #[serde(default)]
    pub gated_domains: Vec<String>,

    /// Domain-category presets expanded into `gated_domains`.
    #[serde(default)]
    pub gated_domain_categories: Vec<String>,
}

fn default_otp_token_ttl_secs() -> u64 {
    30
}

fn default_otp_cache_valid_secs() -> u64 {
    300
}

fn default_otp_gated_actions() -> Vec<String> {
    vec![
        "shell".to_string(),
        "file_write".to_string(),
        "browser_open".to_string(),
        "browser".to_string(),
        "memory_forget".to_string(),
    ]
}

impl Default for OtpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            method: OtpMethod::Totp,
            token_ttl_secs: default_otp_token_ttl_secs(),
            cache_valid_secs: default_otp_cache_valid_secs(),
            gated_actions: default_otp_gated_actions(),
            gated_domains: Vec::new(),
            gated_domain_categories: Vec::new(),
        }
    }
}

/// Emergency stop configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EstopConfig {
    /// Enable emergency stop controls.
    #[serde(default)]
    pub enabled: bool,

    /// File path used to persist estop state.
    #[serde(default = "default_estop_state_file")]
    pub state_file: String,

    /// Require a valid OTP before resume operations.
    #[serde(default = "default_true")]
    pub require_otp_to_resume: bool,
}

fn default_estop_state_file() -> String {
    "~/.plaw/estop-state.json".to_string()
}

impl Default for EstopConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            state_file: default_estop_state_file(),
            require_otp_to_resume: true,
        }
    }
}

/// Syscall anomaly detection configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SyscallAnomalyConfig {
    /// Enable syscall anomaly detection.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Treat denied syscall lines as anomalies even when syscall is in baseline.
    #[serde(default)]
    pub strict_mode: bool,

    /// Emit anomaly alerts when a syscall appears outside the expected baseline.
    #[serde(default = "default_true")]
    pub alert_on_unknown_syscall: bool,

    /// Allowed denied-syscall events per rolling minute before triggering an alert.
    #[serde(default = "default_syscall_anomaly_max_denied_events_per_minute")]
    pub max_denied_events_per_minute: u32,

    /// Allowed total syscall telemetry events per rolling minute before triggering an alert.
    #[serde(default = "default_syscall_anomaly_max_total_events_per_minute")]
    pub max_total_events_per_minute: u32,

    /// Maximum anomaly alerts emitted per rolling minute (global guardrail).
    #[serde(default = "default_syscall_anomaly_max_alerts_per_minute")]
    pub max_alerts_per_minute: u32,

    /// Cooldown between identical anomaly alerts (seconds).
    #[serde(default = "default_syscall_anomaly_alert_cooldown_secs")]
    pub alert_cooldown_secs: u64,

    /// Path to syscall anomaly log file (relative to ~/.plaw unless absolute).
    #[serde(default = "default_syscall_anomaly_log_path")]
    pub log_path: String,

    /// Expected syscall baseline. Unknown syscall names trigger anomaly when enabled.
    #[serde(default = "default_syscall_anomaly_baseline_syscalls")]
    pub baseline_syscalls: Vec<String>,
}

fn default_syscall_anomaly_max_denied_events_per_minute() -> u32 {
    5
}

fn default_syscall_anomaly_max_total_events_per_minute() -> u32 {
    120
}

fn default_syscall_anomaly_max_alerts_per_minute() -> u32 {
    30
}

fn default_syscall_anomaly_alert_cooldown_secs() -> u64 {
    20
}

fn default_syscall_anomaly_log_path() -> String {
    "syscall-anomalies.log".to_string()
}

fn default_syscall_anomaly_baseline_syscalls() -> Vec<String> {
    vec![
        "read".to_string(),
        "write".to_string(),
        "open".to_string(),
        "openat".to_string(),
        "close".to_string(),
        "stat".to_string(),
        "fstat".to_string(),
        "newfstatat".to_string(),
        "lseek".to_string(),
        "mmap".to_string(),
        "mprotect".to_string(),
        "munmap".to_string(),
        "brk".to_string(),
        "rt_sigaction".to_string(),
        "rt_sigprocmask".to_string(),
        "ioctl".to_string(),
        "fcntl".to_string(),
        "access".to_string(),
        "pipe2".to_string(),
        "dup".to_string(),
        "dup2".to_string(),
        "dup3".to_string(),
        "epoll_create1".to_string(),
        "epoll_ctl".to_string(),
        "epoll_wait".to_string(),
        "poll".to_string(),
        "ppoll".to_string(),
        "select".to_string(),
        "futex".to_string(),
        "clock_gettime".to_string(),
        "nanosleep".to_string(),
        "getpid".to_string(),
        "gettid".to_string(),
        "set_tid_address".to_string(),
        "set_robust_list".to_string(),
        "clone".to_string(),
        "clone3".to_string(),
        "fork".to_string(),
        "execve".to_string(),
        "wait4".to_string(),
        "exit".to_string(),
        "exit_group".to_string(),
        "socket".to_string(),
        "connect".to_string(),
        "accept".to_string(),
        "accept4".to_string(),
        "listen".to_string(),
        "sendto".to_string(),
        "recvfrom".to_string(),
        "sendmsg".to_string(),
        "recvmsg".to_string(),
        "getsockname".to_string(),
        "getpeername".to_string(),
        "setsockopt".to_string(),
        "getsockopt".to_string(),
        "getrandom".to_string(),
        "statx".to_string(),
    ]
}

impl Default for SyscallAnomalyConfig {
    fn default() -> Self {
        Self {
            enabled: default_true(),
            strict_mode: false,
            alert_on_unknown_syscall: default_true(),
            max_denied_events_per_minute: default_syscall_anomaly_max_denied_events_per_minute(),
            max_total_events_per_minute: default_syscall_anomaly_max_total_events_per_minute(),
            max_alerts_per_minute: default_syscall_anomaly_max_alerts_per_minute(),
            alert_cooldown_secs: default_syscall_anomaly_alert_cooldown_secs(),
            log_path: default_syscall_anomaly_log_path(),
            baseline_syscalls: default_syscall_anomaly_baseline_syscalls(),
        }
    }
}

/// Sandbox configuration for OS-level isolation
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SandboxConfig {
    /// Enable sandboxing (None = auto-detect, Some = explicit)
    #[serde(default)]
    pub enabled: Option<bool>,

    /// Sandbox backend to use
    #[serde(default)]
    pub backend: SandboxBackend,

    /// Custom Firejail arguments (when backend = firejail)
    #[serde(default)]
    pub firejail_args: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: None, // Auto-detect
            backend: SandboxBackend::Auto,
            firejail_args: Vec::new(),
        }
    }
}

/// Sandbox backend selection
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SandboxBackend {
    /// Auto-detect best available (default)
    #[default]
    Auto,
    /// Landlock (Linux kernel LSM, native)
    Landlock,
    /// Firejail (user-space sandbox)
    Firejail,
    /// Bubblewrap (user namespaces)
    Bubblewrap,
    /// Docker container isolation
    Docker,
    /// Windows Job Object (kernel-level process container, Windows only).
    /// Auto-kills child processes on plaw exit; foundation for future
    /// `SetInformationJobObject` resource limits.
    #[serde(rename = "windows-job-object")]
    WindowsJobObject,
    /// No sandboxing (application-layer only)
    None,
}

/// Resource limits for command execution
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResourceLimitsConfig {
    /// Maximum memory in MB per command
    #[serde(default = "default_max_memory_mb")]
    pub max_memory_mb: u32,

    /// Maximum CPU time in seconds per command
    #[serde(default = "default_max_cpu_time_seconds")]
    pub max_cpu_time_seconds: u64,

    /// Maximum number of subprocesses
    #[serde(default = "default_max_subprocesses")]
    pub max_subprocesses: u32,

    /// Enable memory monitoring
    #[serde(default = "default_memory_monitoring_enabled")]
    pub memory_monitoring: bool,
}

fn default_max_memory_mb() -> u32 {
    512
}

fn default_max_cpu_time_seconds() -> u64 {
    60
}

fn default_max_subprocesses() -> u32 {
    10
}

fn default_memory_monitoring_enabled() -> bool {
    true
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: default_max_memory_mb(),
            max_cpu_time_seconds: default_max_cpu_time_seconds(),
            max_subprocesses: default_max_subprocesses(),
            memory_monitoring: default_memory_monitoring_enabled(),
        }
    }
}

/// Audit logging configuration
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AuditConfig {
    /// Enable audit logging
    #[serde(default = "default_audit_enabled")]
    pub enabled: bool,

    /// Path to audit log file (relative to plaw dir)
    #[serde(default = "default_audit_log_path")]
    pub log_path: String,

    /// Maximum log size in MB before rotation
    #[serde(default = "default_audit_max_size_mb")]
    pub max_size_mb: u32,

    /// Sign events with HMAC for tamper evidence
    #[serde(default)]
    pub sign_events: bool,
}

fn default_audit_enabled() -> bool {
    true
}

fn default_audit_log_path() -> String {
    "audit.log".to_string()
}

fn default_audit_max_size_mb() -> u32 {
    100
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self {
            enabled: default_audit_enabled(),
            log_path: default_audit_log_path(),
            max_size_mb: default_audit_max_size_mb(),
            sign_events: false,
        }
    }
}

/// DingTalk configuration for Stream Mode messaging
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DingTalkConfig {
    /// Client ID (AppKey) from DingTalk developer console
    pub client_id: String,
    /// Client Secret (AppSecret) from DingTalk developer console.
    /// Encrypted at rest via [`crate::security::Secret`]; revealed at channel construction.
    pub client_secret: crate::security::Secret,
    /// Allowed user IDs (staff IDs). Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_users: Vec<String>,
}

impl ChannelConfig for DingTalkConfig {
    fn name() -> &'static str {
        "DingTalk"
    }
    fn desc() -> &'static str {
        "DingTalk Stream Mode"
    }
}

/// QQ Official Bot configuration (Tencent QQ Bot SDK)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum QQReceiveMode {
    Websocket,
    #[default]
    Webhook,
}

/// QQ Official Bot configuration (Tencent QQ Bot SDK)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct QQConfig {
    /// App ID from QQ Bot developer console
    pub app_id: String,
    /// App Secret from QQ Bot developer console.
    /// Encrypted at rest via [`crate::security::Secret`]; revealed at channel construction.
    pub app_secret: crate::security::Secret,
    /// Allowed user IDs. Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Event receive mode: "webhook" (default) or "websocket".
    #[serde(default)]
    pub receive_mode: QQReceiveMode,
    /// INBOUND shared secret for the `/qq` webhook endpoint. The pre-existing
    /// `X-Bot-Appid` header check protects against accidentally cross-wired
    /// bots but NOT against a determined attacker — `app_id` is a public
    /// identifier visible in QQ's developer console. Configure this secret
    /// here AND on the QQ webhook URL (as a query string or `X-Webhook-Secret`
    /// header) for cryptographic-strength auth.
    ///
    /// When `None` AND the gateway is bound to a non-loopback address,
    /// the handler rejects with 401 — secure-by-default. Loopback-only
    /// deployments work without a secret.
    #[serde(default)]
    pub webhook_secret: Option<crate::security::Secret>,
}

impl ChannelConfig for QQConfig {
    fn name() -> &'static str {
        "QQ Official"
    }
    fn desc() -> &'static str {
        "Tencent QQ Bot"
    }
}

/// Nostr channel configuration (NIP-04 + NIP-17 private messages)
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct NostrConfig {
    /// Private key in hex or nsec bech32 format
    pub private_key: String,
    /// Relay URLs (wss://). Defaults to popular public relays if omitted.
    #[serde(default = "default_nostr_relays")]
    pub relays: Vec<String>,
    /// Allowed sender public keys (hex or npub). Empty = deny all, "*" = allow all
    #[serde(default)]
    pub allowed_pubkeys: Vec<String>,
}

impl ChannelConfig for NostrConfig {
    fn name() -> &'static str {
        "Nostr"
    }
    fn desc() -> &'static str {
        "Nostr DMs"
    }
}

pub fn default_nostr_relays() -> Vec<String> {
    vec![
        "wss://relay.damus.io".to_string(),
        "wss://nos.lol".to_string(),
        "wss://relay.primal.net".to_string(),
        "wss://relay.snort.social".to_string(),
    ]
}

// ── Config impl ──────────────────────────────────────────────────

impl Default for Config {
    fn default() -> Self {
        let home =
            UserDirs::new().map_or_else(|| PathBuf::from("."), |u| u.home_dir().to_path_buf());
        let plaw_dir = home.join(".plaw");

        Self {
            workspace_dir: plaw_dir.join("workspace"),
            config_path: plaw_dir.join("config.toml"),
            api_key: None,
            api_url: None,
            // Default provider/model align with plaw/CLAUDE.md §"AI 模型配置":
            // DeepSeek V4 Pro is the current recommended default (China-direct,
            // no proxy needed, strongest domestic model). This is the fallback
            // when config.toml has no [default_provider]; per
            // [[project-model-agnostic-invariant]] users can override to any
            // registered provider via config file with no code change.
            // Constants live at module scope so `apply_env_overrides` knows
            // which value is the "untouched fallback" marker (legacy
            // PROVIDER env var only overrides the fallback, not user choice).
            default_provider: Some(DEFAULT_PROVIDER_FALLBACK.to_string()),
            provider_api: None,
            default_model: Some(DEFAULT_MODEL_FALLBACK.to_string()),
            model_providers: HashMap::new(),
            provider: ProviderConfig::default(),
            default_temperature: 0.7,
            observability: ObservabilityConfig::default(),
            autonomy: AutonomyConfig::default(),
            security: SecurityConfig::default(),
            runtime: RuntimeConfig::default(),
            research: ResearchPhaseConfig::default(),
            reliability: ReliabilityConfig::default(),
            scheduler: SchedulerConfig::default(),
            agent: AgentConfig::default(),
            skills: SkillsConfig::default(),
            model_routes: Vec::new(),
            embedding_routes: Vec::new(),
            heartbeat: HeartbeatConfig::default(),
            cron: CronConfig::default(),
            goal_loop: GoalLoopConfig::default(),
            channels_config: ChannelsConfig::default(),
            memory: MemoryConfig::default(),
            storage: StorageConfig::default(),
            tunnel: TunnelConfig::default(),
            gateway: GatewayConfig::default(),
            composio: ComposioConfig::default(),
            secrets: SecretsConfig::default(),
            browser: BrowserConfig::default(),
            http_request: HttpRequestConfig::default(),
            multimodal: MultimodalConfig::default(),
            web_fetch: WebFetchConfig::default(),
            web_search: WebSearchConfig::default(),
            proxy: ProxyConfig::default(),
            identity: IdentityConfig::default(),
            cost: CostConfig::default(),
            peripherals: PeripheralsConfig::default(),
            agents: HashMap::new(),
            pipelines: HashMap::new(),
            mcp: McpConfig::default(),
            coordination: CoordinationConfig::default(),
            hooks: HooksConfig::default(),
            hardware: HardwareConfig::default(),
            query_classification: QueryClassificationConfig::default(),
            transcription: TranscriptionConfig::default(),
            agents_ipc: AgentsIpcConfig::default(),
            repo_map: RepoMapConfig::default(),
            edit_linter: EditLinterConfig::default(),
            chain_of_verification: ChainOfVerificationConfig::default(),
            model_support_vision: None,
        }
    }
}

fn default_config_and_workspace_dirs() -> Result<(PathBuf, PathBuf)> {
    let config_dir = default_config_dir()?;
    Ok((config_dir.clone(), config_dir.join("workspace")))
}

const ACTIVE_WORKSPACE_STATE_FILE: &str = "active_workspace.toml";

#[derive(Debug, Serialize, Deserialize)]
struct ActiveWorkspaceState {
    config_dir: String,
}

fn default_config_dir() -> Result<PathBuf> {
    let home = UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .context("Could not find home directory")?;
    Ok(home.join(".plaw"))
}

fn active_workspace_state_path(default_dir: &Path) -> PathBuf {
    default_dir.join(ACTIVE_WORKSPACE_STATE_FILE)
}

/// Returns `true` if `path` lives under the OS temp directory.
fn is_temp_directory(path: &Path) -> bool {
    let temp = std::env::temp_dir();
    // Canonicalize when possible to handle symlinks (macOS /var → /private/var)
    let canon_temp = temp.canonicalize().unwrap_or_else(|_| temp.clone());
    let canon_path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    canon_path.starts_with(&canon_temp)
}

async fn load_persisted_workspace_dirs(
    default_config_dir: &Path,
) -> Result<Option<(PathBuf, PathBuf)>> {
    let state_path = active_workspace_state_path(default_config_dir);
    if !state_path.exists() {
        return Ok(None);
    }

    let contents = match fs::read_to_string(&state_path).await {
        Ok(contents) => contents,
        Err(error) => {
            tracing::warn!(
                "Failed to read active workspace marker {}: {error}",
                state_path.display()
            );
            return Ok(None);
        }
    };

    let state: ActiveWorkspaceState = match toml::from_str(&contents) {
        Ok(state) => state,
        Err(error) => {
            tracing::warn!(
                "Failed to parse active workspace marker {}: {error}",
                state_path.display()
            );
            return Ok(None);
        }
    };

    let raw_config_dir = state.config_dir.trim();
    if raw_config_dir.is_empty() {
        tracing::warn!(
            "Ignoring active workspace marker {} because config_dir is empty",
            state_path.display()
        );
        return Ok(None);
    }

    let parsed_dir = PathBuf::from(raw_config_dir);
    let config_dir = if parsed_dir.is_absolute() {
        parsed_dir
    } else {
        default_config_dir.join(parsed_dir)
    };
    Ok(Some((config_dir.clone(), config_dir.join("workspace"))))
}

pub(crate) async fn persist_active_workspace_config_dir(config_dir: &Path) -> Result<()> {
    let default_config_dir = default_config_dir()?;
    let state_path = active_workspace_state_path(&default_config_dir);

    // Guard: never persist a temp-directory path as the active workspace.
    // This prevents transient test runs or one-off invocations from hijacking
    // the daemon's config resolution.
    #[cfg(not(test))]
    if is_temp_directory(config_dir) {
        tracing::warn!(
            path = %config_dir.display(),
            "Refusing to persist temp directory as active workspace marker"
        );
        return Ok(());
    }

    if config_dir == default_config_dir {
        if state_path.exists() {
            fs::remove_file(&state_path).await.with_context(|| {
                format!(
                    "Failed to clear active workspace marker: {}",
                    state_path.display()
                )
            })?;
        }
        return Ok(());
    }

    fs::create_dir_all(&default_config_dir)
        .await
        .with_context(|| {
            format!(
                "Failed to create default config directory: {}",
                default_config_dir.display()
            )
        })?;

    let state = ActiveWorkspaceState {
        config_dir: config_dir.to_string_lossy().into_owned(),
    };
    let serialized =
        toml::to_string_pretty(&state).context("Failed to serialize active workspace marker")?;

    let temp_path = default_config_dir.join(format!(
        ".{ACTIVE_WORKSPACE_STATE_FILE}.tmp-{}",
        uuid::Uuid::new_v4()
    ));
    fs::write(&temp_path, serialized).await.with_context(|| {
        format!(
            "Failed to write temporary active workspace marker: {}",
            temp_path.display()
        )
    })?;

    if let Err(error) = fs::rename(&temp_path, &state_path).await {
        let _ = fs::remove_file(&temp_path).await;
        anyhow::bail!(
            "Failed to atomically persist active workspace marker {}: {error}",
            state_path.display()
        );
    }

    sync_directory(&default_config_dir).await?;
    Ok(())
}

pub(crate) fn resolve_config_dir_for_workspace(workspace_dir: &Path) -> (PathBuf, PathBuf) {
    let workspace_config_dir = workspace_dir.to_path_buf();
    if workspace_config_dir.join("config.toml").exists() {
        return (
            workspace_config_dir.clone(),
            workspace_config_dir.join("workspace"),
        );
    }

    let legacy_config_dir = workspace_dir.parent().map(|parent| parent.join(".plaw"));
    if let Some(legacy_dir) = legacy_config_dir {
        if legacy_dir.join("config.toml").exists() {
            return (legacy_dir, workspace_config_dir);
        }

        if workspace_dir
            .file_name()
            .is_some_and(|name| name == std::ffi::OsStr::new("workspace"))
        {
            return (legacy_dir, workspace_config_dir);
        }
    }

    (
        workspace_config_dir.clone(),
        workspace_config_dir.join("workspace"),
    )
}

/// Resolve the current runtime config/workspace directories for onboarding flows.
///
/// This mirrors the same precedence used by `Config::load_or_init()`:
/// `PLAW_CONFIG_DIR` > `PLAW_WORKSPACE` > active workspace marker > defaults.
pub(crate) async fn resolve_runtime_dirs_for_onboarding() -> Result<(PathBuf, PathBuf)> {
    let (default_plaw_dir, default_workspace_dir) = default_config_and_workspace_dirs()?;
    let (config_dir, workspace_dir, _) =
        resolve_runtime_config_dirs(&default_plaw_dir, &default_workspace_dir).await?;
    Ok((config_dir, workspace_dir))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConfigResolutionSource {
    EnvConfigDir,
    EnvWorkspace,
    ActiveWorkspaceMarker,
    DefaultConfigDir,
}

impl ConfigResolutionSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::EnvConfigDir => "PLAW_CONFIG_DIR",
            Self::EnvWorkspace => "PLAW_WORKSPACE",
            Self::ActiveWorkspaceMarker => "active_workspace.toml",
            Self::DefaultConfigDir => "default",
        }
    }
}

async fn resolve_runtime_config_dirs(
    default_plaw_dir: &Path,
    default_workspace_dir: &Path,
) -> Result<(PathBuf, PathBuf, ConfigResolutionSource)> {
    if let Ok(custom_config_dir) = std::env::var("PLAW_CONFIG_DIR") {
        let custom_config_dir = custom_config_dir.trim();
        if !custom_config_dir.is_empty() {
            let plaw_dir = PathBuf::from(custom_config_dir);
            return Ok((
                plaw_dir.clone(),
                plaw_dir.join("workspace"),
                ConfigResolutionSource::EnvConfigDir,
            ));
        }
    }

    if let Ok(custom_workspace) = std::env::var("PLAW_WORKSPACE") {
        if !custom_workspace.is_empty() {
            let (plaw_dir, workspace_dir) =
                resolve_config_dir_for_workspace(&PathBuf::from(custom_workspace));
            return Ok((
                plaw_dir,
                workspace_dir,
                ConfigResolutionSource::EnvWorkspace,
            ));
        }
    }

    if let Some((plaw_dir, workspace_dir)) = load_persisted_workspace_dirs(default_plaw_dir).await?
    {
        return Ok((
            plaw_dir,
            workspace_dir,
            ConfigResolutionSource::ActiveWorkspaceMarker,
        ));
    }

    Ok((
        default_plaw_dir.to_path_buf(),
        default_workspace_dir.to_path_buf(),
        ConfigResolutionSource::DefaultConfigDir,
    ))
}

fn decrypt_optional_secret(
    store: &crate::security::SecretStore,
    value: &mut Option<String>,
    field_name: &str,
) -> Result<()> {
    if let Some(raw) = value.clone() {
        if crate::security::SecretStore::is_encrypted(&raw) {
            *value = Some(
                store
                    .decrypt(&raw)
                    .with_context(|| format!("Failed to decrypt {field_name}"))?,
            );
        }
    }
    Ok(())
}

fn decrypt_secret(
    store: &crate::security::SecretStore,
    value: &mut String,
    field_name: &str,
) -> Result<()> {
    if crate::security::SecretStore::is_encrypted(value) {
        *value = store
            .decrypt(value)
            .with_context(|| format!("Failed to decrypt {field_name}"))?;
    }
    Ok(())
}

fn decrypt_vec_secrets(
    store: &crate::security::SecretStore,
    values: &mut [String],
    field_name: &str,
) -> Result<()> {
    for (idx, value) in values.iter_mut().enumerate() {
        if crate::security::SecretStore::is_encrypted(value) {
            *value = store
                .decrypt(value)
                .with_context(|| format!("Failed to decrypt {field_name}[{idx}]"))?;
        }
    }
    Ok(())
}

fn encrypt_optional_secret(
    store: &crate::security::SecretStore,
    value: &mut Option<String>,
    field_name: &str,
) -> Result<()> {
    if let Some(raw) = value.clone() {
        if !crate::security::SecretStore::is_encrypted(&raw) {
            *value = Some(
                store
                    .encrypt(&raw)
                    .with_context(|| format!("Failed to encrypt {field_name}"))?,
            );
        }
    }
    Ok(())
}

fn encrypt_secret(
    store: &crate::security::SecretStore,
    value: &mut String,
    field_name: &str,
) -> Result<()> {
    if !crate::security::SecretStore::is_encrypted(value) {
        *value = store
            .encrypt(value)
            .with_context(|| format!("Failed to encrypt {field_name}"))?;
    }
    Ok(())
}

fn encrypt_vec_secrets(
    store: &crate::security::SecretStore,
    values: &mut [String],
    field_name: &str,
) -> Result<()> {
    for (idx, value) in values.iter_mut().enumerate() {
        if !crate::security::SecretStore::is_encrypted(value) {
            *value = store
                .encrypt(value)
                .with_context(|| format!("Failed to encrypt {field_name}[{idx}]"))?;
        }
    }
    Ok(())
}

fn decrypt_channel_secrets(
    store: &crate::security::SecretStore,
    channels: &mut ChannelsConfig,
) -> Result<()> {
    // telegram.bot_token migrated to `Secret` newtype (PR #N — wati pattern):
    // no eager decrypt needed; readers call `.reveal(&store)` on demand.
    // discord.bot_token migrated to `Secret` newtype — no eager decrypt.
    // slack.bot_token + slack.app_token migrated to `Secret` newtype.
    // mattermost.bot_token migrated to `Secret` newtype.
    // webhook.secret migrated to Secret newtype (revealed + hashed at gateway startup).
    // matrix.access_token migrated to Secret newtype (lazy reveal at channel construction).
    // WhatsApp {access_token, verify_token, app_secret} migrated to Secret newtype
    // (lazy reveal at channel construction; no eager decrypt needed here).
    // linq.{api_token, signing_secret} migrated to Secret newtype (lazy reveal).
    // nextcloud_talk.{app_token, webhook_secret} migrated to Secret newtype (lazy reveal).
    // irc.{server,nickserv,sasl}_password migrated to Secret newtype (lazy reveal at channel construction).
    // lark.{app_secret, encrypt_key, verification_token} migrated to `Secret` newtype.
    // dingtalk.client_secret migrated to Secret newtype (lazy reveal at channel construction).
    // qq.app_secret migrated to Secret newtype (lazy reveal at channel construction).
    if let Some(ref mut nostr) = channels.nostr {
        decrypt_secret(
            store,
            &mut nostr.private_key,
            "config.channels_config.nostr.private_key",
        )?;
    }
    if let Some(ref mut clawdtalk) = channels.clawdtalk {
        decrypt_secret(
            store,
            &mut clawdtalk.api_key,
            "config.channels_config.clawdtalk.api_key",
        )?;
        decrypt_optional_secret(
            store,
            &mut clawdtalk.webhook_secret,
            "config.channels_config.clawdtalk.webhook_secret",
        )?;
    }
    Ok(())
}

fn encrypt_channel_secrets(
    store: &crate::security::SecretStore,
    channels: &mut ChannelsConfig,
) -> Result<()> {
    // telegram.bot_token migrated to `Secret` — caller constructs via
    // `Secret::new_from_plaintext(...)` (encryption at construction) so
    // this auto-encrypt pass is a no-op for it.
    // discord.bot_token migrated to `Secret` — no auto-encrypt pass.
    // slack.bot_token + slack.app_token migrated to `Secret` newtype.
    // mattermost.bot_token migrated to `Secret` newtype.
    // webhook.secret migrated to Secret newtype (self-managed at-rest).
    // matrix.access_token migrated to Secret newtype (Secret handles its own at-rest representation).
    // WhatsApp {access_token, verify_token, app_secret} migrated to Secret newtype
    // (Secret handles its own at-rest representation; no eager encrypt needed here).
    // linq.{api_token, signing_secret} migrated to Secret newtype (self-managed at-rest).
    // nextcloud_talk.{app_token, webhook_secret} migrated to Secret newtype (self-managed at-rest).
    // irc.{server,nickserv,sasl}_password migrated to Secret newtype (self-managed at-rest).
    // lark.{app_secret, encrypt_key, verification_token} migrated to `Secret` newtype.
    // dingtalk.client_secret migrated to Secret newtype (self-managed at-rest).
    // qq.app_secret migrated to Secret newtype (self-managed at-rest).
    if let Some(ref mut nostr) = channels.nostr {
        encrypt_secret(
            store,
            &mut nostr.private_key,
            "config.channels_config.nostr.private_key",
        )?;
    }
    if let Some(ref mut clawdtalk) = channels.clawdtalk {
        encrypt_secret(
            store,
            &mut clawdtalk.api_key,
            "config.channels_config.clawdtalk.api_key",
        )?;
        encrypt_optional_secret(
            store,
            &mut clawdtalk.webhook_secret,
            "config.channels_config.clawdtalk.webhook_secret",
        )?;
    }
    Ok(())
}

fn config_dir_creation_error(path: &Path) -> String {
    format!(
        "Failed to create config directory: {}. If running as an OpenRC service, \
         ensure this path is writable by user 'plaw'.",
        path.display()
    )
}

fn is_local_ollama_endpoint(api_url: Option<&str>) -> bool {
    let Some(raw) = api_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };

    reqwest::Url::parse(raw)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
        .is_some_and(|host| matches!(host.as_str(), "localhost" | "127.0.0.1" | "::1" | "0.0.0.0"))
}

fn has_ollama_cloud_credential(config_api_key: Option<&str>) -> bool {
    let config_key_present = config_api_key
        .map(str::trim)
        .is_some_and(|value| !value.is_empty());
    if config_key_present {
        return true;
    }

    ["OLLAMA_API_KEY", "PLAW_API_KEY", "API_KEY"]
        .iter()
        .any(|name| {
            std::env::var(name)
                .ok()
                .is_some_and(|value| !value.trim().is_empty())
        })
}

fn normalize_wire_api(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "responses" => Some("responses"),
        "chat_completions" | "chat-completions" | "chat" | "chatcompletions" => {
            Some("chat_completions")
        }
        _ => None,
    }
}

fn read_codex_openai_api_key() -> Option<String> {
    let home = UserDirs::new()?.home_dir().to_path_buf();
    let auth_path = home.join(".codex").join("auth.json");
    let raw = std::fs::read_to_string(auth_path).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;

    parsed
        .get("OPENAI_API_KEY")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Name of the built-in template agent (always injected, cannot be removed by user config).
const BUILTIN_AGENT_NAME: &str = "browser-agent";

impl Config {
    /// Inject built-in template agents that ship with Plaw.
    /// These are always present regardless of user config.
    /// If the user has already defined an agent with the same name, we do NOT overwrite it
    /// (they may have customized the system_prompt or allowed_tools).
    fn inject_builtin_agents(&mut self) {
        use std::collections::hash_map::Entry;
        if let Entry::Vacant(e) = self.agents.entry(BUILTIN_AGENT_NAME.to_string()) {
            e.insert(DelegateAgentConfig {
                provider: String::new(), // inherit from main config
                model: String::new(),    // inherit from main config
                system_prompt: Some(
                    "You are a browser automation agent. Use the browser tool to complete web tasks. \
                     Workflow: 1) open URL, 2) snapshot (interactive_only: true) to see elements, \
                     3) interact via @e refs (click/fill), 4) snapshot again after navigation (refs change), \
                     5) close when done. Return structured results. \
                     If a page requires login or has anti-bot protection, report it clearly."
                        .to_string(),
                ),
                api_key: None, // inherit from main config
                temperature: None,
                max_depth: 3,
                agentic: true,
                allowed_tools: vec![
                    "browser".into(),
                    "file_read".into(),
                    "write_file".into(),
                    "shell".into(),
                    "web_fetch".into(),
                    "memory_store".into(),
                ],
                max_iterations: 20,
            });
            tracing::debug!("Injected built-in agent: {BUILTIN_AGENT_NAME}");
        }
    }

    pub async fn load_or_init() -> Result<Self> {
        let (default_plaw_dir, default_workspace_dir) = default_config_and_workspace_dirs()?;

        let (plaw_dir, workspace_dir, resolution_source) =
            resolve_runtime_config_dirs(&default_plaw_dir, &default_workspace_dir).await?;

        let config_path = plaw_dir.join("config.toml");

        fs::create_dir_all(&plaw_dir)
            .await
            .with_context(|| config_dir_creation_error(&plaw_dir))?;
        fs::create_dir_all(&workspace_dir)
            .await
            .context("Failed to create workspace directory")?;

        if config_path.exists() {
            // Warn if config file is world-readable (may contain API keys)
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(meta) = fs::metadata(&config_path).await {
                    if meta.permissions().mode() & 0o004 != 0 {
                        tracing::warn!(
                            "Config file {:?} is world-readable (mode {:o}). \
                             Consider restricting with: chmod 600 {:?}",
                            config_path,
                            meta.permissions().mode() & 0o777,
                            config_path,
                        );
                    }
                }
            }

            let contents = fs::read_to_string(&config_path)
                .await
                .context("Failed to read config file")?;

            // Track ignored/unknown config keys to warn users about silent misconfigurations
            // (e.g., using [providers.ollama] which doesn't exist instead of top-level api_url)
            let mut ignored_paths: Vec<String> = Vec::new();
            let mut config: Config = serde_ignored::deserialize(
                toml::de::Deserializer::parse(&contents).context("Failed to parse config file")?,
                |path| {
                    ignored_paths.push(path.to_string());
                },
            )
            .context("Failed to deserialize config file")?;

            // Warn about each unknown config key
            for path in ignored_paths {
                tracing::warn!(
                    "Unknown config key ignored: \"{}\". Check config.toml for typos or deprecated options.",
                    path
                );
            }
            // Set computed paths that are skipped during serialization
            config.config_path = config_path.clone();
            config.workspace_dir = workspace_dir;
            let store = crate::security::SecretStore::new(&plaw_dir, config.secrets.encrypt);
            decrypt_optional_secret(&store, &mut config.api_key, "config.api_key")?;
            decrypt_optional_secret(
                &store,
                &mut config.composio.api_key,
                "config.composio.api_key",
            )?;
            decrypt_optional_secret(
                &store,
                &mut config.proxy.http_proxy,
                "config.proxy.http_proxy",
            )?;
            decrypt_optional_secret(
                &store,
                &mut config.proxy.https_proxy,
                "config.proxy.https_proxy",
            )?;
            decrypt_optional_secret(
                &store,
                &mut config.proxy.all_proxy,
                "config.proxy.all_proxy",
            )?;

            decrypt_optional_secret(
                &store,
                &mut config.browser.computer_use.api_key,
                "config.browser.computer_use.api_key",
            )?;

            decrypt_optional_secret(
                &store,
                &mut config.web_search.brave_api_key,
                "config.web_search.brave_api_key",
            )?;

            decrypt_optional_secret(
                &store,
                &mut config.storage.provider.config.db_url,
                "config.storage.provider.config.db_url",
            )?;
            decrypt_vec_secrets(
                &store,
                &mut config.reliability.api_keys,
                "config.reliability.api_keys",
            )?;
            decrypt_vec_secrets(
                &store,
                &mut config.gateway.paired_tokens,
                "config.gateway.paired_tokens",
            )?;

            for agent in config.agents.values_mut() {
                decrypt_optional_secret(&store, &mut agent.api_key, "config.agents.*.api_key")?;
            }

            decrypt_channel_secrets(&store, &mut config.channels_config)?;

            config.inject_builtin_agents();
            config.apply_env_overrides();
            config.validate()?;
            tracing::info!(
                path = %config.config_path.display(),
                workspace = %config.workspace_dir.display(),
                source = resolution_source.as_str(),
                initialized = false,
                "Config loaded"
            );
            Ok(config)
        } else {
            let mut config = Config::default();
            config.config_path = config_path.clone();
            config.workspace_dir = workspace_dir;
            config.inject_builtin_agents();
            config.save().await?;

            // Restrict permissions on newly created config file (may contain API keys)
            #[cfg(unix)]
            {
                use std::{fs::Permissions, os::unix::fs::PermissionsExt};
                let _ = fs::set_permissions(&config_path, Permissions::from_mode(0o600)).await;
            }

            config.apply_env_overrides();
            config.validate()?;
            tracing::info!(
                path = %config.config_path.display(),
                workspace = %config.workspace_dir.display(),
                source = resolution_source.as_str(),
                initialized = true,
                "Config loaded"
            );
            Ok(config)
        }
    }

    fn normalize_reasoning_level_override(raw: Option<&str>, source: &str) -> Option<String> {
        let value = raw?.trim();
        if value.is_empty() {
            return None;
        }
        let normalized = value.to_ascii_lowercase().replace(['-', '_'], "");
        match normalized.as_str() {
            "minimal" | "low" | "medium" | "high" | "xhigh" => Some(normalized),
            _ => {
                tracing::warn!(
                    reasoning_level = %value,
                    source,
                    "Ignoring invalid reasoning level override"
                );
                None
            }
        }
    }

    /// Resolve provider reasoning level with backward-compatible runtime alias.
    ///
    /// Priority:
    /// 1) `provider.reasoning_level` (canonical)
    /// 2) `runtime.reasoning_level` (deprecated compatibility alias)
    pub fn effective_provider_reasoning_level(&self) -> Option<String> {
        let provider_level = Self::normalize_reasoning_level_override(
            self.provider.reasoning_level.as_deref(),
            "provider.reasoning_level",
        );
        let runtime_level = Self::normalize_reasoning_level_override(
            self.runtime.reasoning_level.as_deref(),
            "runtime.reasoning_level",
        );

        match (provider_level, runtime_level) {
            (Some(provider_level), Some(runtime_level)) => {
                if provider_level == runtime_level {
                    tracing::warn!(
                        reasoning_level = %provider_level,
                        "`runtime.reasoning_level` is deprecated; keep only `provider.reasoning_level`"
                    );
                } else {
                    tracing::warn!(
                        provider_reasoning_level = %provider_level,
                        runtime_reasoning_level = %runtime_level,
                        "`runtime.reasoning_level` is deprecated and ignored when `provider.reasoning_level` is set"
                    );
                }
                Some(provider_level)
            }
            (Some(provider_level), None) => Some(provider_level),
            (None, Some(runtime_level)) => {
                tracing::warn!(
                    reasoning_level = %runtime_level,
                    "`runtime.reasoning_level` is deprecated; using it as compatibility fallback to `provider.reasoning_level`"
                );
                Some(runtime_level)
            }
            (None, None) => None,
        }
    }

    fn lookup_model_provider_profile(
        &self,
        provider_name: &str,
    ) -> Option<(String, ModelProviderConfig)> {
        let needle = provider_name.trim();
        if needle.is_empty() {
            return None;
        }

        self.model_providers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(needle))
            .map(|(name, profile)| (name.clone(), profile.clone()))
    }

    fn apply_named_model_provider_profile(&mut self) {
        let Some(current_provider) = self.default_provider.clone() else {
            return;
        };

        let Some((profile_key, profile)) = self.lookup_model_provider_profile(&current_provider)
        else {
            return;
        };

        let base_url = profile
            .base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string);

        if self
            .api_url
            .as_deref()
            .map(str::trim)
            .is_none_or(|value| value.is_empty())
        {
            if let Some(base_url) = base_url.as_ref() {
                self.api_url = Some(base_url.clone());
            }
        }

        if profile.requires_openai_auth
            && self
                .api_key
                .as_deref()
                .map(str::trim)
                .is_none_or(|value| value.is_empty())
        {
            let codex_key = std::env::var("OPENAI_API_KEY")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .or_else(read_codex_openai_api_key);
            if let Some(codex_key) = codex_key {
                self.api_key = Some(codex_key);
            }
        }

        let normalized_wire_api = profile.wire_api.as_deref().and_then(normalize_wire_api);
        let profile_name = profile
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if normalized_wire_api == Some("responses") {
            self.default_provider = Some("openai-codex".to_string());
            return;
        }

        if let Some(profile_name) = profile_name {
            if !profile_name.eq_ignore_ascii_case(&profile_key) {
                self.default_provider = Some(profile_name.to_string());
                return;
            }
        }

        if let Some(base_url) = base_url {
            self.default_provider = Some(format!("custom:{base_url}"));
        }
    }

    /// Validate configuration values that would cause runtime failures.
    ///
    /// Called after TOML deserialization and env-override application to catch
    /// obviously invalid values early instead of failing at arbitrary runtime points.
    pub fn validate(&self) -> Result<()> {
        // Gateway
        if self.gateway.host.trim().is_empty() {
            anyhow::bail!("gateway.host must not be empty");
        }

        // Autonomy
        if self.autonomy.max_actions_per_hour == 0 {
            anyhow::bail!("autonomy.max_actions_per_hour must be greater than 0");
        }
        for (i, env_name) in self.autonomy.shell_env_passthrough.iter().enumerate() {
            if !is_valid_env_var_name(env_name) {
                anyhow::bail!(
                    "autonomy.shell_env_passthrough[{i}] is invalid ({env_name}); expected [A-Za-z_][A-Za-z0-9_]*"
                );
            }
        }
        let mut seen_non_cli_excluded = std::collections::HashSet::new();
        for (i, tool_name) in self.autonomy.non_cli_excluded_tools.iter().enumerate() {
            let normalized = tool_name.trim();
            if normalized.is_empty() {
                anyhow::bail!("autonomy.non_cli_excluded_tools[{i}] must not be empty");
            }
            if !normalized
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                anyhow::bail!(
                    "autonomy.non_cli_excluded_tools[{i}] contains invalid characters: {normalized}"
                );
            }
            if !seen_non_cli_excluded.insert(normalized.to_string()) {
                anyhow::bail!(
                    "autonomy.non_cli_excluded_tools contains duplicate entry: {normalized}"
                );
            }
        }

        // Security OTP / estop
        if self.security.otp.token_ttl_secs == 0 {
            anyhow::bail!("security.otp.token_ttl_secs must be greater than 0");
        }
        if self.security.otp.cache_valid_secs == 0 {
            anyhow::bail!("security.otp.cache_valid_secs must be greater than 0");
        }
        if self.security.otp.cache_valid_secs < self.security.otp.token_ttl_secs {
            anyhow::bail!(
                "security.otp.cache_valid_secs must be greater than or equal to security.otp.token_ttl_secs"
            );
        }
        for (i, action) in self.security.otp.gated_actions.iter().enumerate() {
            let normalized = action.trim();
            if normalized.is_empty() {
                anyhow::bail!("security.otp.gated_actions[{i}] must not be empty");
            }
            if !normalized
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                anyhow::bail!(
                    "security.otp.gated_actions[{i}] contains invalid characters: {normalized}"
                );
            }
        }
        DomainMatcher::new(
            &self.security.otp.gated_domains,
            &self.security.otp.gated_domain_categories,
        )
        .with_context(|| {
            "Invalid security.otp.gated_domains or security.otp.gated_domain_categories"
        })?;
        if self.security.estop.state_file.trim().is_empty() {
            anyhow::bail!("security.estop.state_file must not be empty");
        }
        if self.security.syscall_anomaly.max_denied_events_per_minute == 0 {
            anyhow::bail!(
                "security.syscall_anomaly.max_denied_events_per_minute must be greater than 0"
            );
        }
        if self.security.syscall_anomaly.max_total_events_per_minute == 0 {
            anyhow::bail!(
                "security.syscall_anomaly.max_total_events_per_minute must be greater than 0"
            );
        }
        if self.security.syscall_anomaly.max_denied_events_per_minute
            > self.security.syscall_anomaly.max_total_events_per_minute
        {
            anyhow::bail!(
                "security.syscall_anomaly.max_denied_events_per_minute must be less than or equal to security.syscall_anomaly.max_total_events_per_minute"
            );
        }
        if self.security.syscall_anomaly.max_alerts_per_minute == 0 {
            anyhow::bail!("security.syscall_anomaly.max_alerts_per_minute must be greater than 0");
        }
        if self.security.syscall_anomaly.alert_cooldown_secs == 0 {
            anyhow::bail!("security.syscall_anomaly.alert_cooldown_secs must be greater than 0");
        }
        if self.security.syscall_anomaly.log_path.trim().is_empty() {
            anyhow::bail!("security.syscall_anomaly.log_path must not be empty");
        }
        for (i, syscall_name) in self
            .security
            .syscall_anomaly
            .baseline_syscalls
            .iter()
            .enumerate()
        {
            let normalized = syscall_name.trim();
            if normalized.is_empty() {
                anyhow::bail!("security.syscall_anomaly.baseline_syscalls[{i}] must not be empty");
            }
            if !normalized
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '#')
            {
                anyhow::bail!(
                    "security.syscall_anomaly.baseline_syscalls[{i}] contains invalid characters: {normalized}"
                );
            }
        }

        // Scheduler
        if self.scheduler.max_concurrent == 0 {
            anyhow::bail!("scheduler.max_concurrent must be greater than 0");
        }
        if self.scheduler.max_tasks == 0 {
            anyhow::bail!("scheduler.max_tasks must be greater than 0");
        }

        // Model routes
        for (i, route) in self.model_routes.iter().enumerate() {
            if route.hint.trim().is_empty() {
                anyhow::bail!("model_routes[{i}].hint must not be empty");
            }
            if route.provider.trim().is_empty() {
                anyhow::bail!("model_routes[{i}].provider must not be empty");
            }
            if route.model.trim().is_empty() {
                anyhow::bail!("model_routes[{i}].model must not be empty");
            }
            if route.max_tokens == Some(0) {
                anyhow::bail!("model_routes[{i}].max_tokens must be greater than 0");
            }
        }

        if self.provider_api.is_some()
            && !self
                .default_provider
                .as_deref()
                .is_some_and(|provider| provider.starts_with("custom:"))
        {
            anyhow::bail!(
                "provider_api is only valid when default_provider uses the custom:<url> format"
            );
        }

        // Embedding routes
        for (i, route) in self.embedding_routes.iter().enumerate() {
            if route.hint.trim().is_empty() {
                anyhow::bail!("embedding_routes[{i}].hint must not be empty");
            }
            if route.provider.trim().is_empty() {
                anyhow::bail!("embedding_routes[{i}].provider must not be empty");
            }
            if route.model.trim().is_empty() {
                anyhow::bail!("embedding_routes[{i}].model must not be empty");
            }
        }

        for (profile_key, profile) in &self.model_providers {
            let profile_name = profile_key.trim();
            if profile_name.is_empty() {
                anyhow::bail!("model_providers contains an empty profile name");
            }

            let has_name = profile
                .name
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty());
            let has_base_url = profile
                .base_url
                .as_deref()
                .map(str::trim)
                .is_some_and(|value| !value.is_empty());

            if !has_name && !has_base_url {
                anyhow::bail!(
                    "model_providers.{profile_name} must define at least one of `name` or `base_url`"
                );
            }

            if let Some(base_url) = profile.base_url.as_deref().map(str::trim) {
                if !base_url.is_empty() {
                    let parsed = reqwest::Url::parse(base_url).with_context(|| {
                        format!("model_providers.{profile_name}.base_url is not a valid URL")
                    })?;
                    if !matches!(parsed.scheme(), "http" | "https") {
                        anyhow::bail!(
                            "model_providers.{profile_name}.base_url must use http/https"
                        );
                    }
                }
            }

            if let Some(wire_api) = profile.wire_api.as_deref().map(str::trim) {
                if !wire_api.is_empty() && normalize_wire_api(wire_api).is_none() {
                    anyhow::bail!(
                        "model_providers.{profile_name}.wire_api must be one of: responses, chat_completions"
                    );
                }
            }
        }

        // Ollama cloud-routing safety checks
        if self
            .default_provider
            .as_deref()
            .is_some_and(|provider| provider.trim().eq_ignore_ascii_case("ollama"))
            && self
                .default_model
                .as_deref()
                .is_some_and(|model| model.trim().ends_with(":cloud"))
        {
            if is_local_ollama_endpoint(self.api_url.as_deref()) {
                anyhow::bail!(
                    "default_model uses ':cloud' with provider 'ollama', but api_url is local or unset. Set api_url to a remote Ollama endpoint (for example https://ollama.com)."
                );
            }

            if !has_ollama_cloud_credential(self.api_key.as_deref()) {
                anyhow::bail!(
                    "default_model uses ':cloud' with provider 'ollama', but no API key is configured. Set api_key or OLLAMA_API_KEY."
                );
            }
        }

        // Proxy (delegate to existing validation)
        self.proxy.validate()?;

        // Delegate coordination runtime safety bounds.
        if self.coordination.enabled && self.coordination.lead_agent.trim().is_empty() {
            anyhow::bail!("coordination.lead_agent must not be empty when coordination is enabled");
        }
        if self.coordination.max_inbox_messages_per_agent == 0 {
            anyhow::bail!("coordination.max_inbox_messages_per_agent must be greater than 0");
        }
        if self.coordination.max_dead_letters == 0 {
            anyhow::bail!("coordination.max_dead_letters must be greater than 0");
        }
        if self.coordination.max_context_entries == 0 {
            anyhow::bail!("coordination.max_context_entries must be greater than 0");
        }
        if self.coordination.max_seen_message_ids == 0 {
            anyhow::bail!("coordination.max_seen_message_ids must be greater than 0");
        }

        Ok(())
    }

    /// Apply environment variable overrides to config
    pub fn apply_env_overrides(&mut self) {
        // API Key: PLAW_API_KEY or API_KEY (generic)
        if let Ok(key) = std::env::var("PLAW_API_KEY").or_else(|_| std::env::var("API_KEY")) {
            if !key.is_empty() {
                self.api_key = Some(key);
            }
        }
        // API Key: GLM_API_KEY overrides when provider is a GLM/Zhipu variant.
        if self.default_provider.as_deref().is_some_and(is_glm_alias) {
            if let Ok(key) = std::env::var("GLM_API_KEY") {
                if !key.is_empty() {
                    self.api_key = Some(key);
                }
            }
        }

        // API Key: ZAI_API_KEY overrides when provider is a Z.AI variant.
        if self.default_provider.as_deref().is_some_and(is_zai_alias) {
            if let Ok(key) = std::env::var("ZAI_API_KEY") {
                if !key.is_empty() {
                    self.api_key = Some(key);
                }
            }
        }

        // Provider override precedence:
        // 1) PLAW_PROVIDER always wins when set.
        // 2) PLAW_MODEL_PROVIDER/MODEL_PROVIDER (Codex app-server style).
        // 3) Legacy PROVIDER is honored only when config still uses default provider.
        if let Ok(provider) = std::env::var("PLAW_PROVIDER") {
            if !provider.is_empty() {
                self.default_provider = Some(provider);
            }
        } else if let Ok(provider) =
            std::env::var("PLAW_MODEL_PROVIDER").or_else(|_| std::env::var("MODEL_PROVIDER"))
        {
            if !provider.is_empty() {
                self.default_provider = Some(provider);
            }
        } else if let Ok(provider) = std::env::var("PROVIDER") {
            // Legacy `PROVIDER` only overrides when the config still
            // holds the untouched fallback (a fresh `Config::default()`
            // with no user-set `default_provider`). Compare against the
            // single source of truth so changing `DEFAULT_PROVIDER_FALLBACK`
            // automatically updates this fallback-marker check.
            let should_apply_legacy_provider =
                self.default_provider.as_deref().map_or(true, |configured| {
                    configured
                        .trim()
                        .eq_ignore_ascii_case(DEFAULT_PROVIDER_FALLBACK)
                });
            if should_apply_legacy_provider && !provider.is_empty() {
                self.default_provider = Some(provider);
            }
        }

        // Model: PLAW_MODEL or MODEL
        if let Ok(model) = std::env::var("PLAW_MODEL").or_else(|_| std::env::var("MODEL")) {
            if !model.is_empty() {
                self.default_model = Some(model);
            }
        }

        // Apply named provider profile remapping (Codex app-server compatibility).
        self.apply_named_model_provider_profile();

        // Workspace directory: PLAW_WORKSPACE
        if let Ok(workspace) = std::env::var("PLAW_WORKSPACE") {
            if !workspace.is_empty() {
                let (_, workspace_dir) =
                    resolve_config_dir_for_workspace(&PathBuf::from(workspace));
                self.workspace_dir = workspace_dir;
            }
        }

        // Open-skills opt-in flag: PLAW_OPEN_SKILLS_ENABLED
        if let Ok(flag) = std::env::var("PLAW_OPEN_SKILLS_ENABLED") {
            if !flag.trim().is_empty() {
                match flag.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" | "yes" | "on" => self.skills.open_skills_enabled = true,
                    "0" | "false" | "no" | "off" => self.skills.open_skills_enabled = false,
                    _ => tracing::warn!(
                        "Ignoring invalid PLAW_OPEN_SKILLS_ENABLED (valid: 1|0|true|false|yes|no|on|off)"
                    ),
                }
            }
        }

        // Open-skills directory override: PLAW_OPEN_SKILLS_DIR
        if let Ok(path) = std::env::var("PLAW_OPEN_SKILLS_DIR") {
            let trimmed = path.trim();
            if !trimmed.is_empty() {
                self.skills.open_skills_dir = Some(trimmed.to_string());
            }
        }

        // Skills prompt mode override: PLAW_SKILLS_PROMPT_MODE
        if let Ok(mode) = std::env::var("PLAW_SKILLS_PROMPT_MODE") {
            if !mode.trim().is_empty() {
                if let Some(parsed) = parse_skills_prompt_injection_mode(&mode) {
                    self.skills.prompt_injection_mode = parsed;
                } else {
                    tracing::warn!(
                        "Ignoring invalid PLAW_SKILLS_PROMPT_MODE (valid: full|compact)"
                    );
                }
            }
        }

        // Gateway port: PLAW_GATEWAY_PORT or PORT
        if let Ok(port_str) = std::env::var("PLAW_GATEWAY_PORT").or_else(|_| std::env::var("PORT"))
        {
            if let Ok(port) = port_str.parse::<u16>() {
                self.gateway.port = port;
            }
        }

        // Gateway host: PLAW_GATEWAY_HOST or HOST
        if let Ok(host) = std::env::var("PLAW_GATEWAY_HOST").or_else(|_| std::env::var("HOST")) {
            if !host.is_empty() {
                self.gateway.host = host;
            }
        }

        // Allow public bind: PLAW_ALLOW_PUBLIC_BIND
        if let Ok(val) = std::env::var("PLAW_ALLOW_PUBLIC_BIND") {
            self.gateway.allow_public_bind = val == "1" || val.eq_ignore_ascii_case("true");
        }

        // Temperature: PLAW_TEMPERATURE
        if let Ok(temp_str) = std::env::var("PLAW_TEMPERATURE") {
            if let Ok(temp) = temp_str.parse::<f64>() {
                if (0.0..=2.0).contains(&temp) {
                    self.default_temperature = temp;
                }
            }
        }

        // Reasoning override: PLAW_REASONING_ENABLED or REASONING_ENABLED
        if let Ok(flag) =
            std::env::var("PLAW_REASONING_ENABLED").or_else(|_| std::env::var("REASONING_ENABLED"))
        {
            let normalized = flag.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "1" | "true" | "yes" | "on" => self.runtime.reasoning_enabled = Some(true),
                "0" | "false" | "no" | "off" => self.runtime.reasoning_enabled = Some(false),
                _ => {}
            }
        }

        // Deprecated reasoning level alias: PLAW_REASONING_LEVEL or REASONING_LEVEL
        let alias_level = std::env::var("PLAW_REASONING_LEVEL")
            .ok()
            .map(|value| ("PLAW_REASONING_LEVEL", value))
            .or_else(|| {
                std::env::var("REASONING_LEVEL")
                    .ok()
                    .map(|value| ("REASONING_LEVEL", value))
            });
        if let Some((env_name, level)) = alias_level {
            if let Some(normalized) =
                Self::normalize_reasoning_level_override(Some(&level), env_name)
            {
                tracing::warn!(
                    env_name,
                    reasoning_level = %normalized,
                    "{env_name} is deprecated; prefer provider.reasoning_level in config"
                );
                self.runtime.reasoning_level = Some(normalized);
            }
        }

        // Vision support override: PLAW_MODEL_SUPPORT_VISION or MODEL_SUPPORT_VISION
        if let Ok(flag) = std::env::var("PLAW_MODEL_SUPPORT_VISION")
            .or_else(|_| std::env::var("MODEL_SUPPORT_VISION"))
        {
            let normalized = flag.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "1" | "true" | "yes" | "on" => self.model_support_vision = Some(true),
                "0" | "false" | "no" | "off" => self.model_support_vision = Some(false),
                _ => {}
            }
        }

        // Web search enabled: PLAW_WEB_SEARCH_ENABLED or WEB_SEARCH_ENABLED
        if let Ok(enabled) = std::env::var("PLAW_WEB_SEARCH_ENABLED")
            .or_else(|_| std::env::var("WEB_SEARCH_ENABLED"))
        {
            self.web_search.enabled = enabled == "1" || enabled.eq_ignore_ascii_case("true");
        }

        // Web search provider: PLAW_WEB_SEARCH_PROVIDER or WEB_SEARCH_PROVIDER
        if let Ok(provider) = std::env::var("PLAW_WEB_SEARCH_PROVIDER")
            .or_else(|_| std::env::var("WEB_SEARCH_PROVIDER"))
        {
            let provider = provider.trim();
            if !provider.is_empty() {
                self.web_search.provider = provider.to_string();
            }
        }

        // Brave API key: PLAW_BRAVE_API_KEY or BRAVE_API_KEY
        if let Ok(api_key) =
            std::env::var("PLAW_BRAVE_API_KEY").or_else(|_| std::env::var("BRAVE_API_KEY"))
        {
            let api_key = api_key.trim();
            if !api_key.is_empty() {
                self.web_search.brave_api_key = Some(api_key.to_string());
            }
        }

        // Web search max results: PLAW_WEB_SEARCH_MAX_RESULTS or WEB_SEARCH_MAX_RESULTS
        if let Ok(max_results) = std::env::var("PLAW_WEB_SEARCH_MAX_RESULTS")
            .or_else(|_| std::env::var("WEB_SEARCH_MAX_RESULTS"))
        {
            if let Ok(max_results) = max_results.parse::<usize>() {
                if (1..=10).contains(&max_results) {
                    self.web_search.max_results = max_results;
                }
            }
        }

        // Web search timeout: PLAW_WEB_SEARCH_TIMEOUT_SECS or WEB_SEARCH_TIMEOUT_SECS
        if let Ok(timeout_secs) = std::env::var("PLAW_WEB_SEARCH_TIMEOUT_SECS")
            .or_else(|_| std::env::var("WEB_SEARCH_TIMEOUT_SECS"))
        {
            if let Ok(timeout_secs) = timeout_secs.parse::<u64>() {
                if timeout_secs > 0 {
                    self.web_search.timeout_secs = timeout_secs;
                }
            }
        }

        // Storage provider key (optional backend override): PLAW_STORAGE_PROVIDER
        if let Ok(provider) = std::env::var("PLAW_STORAGE_PROVIDER") {
            let provider = provider.trim();
            if !provider.is_empty() {
                self.storage.provider.config.provider = provider.to_string();
            }
        }

        // Storage connection URL (for remote backends): PLAW_STORAGE_DB_URL
        if let Ok(db_url) = std::env::var("PLAW_STORAGE_DB_URL") {
            let db_url = db_url.trim();
            if !db_url.is_empty() {
                self.storage.provider.config.db_url = Some(db_url.to_string());
            }
        }

        // Storage connect timeout: PLAW_STORAGE_CONNECT_TIMEOUT_SECS
        if let Ok(timeout_secs) = std::env::var("PLAW_STORAGE_CONNECT_TIMEOUT_SECS") {
            if let Ok(timeout_secs) = timeout_secs.parse::<u64>() {
                if timeout_secs > 0 {
                    self.storage.provider.config.connect_timeout_secs = Some(timeout_secs);
                }
            }
        }
        // Proxy enabled flag: PLAW_PROXY_ENABLED
        let explicit_proxy_enabled = std::env::var("PLAW_PROXY_ENABLED")
            .ok()
            .as_deref()
            .and_then(parse_proxy_enabled);
        if let Some(enabled) = explicit_proxy_enabled {
            self.proxy.enabled = enabled;
        }

        // Proxy URLs: PLAW_* wins, then generic *PROXY vars.
        let mut proxy_url_overridden = false;
        if let Ok(proxy_url) =
            std::env::var("PLAW_HTTP_PROXY").or_else(|_| std::env::var("HTTP_PROXY"))
        {
            self.proxy.http_proxy = normalize_proxy_url_option(Some(&proxy_url));
            proxy_url_overridden = true;
        }
        if let Ok(proxy_url) =
            std::env::var("PLAW_HTTPS_PROXY").or_else(|_| std::env::var("HTTPS_PROXY"))
        {
            self.proxy.https_proxy = normalize_proxy_url_option(Some(&proxy_url));
            proxy_url_overridden = true;
        }
        if let Ok(proxy_url) =
            std::env::var("PLAW_ALL_PROXY").or_else(|_| std::env::var("ALL_PROXY"))
        {
            self.proxy.all_proxy = normalize_proxy_url_option(Some(&proxy_url));
            proxy_url_overridden = true;
        }
        if let Ok(no_proxy) = std::env::var("PLAW_NO_PROXY").or_else(|_| std::env::var("NO_PROXY"))
        {
            self.proxy.no_proxy = normalize_no_proxy_list(vec![no_proxy]);
        }

        if explicit_proxy_enabled.is_none()
            && proxy_url_overridden
            && self.proxy.has_any_proxy_url()
        {
            self.proxy.enabled = true;
        }

        // Proxy scope and service selectors.
        if let Ok(scope_raw) = std::env::var("PLAW_PROXY_SCOPE") {
            if let Some(scope) = parse_proxy_scope(&scope_raw) {
                self.proxy.scope = scope;
            } else {
                tracing::warn!(
                    scope = %scope_raw,
                    "Ignoring invalid PLAW_PROXY_SCOPE (valid: environment|plaw|services)"
                );
            }
        }

        if let Ok(services_raw) = std::env::var("PLAW_PROXY_SERVICES") {
            self.proxy.services = normalize_service_list(vec![services_raw]);
        }

        if let Err(error) = self.proxy.validate() {
            tracing::warn!("Invalid proxy configuration ignored: {error}");
            self.proxy.enabled = false;
        }

        if self.proxy.enabled && self.proxy.scope == ProxyScope::Environment {
            self.proxy.apply_to_process_env();
        } else if !self.proxy.enabled {
            // Explicitly clear inherited proxy env vars so no library picks them up.
            clear_proxy_env_pair("HTTP_PROXY");
            clear_proxy_env_pair("HTTPS_PROXY");
            clear_proxy_env_pair("ALL_PROXY");
        }

        set_runtime_proxy_config(self.proxy.clone());
    }

    pub async fn save(&self) -> Result<()> {
        // Encrypt secrets before serialization
        let mut config_to_save = self.clone();
        let plaw_dir = self
            .config_path
            .parent()
            .context("Config path must have a parent directory")?;
        let store = crate::security::SecretStore::new(plaw_dir, self.secrets.encrypt);

        encrypt_optional_secret(&store, &mut config_to_save.api_key, "config.api_key")?;
        encrypt_optional_secret(
            &store,
            &mut config_to_save.composio.api_key,
            "config.composio.api_key",
        )?;
        encrypt_optional_secret(
            &store,
            &mut config_to_save.proxy.http_proxy,
            "config.proxy.http_proxy",
        )?;
        encrypt_optional_secret(
            &store,
            &mut config_to_save.proxy.https_proxy,
            "config.proxy.https_proxy",
        )?;
        encrypt_optional_secret(
            &store,
            &mut config_to_save.proxy.all_proxy,
            "config.proxy.all_proxy",
        )?;

        encrypt_optional_secret(
            &store,
            &mut config_to_save.browser.computer_use.api_key,
            "config.browser.computer_use.api_key",
        )?;

        encrypt_optional_secret(
            &store,
            &mut config_to_save.web_search.brave_api_key,
            "config.web_search.brave_api_key",
        )?;

        encrypt_optional_secret(
            &store,
            &mut config_to_save.storage.provider.config.db_url,
            "config.storage.provider.config.db_url",
        )?;
        encrypt_vec_secrets(
            &store,
            &mut config_to_save.reliability.api_keys,
            "config.reliability.api_keys",
        )?;
        encrypt_vec_secrets(
            &store,
            &mut config_to_save.gateway.paired_tokens,
            "config.gateway.paired_tokens",
        )?;

        for agent in config_to_save.agents.values_mut() {
            encrypt_optional_secret(&store, &mut agent.api_key, "config.agents.*.api_key")?;
        }

        encrypt_channel_secrets(&store, &mut config_to_save.channels_config)?;

        let toml_str =
            toml::to_string_pretty(&config_to_save).context("Failed to serialize config")?;

        let parent_dir = self
            .config_path
            .parent()
            .context("Config path must have a parent directory")?;

        fs::create_dir_all(parent_dir).await.with_context(|| {
            format!(
                "Failed to create config directory: {}",
                parent_dir.display()
            )
        })?;

        let file_name = self
            .config_path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("config.toml");
        let temp_path = parent_dir.join(format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4()));
        let backup_path = parent_dir.join(format!("{file_name}.bak"));

        let mut temp_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .await
            .with_context(|| {
                format!(
                    "Failed to create temporary config file: {}",
                    temp_path.display()
                )
            })?;
        #[cfg(unix)]
        {
            use std::{fs::Permissions, os::unix::fs::PermissionsExt};
            fs::set_permissions(&temp_path, Permissions::from_mode(0o600))
                .await
                .with_context(|| {
                    format!(
                        "Failed to set secure permissions on temporary config file: {}",
                        temp_path.display()
                    )
                })?;
        }
        temp_file
            .write_all(toml_str.as_bytes())
            .await
            .context("Failed to write temporary config contents")?;
        temp_file
            .sync_all()
            .await
            .context("Failed to fsync temporary config file")?;
        drop(temp_file);

        let had_existing_config = self.config_path.exists();
        if had_existing_config {
            fs::copy(&self.config_path, &backup_path)
                .await
                .with_context(|| {
                    format!(
                        "Failed to create config backup before atomic replace: {}",
                        backup_path.display()
                    )
                })?;
        }

        if let Err(e) = fs::rename(&temp_path, &self.config_path).await {
            let _ = fs::remove_file(&temp_path).await;
            if had_existing_config && backup_path.exists() {
                fs::copy(&backup_path, &self.config_path)
                    .await
                    .context("Failed to restore config backup")?;
            }
            anyhow::bail!("Failed to atomically replace config file: {e}");
        }

        #[cfg(unix)]
        {
            use std::{fs::Permissions, os::unix::fs::PermissionsExt};
            fs::set_permissions(&self.config_path, Permissions::from_mode(0o600))
                .await
                .with_context(|| {
                    format!(
                        "Failed to enforce secure permissions on config file: {}",
                        self.config_path.display()
                    )
                })?;
        }

        sync_directory(parent_dir).await?;

        if had_existing_config {
            let _ = fs::remove_file(&backup_path).await;
        }

        Ok(())
    }
}

async fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        let dir = File::open(path)
            .await
            .with_context(|| format!("Failed to open directory for fsync: {}", path.display()))?;
        dir.sync_all()
            .await
            .with_context(|| format!("Failed to fsync directory metadata: {}", path.display()))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use tempfile::TempDir;
    use tokio::sync::{Mutex, MutexGuard};
    use tokio::test;
    use tokio_stream::wrappers::ReadDirStream;
    use tokio_stream::StreamExt;

    // ── Defaults ─────────────────────────────────────────────

    #[test]
    async fn http_request_config_default_has_correct_values() {
        let cfg = HttpRequestConfig::default();
        assert_eq!(cfg.timeout_secs, 30);
        assert_eq!(cfg.max_response_size, 1_000_000);
        assert!(!cfg.enabled);
        assert!(cfg.allowed_domains.is_empty());
    }

    #[test]
    async fn config_default_has_sane_values() {
        let c = Config::default();
        assert_eq!(
            c.default_provider.as_deref(),
            Some(DEFAULT_PROVIDER_FALLBACK)
        );
        assert_eq!(c.default_model.as_deref(), Some(DEFAULT_MODEL_FALLBACK));
        assert!((c.default_temperature - 0.7).abs() < f64::EPSILON);
        assert!(c.api_key.is_none());
        assert!(!c.skills.open_skills_enabled);
        assert_eq!(
            c.skills.prompt_injection_mode,
            SkillsPromptInjectionMode::Full
        );
        assert!(c.workspace_dir.to_string_lossy().contains("workspace"));
        assert!(c.config_path.to_string_lossy().contains("config.toml"));
    }

    #[test]
    async fn config_debug_redacts_sensitive_values() {
        let mut config = Config::default();
        config.workspace_dir = PathBuf::from("/tmp/workspace");
        config.config_path = PathBuf::from("/tmp/config.toml");
        config.api_key = Some("root-credential".into());
        config.storage.provider.config.db_url = Some("postgres://user:pw@host/db".into());
        config.browser.computer_use.api_key = Some("browser-credential".into());
        config.gateway.paired_tokens = vec!["zc_0123456789abcdef".into()];
        config.channels_config.telegram = Some(TelegramConfig {
            bot_token: crate::security::Secret::from_wire("telegram-credential".into()),
            allowed_users: Vec::new(),
            stream_mode: StreamMode::Off,
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
            group_reply: None,
            base_url: None,
        });
        config.agents.insert(
            "worker".into(),
            DelegateAgentConfig {
                provider: "openrouter".into(),
                model: "model-test".into(),
                system_prompt: None,
                api_key: Some("agent-credential".into()),
                temperature: None,
                max_depth: 3,
                agentic: false,
                allowed_tools: Vec::new(),
                max_iterations: 10,
            },
        );

        let debug_output = format!("{config:?}");
        assert!(debug_output.contains("***REDACTED***"));

        for (idx, secret) in [
            "root-credential",
            "postgres://user:pw@host/db",
            "browser-credential",
            "zc_0123456789abcdef",
            "telegram-credential",
            "agent-credential",
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                !debug_output.contains(secret),
                "debug output leaked secret value at index {idx}"
            );
        }

        assert!(!debug_output.contains("paired_tokens"));
        assert!(!debug_output.contains("bot_token"));
        assert!(!debug_output.contains("db_url"));
    }

    #[test]
    async fn config_dir_creation_error_mentions_openrc_and_path() {
        let msg = config_dir_creation_error(Path::new("/etc/plaw"));
        assert!(msg.contains("/etc/plaw"));
        assert!(msg.contains("OpenRC"));
        assert!(msg.contains("plaw"));
    }

    #[test]
    async fn config_schema_export_contains_expected_contract_shape() {
        let schema = schemars::schema_for!(Config);
        let schema_json = serde_json::to_value(&schema).expect("schema should serialize to json");

        assert_eq!(
            schema_json
                .get("$schema")
                .and_then(serde_json::Value::as_str),
            Some("https://json-schema.org/draft/2020-12/schema")
        );

        let properties = schema_json
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("schema should expose top-level properties");

        assert!(properties.contains_key("default_provider"));
        assert!(properties.contains_key("skills"));
        assert!(properties.contains_key("gateway"));
        assert!(properties.contains_key("channels_config"));
        assert!(!properties.contains_key("workspace_dir"));
        assert!(!properties.contains_key("config_path"));

        assert!(
            schema_json
                .get("$defs")
                .and_then(serde_json::Value::as_object)
                .is_some(),
            "schema should include reusable type definitions"
        );
    }

    #[cfg(unix)]
    #[test]
    async fn save_sets_config_permissions_on_new_file() {
        let temp = TempDir::new().expect("temp dir");
        let config_path = temp.path().join("config.toml");
        let workspace_dir = temp.path().join("workspace");

        let mut config = Config::default();
        config.config_path = config_path.clone();
        config.workspace_dir = workspace_dir;

        config.save().await.expect("save config");

        let mode = std::fs::metadata(&config_path)
            .expect("config metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    async fn observability_config_default() {
        let o = ObservabilityConfig::default();
        assert_eq!(o.backend, "none");
        assert_eq!(o.runtime_trace_mode, "none");
        assert_eq!(o.runtime_trace_path, "state/runtime-trace.jsonl");
        assert_eq!(o.runtime_trace_max_entries, 200);
    }

    #[test]
    async fn autonomy_config_default() {
        let a = AutonomyConfig::default();
        assert_eq!(a.level, AutonomyLevel::Supervised);
        assert!(a.workspace_only);
        assert!(a.allowed_commands.contains(&"git".to_string()));
        assert!(a.allowed_commands.contains(&"cargo".to_string()));
        assert!(a.forbidden_paths.contains(&"/etc".to_string()));
        assert_eq!(a.max_actions_per_hour, 20);
        assert_eq!(a.max_cost_per_day_cents, 500);
        assert!(a.require_approval_for_medium_risk);
        assert!(a.block_high_risk_commands);
        assert!(a.shell_env_passthrough.is_empty());
        assert!(a.non_cli_excluded_tools.contains(&"shell".to_string()));
        assert!(a.non_cli_excluded_tools.contains(&"delegate".to_string()));
    }

    #[test]
    async fn autonomy_config_serde_defaults_non_cli_excluded_tools() {
        let raw = r#"
level = "supervised"
workspace_only = true
allowed_commands = ["git"]
forbidden_paths = ["/etc"]
max_actions_per_hour = 20
max_cost_per_day_cents = 500
require_approval_for_medium_risk = true
block_high_risk_commands = true
shell_env_passthrough = []
auto_approve = ["file_read"]
always_ask = []
allowed_roots = []
"#;
        let parsed: AutonomyConfig = toml::from_str(raw).unwrap();
        assert!(parsed.non_cli_excluded_tools.contains(&"shell".to_string()));
        assert!(parsed
            .non_cli_excluded_tools
            .contains(&"browser".to_string()));
    }

    #[test]
    async fn config_validate_rejects_duplicate_non_cli_excluded_tools() {
        let mut cfg = Config::default();
        cfg.autonomy.non_cli_excluded_tools = vec!["shell".into(), "shell".into()];
        let err = cfg.validate().unwrap_err();
        assert!(err
            .to_string()
            .contains("autonomy.non_cli_excluded_tools contains duplicate entry"));
    }

    #[test]
    async fn runtime_config_default() {
        let r = RuntimeConfig::default();
        assert_eq!(r.kind, "native");
        assert_eq!(r.docker.image, "alpine:3.20");
        assert_eq!(r.docker.network, "none");
        assert_eq!(r.docker.memory_limit_mb, Some(512));
        assert_eq!(r.docker.cpu_limit, Some(1.0));
        assert!(r.docker.read_only_rootfs);
        assert!(r.docker.mount_workspace);
        assert_eq!(r.wasm.tools_dir, "tools/wasm");
        assert_eq!(r.wasm.fuel_limit, 1_000_000);
        assert_eq!(r.wasm.memory_limit_mb, 64);
        assert_eq!(r.wasm.max_module_size_mb, 50);
        assert!(!r.wasm.allow_workspace_read);
        assert!(!r.wasm.allow_workspace_write);
        assert!(r.wasm.allowed_hosts.is_empty());
        assert!(r.wasm.security.require_workspace_relative_tools_dir);
        assert!(r.wasm.security.reject_symlink_modules);
        assert!(r.wasm.security.reject_symlink_tools_dir);
        assert!(r.wasm.security.strict_host_validation);
        assert_eq!(
            r.wasm.security.capability_escalation_mode,
            WasmCapabilityEscalationMode::Deny
        );
        assert_eq!(
            r.wasm.security.module_hash_policy,
            WasmModuleHashPolicy::Warn
        );
        assert!(r.wasm.security.module_sha256.is_empty());
    }

    #[test]
    async fn heartbeat_config_default() {
        let h = HeartbeatConfig::default();
        assert!(!h.enabled);
        assert_eq!(h.interval_minutes, 30);
        assert!(h.message.is_none());
        assert!(h.target.is_none());
        assert!(h.to.is_none());
    }

    #[test]
    async fn heartbeat_config_parses_delivery_aliases() {
        let raw = r#"
enabled = true
interval_minutes = 10
message = "Ping"
channel = "telegram"
recipient = "42"
"#;
        let parsed: HeartbeatConfig = toml::from_str(raw).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.interval_minutes, 10);
        assert_eq!(parsed.message.as_deref(), Some("Ping"));
        assert_eq!(parsed.target.as_deref(), Some("telegram"));
        assert_eq!(parsed.to.as_deref(), Some("42"));
    }

    #[test]
    async fn cron_config_default() {
        let c = CronConfig::default();
        assert!(c.enabled);
        assert_eq!(c.max_run_history, 50);
    }

    #[test]
    async fn cron_config_serde_roundtrip() {
        let c = CronConfig {
            enabled: false,
            max_run_history: 100,
        };
        let json = serde_json::to_string(&c).unwrap();
        let parsed: CronConfig = serde_json::from_str(&json).unwrap();
        assert!(!parsed.enabled);
        assert_eq!(parsed.max_run_history, 100);
    }

    #[test]
    async fn config_defaults_cron_when_section_missing() {
        let toml_str = r#"
workspace_dir = "/tmp/workspace"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;

        let parsed: Config = toml::from_str(toml_str).unwrap();
        assert!(parsed.cron.enabled);
        assert_eq!(parsed.cron.max_run_history, 50);
    }

    #[test]
    async fn memory_config_default_hygiene_settings() {
        let m = MemoryConfig::default();
        assert_eq!(m.backend, "sqlite");
        assert!(m.auto_save);
        assert!(m.hygiene_enabled);
        assert_eq!(m.archive_after_days, 7);
        assert_eq!(m.purge_after_days, 30);
        assert_eq!(m.conversation_retention_days, 30);
        assert!(m.sqlite_open_timeout_secs.is_none());
    }

    #[test]
    async fn storage_provider_config_defaults() {
        let storage = StorageConfig::default();
        assert!(storage.provider.config.provider.is_empty());
        assert!(storage.provider.config.db_url.is_none());
        assert_eq!(storage.provider.config.schema, "public");
        assert_eq!(storage.provider.config.table, "memories");
        assert!(storage.provider.config.connect_timeout_secs.is_none());
    }

    #[test]
    async fn channels_config_default() {
        let c = ChannelsConfig::default();
        assert!(c.cli);
        assert!(c.telegram.is_none());
        assert!(c.discord.is_none());
    }

    // ── Serde round-trip ─────────────────────────────────────

    #[test]
    async fn config_toml_roundtrip() {
        let config = Config {
            workspace_dir: PathBuf::from("/tmp/test/workspace"),
            config_path: PathBuf::from("/tmp/test/config.toml"),
            api_key: Some("sk-test-key".into()),
            api_url: None,
            default_provider: Some("openrouter".into()),
            provider_api: None,
            default_model: Some("gpt-4o".into()),
            model_providers: HashMap::new(),
            provider: ProviderConfig::default(),
            default_temperature: 0.5,
            observability: ObservabilityConfig {
                backend: "log".into(),
                ..ObservabilityConfig::default()
            },
            autonomy: AutonomyConfig {
                level: AutonomyLevel::Full,
                workspace_only: false,
                allowed_commands: vec!["docker".into()],
                forbidden_paths: vec!["/secret".into()],
                max_actions_per_hour: 50,
                max_cost_per_day_cents: 1000,
                require_approval_for_medium_risk: false,
                block_high_risk_commands: true,
                shell_env_passthrough: vec!["DATABASE_URL".into()],
                auto_approve: vec!["file_read".into()],
                always_ask: vec![],
                allowed_roots: vec![],
                non_cli_excluded_tools: vec![],
                non_cli_approval_approvers: vec![],
                non_cli_natural_language_approval_mode:
                    NonCliNaturalLanguageApprovalMode::RequestConfirm,
                non_cli_natural_language_approval_mode_by_channel: HashMap::new(),
            },
            security: SecurityConfig::default(),
            runtime: RuntimeConfig {
                kind: "docker".into(),
                ..RuntimeConfig::default()
            },
            research: ResearchPhaseConfig::default(),
            reliability: ReliabilityConfig::default(),
            scheduler: SchedulerConfig::default(),
            coordination: CoordinationConfig::default(),
            skills: SkillsConfig::default(),
            model_routes: Vec::new(),
            embedding_routes: Vec::new(),
            query_classification: QueryClassificationConfig::default(),
            heartbeat: HeartbeatConfig {
                enabled: true,
                interval_minutes: 15,
                message: Some("Check London time".into()),
                target: Some("telegram".into()),
                to: Some("123456".into()),
            },
            cron: CronConfig::default(),
            goal_loop: GoalLoopConfig::default(),
            channels_config: ChannelsConfig {
                cli: true,
                telegram: Some(TelegramConfig {
                    bot_token: crate::security::Secret::from_wire("123:ABC".into()),
                    allowed_users: vec!["user1".into()],
                    stream_mode: StreamMode::default(),
                    draft_update_interval_ms: default_draft_update_interval_ms(),
                    interrupt_on_new_message: false,
                    mention_only: false,
                    group_reply: None,
                    base_url: None,
                }),
                discord: None,
                slack: None,
                mattermost: None,
                webhook: None,
                imessage: None,
                matrix: None,
                signal: None,
                whatsapp: None,
                linq: None,
                wati: None,
                nextcloud_talk: None,
                email: None,
                irc: None,
                lark: None,
                feishu: None,
                dingtalk: None,
                qq: None,
                nostr: None,
                clawdtalk: None,
                message_timeout_secs: 300,
            },
            memory: MemoryConfig::default(),
            storage: StorageConfig::default(),
            tunnel: TunnelConfig::default(),
            gateway: GatewayConfig::default(),
            composio: ComposioConfig::default(),
            secrets: SecretsConfig::default(),
            browser: BrowserConfig::default(),
            http_request: HttpRequestConfig::default(),
            multimodal: MultimodalConfig::default(),
            web_fetch: WebFetchConfig::default(),
            web_search: WebSearchConfig::default(),
            proxy: ProxyConfig::default(),
            agent: AgentConfig::default(),
            identity: IdentityConfig::default(),
            cost: CostConfig::default(),
            peripherals: PeripheralsConfig::default(),
            agents: HashMap::new(),
            pipelines: HashMap::new(),
            mcp: McpConfig::default(),
            hooks: HooksConfig::default(),
            hardware: HardwareConfig::default(),
            transcription: TranscriptionConfig::default(),
            agents_ipc: AgentsIpcConfig::default(),
            repo_map: RepoMapConfig::default(),
            edit_linter: EditLinterConfig::default(),
            chain_of_verification: ChainOfVerificationConfig::default(),
            model_support_vision: None,
        };

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();

        assert_eq!(parsed.api_key, config.api_key);
        assert_eq!(parsed.default_provider, config.default_provider);
        assert_eq!(parsed.default_model, config.default_model);
        assert!((parsed.default_temperature - config.default_temperature).abs() < f64::EPSILON);
        assert_eq!(parsed.observability.backend, "log");
        assert_eq!(parsed.observability.runtime_trace_mode, "none");
        assert_eq!(parsed.autonomy.level, AutonomyLevel::Full);
        assert!(!parsed.autonomy.workspace_only);
        assert_eq!(parsed.runtime.kind, "docker");
        assert!(parsed.heartbeat.enabled);
        assert_eq!(parsed.heartbeat.interval_minutes, 15);
        assert_eq!(
            parsed.heartbeat.message.as_deref(),
            Some("Check London time")
        );
        assert_eq!(parsed.heartbeat.target.as_deref(), Some("telegram"));
        assert_eq!(parsed.heartbeat.to.as_deref(), Some("123456"));
        assert!(parsed.channels_config.telegram.is_some());
        assert_eq!(
            parsed
                .channels_config
                .telegram
                .unwrap()
                .bot_token
                .as_wire_str(),
            "123:ABC"
        );
    }

    #[test]
    async fn config_minimal_toml_uses_defaults() {
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(parsed.api_key.is_none());
        assert!(parsed.default_provider.is_none());
        assert_eq!(parsed.observability.backend, "none");
        assert_eq!(parsed.observability.runtime_trace_mode, "none");
        assert_eq!(parsed.autonomy.level, AutonomyLevel::Supervised);
        assert_eq!(parsed.runtime.kind, "native");
        assert!(!parsed.heartbeat.enabled);
        assert!(parsed.channels_config.cli);
        assert!(parsed.memory.hygiene_enabled);
        assert_eq!(parsed.memory.archive_after_days, 7);
        assert_eq!(parsed.memory.purge_after_days, 30);
        assert_eq!(parsed.memory.conversation_retention_days, 30);
    }

    #[test]
    async fn storage_provider_dburl_alias_deserializes() {
        let raw = r#"
default_temperature = 0.7

[storage.provider.config]
provider = "postgres"
dbURL = "postgres://postgres:postgres@localhost:5432/plaw"
schema = "public"
table = "memories"
connect_timeout_secs = 12
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.storage.provider.config.provider, "postgres");
        assert_eq!(
            parsed.storage.provider.config.db_url.as_deref(),
            Some("postgres://postgres:postgres@localhost:5432/plaw")
        );
        assert_eq!(parsed.storage.provider.config.schema, "public");
        assert_eq!(parsed.storage.provider.config.table, "memories");
        assert_eq!(
            parsed.storage.provider.config.connect_timeout_secs,
            Some(12)
        );
    }

    #[test]
    async fn runtime_reasoning_enabled_deserializes() {
        let raw = r#"
default_temperature = 0.7

[runtime]
reasoning_enabled = false
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.runtime.reasoning_enabled, Some(false));
    }

    #[test]
    async fn runtime_wasm_deserializes() {
        let raw = r#"
default_temperature = 0.7

[runtime]
kind = "wasm"

[runtime.wasm]
tools_dir = "skills/wasm"
fuel_limit = 500000
memory_limit_mb = 32
max_module_size_mb = 8
allow_workspace_read = true
allow_workspace_write = false
allowed_hosts = ["api.example.com", "cdn.example.com:443"]

[runtime.wasm.security]
require_workspace_relative_tools_dir = false
reject_symlink_modules = false
reject_symlink_tools_dir = false
strict_host_validation = false
capability_escalation_mode = "clamp"
module_hash_policy = "enforce"

[runtime.wasm.security.module_sha256]
calc = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.runtime.kind, "wasm");
        assert_eq!(parsed.runtime.wasm.tools_dir, "skills/wasm");
        assert_eq!(parsed.runtime.wasm.fuel_limit, 500_000);
        assert_eq!(parsed.runtime.wasm.memory_limit_mb, 32);
        assert_eq!(parsed.runtime.wasm.max_module_size_mb, 8);
        assert!(parsed.runtime.wasm.allow_workspace_read);
        assert!(!parsed.runtime.wasm.allow_workspace_write);
        assert_eq!(
            parsed.runtime.wasm.allowed_hosts,
            vec!["api.example.com", "cdn.example.com:443"]
        );
        assert!(
            !parsed
                .runtime
                .wasm
                .security
                .require_workspace_relative_tools_dir
        );
        assert!(!parsed.runtime.wasm.security.reject_symlink_modules);
        assert!(!parsed.runtime.wasm.security.reject_symlink_tools_dir);
        assert!(!parsed.runtime.wasm.security.strict_host_validation);
        assert_eq!(
            parsed.runtime.wasm.security.capability_escalation_mode,
            WasmCapabilityEscalationMode::Clamp
        );
        assert_eq!(
            parsed.runtime.wasm.security.module_hash_policy,
            WasmModuleHashPolicy::Enforce
        );
        assert_eq!(
            parsed.runtime.wasm.security.module_sha256.get("calc"),
            Some(&"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string())
        );
    }

    #[test]
    async fn runtime_wasm_dev_template_deserializes() {
        let raw = include_str!("../../../dev/config.wasm.dev.toml");
        let parsed: Config = toml::from_str(raw).expect("dev wasm template should parse");

        assert_eq!(parsed.runtime.kind, "wasm");
        assert!(parsed.runtime.wasm.allow_workspace_read);
        assert!(parsed.runtime.wasm.allow_workspace_write);
        assert_eq!(
            parsed.runtime.wasm.security.capability_escalation_mode,
            WasmCapabilityEscalationMode::Clamp
        );
    }

    #[test]
    async fn runtime_wasm_staging_template_deserializes() {
        let raw = include_str!("../../../dev/config.wasm.staging.toml");
        let parsed: Config = toml::from_str(raw).expect("staging wasm template should parse");

        assert_eq!(parsed.runtime.kind, "wasm");
        assert!(parsed.runtime.wasm.allow_workspace_read);
        assert!(!parsed.runtime.wasm.allow_workspace_write);
        assert_eq!(
            parsed.runtime.wasm.security.capability_escalation_mode,
            WasmCapabilityEscalationMode::Deny
        );
    }

    #[test]
    async fn runtime_wasm_prod_template_deserializes() {
        let raw = include_str!("../../../dev/config.wasm.prod.toml");
        let parsed: Config = toml::from_str(raw).expect("prod wasm template should parse");

        assert_eq!(parsed.runtime.kind, "wasm");
        assert!(!parsed.runtime.wasm.allow_workspace_read);
        assert!(!parsed.runtime.wasm.allow_workspace_write);
        assert!(parsed.runtime.wasm.allowed_hosts.is_empty());
        assert_eq!(
            parsed.runtime.wasm.security.capability_escalation_mode,
            WasmCapabilityEscalationMode::Deny
        );
    }

    #[test]
    async fn model_support_vision_deserializes() {
        let raw = r#"
default_temperature = 0.7
model_support_vision = true
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.model_support_vision, Some(true));

        // Default (omitted) should be None
        let raw_no_vision = r#"
default_temperature = 0.7
"#;
        let parsed2: Config = toml::from_str(raw_no_vision).unwrap();
        assert_eq!(parsed2.model_support_vision, None);
    }

    #[test]
    async fn provider_reasoning_level_deserializes() {
        let raw = r#"
default_temperature = 0.7

[provider]
reasoning_level = "high"
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.provider.reasoning_level.as_deref(), Some("high"));
        assert_eq!(
            parsed.effective_provider_reasoning_level().as_deref(),
            Some("high")
        );
    }

    #[test]
    async fn runtime_reasoning_level_alias_deserializes() {
        let raw = r#"
default_temperature = 0.7

[runtime]
reasoning_level = "xhigh"
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(parsed.runtime.reasoning_level.as_deref(), Some("xhigh"));
        assert_eq!(
            parsed.effective_provider_reasoning_level().as_deref(),
            Some("xhigh")
        );
    }

    #[test]
    async fn provider_reasoning_level_wins_over_runtime_alias() {
        let raw = r#"
default_temperature = 0.7

[provider]
reasoning_level = "medium"

[runtime]
reasoning_level = "high"
"#;

        let parsed: Config = toml::from_str(raw).unwrap();
        assert_eq!(
            parsed.effective_provider_reasoning_level().as_deref(),
            Some("medium")
        );
    }

    #[test]
    async fn agent_config_defaults() {
        let cfg = AgentConfig::default();
        assert!(!cfg.compact_context);
        // Originally 20; bumped to "effectively unlimited" (i64::MAX, not
        // usize::MAX, so it round-trips through TOML).
        assert_eq!(cfg.max_tool_iterations, i64::MAX as usize);
        assert_eq!(cfg.max_history_messages, 50);
        assert!(!cfg.parallel_tools);
        assert_eq!(cfg.tool_dispatcher, "auto");
        // Phase 3 L1-6: opt-in. Default off until eval validation completes.
        assert!(!cfg.intent_routing_enabled);
    }

    #[test]
    async fn agent_config_deserializes() {
        let raw = r#"
default_temperature = 0.7
[agent]
compact_context = true
max_tool_iterations = 20
max_history_messages = 80
parallel_tools = true
tool_dispatcher = "xml"
"#;
        let parsed: Config = toml::from_str(raw).unwrap();
        assert!(parsed.agent.compact_context);
        assert_eq!(parsed.agent.max_tool_iterations, 20);
        assert_eq!(parsed.agent.max_history_messages, 80);
        assert!(parsed.agent.parallel_tools);
        assert_eq!(parsed.agent.tool_dispatcher, "xml");
    }

    #[tokio::test]
    async fn sync_directory_handles_existing_directory() {
        let dir =
            std::env::temp_dir().join(format!("plaw_test_sync_directory_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).await.unwrap();

        sync_directory(&dir).await.unwrap();

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn config_save_and_load_tmpdir() {
        let dir = std::env::temp_dir().join("plaw_test_config");
        let _ = fs::remove_dir_all(&dir).await;
        fs::create_dir_all(&dir).await.unwrap();

        let config_path = dir.join("config.toml");
        let config = Config {
            workspace_dir: dir.join("workspace"),
            config_path: config_path.clone(),
            api_key: Some("sk-roundtrip".into()),
            api_url: None,
            default_provider: Some("openrouter".into()),
            provider_api: None,
            default_model: Some("test-model".into()),
            model_providers: HashMap::new(),
            provider: ProviderConfig::default(),
            default_temperature: 0.9,
            observability: ObservabilityConfig::default(),
            autonomy: AutonomyConfig::default(),
            security: SecurityConfig::default(),
            runtime: RuntimeConfig::default(),
            research: ResearchPhaseConfig::default(),
            reliability: ReliabilityConfig::default(),
            scheduler: SchedulerConfig::default(),
            coordination: CoordinationConfig::default(),
            skills: SkillsConfig::default(),
            model_routes: Vec::new(),
            embedding_routes: Vec::new(),
            query_classification: QueryClassificationConfig::default(),
            heartbeat: HeartbeatConfig::default(),
            cron: CronConfig::default(),
            goal_loop: GoalLoopConfig::default(),
            channels_config: ChannelsConfig::default(),
            memory: MemoryConfig::default(),
            storage: StorageConfig::default(),
            tunnel: TunnelConfig::default(),
            gateway: GatewayConfig::default(),
            composio: ComposioConfig::default(),
            secrets: SecretsConfig::default(),
            browser: BrowserConfig::default(),
            http_request: HttpRequestConfig::default(),
            multimodal: MultimodalConfig::default(),
            web_fetch: WebFetchConfig::default(),
            web_search: WebSearchConfig::default(),
            proxy: ProxyConfig::default(),
            agent: AgentConfig::default(),
            identity: IdentityConfig::default(),
            cost: CostConfig::default(),
            peripherals: PeripheralsConfig::default(),
            agents: HashMap::new(),
            pipelines: HashMap::new(),
            mcp: McpConfig::default(),
            hooks: HooksConfig::default(),
            hardware: HardwareConfig::default(),
            transcription: TranscriptionConfig::default(),
            agents_ipc: AgentsIpcConfig::default(),
            repo_map: RepoMapConfig::default(),
            edit_linter: EditLinterConfig::default(),
            chain_of_verification: ChainOfVerificationConfig::default(),
            model_support_vision: None,
        };

        config.save().await.unwrap();
        assert!(config_path.exists());

        let contents = tokio::fs::read_to_string(&config_path).await.unwrap();
        let loaded: Config = toml::from_str(&contents).unwrap();
        assert!(loaded
            .api_key
            .as_deref()
            .is_some_and(crate::security::SecretStore::is_encrypted));
        let store = crate::security::SecretStore::new(&dir, true);
        let decrypted = store.decrypt(loaded.api_key.as_deref().unwrap()).unwrap();
        assert_eq!(decrypted, "sk-roundtrip");
        assert_eq!(loaded.default_model.as_deref(), Some("test-model"));
        assert!((loaded.default_temperature - 0.9).abs() < f64::EPSILON);

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn config_save_encrypts_nested_credentials() {
        let dir = std::env::temp_dir().join(format!(
            "plaw_test_nested_credentials_{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).await.unwrap();

        let mut config = Config::default();
        config.workspace_dir = dir.join("workspace");
        config.config_path = dir.join("config.toml");
        config.api_key = Some("root-credential".into());
        config.composio.api_key = Some("composio-credential".into());
        config.proxy.http_proxy = Some("http://user:pass@proxy.internal:8080".into());
        config.proxy.https_proxy = Some("https://user:pass@proxy.internal:8443".into());
        config.proxy.all_proxy = Some("socks5://user:pass@proxy.internal:1080".into());
        config.browser.computer_use.api_key = Some("browser-credential".into());
        config.web_search.brave_api_key = Some("brave-credential".into());
        config.storage.provider.config.db_url = Some("postgres://user:pw@host/db".into());
        config.reliability.api_keys = vec!["backup-credential".into()];
        config.gateway.paired_tokens = vec!["zc_0123456789abcdef".into()];
        config.channels_config.telegram = Some(TelegramConfig {
            bot_token: crate::security::Secret::from_wire("telegram-credential".into()),
            allowed_users: Vec::new(),
            stream_mode: StreamMode::Off,
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
            group_reply: None,
            base_url: None,
        });

        config.agents.insert(
            "worker".into(),
            DelegateAgentConfig {
                provider: "openrouter".into(),
                model: "model-test".into(),
                system_prompt: None,
                api_key: Some("agent-credential".into()),
                temperature: None,
                max_depth: 3,
                agentic: false,
                allowed_tools: Vec::new(),
                max_iterations: 10,
            },
        );

        config.save().await.unwrap();

        let contents = tokio::fs::read_to_string(config.config_path.clone())
            .await
            .unwrap();
        let stored: Config = toml::from_str(&contents).unwrap();
        let store = crate::security::SecretStore::new(&dir, true);

        let root_encrypted = stored.api_key.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(root_encrypted));
        assert_eq!(store.decrypt(root_encrypted).unwrap(), "root-credential");

        let composio_encrypted = stored.composio.api_key.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(
            composio_encrypted
        ));
        assert_eq!(
            store.decrypt(composio_encrypted).unwrap(),
            "composio-credential"
        );

        let proxy_http_encrypted = stored.proxy.http_proxy.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(
            proxy_http_encrypted
        ));
        assert_eq!(
            store.decrypt(proxy_http_encrypted).unwrap(),
            "http://user:pass@proxy.internal:8080"
        );
        let proxy_https_encrypted = stored.proxy.https_proxy.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(
            proxy_https_encrypted
        ));
        assert_eq!(
            store.decrypt(proxy_https_encrypted).unwrap(),
            "https://user:pass@proxy.internal:8443"
        );
        let proxy_all_encrypted = stored.proxy.all_proxy.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(
            proxy_all_encrypted
        ));
        assert_eq!(
            store.decrypt(proxy_all_encrypted).unwrap(),
            "socks5://user:pass@proxy.internal:1080"
        );

        let browser_encrypted = stored.browser.computer_use.api_key.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(
            browser_encrypted
        ));
        assert_eq!(
            store.decrypt(browser_encrypted).unwrap(),
            "browser-credential"
        );

        let web_search_encrypted = stored.web_search.brave_api_key.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(
            web_search_encrypted
        ));
        assert_eq!(
            store.decrypt(web_search_encrypted).unwrap(),
            "brave-credential"
        );

        let worker = stored.agents.get("worker").unwrap();
        let worker_encrypted = worker.api_key.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(worker_encrypted));
        assert_eq!(store.decrypt(worker_encrypted).unwrap(), "agent-credential");

        let storage_db_url = stored.storage.provider.config.db_url.as_deref().unwrap();
        assert!(crate::security::SecretStore::is_encrypted(storage_db_url));
        assert_eq!(
            store.decrypt(storage_db_url).unwrap(),
            "postgres://user:pw@host/db"
        );

        let reliability_key = &stored.reliability.api_keys[0];
        assert!(crate::security::SecretStore::is_encrypted(reliability_key));
        assert_eq!(store.decrypt(reliability_key).unwrap(), "backup-credential");

        let paired_token = &stored.gateway.paired_tokens[0];
        assert!(crate::security::SecretStore::is_encrypted(paired_token));
        assert_eq!(store.decrypt(paired_token).unwrap(), "zc_0123456789abcdef");

        let telegram_token = stored
            .channels_config
            .telegram
            .as_ref()
            .unwrap()
            .bot_token
            .clone();
        // telegram_token is now a Secret newtype — pre-encrypt check
        // changes shape: Secret::new_from_plaintext(...) encrypts at
        // construction, but our test fixture uses Secret::from_wire(...)
        // which stores plaintext. So the round-trip is plaintext-in,
        // plaintext-out via the wire form. Once auto-migration is added,
        // this assertion will need to re-evaluate.
        assert_eq!(telegram_token.as_wire_str(), "telegram-credential");

        let _ = fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn config_save_atomic_cleanup() {
        let dir = std::env::temp_dir().join(format!("plaw_test_config_{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).await.unwrap();

        let config_path = dir.join("config.toml");
        let mut config = Config::default();
        config.workspace_dir = dir.join("workspace");
        config.config_path = config_path.clone();
        config.default_model = Some("model-a".into());
        config.save().await.unwrap();
        assert!(config_path.exists());

        config.default_model = Some("model-b".into());
        config.save().await.unwrap();

        let contents = tokio::fs::read_to_string(&config_path).await.unwrap();
        assert!(contents.contains("model-b"));

        let names: Vec<String> = ReadDirStream::new(fs::read_dir(&dir).await.unwrap())
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect()
            .await;
        assert!(!names.iter().any(|name| name.contains(".tmp-")));
        assert!(!names.iter().any(|name| name.ends_with(".bak")));

        let _ = fs::remove_dir_all(&dir).await;
    }

    // ── Telegram / Discord config ────────────────────────────

    #[test]
    async fn telegram_config_serde() {
        let tc = TelegramConfig {
            bot_token: crate::security::Secret::from_wire("123:XYZ".into()),
            allowed_users: vec!["alice".into(), "bob".into()],
            stream_mode: StreamMode::Partial,
            draft_update_interval_ms: 500,
            interrupt_on_new_message: true,
            mention_only: false,
            group_reply: None,
            base_url: None,
        };
        let json = serde_json::to_string(&tc).unwrap();
        let parsed: TelegramConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bot_token.as_wire_str(), "123:XYZ");
        assert_eq!(parsed.allowed_users.len(), 2);
        assert_eq!(parsed.stream_mode, StreamMode::Partial);
        assert_eq!(parsed.draft_update_interval_ms, 500);
        assert!(parsed.interrupt_on_new_message);
    }

    #[test]
    async fn telegram_config_defaults_stream_off() {
        let json = r#"{"bot_token":"tok","allowed_users":[]}"#;
        let parsed: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.stream_mode, StreamMode::Off);
        assert_eq!(parsed.draft_update_interval_ms, 1000);
        assert!(!parsed.interrupt_on_new_message);
        assert!(parsed.base_url.is_none());
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::AllMessages
        );
        assert!(parsed.group_reply_allowed_sender_ids().is_empty());
    }

    #[test]
    async fn telegram_config_custom_base_url() {
        let json = r#"{"bot_token":"tok","allowed_users":[],"base_url":"https://tapi.bale.ai"}"#;
        let parsed: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.base_url, Some("https://tapi.bale.ai".to_string()));
    }

    #[test]
    async fn telegram_group_reply_config_overrides_legacy_mention_only() {
        let json = r#"{
            "bot_token":"tok",
            "allowed_users":["*"],
            "mention_only":false,
            "group_reply":{
                "mode":"mention_only",
                "allowed_sender_ids":["1001","1002"]
            }
        }"#;

        let parsed: TelegramConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::MentionOnly
        );
        assert_eq!(
            parsed.group_reply_allowed_sender_ids(),
            vec!["1001".to_string(), "1002".to_string()]
        );
    }

    #[test]
    async fn discord_config_serde() {
        let dc = DiscordConfig {
            bot_token: crate::security::Secret::from_wire("discord-token".into()),
            guild_id: Some("12345".into()),
            allowed_users: vec![],
            listen_to_bots: false,
            mention_only: false,
            group_reply: None,
        };
        let json = serde_json::to_string(&dc).unwrap();
        let parsed: DiscordConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bot_token.as_wire_str(), "discord-token");
        assert_eq!(parsed.guild_id.as_deref(), Some("12345"));
    }

    #[test]
    async fn discord_config_optional_guild() {
        let dc = DiscordConfig {
            bot_token: crate::security::Secret::from_wire("tok".into()),
            guild_id: None,
            allowed_users: vec![],
            listen_to_bots: false,
            mention_only: false,
            group_reply: None,
        };
        let json = serde_json::to_string(&dc).unwrap();
        let parsed: DiscordConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.guild_id.is_none());
    }

    #[test]
    async fn discord_group_reply_mode_falls_back_to_legacy_mention_only() {
        let json = r#"{
            "bot_token":"tok",
            "mention_only":true
        }"#;
        let parsed: DiscordConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::MentionOnly
        );
        assert!(parsed.group_reply_allowed_sender_ids().is_empty());
    }

    #[test]
    async fn discord_group_reply_mode_overrides_legacy_mention_only() {
        let json = r#"{
            "bot_token":"tok",
            "mention_only":true,
            "group_reply":{
                "mode":"all_messages",
                "allowed_sender_ids":["111"]
            }
        }"#;
        let parsed: DiscordConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::AllMessages
        );
        assert_eq!(
            parsed.group_reply_allowed_sender_ids(),
            vec!["111".to_string()]
        );
    }

    // ── iMessage / Matrix config ────────────────────────────

    #[test]
    async fn imessage_config_serde() {
        let ic = IMessageConfig {
            allowed_contacts: vec!["+1234567890".into(), "user@icloud.com".into()],
        };
        let json = serde_json::to_string(&ic).unwrap();
        let parsed: IMessageConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.allowed_contacts.len(), 2);
        assert_eq!(parsed.allowed_contacts[0], "+1234567890");
    }

    #[test]
    async fn imessage_config_empty_contacts() {
        let ic = IMessageConfig {
            allowed_contacts: vec![],
        };
        let json = serde_json::to_string(&ic).unwrap();
        let parsed: IMessageConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.allowed_contacts.is_empty());
    }

    #[test]
    async fn imessage_config_wildcard() {
        let ic = IMessageConfig {
            allowed_contacts: vec!["*".into()],
        };
        let toml_str = toml::to_string(&ic).unwrap();
        let parsed: IMessageConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.allowed_contacts, vec!["*"]);
    }

    #[test]
    async fn matrix_config_serde() {
        let mc = MatrixConfig {
            homeserver: "https://matrix.org".into(),
            access_token: crate::security::Secret::from_wire("syt_token_abc".into()),
            user_id: Some("@bot:matrix.org".into()),
            device_id: Some("DEVICE123".into()),
            room_id: "!room123:matrix.org".into(),
            allowed_users: vec!["@user:matrix.org".into()],
            mention_only: false,
        };
        let json = serde_json::to_string(&mc).unwrap();
        let parsed: MatrixConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.homeserver, "https://matrix.org");
        assert_eq!(parsed.access_token.as_wire_str(), "syt_token_abc");
        assert_eq!(parsed.user_id.as_deref(), Some("@bot:matrix.org"));
        assert_eq!(parsed.device_id.as_deref(), Some("DEVICE123"));
        assert_eq!(parsed.room_id, "!room123:matrix.org");
        assert_eq!(parsed.allowed_users.len(), 1);
    }

    #[test]
    async fn matrix_config_toml_roundtrip() {
        let mc = MatrixConfig {
            homeserver: "https://synapse.local:8448".into(),
            access_token: crate::security::Secret::from_wire("tok".into()),
            user_id: None,
            device_id: None,
            room_id: "!abc:synapse.local".into(),
            allowed_users: vec!["@admin:synapse.local".into(), "*".into()],
            mention_only: true,
        };
        let toml_str = toml::to_string(&mc).unwrap();
        let parsed: MatrixConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.homeserver, "https://synapse.local:8448");
        assert_eq!(parsed.allowed_users.len(), 2);
    }

    #[test]
    async fn matrix_config_backward_compatible_without_session_hints() {
        let toml = r#"
homeserver = "https://matrix.org"
access_token = "tok"
room_id = "!ops:matrix.org"
allowed_users = ["@ops:matrix.org"]
"#;

        let parsed: MatrixConfig = toml::from_str(toml).unwrap();
        assert_eq!(parsed.homeserver, "https://matrix.org");
        assert!(parsed.user_id.is_none());
        assert!(parsed.device_id.is_none());
        assert!(!parsed.mention_only);
    }

    #[test]
    async fn signal_config_serde() {
        let sc = SignalConfig {
            http_url: "http://127.0.0.1:8686".into(),
            account: "+1234567890".into(),
            group_id: Some("group123".into()),
            allowed_from: vec!["+1111111111".into()],
            ignore_attachments: true,
            ignore_stories: false,
        };
        let json = serde_json::to_string(&sc).unwrap();
        let parsed: SignalConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.http_url, "http://127.0.0.1:8686");
        assert_eq!(parsed.account, "+1234567890");
        assert_eq!(parsed.group_id.as_deref(), Some("group123"));
        assert_eq!(parsed.allowed_from.len(), 1);
        assert!(parsed.ignore_attachments);
        assert!(!parsed.ignore_stories);
    }

    #[test]
    async fn signal_config_toml_roundtrip() {
        let sc = SignalConfig {
            http_url: "http://localhost:8080".into(),
            account: "+9876543210".into(),
            group_id: None,
            allowed_from: vec!["*".into()],
            ignore_attachments: false,
            ignore_stories: true,
        };
        let toml_str = toml::to_string(&sc).unwrap();
        let parsed: SignalConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.http_url, "http://localhost:8080");
        assert_eq!(parsed.account, "+9876543210");
        assert!(parsed.group_id.is_none());
        assert!(parsed.ignore_stories);
    }

    #[test]
    async fn signal_config_defaults() {
        let json = r#"{"http_url":"http://127.0.0.1:8686","account":"+1234567890"}"#;
        let parsed: SignalConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.group_id.is_none());
        assert!(parsed.allowed_from.is_empty());
        assert!(!parsed.ignore_attachments);
        assert!(!parsed.ignore_stories);
    }

    #[test]
    async fn channels_config_with_imessage_and_matrix() {
        let c = ChannelsConfig {
            cli: true,
            telegram: None,
            discord: None,
            slack: None,
            mattermost: None,
            webhook: None,
            imessage: Some(IMessageConfig {
                allowed_contacts: vec!["+1".into()],
            }),
            matrix: Some(MatrixConfig {
                homeserver: "https://m.org".into(),
                access_token: crate::security::Secret::from_wire("tok".into()),
                user_id: None,
                device_id: None,
                room_id: "!r:m".into(),
                allowed_users: vec!["@u:m".into()],
                mention_only: false,
            }),
            signal: None,
            whatsapp: None,
            linq: None,
            wati: None,
            nextcloud_talk: None,
            email: None,
            irc: None,
            lark: None,
            feishu: None,
            dingtalk: None,
            qq: None,
            nostr: None,
            clawdtalk: None,
            message_timeout_secs: 300,
        };
        let toml_str = toml::to_string_pretty(&c).unwrap();
        let parsed: ChannelsConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.imessage.is_some());
        assert!(parsed.matrix.is_some());
        assert_eq!(parsed.imessage.unwrap().allowed_contacts, vec!["+1"]);
        assert_eq!(parsed.matrix.unwrap().homeserver, "https://m.org");
    }

    #[test]
    async fn channels_config_default_has_no_imessage_matrix() {
        let c = ChannelsConfig::default();
        assert!(c.imessage.is_none());
        assert!(c.matrix.is_none());
    }

    // ── Edge cases: serde(default) for allowed_users ─────────

    #[test]
    async fn discord_config_deserializes_without_allowed_users() {
        // Old configs won't have allowed_users — serde(default) should fill vec![]
        let json = r#"{"bot_token":"tok","guild_id":"123"}"#;
        let parsed: DiscordConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.allowed_users.is_empty());
    }

    #[test]
    async fn discord_config_deserializes_with_allowed_users() {
        let json = r#"{"bot_token":"tok","guild_id":"123","allowed_users":["111","222"]}"#;
        let parsed: DiscordConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.allowed_users, vec!["111", "222"]);
    }

    #[test]
    async fn slack_config_deserializes_without_allowed_users() {
        let json = r#"{"bot_token":"xoxb-tok"}"#;
        let parsed: SlackConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.allowed_users.is_empty());
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::AllMessages
        );
    }

    #[test]
    async fn slack_config_deserializes_with_allowed_users() {
        let json = r#"{"bot_token":"xoxb-tok","allowed_users":["U111"]}"#;
        let parsed: SlackConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.allowed_users, vec!["U111"]);
    }

    #[test]
    async fn discord_config_toml_backward_compat() {
        let toml_str = r#"
bot_token = "tok"
guild_id = "123"
"#;
        let parsed: DiscordConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.allowed_users.is_empty());
        assert_eq!(parsed.bot_token.as_wire_str(), "tok");
    }

    #[test]
    async fn slack_config_toml_backward_compat() {
        let toml_str = r#"
bot_token = "xoxb-tok"
channel_id = "C123"
"#;
        let parsed: SlackConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.allowed_users.is_empty());
        assert_eq!(parsed.channel_id.as_deref(), Some("C123"));
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::AllMessages
        );
    }

    #[test]
    async fn slack_group_reply_config_supports_sender_overrides() {
        let json = r#"{
            "bot_token":"xoxb-tok",
            "group_reply":{
                "mode":"mention_only",
                "allowed_sender_ids":["U111"]
            }
        }"#;
        let parsed: SlackConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::MentionOnly
        );
        assert_eq!(
            parsed.group_reply_allowed_sender_ids(),
            vec!["U111".to_string()]
        );
    }

    #[test]
    async fn mattermost_group_reply_mode_falls_back_to_legacy_mention_only() {
        let json = r#"{
            "url":"https://mm.example.com",
            "bot_token":"token",
            "mention_only":true
        }"#;
        let parsed: MattermostConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::MentionOnly
        );
    }

    #[test]
    async fn mattermost_group_reply_mode_overrides_legacy_mention_only() {
        let json = r#"{
            "url":"https://mm.example.com",
            "bot_token":"token",
            "mention_only":true,
            "group_reply":{
                "mode":"all_messages",
                "allowed_sender_ids":["u1","u2"]
            }
        }"#;
        let parsed: MattermostConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::AllMessages
        );
        assert_eq!(
            parsed.group_reply_allowed_sender_ids(),
            vec!["u1".to_string(), "u2".to_string()]
        );
    }

    #[test]
    async fn webhook_config_with_secret() {
        let json = r#"{"port":8080,"secret":"my-secret-key"}"#;
        let parsed: WebhookConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.secret.as_ref().map(|s| s.as_wire_str()),
            Some("my-secret-key")
        );
    }

    #[test]
    async fn webhook_config_without_secret() {
        let json = r#"{"port":8080}"#;
        let parsed: WebhookConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.secret.is_none());
        assert_eq!(parsed.port, 8080);
    }

    // ── WhatsApp config ──────────────────────────────────────

    #[test]
    async fn whatsapp_config_serde() {
        let wc = WhatsAppConfig {
            access_token: Some(crate::security::Secret::from_wire("EAABx...".into())),
            phone_number_id: Some("123456789".into()),
            verify_token: Some(crate::security::Secret::from_wire("my-verify-token".into())),
            app_secret: None,
            session_path: None,
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["+1234567890".into(), "+9876543210".into()],
        };
        let json = serde_json::to_string(&wc).unwrap();
        let parsed: WhatsAppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(
            parsed.access_token.as_ref().map(|s| s.as_wire_str()),
            Some("EAABx...")
        );
        assert_eq!(parsed.phone_number_id, Some("123456789".into()));
        assert_eq!(
            parsed.verify_token.as_ref().map(|s| s.as_wire_str()),
            Some("my-verify-token")
        );
        assert_eq!(parsed.allowed_numbers.len(), 2);
    }

    #[test]
    async fn whatsapp_config_toml_roundtrip() {
        let wc = WhatsAppConfig {
            access_token: Some(crate::security::Secret::from_wire("tok".into())),
            phone_number_id: Some("12345".into()),
            verify_token: Some(crate::security::Secret::from_wire("verify".into())),
            app_secret: Some(crate::security::Secret::from_wire("secret123".into())),
            session_path: None,
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["+1".into()],
        };
        let toml_str = toml::to_string(&wc).unwrap();
        let parsed: WhatsAppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.phone_number_id, Some("12345".into()));
        assert_eq!(
            parsed.app_secret.as_ref().map(|s| s.as_wire_str()),
            Some("secret123")
        );
        assert_eq!(parsed.allowed_numbers, vec!["+1"]);
    }

    #[test]
    async fn whatsapp_config_deserializes_without_allowed_numbers() {
        let json = r#"{"access_token":"tok","phone_number_id":"123","verify_token":"ver"}"#;
        let parsed: WhatsAppConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.allowed_numbers.is_empty());
    }

    #[test]
    async fn whatsapp_config_wildcard_allowed() {
        let wc = WhatsAppConfig {
            access_token: Some(crate::security::Secret::from_wire("tok".into())),
            phone_number_id: Some("123".into()),
            verify_token: Some(crate::security::Secret::from_wire("ver".into())),
            app_secret: None,
            session_path: None,
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["*".into()],
        };
        let toml_str = toml::to_string(&wc).unwrap();
        let parsed: WhatsAppConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.allowed_numbers, vec!["*"]);
    }

    #[test]
    async fn whatsapp_config_backend_type_cloud_precedence_when_ambiguous() {
        let wc = WhatsAppConfig {
            access_token: Some(crate::security::Secret::from_wire("tok".into())),
            phone_number_id: Some("123".into()),
            verify_token: Some(crate::security::Secret::from_wire("ver".into())),
            app_secret: None,
            session_path: Some("~/.plaw/state/whatsapp-web/session.db".into()),
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec!["+1".into()],
        };
        assert!(wc.is_ambiguous_config());
        assert_eq!(wc.backend_type(), "cloud");
    }

    #[test]
    async fn whatsapp_config_backend_type_web() {
        let wc = WhatsAppConfig {
            access_token: None,
            phone_number_id: None,
            verify_token: None,
            app_secret: None,
            session_path: Some("~/.plaw/state/whatsapp-web/session.db".into()),
            pair_phone: None,
            pair_code: None,
            allowed_numbers: vec![],
        };
        assert!(!wc.is_ambiguous_config());
        assert_eq!(wc.backend_type(), "web");
    }

    #[test]
    async fn channels_config_with_whatsapp() {
        let c = ChannelsConfig {
            cli: true,
            telegram: None,
            discord: None,
            slack: None,
            mattermost: None,
            webhook: None,
            imessage: None,
            matrix: None,
            signal: None,
            whatsapp: Some(WhatsAppConfig {
                access_token: Some(crate::security::Secret::from_wire("tok".into())),
                phone_number_id: Some("123".into()),
                verify_token: Some(crate::security::Secret::from_wire("ver".into())),
                app_secret: None,
                session_path: None,
                pair_phone: None,
                pair_code: None,
                allowed_numbers: vec!["+1".into()],
            }),
            linq: None,
            wati: None,
            nextcloud_talk: None,
            email: None,
            irc: None,
            lark: None,
            feishu: None,
            dingtalk: None,
            qq: None,
            nostr: None,
            clawdtalk: None,
            message_timeout_secs: 300,
        };
        let toml_str = toml::to_string_pretty(&c).unwrap();
        let parsed: ChannelsConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.whatsapp.is_some());
        let wa = parsed.whatsapp.unwrap();
        assert_eq!(wa.phone_number_id, Some("123".into()));
        assert_eq!(wa.allowed_numbers, vec!["+1"]);
    }

    #[test]
    async fn channels_config_default_has_no_whatsapp() {
        let c = ChannelsConfig::default();
        assert!(c.whatsapp.is_none());
    }

    #[test]
    async fn channels_config_default_has_no_nextcloud_talk() {
        let c = ChannelsConfig::default();
        assert!(c.nextcloud_talk.is_none());
    }

    // ══════════════════════════════════════════════════════════
    // SECURITY CHECKLIST TESTS — Gateway config
    // ══════════════════════════════════════════════════════════

    #[test]
    async fn checklist_gateway_default_requires_pairing() {
        let g = GatewayConfig::default();
        assert!(g.require_pairing, "Pairing must be required by default");
    }

    #[test]
    async fn checklist_gateway_default_blocks_public_bind() {
        let g = GatewayConfig::default();
        assert!(
            !g.allow_public_bind,
            "Public bind must be blocked by default"
        );
    }

    #[test]
    async fn checklist_gateway_default_no_tokens() {
        let g = GatewayConfig::default();
        assert!(
            g.paired_tokens.is_empty(),
            "No pre-paired tokens by default"
        );
        assert_eq!(g.pair_rate_limit_per_minute, 10);
        assert_eq!(g.webhook_rate_limit_per_minute, 60);
        assert!(!g.trust_forwarded_headers);
        assert_eq!(g.rate_limit_max_keys, 10_000);
        assert_eq!(g.idempotency_ttl_secs, 300);
        assert_eq!(g.idempotency_max_keys, 10_000);
        assert!(!g.node_control.enabled);
        assert!(g.node_control.auth_token.is_none());
        assert!(g.node_control.allowed_node_ids.is_empty());
    }

    #[test]
    async fn checklist_gateway_cli_default_host_is_localhost() {
        // The CLI default for --host is 127.0.0.1 (checked in main.rs)
        // Here we verify the config default matches
        let c = Config::default();
        assert!(
            c.gateway.require_pairing,
            "Config default must require pairing"
        );
        assert!(
            !c.gateway.allow_public_bind,
            "Config default must block public bind"
        );
    }

    #[test]
    async fn checklist_gateway_serde_roundtrip() {
        let g = GatewayConfig {
            port: 42617,
            host: "127.0.0.1".into(),
            require_pairing: true,
            allow_public_bind: false,
            paired_tokens: vec!["zc_test_token".into()],
            pair_rate_limit_per_minute: 12,
            webhook_rate_limit_per_minute: 80,
            trust_forwarded_headers: true,
            rate_limit_max_keys: 2048,
            idempotency_ttl_secs: 600,
            idempotency_max_keys: 4096,
            node_control: NodeControlConfig {
                enabled: true,
                auth_token: Some("node-token".into()),
                allowed_node_ids: vec!["node-1".into(), "node-2".into()],
            },
        };
        let toml_str = toml::to_string(&g).unwrap();
        let parsed: GatewayConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.require_pairing);
        assert!(!parsed.allow_public_bind);
        assert_eq!(parsed.paired_tokens, vec!["zc_test_token"]);
        assert_eq!(parsed.pair_rate_limit_per_minute, 12);
        assert_eq!(parsed.webhook_rate_limit_per_minute, 80);
        assert!(parsed.trust_forwarded_headers);
        assert_eq!(parsed.rate_limit_max_keys, 2048);
        assert_eq!(parsed.idempotency_ttl_secs, 600);
        assert_eq!(parsed.idempotency_max_keys, 4096);
        assert!(parsed.node_control.enabled);
        assert_eq!(
            parsed.node_control.auth_token.as_deref(),
            Some("node-token")
        );
        assert_eq!(
            parsed.node_control.allowed_node_ids,
            vec!["node-1", "node-2"]
        );
    }

    #[test]
    async fn checklist_gateway_backward_compat_no_gateway_section() {
        // Old configs without [gateway] should get secure defaults
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(
            parsed.gateway.require_pairing,
            "Missing [gateway] must default to require_pairing=true"
        );
        assert!(
            !parsed.gateway.allow_public_bind,
            "Missing [gateway] must default to allow_public_bind=false"
        );
    }

    #[test]
    async fn checklist_autonomy_default_is_workspace_scoped() {
        let a = AutonomyConfig::default();
        assert!(a.workspace_only, "Default autonomy must be workspace_only");
        assert!(
            a.forbidden_paths.contains(&"/etc".to_string()),
            "Must block /etc"
        );
        assert!(
            a.forbidden_paths.contains(&"/proc".to_string()),
            "Must block /proc"
        );
        assert!(
            a.forbidden_paths.contains(&"~/.ssh".to_string()),
            "Must block ~/.ssh"
        );
    }

    // ══════════════════════════════════════════════════════════
    // COMPOSIO CONFIG TESTS
    // ══════════════════════════════════════════════════════════

    #[test]
    async fn composio_config_default_disabled() {
        let c = ComposioConfig::default();
        assert!(!c.enabled, "Composio must be disabled by default");
        assert!(c.api_key.is_none(), "No API key by default");
        assert_eq!(c.entity_id, "default");
    }

    #[test]
    async fn composio_config_serde_roundtrip() {
        let c = ComposioConfig {
            enabled: true,
            api_key: Some("comp-key-123".into()),
            entity_id: "user42".into(),
        };
        let toml_str = toml::to_string(&c).unwrap();
        let parsed: ComposioConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.api_key.as_deref(), Some("comp-key-123"));
        assert_eq!(parsed.entity_id, "user42");
    }

    #[test]
    async fn composio_config_backward_compat_missing_section() {
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(
            !parsed.composio.enabled,
            "Missing [composio] must default to disabled"
        );
        assert!(parsed.composio.api_key.is_none());
    }

    #[test]
    async fn composio_config_partial_toml() {
        let toml_str = r"
enabled = true
";
        let parsed: ComposioConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.enabled);
        assert!(parsed.api_key.is_none());
        assert_eq!(parsed.entity_id, "default");
    }

    #[test]
    async fn composio_config_enable_alias_supported() {
        let toml_str = r"
enable = true
";
        let parsed: ComposioConfig = toml::from_str(toml_str).unwrap();
        assert!(parsed.enabled);
        assert!(parsed.api_key.is_none());
        assert_eq!(parsed.entity_id, "default");
    }

    // ══════════════════════════════════════════════════════════
    // SECRETS CONFIG TESTS
    // ══════════════════════════════════════════════════════════

    #[test]
    async fn secrets_config_default_encrypts() {
        let s = SecretsConfig::default();
        assert!(s.encrypt, "Encryption must be enabled by default");
    }

    #[test]
    async fn secrets_config_serde_roundtrip() {
        let s = SecretsConfig { encrypt: false };
        let toml_str = toml::to_string(&s).unwrap();
        let parsed: SecretsConfig = toml::from_str(&toml_str).unwrap();
        assert!(!parsed.encrypt);
    }

    #[test]
    async fn secrets_config_backward_compat_missing_section() {
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(
            parsed.secrets.encrypt,
            "Missing [secrets] must default to encrypt=true"
        );
    }

    #[test]
    async fn config_default_has_composio_and_secrets() {
        let c = Config::default();
        assert!(!c.composio.enabled);
        assert!(c.composio.api_key.is_none());
        assert!(c.secrets.encrypt);
        assert!(!c.browser.enabled);
        assert!(c.browser.allowed_domains.is_empty());
    }

    #[test]
    async fn browser_config_default_disabled() {
        let b = BrowserConfig::default();
        assert!(!b.enabled);
        assert!(b.allowed_domains.is_empty());
        assert_eq!(b.backend, "agent_browser");
        assert!(b.native_headless);
        assert_eq!(b.native_webdriver_url, "http://127.0.0.1:9515");
        assert!(b.native_chrome_path.is_none());
        assert_eq!(b.computer_use.endpoint, "http://127.0.0.1:8787/v1/actions");
        assert_eq!(b.computer_use.timeout_ms, 15_000);
        assert!(!b.computer_use.allow_remote_endpoint);
        assert!(b.computer_use.window_allowlist.is_empty());
        assert!(b.computer_use.max_coordinate_x.is_none());
        assert!(b.computer_use.max_coordinate_y.is_none());
    }

    #[test]
    async fn browser_config_serde_roundtrip() {
        let b = BrowserConfig {
            enabled: true,
            allowed_domains: vec!["example.com".into(), "docs.example.com".into()],
            browser_open: "chrome".into(),
            session_name: None,
            backend: "auto".into(),
            native_headless: false,
            native_webdriver_url: "http://localhost:4444".into(),
            native_chrome_path: Some("/usr/bin/chromium".into()),
            computer_use: BrowserComputerUseConfig {
                endpoint: "https://computer-use.example.com/v1/actions".into(),
                api_key: Some("test-token".into()),
                timeout_ms: 8_000,
                allow_remote_endpoint: true,
                window_allowlist: vec!["Chrome".into(), "Visual Studio Code".into()],
                max_coordinate_x: Some(3840),
                max_coordinate_y: Some(2160),
            },
        };
        let toml_str = toml::to_string(&b).unwrap();
        let parsed: BrowserConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.allowed_domains.len(), 2);
        assert_eq!(parsed.allowed_domains[0], "example.com");
        assert_eq!(parsed.backend, "auto");
        assert!(!parsed.native_headless);
        assert_eq!(parsed.native_webdriver_url, "http://localhost:4444");
        assert_eq!(
            parsed.native_chrome_path.as_deref(),
            Some("/usr/bin/chromium")
        );
        assert_eq!(
            parsed.computer_use.endpoint,
            "https://computer-use.example.com/v1/actions"
        );
        assert_eq!(parsed.computer_use.api_key.as_deref(), Some("test-token"));
        assert_eq!(parsed.computer_use.timeout_ms, 8_000);
        assert!(parsed.computer_use.allow_remote_endpoint);
        assert_eq!(parsed.computer_use.window_allowlist.len(), 2);
        assert_eq!(parsed.computer_use.max_coordinate_x, Some(3840));
        assert_eq!(parsed.computer_use.max_coordinate_y, Some(2160));
    }

    #[test]
    async fn browser_config_backward_compat_missing_section() {
        let minimal = r#"
workspace_dir = "/tmp/ws"
config_path = "/tmp/config.toml"
default_temperature = 0.7
"#;
        let parsed: Config = toml::from_str(minimal).unwrap();
        assert!(!parsed.browser.enabled);
        assert!(parsed.browser.allowed_domains.is_empty());
    }

    // ── Environment variable overrides (Docker support) ─────────

    async fn env_override_lock() -> MutexGuard<'static, ()> {
        static ENV_OVERRIDE_TEST_LOCK: Mutex<()> = Mutex::const_new(());
        ENV_OVERRIDE_TEST_LOCK.lock().await
    }

    fn clear_proxy_env_test_vars() {
        for key in [
            "PLAW_PROXY_ENABLED",
            "PLAW_HTTP_PROXY",
            "PLAW_HTTPS_PROXY",
            "PLAW_ALL_PROXY",
            "PLAW_NO_PROXY",
            "PLAW_PROXY_SCOPE",
            "PLAW_PROXY_SERVICES",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
            "no_proxy",
        ] {
            std::env::remove_var(key);
        }
    }

    #[test]
    async fn env_override_api_key() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert!(config.api_key.is_none());

        std::env::set_var("PLAW_API_KEY", "sk-test-env-key");
        config.apply_env_overrides();
        assert_eq!(config.api_key.as_deref(), Some("sk-test-env-key"));

        std::env::remove_var("PLAW_API_KEY");
    }

    #[test]
    async fn env_override_api_key_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("PLAW_API_KEY");
        std::env::set_var("API_KEY", "sk-fallback-key");
        config.apply_env_overrides();
        assert_eq!(config.api_key.as_deref(), Some("sk-fallback-key"));

        std::env::remove_var("API_KEY");
    }

    #[test]
    async fn env_override_provider() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("PLAW_PROVIDER", "anthropic");
        config.apply_env_overrides();
        assert_eq!(config.default_provider.as_deref(), Some("anthropic"));

        std::env::remove_var("PLAW_PROVIDER");
    }

    #[test]
    async fn env_override_model_provider_alias() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("PLAW_PROVIDER");
        std::env::set_var("PLAW_MODEL_PROVIDER", "openai-codex");
        config.apply_env_overrides();
        assert_eq!(config.default_provider.as_deref(), Some("openai-codex"));

        std::env::remove_var("PLAW_MODEL_PROVIDER");
    }

    #[test]
    async fn toml_supports_model_provider_and_model_alias_fields() {
        let raw = r#"
default_temperature = 0.7
model_provider = "sub2api"
model = "gpt-5.3-codex"

[model_providers.sub2api]
name = "sub2api"
base_url = "https://api.tonsof.blue/v1"
wire_api = "responses"
requires_openai_auth = true
"#;

        let parsed: Config = toml::from_str(raw).expect("config should parse");
        assert_eq!(parsed.default_provider.as_deref(), Some("sub2api"));
        assert_eq!(parsed.default_model.as_deref(), Some("gpt-5.3-codex"));
        let profile = parsed
            .model_providers
            .get("sub2api")
            .expect("profile should exist");
        assert_eq!(profile.wire_api.as_deref(), Some("responses"));
        assert!(profile.requires_openai_auth);
    }

    #[test]
    async fn env_override_open_skills_enabled_and_dir() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert!(!config.skills.open_skills_enabled);
        assert!(config.skills.open_skills_dir.is_none());
        assert_eq!(
            config.skills.prompt_injection_mode,
            SkillsPromptInjectionMode::Full
        );

        std::env::set_var("PLAW_OPEN_SKILLS_ENABLED", "true");
        std::env::set_var("PLAW_OPEN_SKILLS_DIR", "/tmp/open-skills");
        std::env::set_var("PLAW_SKILLS_PROMPT_MODE", "compact");
        config.apply_env_overrides();

        assert!(config.skills.open_skills_enabled);
        assert_eq!(
            config.skills.open_skills_dir.as_deref(),
            Some("/tmp/open-skills")
        );
        assert_eq!(
            config.skills.prompt_injection_mode,
            SkillsPromptInjectionMode::Compact
        );

        std::env::remove_var("PLAW_OPEN_SKILLS_ENABLED");
        std::env::remove_var("PLAW_OPEN_SKILLS_DIR");
        std::env::remove_var("PLAW_SKILLS_PROMPT_MODE");
    }

    #[test]
    async fn env_override_open_skills_enabled_invalid_value_keeps_existing_value() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        config.skills.open_skills_enabled = true;
        config.skills.prompt_injection_mode = SkillsPromptInjectionMode::Compact;

        std::env::set_var("PLAW_OPEN_SKILLS_ENABLED", "maybe");
        std::env::set_var("PLAW_SKILLS_PROMPT_MODE", "invalid");
        config.apply_env_overrides();

        assert!(config.skills.open_skills_enabled);
        assert_eq!(
            config.skills.prompt_injection_mode,
            SkillsPromptInjectionMode::Compact
        );
        std::env::remove_var("PLAW_OPEN_SKILLS_ENABLED");
        std::env::remove_var("PLAW_SKILLS_PROMPT_MODE");
    }

    #[test]
    async fn env_override_provider_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("PLAW_PROVIDER");
        std::env::set_var("PROVIDER", "openai");
        config.apply_env_overrides();
        assert_eq!(config.default_provider.as_deref(), Some("openai"));

        std::env::remove_var("PROVIDER");
    }

    #[test]
    async fn env_override_provider_fallback_does_not_replace_non_default_provider() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("custom:https://proxy.example.com/v1".to_string()),
            ..Config::default()
        };

        std::env::remove_var("PLAW_PROVIDER");
        std::env::set_var("PROVIDER", "openrouter");
        config.apply_env_overrides();
        assert_eq!(
            config.default_provider.as_deref(),
            Some("custom:https://proxy.example.com/v1")
        );

        std::env::remove_var("PROVIDER");
    }

    #[test]
    async fn env_override_zero_claw_provider_overrides_non_default_provider() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("custom:https://proxy.example.com/v1".to_string()),
            ..Config::default()
        };

        std::env::set_var("PLAW_PROVIDER", "openrouter");
        std::env::set_var("PROVIDER", "anthropic");
        config.apply_env_overrides();
        assert_eq!(config.default_provider.as_deref(), Some("openrouter"));

        std::env::remove_var("PLAW_PROVIDER");
        std::env::remove_var("PROVIDER");
    }

    #[test]
    async fn provider_api_requires_custom_default_provider() {
        let mut config = Config::default();
        config.default_provider = Some("openai".to_string());
        config.provider_api = Some(ProviderApiMode::OpenAiResponses);

        let err = config
            .validate()
            .expect_err("provider_api should be rejected for non-custom provider");
        assert!(err.to_string().contains(
            "provider_api is only valid when default_provider uses the custom:<url> format"
        ));
    }

    #[test]
    async fn provider_api_invalid_value_is_rejected() {
        let toml = r#"
default_provider = "custom:https://example.com/v1"
default_model = "gpt-4o"
default_temperature = 0.7
provider_api = "not-a-real-mode"
"#;
        let parsed = toml::from_str::<Config>(toml);
        assert!(
            parsed.is_err(),
            "invalid provider_api should fail to deserialize"
        );
    }

    #[test]
    async fn model_route_max_tokens_must_be_positive_when_set() {
        let mut config = Config::default();
        config.model_routes = vec![ModelRouteConfig {
            hint: "reasoning".to_string(),
            provider: "openrouter".to_string(),
            model: "anthropic/claude-sonnet-4.6".to_string(),
            max_tokens: Some(0),
            api_key: None,
        }];

        let err = config
            .validate()
            .expect_err("model route max_tokens=0 should be rejected");
        assert!(err
            .to_string()
            .contains("model_routes[0].max_tokens must be greater than 0"));
    }

    #[test]
    async fn env_override_glm_api_key_for_regional_aliases() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("glm-cn".to_string()),
            ..Config::default()
        };

        std::env::set_var("GLM_API_KEY", "glm-regional-key");
        config.apply_env_overrides();
        assert_eq!(config.api_key.as_deref(), Some("glm-regional-key"));

        std::env::remove_var("GLM_API_KEY");
    }

    #[test]
    async fn env_override_zai_api_key_for_regional_aliases() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("zai-cn".to_string()),
            ..Config::default()
        };

        std::env::set_var("ZAI_API_KEY", "zai-regional-key");
        config.apply_env_overrides();
        assert_eq!(config.api_key.as_deref(), Some("zai-regional-key"));

        std::env::remove_var("ZAI_API_KEY");
    }

    #[test]
    async fn env_override_model() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("PLAW_MODEL", "gpt-4o");
        config.apply_env_overrides();
        assert_eq!(config.default_model.as_deref(), Some("gpt-4o"));

        std::env::remove_var("PLAW_MODEL");
    }

    #[test]
    async fn model_provider_profile_maps_to_custom_endpoint() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("sub2api".to_string()),
            model_providers: HashMap::from([(
                "sub2api".to_string(),
                ModelProviderConfig {
                    name: Some("sub2api".to_string()),
                    base_url: Some("https://api.tonsof.blue/v1".to_string()),
                    wire_api: None,
                    requires_openai_auth: false,
                },
            )]),
            ..Config::default()
        };

        config.apply_env_overrides();
        assert_eq!(
            config.default_provider.as_deref(),
            Some("custom:https://api.tonsof.blue/v1")
        );
        assert_eq!(
            config.api_url.as_deref(),
            Some("https://api.tonsof.blue/v1")
        );
    }

    #[test]
    async fn model_provider_profile_responses_uses_openai_codex_and_openai_key() {
        let _env_guard = env_override_lock().await;
        let mut config = Config {
            default_provider: Some("sub2api".to_string()),
            model_providers: HashMap::from([(
                "sub2api".to_string(),
                ModelProviderConfig {
                    name: Some("sub2api".to_string()),
                    base_url: Some("https://api.tonsof.blue".to_string()),
                    wire_api: Some("responses".to_string()),
                    requires_openai_auth: true,
                },
            )]),
            api_key: None,
            ..Config::default()
        };

        std::env::set_var("OPENAI_API_KEY", "sk-test-codex-key");
        config.apply_env_overrides();
        std::env::remove_var("OPENAI_API_KEY");

        assert_eq!(config.default_provider.as_deref(), Some("openai-codex"));
        assert_eq!(config.api_url.as_deref(), Some("https://api.tonsof.blue"));
        assert_eq!(config.api_key.as_deref(), Some("sk-test-codex-key"));
    }

    #[test]
    async fn validate_ollama_cloud_model_requires_remote_api_url() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            default_provider: Some("ollama".to_string()),
            default_model: Some("glm-5:cloud".to_string()),
            api_url: None,
            api_key: Some("ollama-key".to_string()),
            ..Config::default()
        };

        let error = config.validate().expect_err("expected validation to fail");
        assert!(error.to_string().contains(
            "default_model uses ':cloud' with provider 'ollama', but api_url is local or unset"
        ));
    }

    #[test]
    async fn validate_ollama_cloud_model_accepts_remote_endpoint_and_env_key() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            default_provider: Some("ollama".to_string()),
            default_model: Some("glm-5:cloud".to_string()),
            api_url: Some("https://ollama.com/api".to_string()),
            api_key: None,
            ..Config::default()
        };

        std::env::set_var("OLLAMA_API_KEY", "ollama-env-key");
        let result = config.validate();
        std::env::remove_var("OLLAMA_API_KEY");

        assert!(result.is_ok(), "expected validation to pass: {result:?}");
    }

    #[test]
    async fn validate_rejects_unknown_model_provider_wire_api() {
        let _env_guard = env_override_lock().await;
        let config = Config {
            default_provider: Some("sub2api".to_string()),
            model_providers: HashMap::from([(
                "sub2api".to_string(),
                ModelProviderConfig {
                    name: Some("sub2api".to_string()),
                    base_url: Some("https://api.tonsof.blue/v1".to_string()),
                    wire_api: Some("ws".to_string()),
                    requires_openai_auth: false,
                },
            )]),
            ..Config::default()
        };

        let error = config.validate().expect_err("expected validation failure");
        assert!(error
            .to_string()
            .contains("wire_api must be one of: responses, chat_completions"));
    }

    #[test]
    async fn env_override_model_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("PLAW_MODEL");
        std::env::set_var("MODEL", "anthropic/claude-3.5-sonnet");
        config.apply_env_overrides();
        assert_eq!(
            config.default_model.as_deref(),
            Some("anthropic/claude-3.5-sonnet")
        );

        std::env::remove_var("MODEL");
    }

    #[test]
    async fn env_override_workspace() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("PLAW_WORKSPACE", "/custom/workspace");
        config.apply_env_overrides();
        assert_eq!(config.workspace_dir, PathBuf::from("/custom/workspace"));

        std::env::remove_var("PLAW_WORKSPACE");
    }

    #[test]
    async fn resolve_runtime_config_dirs_uses_env_workspace_first() {
        let _env_guard = env_override_lock().await;
        let default_config_dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let default_workspace_dir = default_config_dir.join("workspace");
        let workspace_dir = default_config_dir.join("profile-a");

        std::env::set_var("PLAW_WORKSPACE", &workspace_dir);
        let (config_dir, resolved_workspace_dir, source) =
            resolve_runtime_config_dirs(&default_config_dir, &default_workspace_dir)
                .await
                .unwrap();

        assert_eq!(source, ConfigResolutionSource::EnvWorkspace);
        assert_eq!(config_dir, workspace_dir);
        assert_eq!(resolved_workspace_dir, workspace_dir.join("workspace"));

        std::env::remove_var("PLAW_WORKSPACE");
        let _ = fs::remove_dir_all(default_config_dir).await;
    }

    #[test]
    async fn resolve_runtime_config_dirs_uses_env_config_dir_first() {
        let _env_guard = env_override_lock().await;
        let default_config_dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let default_workspace_dir = default_config_dir.join("workspace");
        let explicit_config_dir = default_config_dir.join("explicit-config");
        let marker_config_dir = default_config_dir.join("profiles").join("alpha");
        let state_path = default_config_dir.join(ACTIVE_WORKSPACE_STATE_FILE);

        fs::create_dir_all(&default_config_dir).await.unwrap();
        let state = ActiveWorkspaceState {
            config_dir: marker_config_dir.to_string_lossy().into_owned(),
        };
        fs::write(&state_path, toml::to_string(&state).unwrap())
            .await
            .unwrap();

        std::env::set_var("PLAW_CONFIG_DIR", &explicit_config_dir);
        std::env::remove_var("PLAW_WORKSPACE");

        let (config_dir, resolved_workspace_dir, source) =
            resolve_runtime_config_dirs(&default_config_dir, &default_workspace_dir)
                .await
                .unwrap();

        assert_eq!(source, ConfigResolutionSource::EnvConfigDir);
        assert_eq!(config_dir, explicit_config_dir);
        assert_eq!(
            resolved_workspace_dir,
            explicit_config_dir.join("workspace")
        );

        std::env::remove_var("PLAW_CONFIG_DIR");
        let _ = fs::remove_dir_all(default_config_dir).await;
    }

    #[test]
    async fn resolve_runtime_config_dirs_uses_active_workspace_marker() {
        let _env_guard = env_override_lock().await;
        let default_config_dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let default_workspace_dir = default_config_dir.join("workspace");
        let marker_config_dir = default_config_dir.join("profiles").join("alpha");
        let state_path = default_config_dir.join(ACTIVE_WORKSPACE_STATE_FILE);

        std::env::remove_var("PLAW_WORKSPACE");
        fs::create_dir_all(&default_config_dir).await.unwrap();
        let state = ActiveWorkspaceState {
            config_dir: marker_config_dir.to_string_lossy().into_owned(),
        };
        fs::write(&state_path, toml::to_string(&state).unwrap())
            .await
            .unwrap();

        let (config_dir, resolved_workspace_dir, source) =
            resolve_runtime_config_dirs(&default_config_dir, &default_workspace_dir)
                .await
                .unwrap();

        assert_eq!(source, ConfigResolutionSource::ActiveWorkspaceMarker);
        assert_eq!(config_dir, marker_config_dir);
        assert_eq!(resolved_workspace_dir, marker_config_dir.join("workspace"));

        let _ = fs::remove_dir_all(default_config_dir).await;
    }

    #[test]
    async fn resolve_runtime_config_dirs_falls_back_to_default_layout() {
        let _env_guard = env_override_lock().await;
        let default_config_dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        let default_workspace_dir = default_config_dir.join("workspace");

        std::env::remove_var("PLAW_WORKSPACE");
        let (config_dir, resolved_workspace_dir, source) =
            resolve_runtime_config_dirs(&default_config_dir, &default_workspace_dir)
                .await
                .unwrap();

        assert_eq!(source, ConfigResolutionSource::DefaultConfigDir);
        assert_eq!(config_dir, default_config_dir);
        assert_eq!(resolved_workspace_dir, default_workspace_dir);

        let _ = fs::remove_dir_all(default_config_dir).await;
    }

    #[test]
    async fn load_or_init_workspace_override_uses_workspace_root_for_config() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("plaw_test_home_{}", uuid::Uuid::new_v4()));
        let workspace_dir = temp_home.join("profile-a");

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::set_var("PLAW_WORKSPACE", &workspace_dir);

        let config = Config::load_or_init().await.unwrap();

        assert_eq!(config.workspace_dir, workspace_dir.join("workspace"));
        assert_eq!(config.config_path, workspace_dir.join("config.toml"));
        assert!(workspace_dir.join("config.toml").exists());

        std::env::remove_var("PLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_workspace_suffix_uses_legacy_config_layout() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("plaw_test_home_{}", uuid::Uuid::new_v4()));
        let workspace_dir = temp_home.join("workspace");
        let legacy_config_path = temp_home.join(".plaw").join("config.toml");

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::set_var("PLAW_WORKSPACE", &workspace_dir);

        let config = Config::load_or_init().await.unwrap();

        assert_eq!(config.workspace_dir, workspace_dir);
        assert_eq!(config.config_path, legacy_config_path);
        assert!(config.config_path.exists());

        std::env::remove_var("PLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_workspace_override_keeps_existing_legacy_config() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("plaw_test_home_{}", uuid::Uuid::new_v4()));
        let workspace_dir = temp_home.join("custom-workspace");
        let legacy_config_dir = temp_home.join(".plaw");
        let legacy_config_path = legacy_config_dir.join("config.toml");

        fs::create_dir_all(&legacy_config_dir).await.unwrap();
        fs::write(
            &legacy_config_path,
            r#"default_temperature = 0.7
default_model = "legacy-model"
"#,
        )
        .await
        .unwrap();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::set_var("PLAW_WORKSPACE", &workspace_dir);

        let config = Config::load_or_init().await.unwrap();

        assert_eq!(config.workspace_dir, workspace_dir);
        assert_eq!(config.config_path, legacy_config_path);
        assert_eq!(config.default_model.as_deref(), Some("legacy-model"));

        std::env::remove_var("PLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_uses_persisted_active_workspace_marker() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("plaw_test_home_{}", uuid::Uuid::new_v4()));
        let custom_config_dir = temp_home.join("profiles").join("agent-alpha");

        fs::create_dir_all(&custom_config_dir).await.unwrap();
        fs::write(
            custom_config_dir.join("config.toml"),
            "default_temperature = 0.7\ndefault_model = \"persisted-profile\"\n",
        )
        .await
        .unwrap();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        std::env::remove_var("PLAW_WORKSPACE");

        persist_active_workspace_config_dir(&custom_config_dir)
            .await
            .unwrap();

        let config = Config::load_or_init().await.unwrap();

        assert_eq!(config.config_path, custom_config_dir.join("config.toml"));
        assert_eq!(config.workspace_dir, custom_config_dir.join("workspace"));
        assert_eq!(config.default_model.as_deref(), Some("persisted-profile"));

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn load_or_init_env_workspace_override_takes_priority_over_marker() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("plaw_test_home_{}", uuid::Uuid::new_v4()));
        let marker_config_dir = temp_home.join("profiles").join("persisted-profile");
        let env_workspace_dir = temp_home.join("env-workspace");

        fs::create_dir_all(&marker_config_dir).await.unwrap();
        fs::write(
            marker_config_dir.join("config.toml"),
            "default_temperature = 0.7\ndefault_model = \"marker-model\"\n",
        )
        .await
        .unwrap();

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);
        persist_active_workspace_config_dir(&marker_config_dir)
            .await
            .unwrap();
        std::env::set_var("PLAW_WORKSPACE", &env_workspace_dir);

        let config = Config::load_or_init().await.unwrap();

        assert_eq!(config.workspace_dir, env_workspace_dir.join("workspace"));
        assert_eq!(config.config_path, env_workspace_dir.join("config.toml"));

        std::env::remove_var("PLAW_WORKSPACE");
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    // Unix-only: relies on HOME env var to override the home dir.
    // Windows uses USERPROFILE (and dirs::home_dir doesn't read HOME),
    // so the test's `set_var("HOME", ...)` doesn't redirect the marker
    // path. Cross-platform fix would need both env vars + a deeper
    // home-dir override hook in production code.
    #[cfg(unix)]
    #[test]
    async fn persist_active_workspace_marker_is_cleared_for_default_config_dir() {
        let _env_guard = env_override_lock().await;
        let temp_home =
            std::env::temp_dir().join(format!("plaw_test_home_{}", uuid::Uuid::new_v4()));
        let default_config_dir = temp_home.join(".plaw");
        let custom_config_dir = temp_home.join("profiles").join("custom-profile");
        let marker_path = default_config_dir.join(ACTIVE_WORKSPACE_STATE_FILE);

        let original_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &temp_home);

        persist_active_workspace_config_dir(&custom_config_dir)
            .await
            .unwrap();
        assert!(marker_path.exists());

        persist_active_workspace_config_dir(&default_config_dir)
            .await
            .unwrap();
        assert!(!marker_path.exists());

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        let _ = fs::remove_dir_all(temp_home).await;
    }

    #[test]
    async fn env_override_empty_values_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        let original_provider = config.default_provider.clone();

        std::env::set_var("PLAW_PROVIDER", "");
        config.apply_env_overrides();
        assert_eq!(config.default_provider, original_provider);

        std::env::remove_var("PLAW_PROVIDER");
    }

    #[test]
    async fn env_override_gateway_port() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.gateway.port, 42617);

        std::env::set_var("PLAW_GATEWAY_PORT", "8080");
        config.apply_env_overrides();
        assert_eq!(config.gateway.port, 8080);

        std::env::remove_var("PLAW_GATEWAY_PORT");
    }

    #[test]
    async fn env_override_port_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("PLAW_GATEWAY_PORT");
        std::env::set_var("PORT", "9000");
        config.apply_env_overrides();
        assert_eq!(config.gateway.port, 9000);

        std::env::remove_var("PORT");
    }

    #[test]
    async fn env_override_gateway_host() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.gateway.host, "127.0.0.1");

        std::env::set_var("PLAW_GATEWAY_HOST", "0.0.0.0");
        config.apply_env_overrides();
        assert_eq!(config.gateway.host, "0.0.0.0");

        std::env::remove_var("PLAW_GATEWAY_HOST");
    }

    #[test]
    async fn env_override_host_fallback() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::remove_var("PLAW_GATEWAY_HOST");
        std::env::set_var("HOST", "0.0.0.0");
        config.apply_env_overrides();
        assert_eq!(config.gateway.host, "0.0.0.0");

        std::env::remove_var("HOST");
    }

    #[test]
    async fn env_override_temperature() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("PLAW_TEMPERATURE", "0.5");
        config.apply_env_overrides();
        assert!((config.default_temperature - 0.5).abs() < f64::EPSILON);

        std::env::remove_var("PLAW_TEMPERATURE");
    }

    #[test]
    async fn env_override_temperature_out_of_range_ignored() {
        let _env_guard = env_override_lock().await;
        // Clean up any leftover env vars from other tests
        std::env::remove_var("PLAW_TEMPERATURE");

        let mut config = Config::default();
        let original_temp = config.default_temperature;

        // Temperature > 2.0 should be ignored
        std::env::set_var("PLAW_TEMPERATURE", "3.0");
        config.apply_env_overrides();
        assert!(
            (config.default_temperature - original_temp).abs() < f64::EPSILON,
            "Temperature 3.0 should be ignored (out of range)"
        );

        std::env::remove_var("PLAW_TEMPERATURE");
    }

    #[test]
    async fn env_override_reasoning_enabled() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.runtime.reasoning_enabled, None);

        std::env::set_var("PLAW_REASONING_ENABLED", "false");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_enabled, Some(false));

        std::env::set_var("PLAW_REASONING_ENABLED", "true");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_enabled, Some(true));

        std::env::remove_var("PLAW_REASONING_ENABLED");
    }

    #[test]
    async fn env_override_reasoning_invalid_value_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        config.runtime.reasoning_enabled = Some(false);

        std::env::set_var("PLAW_REASONING_ENABLED", "maybe");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_enabled, Some(false));

        std::env::remove_var("PLAW_REASONING_ENABLED");
    }

    #[test]
    async fn env_override_reasoning_level_alias() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.runtime.reasoning_level, None);

        std::env::set_var("PLAW_REASONING_LEVEL", "xhigh");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_level.as_deref(), Some("xhigh"));
        assert_eq!(
            config.effective_provider_reasoning_level().as_deref(),
            Some("xhigh")
        );

        std::env::remove_var("PLAW_REASONING_LEVEL");
    }

    #[test]
    async fn env_override_reasoning_level_alias_invalid_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        config.runtime.reasoning_level = Some("medium".to_string());

        std::env::set_var("PLAW_REASONING_LEVEL", "invalid");
        config.apply_env_overrides();
        assert_eq!(config.runtime.reasoning_level.as_deref(), Some("medium"));

        std::env::remove_var("PLAW_REASONING_LEVEL");
    }

    #[test]
    async fn env_override_model_support_vision() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        assert_eq!(config.model_support_vision, None);

        std::env::set_var("PLAW_MODEL_SUPPORT_VISION", "true");
        config.apply_env_overrides();
        assert_eq!(config.model_support_vision, Some(true));

        std::env::set_var("PLAW_MODEL_SUPPORT_VISION", "false");
        config.apply_env_overrides();
        assert_eq!(config.model_support_vision, Some(false));

        std::env::set_var("PLAW_MODEL_SUPPORT_VISION", "maybe");
        config.model_support_vision = Some(true);
        config.apply_env_overrides();
        assert_eq!(config.model_support_vision, Some(true));

        std::env::remove_var("PLAW_MODEL_SUPPORT_VISION");
    }

    #[test]
    async fn env_override_invalid_port_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        let original_port = config.gateway.port;

        std::env::set_var("PORT", "not_a_number");
        config.apply_env_overrides();
        assert_eq!(config.gateway.port, original_port);

        std::env::remove_var("PORT");
    }

    #[test]
    async fn env_override_web_search_config() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("WEB_SEARCH_ENABLED", "false");
        std::env::set_var("WEB_SEARCH_PROVIDER", "brave");
        std::env::set_var("WEB_SEARCH_MAX_RESULTS", "7");
        std::env::set_var("WEB_SEARCH_TIMEOUT_SECS", "20");
        std::env::set_var("BRAVE_API_KEY", "brave-test-key");

        config.apply_env_overrides();

        assert!(!config.web_search.enabled);
        assert_eq!(config.web_search.provider, "brave");
        assert_eq!(config.web_search.max_results, 7);
        assert_eq!(config.web_search.timeout_secs, 20);
        assert_eq!(
            config.web_search.brave_api_key.as_deref(),
            Some("brave-test-key")
        );

        std::env::remove_var("WEB_SEARCH_ENABLED");
        std::env::remove_var("WEB_SEARCH_PROVIDER");
        std::env::remove_var("WEB_SEARCH_MAX_RESULTS");
        std::env::remove_var("WEB_SEARCH_TIMEOUT_SECS");
        std::env::remove_var("BRAVE_API_KEY");
    }

    #[test]
    async fn env_override_web_search_invalid_values_ignored() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();
        let original_max_results = config.web_search.max_results;
        let original_timeout = config.web_search.timeout_secs;

        std::env::set_var("WEB_SEARCH_MAX_RESULTS", "99");
        std::env::set_var("WEB_SEARCH_TIMEOUT_SECS", "0");

        config.apply_env_overrides();

        assert_eq!(config.web_search.max_results, original_max_results);
        assert_eq!(config.web_search.timeout_secs, original_timeout);

        std::env::remove_var("WEB_SEARCH_MAX_RESULTS");
        std::env::remove_var("WEB_SEARCH_TIMEOUT_SECS");
    }

    #[test]
    async fn env_override_storage_provider_config() {
        let _env_guard = env_override_lock().await;
        let mut config = Config::default();

        std::env::set_var("PLAW_STORAGE_PROVIDER", "postgres");
        std::env::set_var("PLAW_STORAGE_DB_URL", "postgres://example/db");
        std::env::set_var("PLAW_STORAGE_CONNECT_TIMEOUT_SECS", "15");

        config.apply_env_overrides();

        assert_eq!(config.storage.provider.config.provider, "postgres");
        assert_eq!(
            config.storage.provider.config.db_url.as_deref(),
            Some("postgres://example/db")
        );
        assert_eq!(
            config.storage.provider.config.connect_timeout_secs,
            Some(15)
        );

        std::env::remove_var("PLAW_STORAGE_PROVIDER");
        std::env::remove_var("PLAW_STORAGE_DB_URL");
        std::env::remove_var("PLAW_STORAGE_CONNECT_TIMEOUT_SECS");
    }

    #[test]
    async fn proxy_config_scope_services_requires_entries_when_enabled() {
        let proxy = ProxyConfig {
            enabled: true,
            http_proxy: Some("http://127.0.0.1:7890".into()),
            https_proxy: None,
            all_proxy: None,
            no_proxy: Vec::new(),
            scope: ProxyScope::Services,
            services: Vec::new(),
        };

        let error = proxy.validate().unwrap_err().to_string();
        assert!(error.contains("proxy.scope='services'"));
    }

    #[test]
    async fn env_override_proxy_scope_services() {
        let _env_guard = env_override_lock().await;
        clear_proxy_env_test_vars();

        let mut config = Config::default();
        std::env::set_var("PLAW_PROXY_ENABLED", "true");
        std::env::set_var("PLAW_HTTP_PROXY", "http://127.0.0.1:7890");
        std::env::set_var("PLAW_PROXY_SERVICES", "provider.openai, tool.http_request");
        std::env::set_var("PLAW_PROXY_SCOPE", "services");

        config.apply_env_overrides();

        assert!(config.proxy.enabled);
        assert_eq!(config.proxy.scope, ProxyScope::Services);
        assert_eq!(
            config.proxy.http_proxy.as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert!(config.proxy.should_apply_to_service("provider.openai"));
        assert!(config.proxy.should_apply_to_service("tool.http_request"));
        assert!(!config.proxy.should_apply_to_service("provider.anthropic"));

        clear_proxy_env_test_vars();
    }

    #[test]
    async fn env_override_proxy_scope_environment_applies_process_env() {
        let _env_guard = env_override_lock().await;
        clear_proxy_env_test_vars();

        let mut config = Config::default();
        std::env::set_var("PLAW_PROXY_ENABLED", "true");
        std::env::set_var("PLAW_PROXY_SCOPE", "environment");
        std::env::set_var("PLAW_HTTP_PROXY", "http://127.0.0.1:7890");
        std::env::set_var("PLAW_HTTPS_PROXY", "http://127.0.0.1:7891");
        std::env::set_var("PLAW_NO_PROXY", "localhost,127.0.0.1");

        config.apply_env_overrides();

        assert_eq!(config.proxy.scope, ProxyScope::Environment);
        assert_eq!(
            std::env::var("HTTP_PROXY").ok().as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            std::env::var("HTTPS_PROXY").ok().as_deref(),
            Some("http://127.0.0.1:7891")
        );
        assert!(std::env::var("NO_PROXY")
            .ok()
            .is_some_and(|value| value.contains("localhost")));

        clear_proxy_env_test_vars();
    }

    fn runtime_proxy_cache_contains(cache_key: &str) -> bool {
        match runtime_proxy_client_cache().read() {
            Ok(guard) => guard.contains_key(cache_key),
            Err(poisoned) => poisoned.into_inner().contains_key(cache_key),
        }
    }

    #[test]
    async fn runtime_proxy_client_cache_reuses_default_profile_key() {
        let service_key = format!(
            "provider.cache_test.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        );
        let cache_key = runtime_proxy_cache_key(&service_key, None, None);

        clear_runtime_proxy_client_cache();
        assert!(!runtime_proxy_cache_contains(&cache_key));

        let _ = build_runtime_proxy_client(&service_key);
        assert!(runtime_proxy_cache_contains(&cache_key));

        let _ = build_runtime_proxy_client(&service_key);
        assert!(runtime_proxy_cache_contains(&cache_key));
    }

    #[test]
    async fn set_runtime_proxy_config_clears_runtime_proxy_client_cache() {
        let service_key = format!(
            "provider.cache_timeout_test.{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        );
        let cache_key = runtime_proxy_cache_key(&service_key, Some(30), Some(5));

        clear_runtime_proxy_client_cache();
        let _ = build_runtime_proxy_client_with_timeouts(&service_key, 30, 5);
        assert!(runtime_proxy_cache_contains(&cache_key));

        set_runtime_proxy_config(ProxyConfig::default());
        assert!(!runtime_proxy_cache_contains(&cache_key));
    }

    #[test]
    async fn gateway_config_default_values() {
        let g = GatewayConfig::default();
        assert_eq!(g.port, 42617);
        assert_eq!(g.host, "127.0.0.1");
        assert!(g.require_pairing);
        assert!(!g.allow_public_bind);
        assert!(g.paired_tokens.is_empty());
        assert!(!g.trust_forwarded_headers);
        assert_eq!(g.rate_limit_max_keys, 10_000);
        assert_eq!(g.idempotency_max_keys, 10_000);
        assert!(!g.node_control.enabled);
        assert!(g.node_control.auth_token.is_none());
        assert!(g.node_control.allowed_node_ids.is_empty());
    }

    // ── Peripherals config ───────────────────────────────────────

    #[test]
    async fn peripherals_config_default_disabled() {
        let p = PeripheralsConfig::default();
        assert!(!p.enabled);
        assert!(p.boards.is_empty());
    }

    #[test]
    async fn peripheral_board_config_defaults() {
        let b = PeripheralBoardConfig::default();
        assert!(b.board.is_empty());
        assert_eq!(b.transport, "serial");
        assert!(b.path.is_none());
        assert_eq!(b.baud, 115_200);
    }

    #[test]
    async fn peripherals_config_toml_roundtrip() {
        let p = PeripheralsConfig {
            enabled: true,
            boards: vec![PeripheralBoardConfig {
                board: "nucleo-f401re".into(),
                transport: "serial".into(),
                path: Some("/dev/ttyACM0".into()),
                baud: 115_200,
            }],
            datasheet_dir: None,
        };
        let toml_str = toml::to_string(&p).unwrap();
        let parsed: PeripheralsConfig = toml::from_str(&toml_str).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.boards.len(), 1);
        assert_eq!(parsed.boards[0].board, "nucleo-f401re");
        assert_eq!(parsed.boards[0].path.as_deref(), Some("/dev/ttyACM0"));
    }

    #[test]
    async fn lark_config_serde() {
        let lc = LarkConfig {
            app_id: "cli_123456".into(),
            app_secret: crate::security::Secret::from_wire("secret_abc".into()),
            encrypt_key: Some(crate::security::Secret::from_wire("encrypt_key".into())),
            verification_token: Some(crate::security::Secret::from_wire("verify_token".into())),
            allowed_users: vec!["user_123".into(), "user_456".into()],
            mention_only: false,
            group_reply: None,
            use_feishu: true,
            receive_mode: LarkReceiveMode::Websocket,
            port: None,
            draft_update_interval_ms: default_lark_draft_update_interval_ms(),
            max_draft_edits: default_lark_max_draft_edits(),
        };
        let json = serde_json::to_string(&lc).unwrap();
        let parsed: LarkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_id, "cli_123456");
        assert_eq!(parsed.app_secret.as_wire_str(), "secret_abc");
        assert_eq!(
            parsed.encrypt_key.as_ref().map(|s| s.as_wire_str()),
            Some("encrypt_key")
        );
        assert_eq!(
            parsed.verification_token.as_ref().map(|s| s.as_wire_str()),
            Some("verify_token")
        );
        assert_eq!(parsed.allowed_users.len(), 2);
        assert!(parsed.use_feishu);
    }

    #[test]
    async fn lark_config_toml_roundtrip() {
        let lc = LarkConfig {
            app_id: "cli_123456".into(),
            app_secret: crate::security::Secret::from_wire("secret_abc".into()),
            encrypt_key: Some(crate::security::Secret::from_wire("encrypt_key".into())),
            verification_token: Some(crate::security::Secret::from_wire("verify_token".into())),
            allowed_users: vec!["*".into()],
            mention_only: false,
            group_reply: None,
            use_feishu: false,
            receive_mode: LarkReceiveMode::Webhook,
            port: Some(9898),
            draft_update_interval_ms: default_lark_draft_update_interval_ms(),
            max_draft_edits: default_lark_max_draft_edits(),
        };
        let toml_str = toml::to_string(&lc).unwrap();
        let parsed: LarkConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.app_id, "cli_123456");
        assert_eq!(parsed.app_secret.as_wire_str(), "secret_abc");
        assert!(!parsed.use_feishu);
    }

    #[test]
    async fn lark_config_deserializes_without_optional_fields() {
        let json = r#"{"app_id":"cli_123","app_secret":"secret"}"#;
        let parsed: LarkConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.encrypt_key.is_none());
        assert!(parsed.verification_token.is_none());
        assert!(parsed.allowed_users.is_empty());
        assert!(!parsed.mention_only);
        assert!(!parsed.use_feishu);
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::AllMessages
        );
    }

    #[test]
    async fn lark_config_defaults_to_lark_endpoint() {
        let json = r#"{"app_id":"cli_123","app_secret":"secret"}"#;
        let parsed: LarkConfig = serde_json::from_str(json).unwrap();
        assert!(
            !parsed.use_feishu,
            "use_feishu should default to false (Lark)"
        );
    }

    #[test]
    async fn lark_config_with_wildcard_allowed_users() {
        let json = r#"{"app_id":"cli_123","app_secret":"secret","allowed_users":["*"]}"#;
        let parsed: LarkConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.allowed_users, vec!["*"]);
    }

    #[test]
    async fn lark_group_reply_mode_overrides_legacy_mention_only() {
        let json = r#"{
            "app_id":"cli_123",
            "app_secret":"secret",
            "mention_only":true,
            "group_reply":{
                "mode":"all_messages",
                "allowed_sender_ids":["ou_1"]
            }
        }"#;
        let parsed: LarkConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::AllMessages
        );
        assert_eq!(
            parsed.group_reply_allowed_sender_ids(),
            vec!["ou_1".to_string()]
        );
    }

    #[test]
    async fn feishu_config_serde() {
        let fc = FeishuConfig {
            app_id: "cli_feishu_123".into(),
            app_secret: crate::security::Secret::from_wire("secret_abc".into()),
            encrypt_key: Some(crate::security::Secret::from_wire("encrypt_key".into())),
            verification_token: Some(crate::security::Secret::from_wire("verify_token".into())),
            allowed_users: vec!["user_123".into(), "user_456".into()],
            group_reply: None,
            receive_mode: LarkReceiveMode::Websocket,
            port: None,
            draft_update_interval_ms: default_lark_draft_update_interval_ms(),
            max_draft_edits: default_lark_max_draft_edits(),
        };
        let json = serde_json::to_string(&fc).unwrap();
        let parsed: FeishuConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.app_id, "cli_feishu_123");
        assert_eq!(parsed.app_secret.as_wire_str(), "secret_abc");
        assert_eq!(
            parsed.encrypt_key.as_ref().map(|s| s.as_wire_str()),
            Some("encrypt_key")
        );
        assert_eq!(
            parsed.verification_token.as_ref().map(|s| s.as_wire_str()),
            Some("verify_token")
        );
        assert_eq!(parsed.allowed_users.len(), 2);
    }

    #[test]
    async fn feishu_config_toml_roundtrip() {
        let fc = FeishuConfig {
            app_id: "cli_feishu_123".into(),
            app_secret: crate::security::Secret::from_wire("secret_abc".into()),
            encrypt_key: Some(crate::security::Secret::from_wire("encrypt_key".into())),
            verification_token: Some(crate::security::Secret::from_wire("verify_token".into())),
            allowed_users: vec!["*".into()],
            group_reply: None,
            receive_mode: LarkReceiveMode::Webhook,
            port: Some(9898),
            draft_update_interval_ms: default_lark_draft_update_interval_ms(),
            max_draft_edits: default_lark_max_draft_edits(),
        };
        let toml_str = toml::to_string(&fc).unwrap();
        let parsed: FeishuConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.app_id, "cli_feishu_123");
        assert_eq!(parsed.app_secret.as_wire_str(), "secret_abc");
        assert_eq!(parsed.receive_mode, LarkReceiveMode::Webhook);
        assert_eq!(parsed.port, Some(9898));
    }

    #[test]
    async fn feishu_config_deserializes_without_optional_fields() {
        let json = r#"{"app_id":"cli_123","app_secret":"secret"}"#;
        let parsed: FeishuConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.encrypt_key.is_none());
        assert!(parsed.verification_token.is_none());
        assert!(parsed.allowed_users.is_empty());
        assert_eq!(parsed.receive_mode, LarkReceiveMode::Websocket);
        assert!(parsed.port.is_none());
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::AllMessages
        );
    }

    #[test]
    async fn feishu_group_reply_mode_supports_mention_only() {
        let json = r#"{
            "app_id":"cli_123",
            "app_secret":"secret",
            "group_reply":{
                "mode":"mention_only",
                "allowed_sender_ids":["ou_9"]
            }
        }"#;
        let parsed: FeishuConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            parsed.effective_group_reply_mode(),
            GroupReplyMode::MentionOnly
        );
        assert_eq!(
            parsed.group_reply_allowed_sender_ids(),
            vec!["ou_9".to_string()]
        );
    }

    #[test]
    async fn qq_config_defaults_to_webhook_receive_mode() {
        let json = r#"{"app_id":"123","app_secret":"secret"}"#;
        let parsed: QQConfig = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.receive_mode, QQReceiveMode::Webhook);
        assert!(parsed.allowed_users.is_empty());
    }

    #[test]
    async fn qq_config_toml_roundtrip_receive_mode() {
        let qc = QQConfig {
            app_id: "123".into(),
            app_secret: crate::security::Secret::from_wire("secret".into()),
            allowed_users: vec!["*".into()],
            receive_mode: QQReceiveMode::Websocket,
            webhook_secret: None,
        };
        let toml_str = toml::to_string(&qc).unwrap();
        let parsed: QQConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.receive_mode, QQReceiveMode::Websocket);
        assert_eq!(parsed.allowed_users, vec!["*"]);
    }

    #[test]
    async fn nextcloud_talk_config_serde() {
        let nc = NextcloudTalkConfig {
            base_url: "https://cloud.example.com".into(),
            app_token: crate::security::Secret::from_wire("app-token".into()),
            webhook_secret: Some(crate::security::Secret::from_wire("webhook-secret".into())),
            allowed_users: vec!["user_a".into(), "*".into()],
        };

        let json = serde_json::to_string(&nc).unwrap();
        let parsed: NextcloudTalkConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.base_url, "https://cloud.example.com");
        assert_eq!(parsed.app_token.as_wire_str(), "app-token");
        assert_eq!(
            parsed.webhook_secret.as_ref().map(|s| s.as_wire_str()),
            Some("webhook-secret")
        );
        assert_eq!(parsed.allowed_users, vec!["user_a", "*"]);
    }

    #[test]
    async fn nextcloud_talk_config_defaults_optional_fields() {
        let json = r#"{"base_url":"https://cloud.example.com","app_token":"app-token"}"#;
        let parsed: NextcloudTalkConfig = serde_json::from_str(json).unwrap();
        assert!(parsed.webhook_secret.is_none());
        assert!(parsed.allowed_users.is_empty());
    }

    // ── Config file permission hardening (Unix only) ───────────────

    #[cfg(unix)]
    #[test]
    async fn new_config_file_has_restricted_permissions() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");

        // Create a config and save it
        let mut config = Config::default();
        config.config_path = config_path.clone();
        config.save().await.unwrap();

        let meta = fs::metadata(&config_path).await.unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "New config file should be owner-only (0600), got {mode:o}"
        );
    }

    #[cfg(unix)]
    #[test]
    async fn save_restricts_existing_world_readable_config_to_owner_only() {
        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");

        let mut config = Config::default();
        config.config_path = config_path.clone();
        config.save().await.unwrap();

        // Simulate the regression state observed in issue #1345.
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loose_mode = std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            loose_mode, 0o644,
            "test setup requires world-readable config"
        );

        config.default_temperature = 0.6;
        config.save().await.unwrap();

        let hardened_mode = std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            hardened_mode, 0o600,
            "Saving config should restore owner-only permissions (0600)"
        );
    }

    #[cfg(unix)]
    #[test]
    async fn world_readable_config_is_detectable() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::TempDir::new().unwrap();
        let config_path = tmp.path().join("config.toml");

        // Create a config file with intentionally loose permissions
        std::fs::write(&config_path, "# test config").unwrap();
        std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let meta = std::fs::metadata(&config_path).unwrap();
        let mode = meta.permissions().mode();
        assert!(
            mode & 0o004 != 0,
            "Test setup: file should be world-readable (mode {mode:o})"
        );
    }

    #[test]
    async fn transcription_config_defaults() {
        let tc = TranscriptionConfig::default();
        assert!(!tc.enabled);
        assert!(tc.api_url.contains("groq.com"));
        assert_eq!(tc.model, "whisper-large-v3-turbo");
        assert!(tc.language.is_none());
        assert_eq!(tc.max_duration_secs, 120);
    }

    #[test]
    async fn config_roundtrip_with_transcription() {
        let mut config = Config::default();
        config.transcription.enabled = true;
        config.transcription.language = Some("en".into());

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();

        assert!(parsed.transcription.enabled);
        assert_eq!(parsed.transcription.language.as_deref(), Some("en"));
        assert_eq!(parsed.transcription.model, "whisper-large-v3-turbo");
    }

    #[test]
    async fn config_without_transcription_uses_defaults() {
        let toml_str = r#"
            default_provider = "openrouter"
            default_model = "test-model"
            default_temperature = 0.7
        "#;
        let parsed: Config = toml::from_str(toml_str).unwrap();
        assert!(!parsed.transcription.enabled);
        assert_eq!(parsed.transcription.max_duration_secs, 120);
    }

    #[test]
    async fn security_defaults_are_backward_compatible() {
        let parsed: Config = toml::from_str(
            r#"
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4.6"
default_temperature = 0.7
"#,
        )
        .unwrap();

        assert!(!parsed.security.otp.enabled);
        assert_eq!(parsed.security.otp.method, OtpMethod::Totp);
        assert!(!parsed.security.estop.enabled);
        assert!(parsed.security.estop.require_otp_to_resume);
        assert!(parsed.security.syscall_anomaly.enabled);
        assert!(parsed.security.syscall_anomaly.alert_on_unknown_syscall);
        assert!(!parsed.security.syscall_anomaly.baseline_syscalls.is_empty());
    }

    #[test]
    async fn security_toml_parses_otp_and_estop_sections() {
        let parsed: Config = toml::from_str(
            r#"
default_provider = "openrouter"
default_model = "anthropic/claude-sonnet-4.6"
default_temperature = 0.7

[security.otp]
enabled = true
method = "totp"
token_ttl_secs = 30
cache_valid_secs = 120
gated_actions = ["shell", "browser_open"]
gated_domains = ["*.chase.com", "accounts.google.com"]
gated_domain_categories = ["banking"]

[security.estop]
enabled = true
state_file = "~/.plaw/estop-state.json"
require_otp_to_resume = true

[security.syscall_anomaly]
enabled = true
strict_mode = true
alert_on_unknown_syscall = true
max_denied_events_per_minute = 3
max_total_events_per_minute = 60
max_alerts_per_minute = 10
alert_cooldown_secs = 15
log_path = "syscall-anomalies.log"
baseline_syscalls = ["read", "write", "openat", "close"]
"#,
        )
        .unwrap();

        assert!(parsed.security.otp.enabled);
        assert!(parsed.security.estop.enabled);
        assert!(parsed.security.syscall_anomaly.strict_mode);
        assert_eq!(
            parsed.security.syscall_anomaly.max_denied_events_per_minute,
            3
        );
        assert_eq!(
            parsed.security.syscall_anomaly.max_total_events_per_minute,
            60
        );
        assert_eq!(parsed.security.syscall_anomaly.max_alerts_per_minute, 10);
        assert_eq!(parsed.security.syscall_anomaly.alert_cooldown_secs, 15);
        assert_eq!(parsed.security.syscall_anomaly.baseline_syscalls.len(), 4);
        assert_eq!(parsed.security.otp.gated_actions.len(), 2);
        assert_eq!(parsed.security.otp.gated_domains.len(), 2);
        parsed.validate().unwrap();
    }

    #[test]
    async fn security_validation_rejects_invalid_domain_glob() {
        let mut config = Config::default();
        config.security.otp.gated_domains = vec!["bad domain.com".into()];

        let err = config.validate().expect_err("expected invalid domain glob");
        assert!(err.to_string().contains("gated_domains"));
    }

    #[test]
    async fn security_validation_rejects_unknown_domain_category() {
        let mut config = Config::default();
        config.security.otp.gated_domain_categories = vec!["not_real".into()];

        let err = config
            .validate()
            .expect_err("expected unknown domain category");
        assert!(err.to_string().contains("gated_domain_categories"));
    }

    #[test]
    async fn security_validation_rejects_zero_token_ttl() {
        let mut config = Config::default();
        config.security.otp.token_ttl_secs = 0;

        let err = config
            .validate()
            .expect_err("expected ttl validation failure");
        assert!(err.to_string().contains("token_ttl_secs"));
    }

    #[test]
    async fn security_validation_rejects_zero_syscall_threshold() {
        let mut config = Config::default();
        config.security.syscall_anomaly.max_denied_events_per_minute = 0;

        let err = config
            .validate()
            .expect_err("expected syscall threshold validation failure");
        assert!(err.to_string().contains("max_denied_events_per_minute"));
    }

    #[test]
    async fn security_validation_rejects_invalid_syscall_baseline_name() {
        let mut config = Config::default();
        config.security.syscall_anomaly.baseline_syscalls =
            vec!["openat".into(), "bad name".into()];

        let err = config
            .validate()
            .expect_err("expected syscall baseline name validation failure");
        assert!(err.to_string().contains("baseline_syscalls"));
    }

    #[test]
    async fn security_validation_rejects_zero_syscall_alert_budget() {
        let mut config = Config::default();
        config.security.syscall_anomaly.max_alerts_per_minute = 0;

        let err = config
            .validate()
            .expect_err("expected syscall alert budget validation failure");
        assert!(err.to_string().contains("max_alerts_per_minute"));
    }

    #[test]
    async fn security_validation_rejects_zero_syscall_cooldown() {
        let mut config = Config::default();
        config.security.syscall_anomaly.alert_cooldown_secs = 0;

        let err = config
            .validate()
            .expect_err("expected syscall cooldown validation failure");
        assert!(err.to_string().contains("alert_cooldown_secs"));
    }

    #[test]
    async fn security_validation_rejects_denied_threshold_above_total_threshold() {
        let mut config = Config::default();
        config.security.syscall_anomaly.max_denied_events_per_minute = 10;
        config.security.syscall_anomaly.max_total_events_per_minute = 5;

        let err = config
            .validate()
            .expect_err("expected syscall threshold ordering validation failure");
        assert!(err
            .to_string()
            .contains("max_denied_events_per_minute must be less than or equal"));
    }

    #[test]
    async fn coordination_config_defaults() {
        let config = Config::default();
        assert!(config.coordination.enabled);
        assert_eq!(config.coordination.lead_agent, "delegate-lead");
        assert_eq!(config.coordination.max_inbox_messages_per_agent, 256);
        assert_eq!(config.coordination.max_dead_letters, 256);
        assert_eq!(config.coordination.max_context_entries, 512);
        assert_eq!(config.coordination.max_seen_message_ids, 4096);
    }

    #[test]
    async fn config_roundtrip_with_coordination_section() {
        let mut config = Config::default();
        config.coordination.enabled = true;
        config.coordination.lead_agent = "runtime-lead".into();
        config.coordination.max_inbox_messages_per_agent = 128;
        config.coordination.max_dead_letters = 64;
        config.coordination.max_context_entries = 32;
        config.coordination.max_seen_message_ids = 1024;

        let toml_str = toml::to_string_pretty(&config).unwrap();
        let parsed: Config = toml::from_str(&toml_str).unwrap();
        assert!(parsed.coordination.enabled);
        assert_eq!(parsed.coordination.lead_agent, "runtime-lead");
        assert_eq!(parsed.coordination.max_inbox_messages_per_agent, 128);
        assert_eq!(parsed.coordination.max_dead_letters, 64);
        assert_eq!(parsed.coordination.max_context_entries, 32);
        assert_eq!(parsed.coordination.max_seen_message_ids, 1024);
    }

    #[test]
    async fn coordination_validation_rejects_invalid_limits_and_lead_agent() {
        let mut config = Config::default();
        config.coordination.max_inbox_messages_per_agent = 0;
        let err = config
            .validate()
            .expect_err("expected coordination inbox limit validation failure");
        assert!(err
            .to_string()
            .contains("coordination.max_inbox_messages_per_agent"));

        let mut config = Config::default();
        config.coordination.max_dead_letters = 0;
        let err = config
            .validate()
            .expect_err("expected coordination dead-letter limit validation failure");
        assert!(err.to_string().contains("coordination.max_dead_letters"));

        let mut config = Config::default();
        config.coordination.max_context_entries = 0;
        let err = config
            .validate()
            .expect_err("expected coordination context limit validation failure");
        assert!(err.to_string().contains("coordination.max_context_entries"));

        let mut config = Config::default();
        config.coordination.max_seen_message_ids = 0;
        let err = config
            .validate()
            .expect_err("expected coordination dedupe-window validation failure");
        assert!(err
            .to_string()
            .contains("coordination.max_seen_message_ids"));

        let mut config = Config::default();
        config.coordination.lead_agent = "   ".into();
        let err = config
            .validate()
            .expect_err("expected coordination lead-agent validation failure");
        assert!(err.to_string().contains("coordination.lead_agent"));
    }

    #[test]
    async fn coordination_validation_allows_empty_lead_agent_when_disabled() {
        let mut config = Config::default();
        config.coordination.enabled = false;
        config.coordination.lead_agent = String::new();
        config
            .validate()
            .expect("disabled coordination should allow empty lead agent");
    }

    #[test]
    async fn repo_map_config_defaults_to_disabled() {
        let cfg = Config::default();
        assert!(!cfg.repo_map.enabled, "repo_map must default to disabled");
        assert_eq!(cfg.repo_map.max_tokens, 1024);
        assert!(cfg.repo_map.root.is_none());
    }

    #[test]
    async fn repo_map_config_round_trips_through_toml() {
        let toml_src = r#"
enabled = true
max_tokens = 2048
root = "/tmp/my-project"
"#;
        let parsed: RepoMapConfig = toml::from_str(toml_src).expect("toml parse");
        assert!(parsed.enabled);
        assert_eq!(parsed.max_tokens, 2048);
        assert_eq!(
            parsed.root.as_deref(),
            Some(std::path::Path::new("/tmp/my-project"))
        );
    }

    #[test]
    async fn repo_map_config_partial_toml_fills_defaults() {
        // Only `enabled` set — the other two fields fall back to their defaults.
        let parsed: RepoMapConfig = toml::from_str("enabled = true\n").expect("toml parse");
        assert!(parsed.enabled);
        assert_eq!(parsed.max_tokens, 1024);
        assert!(parsed.root.is_none());
    }
}
