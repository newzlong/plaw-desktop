//! Runtime adapter configuration (`[runtime]` section).
//!
//! Extracted from `config/schema/mod.rs` on 2026-05-24 as the third
//! focused slice of the 10K-LoC `schema.rs` mega-file split (audit
//! Top-4 #3b — see [[project-2026-05-23-four-lens-synthesis]]).
//!
//! Covers `RuntimeConfig`, `DockerRuntimeConfig`, `WasmRuntimeConfig`,
//! `WasmSecurityConfig`, and the two Wasm policy enums. Public items
//! are re-exported from `config::schema` via `pub use runtime::*` so
//! `crate::config::RuntimeConfig` and friends keep working without
//! consumer churn.

use super::default_true;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Runtime ──────────────────────────────────────────────────────

/// Runtime adapter configuration (`[runtime]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeConfig {
    /// Runtime kind (`native` | `docker` | `wasm`).
    #[serde(default = "default_runtime_kind")]
    pub kind: String,

    /// Docker runtime settings (used when `kind = "docker"`).
    #[serde(default)]
    pub docker: DockerRuntimeConfig,

    /// WASM runtime settings (used when `kind = "wasm"`).
    #[serde(default)]
    pub wasm: WasmRuntimeConfig,

    /// Global reasoning override for providers that expose explicit controls.
    /// - `None`: provider default behavior
    /// - `Some(true)`: request reasoning/thinking when supported
    /// - `Some(false)`: disable reasoning/thinking when supported
    #[serde(default)]
    pub reasoning_enabled: Option<bool>,

    /// Deprecated compatibility alias for `[provider].reasoning_level`.
    /// - Canonical key: `provider.reasoning_level`
    /// - Legacy key accepted for compatibility: `runtime.reasoning_level`
    /// - When both are set, provider-level value wins.
    #[serde(default)]
    pub reasoning_level: Option<String>,
}

/// Docker runtime configuration (`[runtime.docker]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DockerRuntimeConfig {
    /// Runtime image used to execute shell commands.
    #[serde(default = "default_docker_image")]
    pub image: String,

    /// Docker network mode (`none`, `bridge`, etc.).
    #[serde(default = "default_docker_network")]
    pub network: String,

    /// Optional memory limit in MB (`None` = no explicit limit).
    #[serde(default = "default_docker_memory_limit_mb")]
    pub memory_limit_mb: Option<u64>,

    /// Optional CPU limit (`None` = no explicit limit).
    #[serde(default = "default_docker_cpu_limit")]
    pub cpu_limit: Option<f64>,

    /// Mount root filesystem as read-only.
    #[serde(default = "default_true")]
    pub read_only_rootfs: bool,

    /// Mount configured workspace into `/workspace`.
    #[serde(default = "default_true")]
    pub mount_workspace: bool,

    /// Optional workspace root allowlist for Docker mount validation.
    #[serde(default)]
    pub allowed_workspace_roots: Vec<String>,
}

/// WASM runtime configuration (`[runtime.wasm]` section).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WasmRuntimeConfig {
    /// Workspace-relative directory that stores `.wasm` modules.
    #[serde(default = "default_wasm_tools_dir")]
    pub tools_dir: String,

    /// Fuel limit per invocation (instruction budget).
    #[serde(default = "default_wasm_fuel_limit")]
    pub fuel_limit: u64,

    /// Memory limit per invocation in MB.
    #[serde(default = "default_wasm_memory_limit_mb")]
    pub memory_limit_mb: u64,

    /// Maximum `.wasm` module size in MB.
    #[serde(default = "default_wasm_max_module_size_mb")]
    pub max_module_size_mb: u64,

    /// Allow reading files from workspace inside WASM host calls (future-facing).
    #[serde(default)]
    pub allow_workspace_read: bool,

    /// Allow writing files to workspace inside WASM host calls (future-facing).
    #[serde(default)]
    pub allow_workspace_write: bool,

    /// Explicit host allowlist for outbound HTTP from WASM modules (future-facing).
    #[serde(default)]
    pub allowed_hosts: Vec<String>,

    /// WASM runtime security controls (`[runtime.wasm.security]` section).
    #[serde(default)]
    pub security: WasmSecurityConfig,
}

/// How to handle invocation capabilities that exceed baseline runtime policy.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WasmCapabilityEscalationMode {
    /// Reject any invocation that asks for capabilities above runtime config.
    #[default]
    Deny,
    /// Automatically clamp invocation capabilities to runtime config ceilings.
    Clamp,
}

/// Integrity policy for WASM modules pinned by SHA-256 digest.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WasmModuleHashPolicy {
    /// Disable module hash validation.
    Disabled,
    /// Warn on missing or mismatched hashes, but allow execution.
    #[default]
    Warn,
    /// Require exact hash match before execution.
    Enforce,
}

/// Security policy controls for WASM runtime hardening.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct WasmSecurityConfig {
    /// Require `runtime.wasm.tools_dir` to stay workspace-relative and traversal-free.
    #[serde(default = "default_true")]
    pub require_workspace_relative_tools_dir: bool,

    /// Reject module files that are symlinks before execution.
    #[serde(default = "default_true")]
    pub reject_symlink_modules: bool,

    /// Reject `runtime.wasm.tools_dir` when it is itself a symlink.
    #[serde(default = "default_true")]
    pub reject_symlink_tools_dir: bool,

    /// Strictly validate host allowlist entries (`host` or `host:port` only).
    #[serde(default = "default_true")]
    pub strict_host_validation: bool,

    /// Capability escalation handling policy.
    #[serde(default)]
    pub capability_escalation_mode: WasmCapabilityEscalationMode,

    /// Module digest verification policy.
    #[serde(default)]
    pub module_hash_policy: WasmModuleHashPolicy,

    /// Optional pinned SHA-256 digest map keyed by module name (without `.wasm`).
    #[serde(default)]
    pub module_sha256: BTreeMap<String, String>,
}

fn default_runtime_kind() -> String {
    "native".into()
}

fn default_docker_image() -> String {
    "alpine:3.20".into()
}

fn default_docker_network() -> String {
    "none".into()
}

fn default_docker_memory_limit_mb() -> Option<u64> {
    Some(512)
}

fn default_docker_cpu_limit() -> Option<f64> {
    Some(1.0)
}

fn default_wasm_tools_dir() -> String {
    "tools/wasm".into()
}

fn default_wasm_fuel_limit() -> u64 {
    1_000_000
}

fn default_wasm_memory_limit_mb() -> u64 {
    64
}

fn default_wasm_max_module_size_mb() -> u64 {
    50
}

impl Default for DockerRuntimeConfig {
    fn default() -> Self {
        Self {
            image: default_docker_image(),
            network: default_docker_network(),
            memory_limit_mb: default_docker_memory_limit_mb(),
            cpu_limit: default_docker_cpu_limit(),
            read_only_rootfs: true,
            mount_workspace: true,
            allowed_workspace_roots: Vec::new(),
        }
    }
}

impl Default for WasmRuntimeConfig {
    fn default() -> Self {
        Self {
            tools_dir: default_wasm_tools_dir(),
            fuel_limit: default_wasm_fuel_limit(),
            memory_limit_mb: default_wasm_memory_limit_mb(),
            max_module_size_mb: default_wasm_max_module_size_mb(),
            allow_workspace_read: false,
            allow_workspace_write: false,
            allowed_hosts: Vec::new(),
            security: WasmSecurityConfig::default(),
        }
    }
}

impl Default for WasmSecurityConfig {
    fn default() -> Self {
        Self {
            require_workspace_relative_tools_dir: true,
            reject_symlink_modules: true,
            reject_symlink_tools_dir: true,
            strict_host_validation: true,
            capability_escalation_mode: WasmCapabilityEscalationMode::Deny,
            module_hash_policy: WasmModuleHashPolicy::Warn,
            module_sha256: BTreeMap::new(),
        }
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            kind: default_runtime_kind(),
            docker: DockerRuntimeConfig::default(),
            wasm: WasmRuntimeConfig::default(),
            reasoning_enabled: None,
            reasoning_level: None,
        }
    }
}
