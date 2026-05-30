//! Interactive approval workflow for supervised mode.
//!
//! Provides a pre-execution hook that prompts the user before tool calls,
//! with session-scoped "Always" allowlists and audit logging.

use crate::config::{AutonomyConfig, NonCliNaturalLanguageApprovalMode};
use crate::security::AutonomyLevel;
use chrono::{Duration, Utc};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Write};
use uuid::Uuid;

// ── Types ────────────────────────────────────────────────────────

/// A request to approve a tool call before execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub tool_name: String,
    pub arguments: serde_json::Value,
}

/// The user's response to an approval request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalResponse {
    /// Execute this one call.
    Yes,
    /// Deny this call.
    No,
    /// Execute and add tool to session-scoped allowlist.
    Always,
}

/// A single audit log entry for an approval decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalLogEntry {
    pub timestamp: String,
    pub tool_name: String,
    pub arguments_summary: String,
    pub decision: ApprovalResponse,
    pub channel: String,
}

/// A pending non-CLI approval request that still requires explicit confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingNonCliApprovalRequest {
    pub request_id: String,
    pub tool_name: String,
    pub requested_by: String,
    pub requested_channel: String,
    pub requested_reply_target: String,
    pub reason: Option<String>,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingApprovalError {
    NotFound,
    Expired,
    RequesterMismatch,
}

// ── ApprovalManager ──────────────────────────────────────────────

/// Manages the interactive approval workflow.
///
/// - Checks config-level `auto_approve` / `always_ask` lists
/// - Maintains a session-scoped "always" allowlist
/// - Records an audit trail of all decisions
pub struct ApprovalManager {
    /// Tools that never need approval (config + runtime updates).
    auto_approve: RwLock<HashSet<String>>,
    /// Tools that always need approval, ignoring session allowlist (config + runtime updates).
    always_ask: RwLock<HashSet<String>>,
    /// Autonomy level from config. Stage 6 collapse: the tier is no longer
    /// consulted by `needs_approval` — the action card + `auto_approve` is
    /// the sole gate. Retained on the struct so config snapshots / future
    /// hot-reload paths (and any out-of-tree consumers) can still read it.
    #[allow(dead_code)]
    autonomy_level: AutonomyLevel,
    /// Session-scoped allowlist built from "Always" responses.
    session_allowlist: Mutex<HashSet<String>>,
    /// Session-scoped allowlist for non-CLI channels after explicit human approval.
    non_cli_allowlist: Mutex<HashSet<String>>,
    /// One-time non-CLI bypass tokens that allow a full tool loop turn without prompts.
    non_cli_allow_all_once_remaining: Mutex<u32>,
    /// Optional allowlist of senders allowed to manage non-CLI approvals.
    non_cli_approval_approvers: RwLock<HashSet<String>>,
    /// Default natural-language handling mode for non-CLI approval-management commands.
    non_cli_natural_language_approval_mode: RwLock<NonCliNaturalLanguageApprovalMode>,
    /// Optional per-channel overrides for natural-language approval mode.
    non_cli_natural_language_approval_mode_by_channel:
        RwLock<HashMap<String, NonCliNaturalLanguageApprovalMode>>,
    /// Pending non-CLI approval requests awaiting explicit human confirmation.
    pending_non_cli_requests: Mutex<HashMap<String, PendingNonCliApprovalRequest>>,
    /// Resolved decision snapshots for pending non-CLI requests, consumed by
    /// waiting tool loops.
    resolved_non_cli_requests: Mutex<HashMap<String, ApprovalResponse>>,
    /// Audit trail of approval decisions.
    audit_log: Mutex<Vec<ApprovalLogEntry>>,
}

impl ApprovalManager {
    fn normalize_non_cli_approvers(entries: &[String]) -> HashSet<String> {
        entries
            .iter()
            .map(|entry| entry.trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect()
    }

    fn normalize_non_cli_natural_language_mode_by_channel(
        entries: &HashMap<String, NonCliNaturalLanguageApprovalMode>,
    ) -> HashMap<String, NonCliNaturalLanguageApprovalMode> {
        entries
            .iter()
            .filter_map(|(channel, mode)| {
                let normalized = channel.trim().to_ascii_lowercase();
                if normalized.is_empty() {
                    None
                } else {
                    Some((normalized, *mode))
                }
            })
            .collect()
    }

    /// Create from autonomy config.
    pub fn from_config(config: &AutonomyConfig) -> Self {
        Self {
            auto_approve: RwLock::new(config.auto_approve.iter().cloned().collect()),
            always_ask: RwLock::new(config.always_ask.iter().cloned().collect()),
            autonomy_level: config.level,
            session_allowlist: Mutex::new(HashSet::new()),
            non_cli_allowlist: Mutex::new(HashSet::new()),
            non_cli_allow_all_once_remaining: Mutex::new(0),
            non_cli_approval_approvers: RwLock::new(Self::normalize_non_cli_approvers(
                &config.non_cli_approval_approvers,
            )),
            non_cli_natural_language_approval_mode: RwLock::new(
                config.non_cli_natural_language_approval_mode,
            ),
            non_cli_natural_language_approval_mode_by_channel: RwLock::new(
                Self::normalize_non_cli_natural_language_mode_by_channel(
                    &config.non_cli_natural_language_approval_mode_by_channel,
                ),
            ),
            pending_non_cli_requests: Mutex::new(HashMap::new()),
            resolved_non_cli_requests: Mutex::new(HashMap::new()),
            audit_log: Mutex::new(Vec::new()),
        }
    }

    /// Check whether a tool call requires interactive approval.
    ///
    /// Returns `true` if the call needs a prompt, `false` if it can proceed.
    ///
    /// `args` is the tool's argument object. It is consulted only for
    /// `shell:<prefix>` command-prefix grants; whole-tool grants ignore it.
    ///
    /// The autonomy tier (`autonomy_level`) is intentionally NOT consulted
    /// here: per-operation approval is now the sole gate (defense-in-depth
    /// like workspace_only / command allowlist still applies in
    /// `SecurityPolicy`). To opt out of prompts entirely, put the literal
    /// wildcard `"*"` in `auto_approve` (see [`entries_match_call`]).
    pub fn needs_approval(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        // always_ask overrides everything — including the "*" wildcard.
        if self.always_ask.read().contains(tool_name) {
            return true;
        }

        // auto_approve / session allowlist (whole-tool, "*" wildcard,
        // OR safe shell prefix).
        if self.call_is_pre_approved(tool_name, args) {
            return false;
        }

        // Default: fail-closed — supervised semantics for everything else.
        true
    }

    /// Whether a specific tool call is pre-approved by the runtime
    /// `auto_approve` set or the session allowlist.
    ///
    /// Two entry shapes are honored (see module docs / config schema):
    ///   - `"<tool>"`           whole-tool grant — matches any call to that tool.
    ///   - `"shell:<prefix>"`   shell command-prefix grant — matches a `shell`
    ///                          call only when the command satisfies the safe
    ///                          prefix rule (see [`prefix_match_is_safe`]).
    ///
    /// This deliberately does NOT consult `always_ask`; the caller
    /// ([`needs_approval`]) applies the always_ask override first.
    pub fn call_is_pre_approved(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        let auto = self.auto_approve.read();
        if Self::entries_match_call(&auto, tool_name, args) {
            return true;
        }
        drop(auto);

        let session = self.session_allowlist.lock();
        Self::entries_match_call(&session, tool_name, args)
    }

    /// Returns true if any entry in `entries` grants this `tool_name`/`args` call.
    ///
    /// Match order (deliberate):
    ///   1. Whole-tool entry `"<tool_name>"` — backwards-compatible.
    ///   2. Literal `"*"` wildcard entry — approves any tool with any args,
    ///      including shell with metachars. This is "I want full autonomy";
    ///      the caller ([`needs_approval`]) still enforces `always_ask`
    ///      OUTSIDE this function so the wildcard can never bypass it.
    ///      Only the exact one-character string `"*"` is a wildcard — entries
    ///      like `"*git*"`, `"shell*"`, or `"file_*"` are treated as literal
    ///      tool names (no glob / prefix expansion).
    ///   3. `"shell:<prefix>"` safe-prefix grant — applies only to the
    ///      shell tool and only if [`prefix_match_is_safe`] holds.
    fn entries_match_call(
        entries: &HashSet<String>,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> bool {
        // (1) Whole-tool grant matches first (cheap, also backwards-compatible).
        if entries.contains(tool_name) {
            return true;
        }

        // (2) Literal "*" wildcard — opt-in "no prompts" for any tool.
        // Exact-string match only; "*foo" / "foo*" / "*" surrounded by other
        // characters in another entry never act as wildcards.
        if entries.contains("*") {
            return true;
        }

        // (3) shell:<prefix> grants apply only to the shell tool.
        if tool_name != "shell" {
            return false;
        }
        let Some(command) = shell_command_str(args) else {
            return false;
        };
        let shell_prefix = "shell:";
        entries.iter().any(|entry| {
            entry
                .strip_prefix(shell_prefix)
                .is_some_and(|prefix| prefix_match_is_safe(command, prefix))
        })
    }

    /// Record an approval decision and update session state.
    pub fn record_decision(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        decision: ApprovalResponse,
        channel: &str,
    ) {
        // If "Always", add to session allowlist.
        if decision == ApprovalResponse::Always {
            let mut allowlist = self.session_allowlist.lock();
            allowlist.insert(tool_name.to_string());
        }

        // Append to audit log.
        let summary = summarize_args(args);
        let entry = ApprovalLogEntry {
            timestamp: Utc::now().to_rfc3339(),
            tool_name: tool_name.to_string(),
            arguments_summary: summary,
            decision,
            channel: channel.to_string(),
        };
        let mut log = self.audit_log.lock();
        log.push(entry);
    }

    /// Get a snapshot of the audit log. Currently no in-tree consumer
    /// reads the log back (audit data is collected for future dashboard
    /// or `plaw doctor`-style export).
    #[allow(dead_code)]
    pub fn audit_log(&self) -> Vec<ApprovalLogEntry> {
        self.audit_log.lock().clone()
    }

    /// Get the current session allowlist. Currently no in-tree consumer
    /// inspects the allowlist contents directly; the lookup path uses
    /// `is_*_granted` predicates.
    #[allow(dead_code)]
    pub fn session_allowlist(&self) -> HashSet<String> {
        self.session_allowlist.lock().clone()
    }

    /// Grant session-scoped non-CLI approval for a specific tool.
    pub fn grant_non_cli_session(&self, tool_name: &str) {
        let mut allowlist = self.non_cli_allowlist.lock();
        allowlist.insert(tool_name.to_string());
    }

    /// Revoke session-scoped non-CLI approval for a specific tool.
    pub fn revoke_non_cli_session(&self, tool_name: &str) -> bool {
        let mut allowlist = self.non_cli_allowlist.lock();
        allowlist.remove(tool_name)
    }

    /// Check whether non-CLI session approval exists for a tool.
    /// Currently no in-tree caller — the non-CLI approval flow uses
    /// the per-request `await_non_cli_approval_decision` path
    /// (loop_/non_cli_approval.rs) rather than a pre-granted session
    /// allowlist. Kept for the eventual session-grant UX.
    #[allow(dead_code)]
    pub fn is_non_cli_session_granted(&self, tool_name: &str) -> bool {
        let allowlist = self.non_cli_allowlist.lock();
        allowlist.contains(tool_name)
    }

    /// Get the current non-CLI session allowlist.
    pub fn non_cli_session_allowlist(&self) -> HashSet<String> {
        self.non_cli_allowlist.lock().clone()
    }

    /// Grant one non-CLI "allow all tools/commands for one turn" token.
    ///
    /// Returns the remaining token count after increment.
    pub fn grant_non_cli_allow_all_once(&self) -> u32 {
        let mut remaining = self.non_cli_allow_all_once_remaining.lock();
        *remaining = remaining.saturating_add(1);
        *remaining
    }

    /// Consume one non-CLI "allow all tools/commands for one turn" token.
    ///
    /// Returns `true` when a token was consumed, `false` when none existed.
    pub fn consume_non_cli_allow_all_once(&self) -> bool {
        let mut remaining = self.non_cli_allow_all_once_remaining.lock();
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        true
    }

    /// Remaining one-time non-CLI "allow all tools/commands" tokens.
    pub fn non_cli_allow_all_once_remaining(&self) -> u32 {
        *self.non_cli_allow_all_once_remaining.lock()
    }

    /// Snapshot configured non-CLI approval approver entries.
    pub fn non_cli_approval_approvers(&self) -> HashSet<String> {
        self.non_cli_approval_approvers.read().clone()
    }

    /// Natural-language handling mode for non-CLI approval-management commands.
    pub fn non_cli_natural_language_approval_mode(&self) -> NonCliNaturalLanguageApprovalMode {
        *self.non_cli_natural_language_approval_mode.read()
    }

    /// Snapshot per-channel natural-language approval mode overrides.
    pub fn non_cli_natural_language_approval_mode_by_channel(
        &self,
    ) -> HashMap<String, NonCliNaturalLanguageApprovalMode> {
        self.non_cli_natural_language_approval_mode_by_channel
            .read()
            .clone()
    }

    /// Effective natural-language approval mode for a specific channel.
    pub fn non_cli_natural_language_approval_mode_for_channel(
        &self,
        channel: &str,
    ) -> NonCliNaturalLanguageApprovalMode {
        let normalized = channel.trim().to_ascii_lowercase();
        self.non_cli_natural_language_approval_mode_by_channel
            .read()
            .get(&normalized)
            .copied()
            .unwrap_or_else(|| self.non_cli_natural_language_approval_mode())
    }

    /// Check whether `sender` on `channel` may manage non-CLI approvals.
    ///
    /// If no approver entries are configured, this defaults to `true` so
    /// existing setups continue to behave as before.
    pub fn is_non_cli_approval_actor_allowed(&self, channel: &str, sender: &str) -> bool {
        let approvers = self.non_cli_approval_approvers.read();
        if approvers.is_empty() {
            return true;
        }

        if approvers.contains("*") || approvers.contains(sender) {
            return true;
        }

        let exact = format!("{channel}:{sender}");
        if approvers.contains(&exact) {
            return true;
        }

        let any_on_channel = format!("{channel}:*");
        if approvers.contains(&any_on_channel) {
            return true;
        }

        let sender_any_channel = format!("*:{sender}");
        approvers.contains(&sender_any_channel)
    }

    /// Apply runtime + persisted approval grant semantics:
    /// add to auto_approve and remove from always_ask.
    pub fn apply_persistent_runtime_grant(&self, tool_name: &str) {
        {
            let mut auto = self.auto_approve.write();
            auto.insert(tool_name.to_string());
        }
        let mut always = self.always_ask.write();
        always.remove(tool_name);
    }

    /// Apply runtime + persisted approval revoke semantics:
    /// remove from auto_approve.
    pub fn apply_persistent_runtime_revoke(&self, tool_name: &str) -> bool {
        let mut auto = self.auto_approve.write();
        auto.remove(tool_name)
    }

    /// Replace runtime-persistent non-CLI policy from config hot-reload.
    ///
    /// This updates the effective policy sets used by non-CLI approval commands
    /// without restarting the daemon.
    pub fn replace_runtime_non_cli_policy(
        &self,
        auto_approve: &[String],
        always_ask: &[String],
        non_cli_approval_approvers: &[String],
        non_cli_natural_language_approval_mode: NonCliNaturalLanguageApprovalMode,
        non_cli_natural_language_approval_mode_by_channel: &HashMap<
            String,
            NonCliNaturalLanguageApprovalMode,
        >,
    ) {
        {
            let mut auto = self.auto_approve.write();
            *auto = auto_approve.iter().cloned().collect();
        }
        {
            let mut always = self.always_ask.write();
            *always = always_ask.iter().cloned().collect();
        }
        {
            let mut approvers = self.non_cli_approval_approvers.write();
            *approvers = Self::normalize_non_cli_approvers(non_cli_approval_approvers);
        }
        {
            let mut mode = self.non_cli_natural_language_approval_mode.write();
            *mode = non_cli_natural_language_approval_mode;
        }
        {
            let mut mode_by_channel = self
                .non_cli_natural_language_approval_mode_by_channel
                .write();
            *mode_by_channel = Self::normalize_non_cli_natural_language_mode_by_channel(
                non_cli_natural_language_approval_mode_by_channel,
            );
        }
    }

    /// Snapshot runtime auto_approve entries.
    pub fn auto_approve_tools(&self) -> HashSet<String> {
        self.auto_approve.read().clone()
    }

    /// Snapshot runtime always_ask entries.
    pub fn always_ask_tools(&self) -> HashSet<String> {
        self.always_ask.read().clone()
    }

    /// Create a pending non-CLI approval request. If a matching active request
    /// already exists for (tool, requester, channel), returns that existing request.
    pub fn create_non_cli_pending_request(
        &self,
        tool_name: &str,
        requested_by: &str,
        requested_channel: &str,
        requested_reply_target: &str,
        reason: Option<String>,
    ) -> PendingNonCliApprovalRequest {
        let mut pending = self.pending_non_cli_requests.lock();
        prune_expired_pending_requests(&mut pending);

        if let Some(existing) = pending
            .values()
            .find(|req| {
                req.tool_name == tool_name
                    && req.requested_by == requested_by
                    && req.requested_channel == requested_channel
                    && req.requested_reply_target == requested_reply_target
            })
            .cloned()
        {
            return existing;
        }

        let now = Utc::now();
        let expires = now + Duration::minutes(30);
        let mut request_id = format!("apr-{}", &Uuid::new_v4().simple().to_string()[..8]);
        while pending.contains_key(&request_id) {
            request_id = format!("apr-{}", &Uuid::new_v4().simple().to_string()[..8]);
        }

        let req = PendingNonCliApprovalRequest {
            request_id: request_id.clone(),
            tool_name: tool_name.to_string(),
            requested_by: requested_by.to_string(),
            requested_channel: requested_channel.to_string(),
            requested_reply_target: requested_reply_target.to_string(),
            reason,
            created_at: now.to_rfc3339(),
            expires_at: expires.to_rfc3339(),
        };
        pending.insert(request_id, req.clone());
        self.resolved_non_cli_requests
            .lock()
            .remove(&req.request_id);
        req
    }

    /// Confirm a pending non-CLI approval request.
    /// Confirmation must come from the same sender in the same channel.
    pub fn confirm_non_cli_pending_request(
        &self,
        request_id: &str,
        confirmed_by: &str,
        confirmed_channel: &str,
        confirmed_reply_target: &str,
    ) -> Result<PendingNonCliApprovalRequest, PendingApprovalError> {
        let mut pending = self.pending_non_cli_requests.lock();
        prune_expired_pending_requests(&mut pending);

        let Some(req) = pending.remove(request_id) else {
            return Err(PendingApprovalError::NotFound);
        };

        if is_pending_request_expired(&req) {
            return Err(PendingApprovalError::Expired);
        }

        if req.requested_by != confirmed_by
            || req.requested_channel != confirmed_channel
            || req.requested_reply_target != confirmed_reply_target
        {
            pending.insert(req.request_id.clone(), req);
            return Err(PendingApprovalError::RequesterMismatch);
        }

        Ok(req)
    }

    /// Reject a pending non-CLI approval request.
    /// Rejection must come from the same sender in the same channel.
    pub fn reject_non_cli_pending_request(
        &self,
        request_id: &str,
        rejected_by: &str,
        rejected_channel: &str,
        rejected_reply_target: &str,
    ) -> Result<PendingNonCliApprovalRequest, PendingApprovalError> {
        let mut pending = self.pending_non_cli_requests.lock();
        prune_expired_pending_requests(&mut pending);

        let Some(req) = pending.remove(request_id) else {
            return Err(PendingApprovalError::NotFound);
        };

        if is_pending_request_expired(&req) {
            return Err(PendingApprovalError::Expired);
        }

        if req.requested_by != rejected_by
            || req.requested_channel != rejected_channel
            || req.requested_reply_target != rejected_reply_target
        {
            pending.insert(req.request_id.clone(), req);
            return Err(PendingApprovalError::RequesterMismatch);
        }

        Ok(req)
    }

    /// Return whether a pending non-CLI request still exists.
    pub fn has_non_cli_pending_request(&self, request_id: &str) -> bool {
        let mut pending = self.pending_non_cli_requests.lock();
        prune_expired_pending_requests(&mut pending);
        pending.contains_key(request_id)
    }

    /// Look up the tool name of a pending non-CLI request by its id.
    ///
    /// Used by the desktop gateway to build a persistent grant pattern
    /// (`"<tool>"` or `"shell:<prefix>"`) when the user picks
    /// "allow & remember". Returns `None` if the request is unknown/expired.
    pub fn non_cli_pending_request_tool_name(&self, request_id: &str) -> Option<String> {
        let mut pending = self.pending_non_cli_requests.lock();
        prune_expired_pending_requests(&mut pending);
        pending.get(request_id).map(|req| req.tool_name.clone())
    }

    /// Record a yes/no resolution for a pending non-CLI request.
    pub fn record_non_cli_pending_resolution(&self, request_id: &str, decision: ApprovalResponse) {
        if !matches!(decision, ApprovalResponse::Yes | ApprovalResponse::No) {
            return;
        }

        let mut resolved = self.resolved_non_cli_requests.lock();
        if resolved.len() >= 1024 {
            if let Some(first_key) = resolved.keys().next().cloned() {
                resolved.remove(&first_key);
            }
        }
        resolved.insert(request_id.to_string(), decision);
    }

    /// Consume a resolved pending-request decision if present.
    pub fn take_non_cli_pending_resolution(&self, request_id: &str) -> Option<ApprovalResponse> {
        self.resolved_non_cli_requests.lock().remove(request_id)
    }

    /// List active pending non-CLI approval requests.
    pub fn list_non_cli_pending_requests(
        &self,
        requested_by: Option<&str>,
        requested_channel: Option<&str>,
        requested_reply_target: Option<&str>,
    ) -> Vec<PendingNonCliApprovalRequest> {
        let mut pending = self.pending_non_cli_requests.lock();
        prune_expired_pending_requests(&mut pending);

        let mut rows = pending
            .values()
            .filter(|req| {
                requested_by.map_or(true, |by| req.requested_by == by)
                    && requested_channel.map_or(true, |channel| req.requested_channel == channel)
                    && requested_reply_target.map_or(true, |reply_target| {
                        req.requested_reply_target == reply_target
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        rows
    }

    /// Remove all pending requests for a tool.
    pub fn clear_non_cli_pending_requests_for_tool(&self, tool_name: &str) -> usize {
        let mut pending = self.pending_non_cli_requests.lock();
        prune_expired_pending_requests(&mut pending);
        let mut resolved = self.resolved_non_cli_requests.lock();
        let before = pending.len();
        pending.retain(|request_id, req| {
            let keep = req.tool_name != tool_name;
            if !keep {
                resolved.remove(request_id);
            }
            keep
        });
        before.saturating_sub(pending.len())
    }

    /// Prompt the user on the CLI and return their decision.
    ///
    /// For non-CLI channels, returns `Yes` automatically (interactive
    /// approval is only supported on CLI for now).
    pub fn prompt_cli(&self, request: &ApprovalRequest) -> ApprovalResponse {
        prompt_cli_interactive(request)
    }
}

// ── CLI prompt ───────────────────────────────────────────────────

/// Display the approval prompt and read user input from stdin.
fn prompt_cli_interactive(request: &ApprovalRequest) -> ApprovalResponse {
    let summary = summarize_args(&request.arguments);
    eprintln!();
    eprintln!("🔧 Agent wants to execute: {}", request.tool_name);
    eprintln!("   {summary}");
    eprint!("   [Y]es / [N]o / [A]lways for {}: ", request.tool_name);
    let _ = io::stderr().flush();

    let stdin = io::stdin();
    let mut line = String::new();
    if stdin.lock().read_line(&mut line).is_err() {
        return ApprovalResponse::No;
    }

    match line.trim().to_ascii_lowercase().as_str() {
        "y" | "yes" => ApprovalResponse::Yes,
        "a" | "always" => ApprovalResponse::Always,
        _ => ApprovalResponse::No,
    }
}

/// Extract the `command` string from a `shell` tool's arguments, if present.
fn shell_command_str(args: &serde_json::Value) -> Option<&str> {
    args.get("command").and_then(|v| v.as_str())
}

/// Shell control / chaining metacharacters. If ANY of these appear anywhere in
/// the command we refuse to auto-approve, regardless of prefix — a prefix grant
/// must never approve a chained / piped / substituted / redirected command.
const SHELL_CONTROL_METACHARS: &[char] = &[
    ';', '|', '&', '$', '`', '>', '<', '(', ')', '{', '}', '\n', '\r',
];

/// Whether a `shell:<prefix>` grant safely matches command string `command`.
///
/// All of the following must hold (fail-closed otherwise):
///   1. `prefix` is non-empty after trimming (empty prefix is NOT a wildcard;
///      it never matches — an empty/whole-tool grant is expressed as `"shell"`).
///   2. The command, with leading whitespace trimmed, `starts_with(prefix)`.
///   3. Boundary: the match ends at a word boundary — either the command is
///      exactly the prefix, or the next byte is ASCII whitespace. So
///      `"git status"` matches `"git status -s"` but NOT `"git statusfoo"`.
///   4. The command contains NONE of [`SHELL_CONTROL_METACHARS`] anywhere.
fn prefix_match_is_safe(command: &str, prefix: &str) -> bool {
    // (1) Empty/whitespace-only prefix never matches via this path.
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return false;
    }

    // (4) Any control/chaining metacharacter forces a prompt. Check the full
    // (untrimmed) command so leading/embedded metachars cannot slip through.
    if command.contains(SHELL_CONTROL_METACHARS) {
        return false;
    }

    // (2) Prefix match against the leading-whitespace-trimmed command.
    let c = command.trim_start();
    let Some(rest) = c.strip_prefix(prefix) else {
        return false;
    };

    // (3) Word boundary: exact match, or next byte is ASCII whitespace.
    match rest.as_bytes().first() {
        None => true,
        Some(b) => b.is_ascii_whitespace(),
    }
}

/// Produce a short human-readable summary of tool arguments.
fn summarize_args(args: &serde_json::Value) -> String {
    match args {
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => truncate_for_summary(s, 80),
                        other => {
                            let s = other.to_string();
                            truncate_for_summary(&s, 80)
                        }
                    };
                    format!("{k}: {val}")
                })
                .collect();
            parts.join(", ")
        }
        other => {
            let s = other.to_string();
            truncate_for_summary(&s, 120)
        }
    }
}

fn truncate_for_summary(input: &str, max_chars: usize) -> String {
    let mut chars = input.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}…")
    } else {
        input.to_string()
    }
}

fn is_pending_request_expired(req: &PendingNonCliApprovalRequest) -> bool {
    chrono::DateTime::parse_from_rfc3339(&req.expires_at)
        .map(|dt| dt.with_timezone(&Utc) <= Utc::now())
        .unwrap_or(true)
}

fn prune_expired_pending_requests(
    pending: &mut HashMap<String, PendingNonCliApprovalRequest>,
) -> usize {
    let before = pending.len();
    pending.retain(|_, req| !is_pending_request_expired(req));
    before.saturating_sub(pending.len())
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutonomyConfig;

    fn supervised_config() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["file_read".into(), "memory_recall".into()],
            always_ask: vec!["shell".into()],
            ..AutonomyConfig::default()
        }
    }

    fn full_config() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Full,
            ..AutonomyConfig::default()
        }
    }

    // ── needs_approval ───────────────────────────────────────

    #[test]
    fn auto_approve_tools_skip_prompt() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert!(!mgr.needs_approval("file_read", &serde_json::Value::Null));
        assert!(!mgr.needs_approval("memory_recall", &serde_json::Value::Null));
    }

    #[test]
    fn always_ask_tools_always_prompt() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert!(mgr.needs_approval("shell", &serde_json::Value::Null));
    }

    #[test]
    fn unknown_tool_needs_approval_in_supervised() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert!(mgr.needs_approval("file_write", &serde_json::Value::Null));
        assert!(mgr.needs_approval("http_request", &serde_json::Value::Null));
    }

    #[test]
    fn full_tier_alone_no_longer_skips_prompt() {
        // Stage 6 collapse: the autonomy tier no longer short-circuits
        // needs_approval. A bare "level = full" config with an empty
        // auto_approve set must now PROMPT for every tool — to opt out the
        // user has to explicitly add "*" to auto_approve (see migration in
        // src-tauri/services/config.rs ensure_config_defaults).
        let mgr = ApprovalManager::from_config(&full_config());
        assert!(mgr.needs_approval("shell", &serde_json::Value::Null));
        assert!(mgr.needs_approval("file_write", &serde_json::Value::Null));
        assert!(mgr.needs_approval("anything", &serde_json::Value::Null));
    }

    #[test]
    fn readonly_tier_alone_no_longer_skips_prompt() {
        // Stage 6 collapse: ReadOnly used to early-return false (no prompt
        // — blocking happens elsewhere). The tier is no longer consulted,
        // so the prompt fires just like Supervised. SecurityPolicy still
        // enforces ReadOnly-flavoured denials at a lower layer.
        let config = AutonomyConfig {
            level: AutonomyLevel::ReadOnly,
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&config);
        assert!(mgr.needs_approval("shell", &serde_json::Value::Null));
    }

    // ── session allowlist ────────────────────────────────────

    #[test]
    fn always_response_adds_to_session_allowlist() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert!(mgr.needs_approval("file_write", &serde_json::Value::Null));

        mgr.record_decision(
            "file_write",
            &serde_json::json!({"path": "test.txt"}),
            ApprovalResponse::Always,
            "cli",
        );

        // Now file_write should be in session allowlist.
        assert!(!mgr.needs_approval("file_write", &serde_json::Value::Null));
    }

    #[test]
    fn always_ask_overrides_session_allowlist() {
        let mgr = ApprovalManager::from_config(&supervised_config());

        // Even after "Always" for shell, it should still prompt.
        mgr.record_decision(
            "shell",
            &serde_json::json!({"command": "ls"}),
            ApprovalResponse::Always,
            "cli",
        );

        // shell is in always_ask, so it still needs approval.
        assert!(mgr.needs_approval("shell", &serde_json::Value::Null));
    }

    #[test]
    fn yes_response_does_not_add_to_allowlist() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        mgr.record_decision(
            "file_write",
            &serde_json::json!({}),
            ApprovalResponse::Yes,
            "cli",
        );
        assert!(mgr.needs_approval("file_write", &serde_json::Value::Null));
    }

    #[test]
    fn non_cli_session_approval_persists_across_checks() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert!(!mgr.is_non_cli_session_granted("shell"));

        mgr.grant_non_cli_session("shell");
        assert!(mgr.is_non_cli_session_granted("shell"));
        assert!(mgr.is_non_cli_session_granted("shell"));
    }

    #[test]
    fn non_cli_session_approval_can_be_revoked() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        mgr.grant_non_cli_session("shell");
        assert!(mgr.is_non_cli_session_granted("shell"));

        assert!(mgr.revoke_non_cli_session("shell"));
        assert!(!mgr.is_non_cli_session_granted("shell"));
        assert!(!mgr.revoke_non_cli_session("shell"));
    }

    #[test]
    fn non_cli_session_allowlist_snapshot_lists_granted_tools() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        mgr.grant_non_cli_session("shell");
        mgr.grant_non_cli_session("file_write");

        let allowlist = mgr.non_cli_session_allowlist();
        assert!(allowlist.contains("shell"));
        assert!(allowlist.contains("file_write"));
    }

    #[test]
    fn non_cli_allow_all_once_tokens_are_counted_and_consumed() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert_eq!(mgr.non_cli_allow_all_once_remaining(), 0);
        assert!(!mgr.consume_non_cli_allow_all_once());

        assert_eq!(mgr.grant_non_cli_allow_all_once(), 1);
        assert_eq!(mgr.grant_non_cli_allow_all_once(), 2);
        assert_eq!(mgr.non_cli_allow_all_once_remaining(), 2);

        assert!(mgr.consume_non_cli_allow_all_once());
        assert_eq!(mgr.non_cli_allow_all_once_remaining(), 1);
        assert!(mgr.consume_non_cli_allow_all_once());
        assert_eq!(mgr.non_cli_allow_all_once_remaining(), 0);
        assert!(!mgr.consume_non_cli_allow_all_once());
    }

    #[test]
    fn persistent_runtime_grant_updates_policy_immediately() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert!(mgr.needs_approval("shell", &serde_json::Value::Null));

        mgr.apply_persistent_runtime_grant("shell");
        assert!(!mgr.needs_approval("shell", &serde_json::Value::Null));
        assert!(mgr.auto_approve_tools().contains("shell"));
        assert!(!mgr.always_ask_tools().contains("shell"));
    }

    #[test]
    fn persistent_runtime_revoke_updates_policy_immediately() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert!(!mgr.needs_approval("file_read", &serde_json::Value::Null));

        assert!(mgr.apply_persistent_runtime_revoke("file_read"));
        assert!(mgr.needs_approval("file_read", &serde_json::Value::Null));
        assert!(!mgr.apply_persistent_runtime_revoke("file_read"));
    }

    #[test]
    fn create_and_confirm_pending_non_cli_approval_request() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        let req = mgr.create_non_cli_pending_request("shell", "alice", "telegram", "chat-1", None);
        assert_eq!(req.tool_name, "shell");
        assert!(req.request_id.starts_with("apr-"));

        let confirmed = mgr
            .confirm_non_cli_pending_request(&req.request_id, "alice", "telegram", "chat-1")
            .expect("request should confirm");
        assert_eq!(confirmed.request_id, req.request_id);
        assert!(mgr
            .confirm_non_cli_pending_request(&req.request_id, "alice", "telegram", "chat-1")
            .is_err());
    }

    #[test]
    fn create_and_reject_pending_non_cli_approval_request() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        let req = mgr.create_non_cli_pending_request("shell", "alice", "telegram", "chat-1", None);

        let rejected = mgr
            .reject_non_cli_pending_request(&req.request_id, "alice", "telegram", "chat-1")
            .expect("request should reject");
        assert_eq!(rejected.request_id, req.request_id);
        assert!(!mgr.has_non_cli_pending_request(&req.request_id));
    }

    #[test]
    fn pending_non_cli_resolution_is_recorded_and_consumed() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        let req = mgr.create_non_cli_pending_request("shell", "alice", "telegram", "chat-1", None);

        mgr.record_non_cli_pending_resolution(&req.request_id, ApprovalResponse::Yes);
        assert_eq!(
            mgr.take_non_cli_pending_resolution(&req.request_id),
            Some(ApprovalResponse::Yes)
        );
        assert_eq!(mgr.take_non_cli_pending_resolution(&req.request_id), None);
    }

    #[test]
    fn pending_non_cli_approval_requires_same_sender_and_channel() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        let req = mgr.create_non_cli_pending_request("shell", "alice", "telegram", "chat-1", None);

        let err = mgr
            .confirm_non_cli_pending_request(&req.request_id, "bob", "telegram", "chat-1")
            .expect_err("mismatched sender should fail");
        assert_eq!(err, PendingApprovalError::RequesterMismatch);

        // Request remains pending after mismatch.
        let pending =
            mgr.list_non_cli_pending_requests(Some("alice"), Some("telegram"), Some("chat-1"));
        assert_eq!(pending.len(), 1);

        let err = mgr
            .confirm_non_cli_pending_request(&req.request_id, "alice", "discord", "chat-1")
            .expect_err("mismatched channel should fail");
        assert_eq!(err, PendingApprovalError::RequesterMismatch);

        let err = mgr
            .confirm_non_cli_pending_request(&req.request_id, "alice", "telegram", "chat-2")
            .expect_err("mismatched reply target should fail");
        assert_eq!(err, PendingApprovalError::RequesterMismatch);
    }

    #[test]
    fn list_pending_non_cli_approvals_filters_scope() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        mgr.create_non_cli_pending_request("shell", "alice", "telegram", "chat-1", None);
        mgr.create_non_cli_pending_request("file_write", "bob", "telegram", "chat-1", None);
        mgr.create_non_cli_pending_request("browser_open", "alice", "discord", "chat-9", None);
        mgr.create_non_cli_pending_request("schedule", "alice", "telegram", "chat-2", None);

        let alice_telegram =
            mgr.list_non_cli_pending_requests(Some("alice"), Some("telegram"), Some("chat-1"));
        assert_eq!(alice_telegram.len(), 1);
        assert_eq!(alice_telegram[0].tool_name, "shell");

        let telegram_chat1 =
            mgr.list_non_cli_pending_requests(None, Some("telegram"), Some("chat-1"));
        assert_eq!(telegram_chat1.len(), 2);
    }

    #[test]
    fn pending_non_cli_approval_expiry_is_pruned() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        let req = mgr.create_non_cli_pending_request("shell", "alice", "telegram", "chat-1", None);

        {
            let mut pending = mgr.pending_non_cli_requests.lock();
            let row = pending.get_mut(&req.request_id).expect("request row");
            row.expires_at = (Utc::now() - Duration::minutes(1)).to_rfc3339();
        }

        let rows = mgr.list_non_cli_pending_requests(None, None, None);
        assert!(rows.is_empty());
        let err = mgr
            .confirm_non_cli_pending_request(&req.request_id, "alice", "telegram", "chat-1")
            .expect_err("expired request should not confirm");
        assert_eq!(err, PendingApprovalError::NotFound);
    }

    #[test]
    fn non_cli_approval_actor_defaults_to_allow_when_not_configured() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert!(mgr.is_non_cli_approval_actor_allowed("telegram", "alice"));
        assert!(mgr.is_non_cli_approval_actor_allowed("discord", "bob"));
    }

    #[test]
    fn non_cli_natural_language_approval_mode_defaults_to_direct() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        assert_eq!(
            mgr.non_cli_natural_language_approval_mode(),
            NonCliNaturalLanguageApprovalMode::Direct
        );
    }

    #[test]
    fn non_cli_approval_actor_allowlist_supports_exact_and_wildcards() {
        let mut cfg = supervised_config();
        cfg.non_cli_approval_approvers = vec![
            "alice".to_string(),
            "telegram:bob".to_string(),
            "discord:*".to_string(),
            "*:carol".to_string(),
        ];
        let mgr = ApprovalManager::from_config(&cfg);

        assert!(mgr.is_non_cli_approval_actor_allowed("telegram", "alice"));
        assert!(mgr.is_non_cli_approval_actor_allowed("telegram", "bob"));
        assert!(mgr.is_non_cli_approval_actor_allowed("discord", "anyone"));
        assert!(mgr.is_non_cli_approval_actor_allowed("matrix", "carol"));

        assert!(!mgr.is_non_cli_approval_actor_allowed("telegram", "mallory"));
        assert!(!mgr.is_non_cli_approval_actor_allowed("matrix", "bob"));
    }

    #[test]
    fn non_cli_natural_language_approval_mode_honors_config_override() {
        let mut cfg = supervised_config();
        cfg.non_cli_natural_language_approval_mode =
            NonCliNaturalLanguageApprovalMode::RequestConfirm;
        let mgr = ApprovalManager::from_config(&cfg);
        assert_eq!(
            mgr.non_cli_natural_language_approval_mode(),
            NonCliNaturalLanguageApprovalMode::RequestConfirm
        );
    }

    #[test]
    fn non_cli_natural_language_approval_mode_supports_per_channel_override() {
        let mut cfg = supervised_config();
        cfg.non_cli_natural_language_approval_mode = NonCliNaturalLanguageApprovalMode::Direct;
        cfg.non_cli_natural_language_approval_mode_by_channel
            .insert(
                "discord".to_string(),
                NonCliNaturalLanguageApprovalMode::RequestConfirm,
            );
        let mgr = ApprovalManager::from_config(&cfg);

        assert_eq!(
            mgr.non_cli_natural_language_approval_mode_for_channel("telegram"),
            NonCliNaturalLanguageApprovalMode::Direct
        );
        assert_eq!(
            mgr.non_cli_natural_language_approval_mode_for_channel("discord"),
            NonCliNaturalLanguageApprovalMode::RequestConfirm
        );
    }

    #[test]
    fn replace_runtime_non_cli_policy_updates_modes_and_approvers() {
        let cfg = supervised_config();
        let mgr = ApprovalManager::from_config(&cfg);

        let mut mode_overrides = HashMap::new();
        mode_overrides.insert(
            "telegram".to_string(),
            NonCliNaturalLanguageApprovalMode::Disabled,
        );
        mode_overrides.insert(
            "discord".to_string(),
            NonCliNaturalLanguageApprovalMode::RequestConfirm,
        );

        mgr.replace_runtime_non_cli_policy(
            &["mock_price".to_string()],
            &["shell".to_string()],
            &["telegram:alice".to_string()],
            NonCliNaturalLanguageApprovalMode::Direct,
            &mode_overrides,
        );

        assert!(!mgr.needs_approval("mock_price", &serde_json::Value::Null));
        assert!(mgr.needs_approval("shell", &serde_json::Value::Null));
        assert!(mgr.is_non_cli_approval_actor_allowed("telegram", "alice"));
        assert!(!mgr.is_non_cli_approval_actor_allowed("telegram", "bob"));
        assert_eq!(
            mgr.non_cli_natural_language_approval_mode_for_channel("telegram"),
            NonCliNaturalLanguageApprovalMode::Disabled
        );
        assert_eq!(
            mgr.non_cli_natural_language_approval_mode_for_channel("discord"),
            NonCliNaturalLanguageApprovalMode::RequestConfirm
        );
        assert_eq!(
            mgr.non_cli_natural_language_approval_mode_for_channel("slack"),
            NonCliNaturalLanguageApprovalMode::Direct
        );
    }

    // ── audit log ────────────────────────────────────────────

    #[test]
    fn audit_log_records_decisions() {
        let mgr = ApprovalManager::from_config(&supervised_config());

        mgr.record_decision(
            "shell",
            &serde_json::json!({"command": "rm -rf ./build/"}),
            ApprovalResponse::No,
            "cli",
        );
        mgr.record_decision(
            "file_write",
            &serde_json::json!({"path": "out.txt", "content": "hello"}),
            ApprovalResponse::Yes,
            "cli",
        );

        let log = mgr.audit_log();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].tool_name, "shell");
        assert_eq!(log[0].decision, ApprovalResponse::No);
        assert_eq!(log[1].tool_name, "file_write");
        assert_eq!(log[1].decision, ApprovalResponse::Yes);
    }

    #[test]
    fn audit_log_contains_timestamp_and_channel() {
        let mgr = ApprovalManager::from_config(&supervised_config());
        mgr.record_decision(
            "shell",
            &serde_json::json!({"command": "ls"}),
            ApprovalResponse::Yes,
            "telegram",
        );

        let log = mgr.audit_log();
        assert_eq!(log.len(), 1);
        assert!(!log[0].timestamp.is_empty());
        assert_eq!(log[0].channel, "telegram");
    }

    // ── summarize_args ───────────────────────────────────────

    #[test]
    fn summarize_args_object() {
        let args = serde_json::json!({"command": "ls -la", "cwd": "/tmp"});
        let summary = summarize_args(&args);
        assert!(summary.contains("command: ls -la"));
        assert!(summary.contains("cwd: /tmp"));
    }

    #[test]
    fn summarize_args_truncates_long_values() {
        let long_val = "x".repeat(200);
        let args = serde_json::json!({ "content": long_val });
        let summary = summarize_args(&args);
        assert!(summary.contains('…'));
        assert!(summary.len() < 200);
    }

    #[test]
    fn summarize_args_unicode_safe_truncation() {
        let long_val = "🦀".repeat(120);
        let args = serde_json::json!({ "content": long_val });
        let summary = summarize_args(&args);
        assert!(summary.contains("content:"));
        assert!(summary.contains('…'));
    }

    #[test]
    fn summarize_args_non_object() {
        let args = serde_json::json!("just a string");
        let summary = summarize_args(&args);
        assert!(summary.contains("just a string"));
    }

    // ── ApprovalResponse serde ───────────────────────────────

    #[test]
    fn approval_response_serde_roundtrip() {
        let json = serde_json::to_string(&ApprovalResponse::Always).unwrap();
        assert_eq!(json, "\"always\"");
        let parsed: ApprovalResponse = serde_json::from_str("\"no\"").unwrap();
        assert_eq!(parsed, ApprovalResponse::No);
    }

    // ── ApprovalRequest ──────────────────────────────────────

    #[test]
    fn approval_request_serde() {
        let req = ApprovalRequest {
            tool_name: "shell".into(),
            arguments: serde_json::json!({"command": "echo hi"}),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: ApprovalRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tool_name, "shell");
    }

    // ── prefix_match_is_safe (security-critical core) ─────────

    #[test]
    fn prefix_match_approves_exact_and_word_boundary_extensions() {
        // Exact match.
        assert!(prefix_match_is_safe("git status", "git status"));
        // Single-space extension.
        assert!(prefix_match_is_safe("git status -s", "git status"));
        // Multi-space extension (boundary is the first byte after the prefix).
        assert!(prefix_match_is_safe("git status  --short", "git status"));
        // Leading whitespace on the command is trimmed before matching.
        assert!(prefix_match_is_safe("   git status -s", "git status"));
    }

    #[test]
    fn prefix_match_rejects_word_boundary_violation() {
        // "git statusfoo" must NOT match prefix "git status".
        assert!(!prefix_match_is_safe("git statusfoo", "git status"));
        assert!(!prefix_match_is_safe("git status-extra", "git status"));
    }

    #[test]
    fn prefix_match_rejects_shell_metacharacters() {
        // Every chaining / piping / substitution / redirection metachar forces
        // a prompt even when the prefix would otherwise match.
        assert!(!prefix_match_is_safe("git status; rm -rf /", "git status"));
        assert!(!prefix_match_is_safe("git status && x", "git status"));
        assert!(!prefix_match_is_safe("git status | sh", "git status"));
        assert!(!prefix_match_is_safe("git status $(x)", "git status"));
        assert!(!prefix_match_is_safe("git status `x`", "git status"));
        assert!(!prefix_match_is_safe("git status > /etc/passwd", "git status"));
        assert!(!prefix_match_is_safe("git status < in", "git status"));
        assert!(!prefix_match_is_safe("git status (x)", "git status"));
        assert!(!prefix_match_is_safe("git status {x}", "git status"));
        assert!(!prefix_match_is_safe("git status\nrm -rf /", "git status"));
        assert!(!prefix_match_is_safe("git status\rrm -rf /", "git status"));
        // A bare `&` (background) is also a control metachar.
        assert!(!prefix_match_is_safe("git status &", "git status"));
    }

    #[test]
    fn prefix_match_rejects_empty_and_whitespace_only_prefix() {
        assert!(!prefix_match_is_safe("git status", ""));
        assert!(!prefix_match_is_safe("git status", "   "));
        // Even an empty command with empty prefix must not match.
        assert!(!prefix_match_is_safe("", ""));
    }

    #[test]
    fn prefix_match_rejects_non_prefix_command() {
        assert!(!prefix_match_is_safe("ls -la", "git status"));
    }

    // ── shell_command_str ────────────────────────────────────

    #[test]
    fn shell_command_str_extracts_command_field() {
        let args = serde_json::json!({"command": "git status"});
        assert_eq!(shell_command_str(&args), Some("git status"));
        assert_eq!(shell_command_str(&serde_json::json!({})), None);
        assert_eq!(shell_command_str(&serde_json::Value::Null), None);
        assert_eq!(shell_command_str(&serde_json::json!({"command": 5})), None);
    }

    // ── call_is_pre_approved ──────────────────────────────────

    fn empty_config() -> AutonomyConfig {
        AutonomyConfig {
            level: AutonomyLevel::Supervised,
            ..AutonomyConfig::default()
        }
    }

    #[test]
    fn call_is_pre_approved_whole_tool_grant() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["file_read".into()],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        // Whole-tool grant matches any args (including none).
        assert!(mgr.call_is_pre_approved("file_read", &serde_json::Value::Null));
        assert!(mgr.call_is_pre_approved("file_read", &serde_json::json!({"path": "x"})));
        assert!(!mgr.call_is_pre_approved("file_write", &serde_json::Value::Null));
    }

    #[test]
    fn call_is_pre_approved_bare_shell_is_backwards_compatible() {
        // A bare "shell" entry (today's format) approves every shell call.
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["shell".into()],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        assert!(mgr.call_is_pre_approved("shell", &serde_json::json!({"command": "rm -rf /"})));
    }

    #[test]
    fn call_is_pre_approved_shell_prefix_grant_matches_safely() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["shell:git status".into()],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        // Safe matches.
        assert!(mgr.call_is_pre_approved("shell", &serde_json::json!({"command": "git status"})));
        assert!(
            mgr.call_is_pre_approved("shell", &serde_json::json!({"command": "git status -s"}))
        );
        // Boundary violation — does NOT match.
        assert!(
            !mgr.call_is_pre_approved("shell", &serde_json::json!({"command": "git statusfoo"}))
        );
        // Metachar — does NOT match.
        assert!(!mgr.call_is_pre_approved(
            "shell",
            &serde_json::json!({"command": "git status; rm -rf /"})
        ));
        // Different command — does NOT match.
        assert!(!mgr.call_is_pre_approved("shell", &serde_json::json!({"command": "ls"})));
        // A shell:<prefix> grant never approves a non-shell tool.
        assert!(!mgr.call_is_pre_approved("file_read", &serde_json::json!({"command": "git status"})));
    }

    #[test]
    fn call_is_pre_approved_honors_session_allowlist_prefix() {
        let mgr = ApprovalManager::from_config(&empty_config());
        assert!(!mgr.call_is_pre_approved("shell", &serde_json::json!({"command": "git status"})));

        // record_decision(Always) inserts the bare tool name; whole-tool grant.
        mgr.record_decision(
            "file_write",
            &serde_json::json!({"path": "x"}),
            ApprovalResponse::Always,
            "cli",
        );
        assert!(mgr.call_is_pre_approved("file_write", &serde_json::Value::Null));
    }

    #[test]
    fn needs_approval_shell_prefix_grant_via_auto_approve() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["shell:git status".into()],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        // Pre-approved safe shell call skips the prompt.
        assert!(!mgr.needs_approval("shell", &serde_json::json!({"command": "git status -s"})));
        // Non-matching shell call still prompts.
        assert!(mgr.needs_approval("shell", &serde_json::json!({"command": "rm -rf /"})));
        // Chained call still prompts even though prefix matches.
        assert!(mgr.needs_approval(
            "shell",
            &serde_json::json!({"command": "git status && rm -rf /"})
        ));
    }

    #[test]
    fn needs_approval_always_ask_overrides_shell_prefix_grant() {
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["shell:git status".into()],
            always_ask: vec!["shell".into()],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        // always_ask("shell") forces a prompt regardless of the prefix grant.
        assert!(mgr.needs_approval("shell", &serde_json::json!({"command": "git status"})));
    }

    #[test]
    fn apply_persistent_runtime_grant_accepts_shell_prefix_pattern() {
        let mgr = ApprovalManager::from_config(&empty_config());
        assert!(mgr.needs_approval("shell", &serde_json::json!({"command": "git status"})));

        // The gateway passes a full "shell:<prefix>" pattern verbatim.
        mgr.apply_persistent_runtime_grant("shell:git status");
        assert!(!mgr.needs_approval("shell", &serde_json::json!({"command": "git status -s"})));
        assert!(mgr.needs_approval("shell", &serde_json::json!({"command": "git push"})));
    }

    // ── "*" wildcard (Stage 6 collapse) ───────────────────────

    #[test]
    fn wildcard_approves_any_tool() {
        // auto_approve = ["*"] is "I want full autonomy" — every tool with
        // any args is pre-approved, including shell with arbitrary commands.
        // The wildcard deliberately skips the prefix-safety check (it's the
        // user's explicit opt-out of the supervised default).
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["*".into()],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        assert!(!mgr.needs_approval("file_read", &serde_json::Value::Null));
        assert!(!mgr.needs_approval("file_write", &serde_json::json!({"path": "x"})));
        assert!(!mgr.needs_approval("shell", &serde_json::json!({"command": "ls"})));
        // Shell metachars are NOT a barrier under the wildcard — by design.
        assert!(!mgr.needs_approval(
            "shell",
            &serde_json::json!({"command": "rm -rf / ; echo pwned"})
        ));
        assert!(!mgr.needs_approval("any_unknown_tool", &serde_json::Value::Null));
    }

    #[test]
    fn wildcard_does_not_bypass_always_ask() {
        // always_ask is checked OUTSIDE entries_match_call so the "*"
        // wildcard cannot override an explicit always_ask entry. The user
        // can opt out of prompts globally with "*" but still force prompts
        // for specific dangerous tools.
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["*".into()],
            always_ask: vec!["shell".into()],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        assert!(mgr.needs_approval("shell", &serde_json::json!({"command": "ls"})));
        // Other tools are still wildcard-approved.
        assert!(!mgr.needs_approval("file_write", &serde_json::Value::Null));
    }

    #[test]
    fn wildcard_is_exact_string_match_only() {
        // "*git*" / "shell*" / "file_*" are literal tool names — NOT globs.
        // Only the exact one-character "*" entry triggers the wildcard path.
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec!["*git*".into(), "shell*".into(), "file_*".into()],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        // None of these should be wildcard-matched.
        assert!(mgr.needs_approval("git_operations", &serde_json::Value::Null));
        assert!(mgr.needs_approval("shell", &serde_json::json!({"command": "ls"})));
        assert!(mgr.needs_approval("file_read", &serde_json::Value::Null));
        assert!(mgr.needs_approval("file_write", &serde_json::Value::Null));
        // The literal "shell*" entry would match a tool literally named
        // "shell*" — exercise that to prove it's whole-tool matching.
        assert!(!mgr.needs_approval("shell*", &serde_json::Value::Null));
    }

    #[test]
    fn empty_auto_approve_still_prompts() {
        // No regression: when auto_approve is genuinely empty (clear the
        // schema default which seeds {file_read, memory_recall}), every
        // non-pre-approved tool prompts.
        let cfg = AutonomyConfig {
            level: AutonomyLevel::Supervised,
            auto_approve: vec![],
            ..AutonomyConfig::default()
        };
        let mgr = ApprovalManager::from_config(&cfg);
        assert!(mgr.needs_approval("file_read", &serde_json::Value::Null));
        assert!(mgr.needs_approval("shell", &serde_json::json!({"command": "ls"})));
        assert!(mgr.needs_approval("anything", &serde_json::Value::Null));
    }
}
