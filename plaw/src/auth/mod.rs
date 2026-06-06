pub mod anthropic_token;
pub mod gemini_oauth;
pub mod oauth_common;
pub mod openai_oauth;
pub mod profiles;

use crate::auth::openai_oauth::refresh_access_token;
use crate::auth::profiles::{
    profile_id, AuthProfile, AuthProfileKind, AuthProfilesData, AuthProfilesStore, TokenSet,
};
use crate::config::Config;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const OPENAI_CODEX_PROVIDER: &str = "openai-codex";
const ANTHROPIC_PROVIDER: &str = "anthropic";
const GEMINI_PROVIDER: &str = "gemini";
const DEFAULT_PROFILE_NAME: &str = "default";
const OPENAI_REFRESH_SKEW_SECS: u64 = 90;
const OPENAI_REFRESH_FAILURE_BACKOFF_SECS: u64 = 10;
const OAUTH_REFRESH_MAX_ATTEMPTS: usize = 3;
const OAUTH_REFRESH_RETRY_BASE_DELAY_MS: u64 = 350;
/// PR #80: provider-id prefix for MCP OAuth profiles. A profile for
/// the GitHub MCP server gets `provider = "mcp:github"` so existing
/// `select_profile_id` / `set_active_profile` / `get_profile`
/// helpers work without code changes.
pub const MCP_PROVIDER_PREFIX: &str = "mcp:";
/// Proactively refresh MCP access tokens this many seconds BEFORE
/// the `expires_at`. 60 s gives time for the IdP roundtrip even on
/// slow networks. Slightly tighter than OpenAI's 90 s because MCP
/// access tokens are typically shorter-lived (15-60 min vs hours).
const MCP_REFRESH_SKEW_SECS: u64 = 60;
static REFRESH_BACKOFFS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

#[derive(Clone)]
pub struct AuthService {
    store: AuthProfilesStore,
    client: reqwest::Client,
}

impl AuthService {
    pub fn from_config(config: &Config) -> Self {
        let state_dir = state_dir_from_config(config);
        Self::new(&state_dir, config.secrets.encrypt)
    }

    pub fn new(state_dir: &Path, encrypt_secrets: bool) -> Self {
        Self {
            store: AuthProfilesStore::new(state_dir, encrypt_secrets),
            client: reqwest::Client::new(),
        }
    }

    pub async fn load_profiles(&self) -> Result<AuthProfilesData> {
        self.store.load().await
    }

    pub async fn store_openai_tokens(
        &self,
        profile_name: &str,
        token_set: crate::auth::profiles::TokenSet,
        account_id: Option<String>,
        set_active: bool,
    ) -> Result<AuthProfile> {
        let mut profile = AuthProfile::new_oauth(OPENAI_CODEX_PROVIDER, profile_name, token_set);
        profile.account_id = account_id;
        self.store
            .upsert_profile(profile.clone(), set_active)
            .await?;
        Ok(profile)
    }

    pub async fn store_gemini_tokens(
        &self,
        profile_name: &str,
        token_set: crate::auth::profiles::TokenSet,
        account_id: Option<String>,
        set_active: bool,
    ) -> Result<AuthProfile> {
        let mut profile = AuthProfile::new_oauth(GEMINI_PROVIDER, profile_name, token_set);
        profile.account_id = account_id;
        self.store
            .upsert_profile(profile.clone(), set_active)
            .await?;
        Ok(profile)
    }

    pub async fn store_provider_token(
        &self,
        provider: &str,
        profile_name: &str,
        token: &str,
        metadata: HashMap<String, String>,
        set_active: bool,
    ) -> Result<AuthProfile> {
        let mut profile = AuthProfile::new_token(provider, profile_name, token.to_string());
        profile.metadata.extend(metadata);
        self.store
            .upsert_profile(profile.clone(), set_active)
            .await?;
        Ok(profile)
    }

    pub async fn set_active_profile(
        &self,
        provider: &str,
        requested_profile: &str,
    ) -> Result<String> {
        let provider = normalize_provider(provider)?;
        let data = self.store.load().await?;
        let profile_id = resolve_requested_profile_id(&provider, requested_profile);

        let profile = data
            .profiles
            .get(&profile_id)
            .ok_or_else(|| anyhow::anyhow!("Auth profile not found: {profile_id}"))?;

        if profile.provider != provider {
            anyhow::bail!(
                "Profile {profile_id} belongs to provider {}, not {}",
                profile.provider,
                provider
            );
        }

        self.store
            .set_active_profile(&provider, &profile_id)
            .await?;
        Ok(profile_id)
    }

    pub async fn remove_profile(&self, provider: &str, requested_profile: &str) -> Result<bool> {
        let provider = normalize_provider(provider)?;
        let profile_id = resolve_requested_profile_id(&provider, requested_profile);
        self.store.remove_profile(&profile_id).await
    }

    pub async fn get_profile(
        &self,
        provider: &str,
        profile_override: Option<&str>,
    ) -> Result<Option<AuthProfile>> {
        let provider = normalize_provider(provider)?;
        let data = self.store.load().await?;
        let Some(profile_id) = select_profile_id(&data, &provider, profile_override) else {
            return Ok(None);
        };
        Ok(data.profiles.get(&profile_id).cloned())
    }

    pub async fn get_provider_bearer_token(
        &self,
        provider: &str,
        profile_override: Option<&str>,
    ) -> Result<Option<String>> {
        let profile = self.get_profile(provider, profile_override).await?;
        let Some(profile) = profile else {
            return Ok(None);
        };

        let credential = match profile.kind {
            AuthProfileKind::Token => profile.token,
            AuthProfileKind::OAuth => profile.token_set.map(|t| t.access_token),
        };

        Ok(credential.filter(|t| !t.trim().is_empty()))
    }

    pub async fn get_valid_openai_access_token(
        &self,
        profile_override: Option<&str>,
    ) -> Result<Option<String>> {
        let data = self.store.load().await?;
        let Some(profile_id) = select_profile_id(&data, OPENAI_CODEX_PROVIDER, profile_override)
        else {
            return Ok(None);
        };

        let Some(profile) = data.profiles.get(&profile_id) else {
            return Ok(None);
        };

        let Some(token_set) = profile.token_set.as_ref() else {
            anyhow::bail!("OpenAI Codex auth profile is not OAuth-based: {profile_id}");
        };

        if !token_set.is_expiring_within(Duration::from_secs(OPENAI_REFRESH_SKEW_SECS)) {
            return Ok(Some(token_set.access_token.clone()));
        }

        let Some(refresh_token) = token_set.refresh_token.clone() else {
            return Ok(Some(token_set.access_token.clone()));
        };

        let refresh_lock = refresh_lock_for_profile(&profile_id);
        let _guard = refresh_lock.lock().await;

        // Re-load after waiting for lock to avoid duplicate refreshes.
        let data = self.store.load().await?;
        let Some(latest_profile) = data.profiles.get(&profile_id) else {
            return Ok(None);
        };

        let Some(latest_tokens) = latest_profile.token_set.as_ref() else {
            anyhow::bail!("OpenAI Codex auth profile is missing token set: {profile_id}");
        };

        if !latest_tokens.is_expiring_within(Duration::from_secs(OPENAI_REFRESH_SKEW_SECS)) {
            return Ok(Some(latest_tokens.access_token.clone()));
        }

        let refresh_token = latest_tokens.refresh_token.clone().unwrap_or(refresh_token);

        if let Some(remaining) = refresh_backoff_remaining(&profile_id) {
            anyhow::bail!(
                "OpenAI token refresh is in backoff for {remaining}s due to previous failures"
            );
        }

        let mut refreshed =
            match refresh_openai_access_token_with_retries(&self.client, &refresh_token).await {
                Ok(tokens) => {
                    clear_refresh_backoff(&profile_id);
                    tokens
                }
                Err(err) => {
                    set_refresh_backoff(
                        &profile_id,
                        Duration::from_secs(OPENAI_REFRESH_FAILURE_BACKOFF_SECS),
                    );
                    return Err(err);
                }
            };
        if refreshed.refresh_token.is_none() {
            refreshed
                .refresh_token
                .clone_from(&latest_tokens.refresh_token);
        }

        let account_id = openai_oauth::extract_account_id_from_jwt(&refreshed.access_token)
            .or_else(|| latest_profile.account_id.clone());

        let updated = self
            .store
            .update_profile(&profile_id, |profile| {
                profile.kind = AuthProfileKind::OAuth;
                profile.token_set = Some(refreshed.clone());
                profile.account_id.clone_from(&account_id);
                Ok(())
            })
            .await?;

        Ok(updated.token_set.map(|t| t.access_token))
    }

    /// Get a valid Gemini OAuth access token, refreshing if necessary.
    ///
    /// Returns `None` if no Gemini profile exists.
    pub async fn get_valid_gemini_access_token(
        &self,
        profile_override: Option<&str>,
    ) -> Result<Option<String>> {
        let data = self.store.load().await?;
        let Some(profile_id) = select_profile_id(&data, GEMINI_PROVIDER, profile_override) else {
            return Ok(None);
        };

        let Some(profile) = data.profiles.get(&profile_id) else {
            return Ok(None);
        };

        let Some(token_set) = profile.token_set.as_ref() else {
            anyhow::bail!("Gemini auth profile is not OAuth-based: {profile_id}");
        };

        if !token_set.is_expiring_within(Duration::from_secs(OPENAI_REFRESH_SKEW_SECS)) {
            return Ok(Some(token_set.access_token.clone()));
        }

        let Some(refresh_token) = token_set.refresh_token.clone() else {
            return Ok(Some(token_set.access_token.clone()));
        };

        let refresh_lock = refresh_lock_for_profile(&profile_id);
        let _guard = refresh_lock.lock().await;

        // Re-load after waiting for lock to avoid duplicate refreshes.
        let data = self.store.load().await?;
        let Some(latest_profile) = data.profiles.get(&profile_id) else {
            return Ok(None);
        };

        let Some(latest_tokens) = latest_profile.token_set.as_ref() else {
            anyhow::bail!("Gemini auth profile is missing token set: {profile_id}");
        };

        if !latest_tokens.is_expiring_within(Duration::from_secs(OPENAI_REFRESH_SKEW_SECS)) {
            return Ok(Some(latest_tokens.access_token.clone()));
        }

        let refresh_token = latest_tokens.refresh_token.clone().unwrap_or(refresh_token);

        if let Some(remaining) = refresh_backoff_remaining(&profile_id) {
            anyhow::bail!(
                "Gemini token refresh is in backoff for {remaining}s due to previous failures"
            );
        }

        let mut refreshed =
            match refresh_gemini_access_token_with_retries(&self.client, &refresh_token).await {
                Ok(tokens) => {
                    clear_refresh_backoff(&profile_id);
                    tokens
                }
                Err(err) => {
                    set_refresh_backoff(
                        &profile_id,
                        Duration::from_secs(OPENAI_REFRESH_FAILURE_BACKOFF_SECS),
                    );
                    return Err(err);
                }
            };
        if refreshed.refresh_token.is_none() {
            refreshed
                .refresh_token
                .clone_from(&latest_tokens.refresh_token);
        }

        let account_id = refreshed
            .id_token
            .as_deref()
            .and_then(gemini_oauth::extract_account_email_from_id_token)
            .or_else(|| latest_profile.account_id.clone());

        let updated = self
            .store
            .update_profile(&profile_id, |profile| {
                profile.kind = AuthProfileKind::OAuth;
                profile.token_set = Some(refreshed.clone());
                profile.account_id.clone_from(&account_id);
                Ok(())
            })
            .await?;

        Ok(updated.token_set.map(|t| t.access_token))
    }

    /// Get Gemini profile info (for provider initialization).
    pub async fn get_gemini_profile(
        &self,
        profile_override: Option<&str>,
    ) -> Result<Option<AuthProfile>> {
        self.get_profile(GEMINI_PROVIDER, profile_override).await
    }

    // ── PR #80: MCP OAuth lifecycle ──────────────────────────────────

    /// Persist an MCP OAuth profile after a successful ceremony.
    ///
    /// The `provider` field is set to `mcp:<server_name>` so existing
    /// helpers (`select_profile_id`, `set_active_profile`, etc.) see
    /// MCP profiles the same way they see OpenAI / Gemini profiles.
    /// `profile_name = "default"` is hardcoded — multi-profile-per-
    /// server is Phase 2 (per PR #79 anti-scope memo).
    ///
    /// `oauth_client_id` is stored plaintext per RFC 7591 §2;
    /// `oauth_client_secret` (when present — GitHub-style flows)
    /// rides the encrypt_optional path already exercised by
    /// `refresh_token`.
    pub async fn store_mcp_oauth(
        &self,
        server_name: &str,
        token_set: TokenSet,
        client_id: Option<String>,
        client_secret: Option<String>,
        metadata: HashMap<String, String>,
    ) -> Result<AuthProfile> {
        let provider = format!("{MCP_PROVIDER_PREFIX}{server_name}");
        let mut profile = AuthProfile::new_oauth(&provider, DEFAULT_PROFILE_NAME, token_set);
        profile.oauth_client_id = client_id;
        profile.oauth_client_secret = client_secret;
        profile.metadata.extend(metadata);
        // Set active on store — there's exactly one profile per MCP
        // server in Phase 1, so "active" is redundant but matches the
        // store_openai_tokens convention.
        self.store.upsert_profile(profile.clone(), true).await?;
        Ok(profile)
    }

    /// Return a valid access token for the given MCP server,
    /// refreshing through the IdP if the cached token is within
    /// [`MCP_REFRESH_SKEW_SECS`] of expiry.
    ///
    /// Returns `Ok(None)` when no profile exists for the server — the
    /// caller (PR #81's `HttpTransport::request` 401 branch) should
    /// surface a clear "run plaw auth login --provider mcp:<name>"
    /// instruction in that case.
    ///
    /// Single-flight refresh via the per-profile `tokio::Mutex` reused
    /// from the OpenAI / Gemini implementations. Refresh failures
    /// trigger the same exponential-backoff dampener so a broken
    /// IdP does not thrash plaw against the token endpoint.
    ///
    /// Token rotation: when the IdP returns a new `refresh_token`
    /// (Linear + Notion rotate on every use), persist BEFORE
    /// returning the access_token. This is the lens C invariant —
    /// persisting the OLD token after a successful refresh would
    /// brick the connection on the next call.
    pub async fn get_valid_mcp_access_token(&self, server_name: &str) -> Result<Option<String>> {
        let provider = format!("{MCP_PROVIDER_PREFIX}{server_name}");
        let profile_id = profile_id(&provider, DEFAULT_PROFILE_NAME);

        let data = self.store.load().await?;
        let Some(profile) = data.profiles.get(&profile_id) else {
            return Ok(None);
        };

        let Some(token_set) = profile.token_set.as_ref() else {
            anyhow::bail!("MCP profile '{profile_id}' is not OAuth-based");
        };

        if !token_set.is_expiring_within(Duration::from_secs(MCP_REFRESH_SKEW_SECS)) {
            return Ok(Some(token_set.access_token.clone()));
        }

        let Some(refresh_token) = token_set.refresh_token.clone() else {
            // No refresh_token — surface the still-valid (or
            // expiring) access_token. The transport will re-auth on
            // its next 401.
            return Ok(Some(token_set.access_token.clone()));
        };

        let refresh_lock = refresh_lock_for_profile(&profile_id);
        let _guard = refresh_lock.lock().await;

        // Double-check after acquiring the lock — another task may
        // have refreshed while we were waiting.
        let data = self.store.load().await?;
        let Some(latest_profile) = data.profiles.get(&profile_id) else {
            return Ok(None);
        };
        let Some(latest_tokens) = latest_profile.token_set.as_ref() else {
            anyhow::bail!("MCP profile '{profile_id}' lost its token_set during refresh");
        };
        if !latest_tokens.is_expiring_within(Duration::from_secs(MCP_REFRESH_SKEW_SECS)) {
            return Ok(Some(latest_tokens.access_token.clone()));
        }
        let refresh_token = latest_tokens.refresh_token.clone().unwrap_or(refresh_token);

        if let Some(remaining) = refresh_backoff_remaining(&profile_id) {
            anyhow::bail!(
                "MCP token refresh for '{server_name}' is in backoff for {remaining}s due to previous failures"
            );
        }

        // Reconstruct the ClientCredentials + endpoints needed by the
        // ceremony's refresh_token_grant.
        let creds = mcp_client_credentials_from_profile(latest_profile)?;
        let token_endpoint = latest_profile
            .metadata
            .get("token_endpoint")
            .ok_or_else(|| anyhow::anyhow!(
                "MCP profile '{profile_id}' is missing `token_endpoint` metadata; re-run plaw auth login --provider mcp:{server_name}"
            ))?
            .clone();
        let resource = latest_profile
            .metadata
            .get("resource")
            .ok_or_else(|| anyhow::anyhow!(
                "MCP profile '{profile_id}' is missing `resource` metadata; re-run plaw auth login --provider mcp:{server_name}"
            ))?
            .clone();

        let refreshed = match refresh_mcp_access_token_with_retries(
            &self.client,
            &token_endpoint,
            &refresh_token,
            &creds,
            &resource,
        )
        .await
        {
            Ok(tokens) => {
                clear_refresh_backoff(&profile_id);
                tokens
            }
            Err(err) => {
                set_refresh_backoff(
                    &profile_id,
                    Duration::from_secs(OPENAI_REFRESH_FAILURE_BACKOFF_SECS),
                );
                return Err(err);
            }
        };

        // Persist BEFORE returning the access token. If the new
        // response did not include a fresh refresh_token (some IdPs
        // omit it when rotation isn't enabled), carry the existing
        // one forward.
        let mut persisted_tokens = refreshed.clone();
        if persisted_tokens.refresh_token.is_none() {
            persisted_tokens
                .refresh_token
                .clone_from(&latest_tokens.refresh_token);
        }

        let updated = self
            .store
            .update_profile(&profile_id, |profile| {
                profile.kind = AuthProfileKind::OAuth;
                profile.token_set = Some(persisted_tokens.clone());
                Ok(())
            })
            .await?;

        Ok(updated.token_set.map(|t| t.access_token))
    }

    /// Delete the MCP profile for `server_name`. Used by the transport
    /// layer (PR #81) when a refresh attempt returns `invalid_grant`
    /// — the refresh_token is permanently dead, the user needs to
    /// re-auth, and leaving the dead profile around would cause
    /// `get_valid_mcp_access_token` to thrash on every transport
    /// call.
    pub async fn invalidate_mcp_profile(&self, server_name: &str) -> Result<bool> {
        let provider = format!("{MCP_PROVIDER_PREFIX}{server_name}");
        self.remove_profile(&provider, DEFAULT_PROFILE_NAME).await
    }

    /// Run the full ceremony (discovery → DCR/pre-registered → PKCE
    /// → loopback redirect → token exchange → persist) for one MCP
    /// server. Driven by the `plaw auth login --provider mcp:<name>`
    /// CLI subcommand (wired in PR #80's main.rs change).
    ///
    /// On success, the user can re-run their plaw session and the
    /// transport layer (PR #81) will find a valid token via
    /// [`Self::get_valid_mcp_access_token`].
    ///
    /// Errors are returned verbatim to the CLI — the ceremony writes
    /// nothing on failure, so a partial run leaves no garbage in
    /// `auth-profiles.json`.
    pub async fn run_mcp_login(
        &self,
        server_name: &str,
        mcp_url: &str,
        oauth_config: &crate::config::McpOAuthConfig,
    ) -> Result<AuthProfile> {
        use super::tools::mcp::oauth::{ceremony, dcr, discovery, ClientCredentials};

        let mcp_origin = origin_of_url(mcp_url)?;
        let prm_url = format!("{mcp_origin}/.well-known/oauth-protected-resource");

        let prm = discovery::fetch_prm(&self.client, &prm_url, &mcp_origin)
            .await
            .with_context(|| format!("fetching PRM for MCP server '{server_name}'"))?;
        let issuer = prm
            .authorization_servers
            .first()
            .ok_or_else(|| anyhow::anyhow!("PRM lists no authorization servers"))?
            .clone();
        let as_metadata = discovery::fetch_as_metadata(&self.client, &issuer)
            .await
            .with_context(|| format!("fetching AS metadata for issuer '{issuer}'"))?;

        // Resolve ClientCredentials: prefer pre-registered config; fall
        // back to RFC 7591 Dynamic Client Registration when supported.
        let secret_store = crate::security::SecretStore::new(std::path::Path::new(""), false);
        let creds = if let Some(client_id) = oauth_config.client_id.clone() {
            if let Some(secret) = oauth_config.client_secret.as_ref() {
                let revealed = secret
                    .reveal(&secret_store)
                    .context("revealing MCP OAuth client_secret from config")?;
                ClientCredentials::PreRegistered {
                    client_id,
                    client_secret: revealed,
                }
            } else {
                ClientCredentials::Public { client_id }
            }
        } else if let Some(registration_endpoint) = as_metadata.registration_endpoint.as_deref() {
            let port = oauth_config.loopback_port.unwrap_or(47830);
            let redirect_uri = ceremony::build_redirect_uri(port);
            let dcr_result =
                dcr::dynamic_register(&self.client, registration_endpoint, &redirect_uri, "plaw")
                    .await
                    .context("RFC 7591 Dynamic Client Registration failed")?;
            if let Some(secret) = dcr_result.client_secret {
                ClientCredentials::PreRegistered {
                    client_id: dcr_result.client_id,
                    client_secret: secret,
                }
            } else {
                ClientCredentials::Public {
                    client_id: dcr_result.client_id,
                }
            }
        } else {
            anyhow::bail!(
                "MCP server '{server_name}' authorization server does not support Dynamic Client \
                 Registration AND no `client_id` is configured. Add `client_id = \"...\"` (and \
                 `client_secret = \"...\"` if required) under `[mcp.servers.{server_name}.transport.oauth]`."
            );
        };

        let token_set = ceremony::run_authorization_code_flow(
            &self.client,
            &prm,
            &as_metadata,
            &creds,
            &oauth_config.scopes,
            oauth_config.loopback_port,
        )
        .await?;

        // Capture the bits needed to refresh later. `resource` is
        // critical (RFC 8707) — store it verbatim.
        let mut metadata = HashMap::new();
        metadata.insert("token_endpoint".into(), as_metadata.token_endpoint.clone());
        metadata.insert(
            "authorize_endpoint".into(),
            as_metadata.authorization_endpoint.clone(),
        );
        metadata.insert("issuer".into(), as_metadata.issuer.clone());
        metadata.insert("resource".into(), prm.resource.clone());

        let (client_id, client_secret) = match creds {
            ClientCredentials::Public { client_id } => (Some(client_id), None),
            ClientCredentials::PreRegistered {
                client_id,
                client_secret,
            } => (Some(client_id), Some(client_secret)),
        };

        self.store_mcp_oauth(server_name, token_set, client_id, client_secret, metadata)
            .await
    }
}

pub fn normalize_provider(provider: &str) -> Result<String> {
    let normalized = provider.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "openai-codex" | "openai_codex" | "codex" => Ok(OPENAI_CODEX_PROVIDER.to_string()),
        "anthropic" | "claude" | "claude-code" => Ok(ANTHROPIC_PROVIDER.to_string()),
        "gemini" | "google" | "vertex" => Ok(GEMINI_PROVIDER.to_string()),
        other if !other.is_empty() => Ok(other.to_string()),
        _ => anyhow::bail!("Provider name cannot be empty"),
    }
}

pub fn state_dir_from_config(config: &Config) -> PathBuf {
    config
        .config_path
        .parent()
        .map_or_else(|| PathBuf::from("."), PathBuf::from)
}

pub fn default_profile_id(provider: &str) -> String {
    profile_id(provider, DEFAULT_PROFILE_NAME)
}

fn resolve_requested_profile_id(provider: &str, requested: &str) -> String {
    if requested.contains(':') {
        requested.to_string()
    } else {
        profile_id(provider, requested)
    }
}

pub fn select_profile_id(
    data: &AuthProfilesData,
    provider: &str,
    profile_override: Option<&str>,
) -> Option<String> {
    if let Some(override_profile) = profile_override {
        let requested = resolve_requested_profile_id(provider, override_profile);
        if data.profiles.contains_key(&requested) {
            return Some(requested);
        }
        return None;
    }

    if let Some(active) = data.active_profiles.get(provider) {
        if data.profiles.contains_key(active) {
            return Some(active.clone());
        }
    }

    let default = default_profile_id(provider);
    if data.profiles.contains_key(&default) {
        return Some(default);
    }

    data.profiles
        .iter()
        .find_map(|(id, profile)| (profile.provider == provider).then(|| id.clone()))
}

async fn refresh_openai_access_token_with_retries(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<TokenSet> {
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=OAUTH_REFRESH_MAX_ATTEMPTS {
        match refresh_access_token(client, refresh_token).await {
            Ok(tokens) => return Ok(tokens),
            Err(err) => {
                let should_retry = attempt < OAUTH_REFRESH_MAX_ATTEMPTS;
                tracing::warn!(
                    attempt,
                    max_attempts = OAUTH_REFRESH_MAX_ATTEMPTS,
                    retry = should_retry,
                    error = %err,
                    "OpenAI token refresh failed"
                );
                last_error = Some(err);
                if should_retry {
                    tokio::time::sleep(Duration::from_millis(
                        OAUTH_REFRESH_RETRY_BASE_DELAY_MS * attempt as u64,
                    ))
                    .await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("OpenAI token refresh failed")))
}

async fn refresh_gemini_access_token_with_retries(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<TokenSet> {
    let mut last_error: Option<anyhow::Error> = None;

    for attempt in 1..=OAUTH_REFRESH_MAX_ATTEMPTS {
        match gemini_oauth::refresh_access_token(client, refresh_token).await {
            Ok(tokens) => return Ok(tokens),
            Err(err) => {
                let should_retry = attempt < OAUTH_REFRESH_MAX_ATTEMPTS;
                tracing::warn!(
                    attempt,
                    max_attempts = OAUTH_REFRESH_MAX_ATTEMPTS,
                    retry = should_retry,
                    error = %err,
                    "Gemini token refresh failed"
                );
                last_error = Some(err);
                if should_retry {
                    tokio::time::sleep(Duration::from_millis(
                        OAUTH_REFRESH_RETRY_BASE_DELAY_MS * attempt as u64,
                    ))
                    .await;
                }
            }
        }
    }

    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Gemini token refresh failed")))
}

fn refresh_lock_for_profile(profile_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();

    let table = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = table.lock().expect("refresh lock table poisoned");

    guard
        .entry(profile_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn refresh_backoff_remaining(profile_id: &str) -> Option<u64> {
    let map = REFRESH_BACKOFFS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().ok()?;
    let now = Instant::now();
    let deadline = guard.get(profile_id).copied()?;
    if deadline <= now {
        guard.remove(profile_id);
        return None;
    }
    Some((deadline - now).as_secs().max(1))
}

fn set_refresh_backoff(profile_id: &str, duration: Duration) {
    let map = REFRESH_BACKOFFS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut guard) = map.lock() {
        guard.insert(profile_id.to_string(), Instant::now() + duration);
    }
}

/// PR #80: MCP refresh-token grant with the same exponential-backoff
/// retry pattern as OpenAI / Gemini. Mirrors
/// `refresh_openai_access_token_with_retries` byte-for-byte; the only
/// difference is the inner call routes through
/// `tools::mcp::oauth::ceremony::refresh_token_grant` which carries
/// the RFC 8707 `resource=` parameter every IdP-bound MCP server
/// requires.
async fn refresh_mcp_access_token_with_retries(
    client: &reqwest::Client,
    token_endpoint: &str,
    refresh_token: &str,
    creds: &crate::tools::mcp::oauth::ClientCredentials,
    resource: &str,
) -> Result<TokenSet> {
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 1..=OAUTH_REFRESH_MAX_ATTEMPTS {
        match crate::tools::mcp::oauth::ceremony::refresh_token_grant(
            client,
            token_endpoint,
            refresh_token,
            creds,
            resource,
        )
        .await
        {
            Ok(tokens) => return Ok(tokens),
            Err(err) => {
                let should_retry = attempt < OAUTH_REFRESH_MAX_ATTEMPTS;
                tracing::warn!(
                    attempt,
                    max_attempts = OAUTH_REFRESH_MAX_ATTEMPTS,
                    retry = should_retry,
                    error = %err,
                    "MCP token refresh failed"
                );
                last_error = Some(err);
                if should_retry {
                    tokio::time::sleep(Duration::from_millis(
                        OAUTH_REFRESH_RETRY_BASE_DELAY_MS * attempt as u64,
                    ))
                    .await;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| anyhow::anyhow!("MCP token refresh failed")))
}

/// Build a [`ClientCredentials`] from a persisted MCP `AuthProfile`.
/// Used during refresh to reconstruct what the original ceremony saw.
fn mcp_client_credentials_from_profile(
    profile: &AuthProfile,
) -> Result<crate::tools::mcp::oauth::ClientCredentials> {
    let client_id = profile.oauth_client_id.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "MCP profile '{}' is missing oauth_client_id; re-run plaw auth login --provider {}",
            profile.id,
            profile.provider
        )
    })?;
    if let Some(secret) = profile.oauth_client_secret.clone() {
        Ok(crate::tools::mcp::oauth::ClientCredentials::PreRegistered {
            client_id,
            client_secret: secret,
        })
    } else {
        Ok(crate::tools::mcp::oauth::ClientCredentials::Public { client_id })
    }
}

/// Compute the `scheme://host[:port]` origin of an MCP server URL.
/// Used to build the PRM well-known URL when WWW-Authenticate did
/// not carry `resource_metadata=` (the loginCLI path; the transport
/// path uses the header in PR #81).
fn origin_of_url(url: &str) -> Result<String> {
    let parsed =
        reqwest::Url::parse(url).with_context(|| format!("MCP url '{url}' did not parse"))?;
    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("MCP url '{url}' has no host"))?;
    let origin = match parsed.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    };
    Ok(origin)
}

fn clear_refresh_backoff(profile_id: &str) {
    let map = REFRESH_BACKOFFS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut guard) = map.lock() {
        guard.remove(profile_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::profiles::{AuthProfile, AuthProfileKind};

    #[test]
    fn normalize_provider_aliases() {
        assert_eq!(normalize_provider("codex").unwrap(), "openai-codex");
        assert_eq!(normalize_provider("claude").unwrap(), "anthropic");
        assert_eq!(normalize_provider("openai").unwrap(), "openai");
    }

    #[test]
    fn select_profile_prefers_override_then_active_then_default() {
        let mut data = AuthProfilesData::default();
        let id_active = profile_id("openai-codex", "work");
        let id_default = profile_id("openai-codex", "default");

        data.profiles.insert(
            id_default.clone(),
            AuthProfile {
                id: id_default.clone(),
                provider: "openai-codex".into(),
                profile_name: "default".into(),
                kind: AuthProfileKind::Token,
                account_id: None,
                workspace_id: None,
                token_set: None,
                token: Some("x".into()),
                metadata: std::collections::BTreeMap::default(),
                oauth_client_id: None,
                oauth_client_secret: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        );
        data.profiles.insert(
            id_active.clone(),
            AuthProfile {
                id: id_active.clone(),
                provider: "openai-codex".into(),
                profile_name: "work".into(),
                kind: AuthProfileKind::Token,
                account_id: None,
                workspace_id: None,
                token_set: None,
                token: Some("y".into()),
                metadata: std::collections::BTreeMap::default(),
                oauth_client_id: None,
                oauth_client_secret: None,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            },
        );

        data.active_profiles
            .insert("openai-codex".into(), id_active.clone());

        assert_eq!(
            select_profile_id(&data, "openai-codex", Some("default")),
            Some(id_default)
        );
        assert_eq!(
            select_profile_id(&data, "openai-codex", None),
            Some(id_active)
        );
    }

    // ── PR #80: MCP OAuth AuthService methods ────────────────────────

    use tempfile::TempDir;

    fn make_token_set(access: &str, refresh: Option<&str>, secs_left: i64) -> TokenSet {
        TokenSet {
            access_token: access.into(),
            refresh_token: refresh.map(str::to_string),
            id_token: None,
            expires_at: Some(chrono::Utc::now() + chrono::Duration::seconds(secs_left)),
            token_type: Some("Bearer".into()),
            scope: Some("read write".into()),
        }
    }

    #[tokio::test]
    async fn store_mcp_oauth_creates_profile_with_mcp_prefix() {
        let tmp = TempDir::new().unwrap();
        let svc = AuthService::new(tmp.path(), false);

        let mut metadata = std::collections::HashMap::new();
        metadata.insert("token_endpoint".into(), "https://t/".into());
        metadata.insert("resource".into(), "https://r/".into());

        let profile = svc
            .store_mcp_oauth(
                "plaw_workspace",
                make_token_set("access1", Some("refresh1"), 3600),
                Some("client_id_value".into()),
                Some("secret_value".into()),
                metadata,
            )
            .await
            .unwrap();

        assert_eq!(profile.provider, "mcp:plaw_workspace");
        assert_eq!(profile.profile_name, "default");
        assert_eq!(profile.oauth_client_id.as_deref(), Some("client_id_value"));
        // The secret survives the round-trip through encrypt_optional
        // when encrypt_secrets = false; when true it would arrive
        // pre-encrypted. Either way the round-trip preserves
        // identity.
        assert_eq!(profile.oauth_client_secret.as_deref(), Some("secret_value"));
        assert!(profile.metadata.contains_key("token_endpoint"));
        assert!(profile.metadata.contains_key("resource"));
    }

    #[tokio::test]
    async fn get_valid_mcp_access_token_returns_existing_when_not_expiring() {
        let tmp = TempDir::new().unwrap();
        let svc = AuthService::new(tmp.path(), false);
        svc.store_mcp_oauth(
            "plaw_workspace",
            make_token_set("still_fresh", Some("r"), 3600),
            Some("cid".into()),
            None,
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();

        let token = svc
            .get_valid_mcp_access_token("plaw_workspace")
            .await
            .unwrap();
        assert_eq!(token.as_deref(), Some("still_fresh"));
    }

    #[tokio::test]
    async fn get_valid_mcp_access_token_returns_none_for_missing_profile() {
        let tmp = TempDir::new().unwrap();
        let svc = AuthService::new(tmp.path(), false);
        let token = svc.get_valid_mcp_access_token("missing").await.unwrap();
        assert!(token.is_none());
    }

    #[tokio::test]
    async fn invalidate_mcp_profile_deletes_existing_profile() {
        let tmp = TempDir::new().unwrap();
        let svc = AuthService::new(tmp.path(), false);
        svc.store_mcp_oauth(
            "plaw_workspace",
            make_token_set("a", Some("r"), 3600),
            Some("cid".into()),
            None,
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();

        let removed = svc.invalidate_mcp_profile("plaw_workspace").await.unwrap();
        assert!(removed);

        // Subsequent get_valid returns None — the profile is truly gone.
        let token = svc
            .get_valid_mcp_access_token("plaw_workspace")
            .await
            .unwrap();
        assert!(token.is_none());
    }

    #[tokio::test]
    async fn invalidate_mcp_profile_returns_false_when_missing() {
        let tmp = TempDir::new().unwrap();
        let svc = AuthService::new(tmp.path(), false);
        let removed = svc.invalidate_mcp_profile("never_existed").await.unwrap();
        assert!(!removed);
    }

    /// Origin helper round-trips http + https + custom port.
    #[test]
    fn origin_of_url_handles_default_and_custom_ports() {
        assert_eq!(
            origin_of_url("https://mcp.example.com/some/path").unwrap(),
            "https://mcp.example.com"
        );
        assert_eq!(
            origin_of_url("http://127.0.0.1:8080/mcp").unwrap(),
            "http://127.0.0.1:8080"
        );
    }

    /// `mcp_client_credentials_from_profile` returns `Public` when
    /// only `oauth_client_id` is set, `PreRegistered` when both are.
    #[test]
    fn mcp_client_credentials_from_profile_chooses_variant_by_secret_presence() {
        let mut profile =
            AuthProfile::new_oauth("mcp:test", "default", make_token_set("a", Some("r"), 3600));
        profile.oauth_client_id = Some("cid".into());
        let creds = mcp_client_credentials_from_profile(&profile).unwrap();
        match creds {
            crate::tools::mcp::oauth::ClientCredentials::Public { client_id } => {
                assert_eq!(client_id, "cid");
            }
            _ => panic!("expected Public variant when no secret"),
        }
        profile.oauth_client_secret = Some("sek".into());
        let creds = mcp_client_credentials_from_profile(&profile).unwrap();
        match creds {
            crate::tools::mcp::oauth::ClientCredentials::PreRegistered {
                client_id,
                client_secret,
            } => {
                assert_eq!(client_id, "cid");
                assert_eq!(client_secret, "sek");
            }
            _ => panic!("expected PreRegistered variant when secret present"),
        }
    }
}
