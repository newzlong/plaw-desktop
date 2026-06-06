use crate::security::SecretStore;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::time::sleep;

/// On-disk schema version for `auth-profiles.json`.
///
/// Bumped from 1 → 2 in PR #79 (MCP OAuth foundations) to accept
/// the new `oauth_client_id` + `oauth_client_secret` fields on each
/// profile. Forward-compat is verified by an explicit unit test:
/// a v1 file (no oauth_* fields) loads cleanly under v2 because both
/// new fields default to `None` via `#[serde(default)]`.
///
/// Backward-compat: a v2 file CANNOT load under code that thinks
/// CURRENT = 1; users who roll back PR #79 will get a "schema
/// version 2 unsupported" error on next plaw boot. Acceptable per
/// PR plan — auth-profiles.json is local user state, no
/// cross-version distribution.
const CURRENT_SCHEMA_VERSION: u32 = 2;
const PROFILES_FILENAME: &str = "auth-profiles.json";
const LOCK_FILENAME: &str = "auth-profiles.lock";
const LOCK_WAIT_MS: u64 = 50;
const LOCK_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthProfileKind {
    OAuth,
    Token,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

impl TokenSet {
    pub fn is_expiring_within(&self, skew: Duration) -> bool {
        match self.expires_at {
            Some(expires_at) => {
                let now_plus_skew =
                    Utc::now() + chrono::Duration::from_std(skew).unwrap_or_default();
                expires_at <= now_plus_skew
            }
            None => false,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AuthProfile {
    pub id: String,
    pub provider: String,
    pub profile_name: String,
    pub kind: AuthProfileKind,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub token_set: Option<TokenSet>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// RFC 7591 Dynamic Client Registration result OR pre-registered
    /// OAuth App `client_id`. Plaintext per RFC 7591 — client_id is a
    /// public identifier. PR #79 adds this field as a Phase 1
    /// foundation; populated by the OAuth ceremony in PR #80.
    #[serde(default)]
    pub oauth_client_id: Option<String>,
    /// Pre-registered OAuth App `client_secret`. Encrypted at rest
    /// (same encrypt_optional path as `refresh_token`). Some OAuth
    /// providers (GitHub) require a secret even for native
    /// public-client flows; many (Linear, Notion via DCR) do not.
    /// `None` for public clients per RFC 7591 §2 + RFC 7636 PKCE.
    #[serde(default)]
    pub oauth_client_secret: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl std::fmt::Debug for AuthProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `oauth_client_secret` and the `token_set` access/refresh tokens
        // are intentionally NOT included — Debug output reaches tracing
        // events, panic backtraces, and other channels where leaking a
        // secret has real consequences.
        f.debug_struct("AuthProfile")
            .field("id", &self.id)
            .field("provider", &self.provider)
            .field("profile_name", &self.profile_name)
            .field("kind", &self.kind)
            .field("workspace_id", &self.workspace_id)
            .field("metadata", &self.metadata)
            .field("oauth_client_id", &self.oauth_client_id)
            .field("created_at", &self.created_at)
            .field("updated_at", &self.updated_at)
            .finish_non_exhaustive()
    }
}

impl AuthProfile {
    pub fn new_oauth(provider: &str, profile_name: &str, token_set: TokenSet) -> Self {
        let now = Utc::now();
        let id = profile_id(provider, profile_name);
        Self {
            id,
            provider: provider.to_string(),
            profile_name: profile_name.to_string(),
            kind: AuthProfileKind::OAuth,
            account_id: None,
            workspace_id: None,
            token_set: Some(token_set),
            token: None,
            metadata: BTreeMap::new(),
            oauth_client_id: None,
            oauth_client_secret: None,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn new_token(provider: &str, profile_name: &str, token: String) -> Self {
        let now = Utc::now();
        let id = profile_id(provider, profile_name);
        Self {
            id,
            provider: provider.to_string(),
            profile_name: profile_name.to_string(),
            kind: AuthProfileKind::Token,
            account_id: None,
            workspace_id: None,
            token_set: None,
            token: Some(token),
            metadata: BTreeMap::new(),
            oauth_client_id: None,
            oauth_client_secret: None,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthProfilesData {
    pub schema_version: u32,
    pub updated_at: DateTime<Utc>,
    pub active_profiles: BTreeMap<String, String>,
    pub profiles: BTreeMap<String, AuthProfile>,
}

impl Default for AuthProfilesData {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            updated_at: Utc::now(),
            active_profiles: BTreeMap::new(),
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AuthProfilesStore {
    path: PathBuf,
    lock_path: PathBuf,
    secret_store: SecretStore,
}

impl AuthProfilesStore {
    pub fn new(state_dir: &Path, encrypt_secrets: bool) -> Self {
        Self {
            path: state_dir.join(PROFILES_FILENAME),
            lock_path: state_dir.join(LOCK_FILENAME),
            secret_store: SecretStore::new(state_dir, encrypt_secrets),
        }
    }

    /// Test-only accessor for the on-disk profiles file path. Production
    /// code accesses the path indirectly through the load/save methods;
    /// the in-tree consumers are the two unit tests at the bottom of
    /// this file. Gated `#[cfg(test)]` so it doesn't fire dead_code in
    /// non-test builds.
    #[cfg(test)]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn load(&self) -> Result<AuthProfilesData> {
        let _lock = self.acquire_lock().await?;
        self.load_locked().await
    }

    pub async fn upsert_profile(&self, mut profile: AuthProfile, set_active: bool) -> Result<()> {
        let _lock = self.acquire_lock().await?;
        let mut data = self.load_locked().await?;

        profile.updated_at = Utc::now();
        if let Some(existing) = data.profiles.get(&profile.id) {
            profile.created_at = existing.created_at;
        }

        if set_active {
            data.active_profiles
                .insert(profile.provider.clone(), profile.id.clone());
        }

        data.profiles.insert(profile.id.clone(), profile);
        data.updated_at = Utc::now();

        self.save_locked(&data).await
    }

    pub async fn remove_profile(&self, profile_id: &str) -> Result<bool> {
        let _lock = self.acquire_lock().await?;
        let mut data = self.load_locked().await?;

        let removed = data.profiles.remove(profile_id).is_some();
        if !removed {
            return Ok(false);
        }

        data.active_profiles
            .retain(|_, active| active != profile_id);
        data.updated_at = Utc::now();
        self.save_locked(&data).await?;
        Ok(true)
    }

    pub async fn set_active_profile(&self, provider: &str, profile_id: &str) -> Result<()> {
        let _lock = self.acquire_lock().await?;
        let mut data = self.load_locked().await?;

        if !data.profiles.contains_key(profile_id) {
            anyhow::bail!("Auth profile not found: {profile_id}");
        }

        data.active_profiles
            .insert(provider.to_string(), profile_id.to_string());
        data.updated_at = Utc::now();
        self.save_locked(&data).await
    }

    pub async fn update_profile<F>(&self, profile_id: &str, mut updater: F) -> Result<AuthProfile>
    where
        F: FnMut(&mut AuthProfile) -> Result<()>,
    {
        let _lock = self.acquire_lock().await?;
        let mut data = self.load_locked().await?;

        let profile = data
            .profiles
            .get_mut(profile_id)
            .ok_or_else(|| anyhow::anyhow!("Auth profile not found: {profile_id}"))?;

        updater(profile)?;
        profile.updated_at = Utc::now();
        let updated_profile = profile.clone();
        data.updated_at = Utc::now();
        self.save_locked(&data).await?;
        Ok(updated_profile)
    }

    async fn load_locked(&self) -> Result<AuthProfilesData> {
        let mut persisted = self.read_persisted_locked().await?;
        let mut migrated = false;

        let mut profiles = BTreeMap::new();
        for (id, p) in &mut persisted.profiles {
            let (access_token, access_migrated) =
                self.decrypt_optional(p.access_token.as_deref())?;
            let (refresh_token, refresh_migrated) =
                self.decrypt_optional(p.refresh_token.as_deref())?;
            let (id_token, id_migrated) = self.decrypt_optional(p.id_token.as_deref())?;
            let (token, token_migrated) = self.decrypt_optional(p.token.as_deref())?;
            // PR #79: oauth_client_secret rides the same encrypted-at-rest
            // path as refresh_token / access_token. oauth_client_id stays
            // plaintext per RFC 7591 (it's a public identifier).
            let (oauth_client_secret, oauth_client_secret_migrated) =
                self.decrypt_optional(p.oauth_client_secret.as_deref())?;

            if let Some(value) = access_migrated {
                p.access_token = Some(value);
                migrated = true;
            }
            if let Some(value) = refresh_migrated {
                p.refresh_token = Some(value);
                migrated = true;
            }
            if let Some(value) = id_migrated {
                p.id_token = Some(value);
                migrated = true;
            }
            if let Some(value) = token_migrated {
                p.token = Some(value);
                migrated = true;
            }
            if let Some(value) = oauth_client_secret_migrated {
                p.oauth_client_secret = Some(value);
                migrated = true;
            }

            let kind = parse_profile_kind(&p.kind)?;
            let token_set = match kind {
                AuthProfileKind::OAuth => {
                    let access = access_token.ok_or_else(|| {
                        anyhow::anyhow!("OAuth profile missing access_token: {id}")
                    })?;
                    Some(TokenSet {
                        access_token: access,
                        refresh_token,
                        id_token,
                        expires_at: parse_optional_datetime(p.expires_at.as_deref())?,
                        token_type: p.token_type.clone(),
                        scope: p.scope.clone(),
                    })
                }
                AuthProfileKind::Token => None,
            };

            profiles.insert(
                id.clone(),
                AuthProfile {
                    id: id.clone(),
                    provider: p.provider.clone(),
                    profile_name: p.profile_name.clone(),
                    kind,
                    account_id: p.account_id.clone(),
                    workspace_id: p.workspace_id.clone(),
                    token_set,
                    token,
                    metadata: p.metadata.clone(),
                    oauth_client_id: p.oauth_client_id.clone(),
                    oauth_client_secret,
                    created_at: parse_datetime_with_fallback(&p.created_at),
                    updated_at: parse_datetime_with_fallback(&p.updated_at),
                },
            );
        }

        if migrated {
            self.write_persisted_locked(&persisted).await?;
        }

        Ok(AuthProfilesData {
            schema_version: persisted.schema_version,
            updated_at: parse_datetime_with_fallback(&persisted.updated_at),
            active_profiles: persisted.active_profiles,
            profiles,
        })
    }

    async fn save_locked(&self, data: &AuthProfilesData) -> Result<()> {
        let mut persisted = PersistedAuthProfiles {
            schema_version: CURRENT_SCHEMA_VERSION,
            updated_at: data.updated_at.to_rfc3339(),
            active_profiles: data.active_profiles.clone(),
            profiles: BTreeMap::new(),
        };

        for (id, profile) in &data.profiles {
            let (access_token, refresh_token, id_token, expires_at, token_type, scope) =
                match (&profile.kind, &profile.token_set) {
                    (AuthProfileKind::OAuth, Some(token_set)) => (
                        self.encrypt_optional(Some(&token_set.access_token))?,
                        self.encrypt_optional(token_set.refresh_token.as_deref())?,
                        self.encrypt_optional(token_set.id_token.as_deref())?,
                        token_set.expires_at.as_ref().map(DateTime::to_rfc3339),
                        token_set.token_type.clone(),
                        token_set.scope.clone(),
                    ),
                    _ => (None, None, None, None, None, None),
                };

            let token = self.encrypt_optional(profile.token.as_deref())?;
            // PR #79: encrypt oauth_client_secret at rest. oauth_client_id
            // stays plaintext (RFC 7591 public identifier).
            let oauth_client_secret =
                self.encrypt_optional(profile.oauth_client_secret.as_deref())?;

            persisted.profiles.insert(
                id.clone(),
                PersistedAuthProfile {
                    provider: profile.provider.clone(),
                    profile_name: profile.profile_name.clone(),
                    kind: profile_kind_to_string(profile.kind).to_string(),
                    account_id: profile.account_id.clone(),
                    workspace_id: profile.workspace_id.clone(),
                    access_token,
                    refresh_token,
                    id_token,
                    token,
                    expires_at,
                    token_type,
                    scope,
                    metadata: profile.metadata.clone(),
                    created_at: profile.created_at.to_rfc3339(),
                    updated_at: profile.updated_at.to_rfc3339(),
                    oauth_client_id: profile.oauth_client_id.clone(),
                    oauth_client_secret,
                },
            );
        }

        self.write_persisted_locked(&persisted).await
    }

    async fn read_persisted_locked(&self) -> Result<PersistedAuthProfiles> {
        if !self.path.exists() {
            return Ok(PersistedAuthProfiles::default());
        }

        let bytes = fs::read(&self.path).await.with_context(|| {
            format!(
                "Failed to read auth profile store at {}",
                self.path.display()
            )
        })?;

        if bytes.is_empty() {
            return Ok(PersistedAuthProfiles::default());
        }

        let mut persisted: PersistedAuthProfiles =
            serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "Failed to parse auth profile store at {}",
                    self.path.display()
                )
            })?;

        if persisted.schema_version == 0 {
            persisted.schema_version = CURRENT_SCHEMA_VERSION;
        }

        if persisted.schema_version > CURRENT_SCHEMA_VERSION {
            anyhow::bail!(
                "Unsupported auth profile schema version {} (max supported: {})",
                persisted.schema_version,
                CURRENT_SCHEMA_VERSION
            );
        }

        Ok(persisted)
    }

    async fn write_persisted_locked(&self, persisted: &PersistedAuthProfiles) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!(
                    "Failed to create auth profile directory at {}",
                    parent.display()
                )
            })?;
        }

        let json =
            serde_json::to_vec_pretty(persisted).context("Failed to serialize auth profiles")?;
        let tmp_name = format!(
            "{}.tmp.{}.{}",
            PROFILES_FILENAME,
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let tmp_path = self.path.with_file_name(tmp_name);

        fs::write(&tmp_path, &json).await.with_context(|| {
            format!(
                "Failed to write temporary auth profile file at {}",
                tmp_path.display()
            )
        })?;

        fs::rename(&tmp_path, &self.path).await.with_context(|| {
            format!(
                "Failed to replace auth profile store at {}",
                self.path.display()
            )
        })?;

        Ok(())
    }

    fn encrypt_optional(&self, value: Option<&str>) -> Result<Option<String>> {
        match value {
            Some(value) if !value.is_empty() => self.secret_store.encrypt(value).map(Some),
            Some(_) | None => Ok(None),
        }
    }

    fn decrypt_optional(&self, value: Option<&str>) -> Result<(Option<String>, Option<String>)> {
        match value {
            Some(value) if !value.is_empty() => {
                let (plaintext, migrated) = self.secret_store.decrypt_and_migrate(value)?;
                Ok((Some(plaintext), migrated))
            }
            Some(_) | None => Ok((None, None)),
        }
    }

    async fn acquire_lock(&self) -> Result<AuthProfileLockGuard> {
        if let Some(parent) = self.lock_path.parent() {
            fs::create_dir_all(parent).await.with_context(|| {
                format!("Failed to create lock directory at {}", parent.display())
            })?;
        }

        let mut waited = 0_u64;
        loop {
            match OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&self.lock_path)
                .await
            {
                Ok(mut file) => {
                    let mut buffer = Vec::new();
                    writeln!(&mut buffer, "pid={}", std::process::id())?;
                    if let Err(e) = file.write_all(&buffer).await {
                        fs::remove_file(&self.lock_path)
                            .await
                            .inspect(|e| {
                                tracing::error!("Failed to remove auth profile lock file: {e:?}");
                            })
                            .ok();
                        return Err(e).with_context(|| {
                            format!(
                                "Failed to write auth profile lock at {}",
                                self.lock_path.display()
                            )
                        });
                    }
                    return Ok(AuthProfileLockGuard {
                        lock_path: self.lock_path.clone(),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if waited >= LOCK_TIMEOUT_MS {
                        anyhow::bail!(
                            "Timed out waiting for auth profile lock at {}",
                            self.lock_path.display()
                        );
                    }
                    sleep(Duration::from_millis(LOCK_WAIT_MS)).await;
                    waited = waited.saturating_add(LOCK_WAIT_MS);
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!(
                            "Failed to create auth profile lock at {}",
                            self.lock_path.display()
                        )
                    });
                }
            }
        }
    }
}

struct AuthProfileLockGuard {
    lock_path: PathBuf,
}

impl Drop for AuthProfileLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAuthProfiles {
    #[serde(default = "default_schema_version")]
    schema_version: u32,
    #[serde(default = "default_now_rfc3339")]
    updated_at: String,
    #[serde(default)]
    active_profiles: BTreeMap<String, String>,
    #[serde(default)]
    profiles: BTreeMap<String, PersistedAuthProfile>,
}

impl Default for PersistedAuthProfiles {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            updated_at: default_now_rfc3339(),
            active_profiles: BTreeMap::new(),
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PersistedAuthProfile {
    provider: String,
    profile_name: String,
    kind: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    workspace_id: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default = "default_now_rfc3339")]
    created_at: String,
    #[serde(default = "default_now_rfc3339")]
    updated_at: String,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    /// PR #79 — see [`AuthProfile::oauth_client_id`]. Plaintext.
    /// `#[serde(default)]` so v1 files (no field) load cleanly as `None`.
    #[serde(default)]
    oauth_client_id: Option<String>,
    /// PR #79 — see [`AuthProfile::oauth_client_secret`]. Encrypted at
    /// rest via `encrypt_optional` (same path as `refresh_token`).
    #[serde(default)]
    oauth_client_secret: Option<String>,
}

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

fn default_now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

fn parse_profile_kind(value: &str) -> Result<AuthProfileKind> {
    match value {
        "oauth" => Ok(AuthProfileKind::OAuth),
        "token" => Ok(AuthProfileKind::Token),
        other => anyhow::bail!("Unsupported auth profile kind: {other}"),
    }
}

fn profile_kind_to_string(kind: AuthProfileKind) -> &'static str {
    match kind {
        AuthProfileKind::OAuth => "oauth",
        AuthProfileKind::Token => "token",
    }
}

fn parse_optional_datetime(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    value.map(parse_datetime).transpose()
}

fn parse_datetime(value: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .with_context(|| format!("Invalid RFC3339 timestamp: {value}"))
}

fn parse_datetime_with_fallback(value: &str) -> DateTime<Utc> {
    parse_datetime(value).unwrap_or_else(|_| Utc::now())
}

pub fn profile_id(provider: &str, profile_name: &str) -> String {
    format!("{}:{}", provider.trim(), profile_name.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn profile_id_format() {
        assert_eq!(
            profile_id("openai-codex", "default"),
            "openai-codex:default"
        );
    }

    #[test]
    fn token_expiry_math() {
        let token_set = TokenSet {
            access_token: "token".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: Some(Utc::now() + chrono::Duration::seconds(10)),
            token_type: Some("Bearer".into()),
            scope: None,
        };

        assert!(token_set.is_expiring_within(Duration::from_secs(15)));
        assert!(!token_set.is_expiring_within(Duration::from_secs(1)));
    }

    #[tokio::test]
    async fn store_roundtrip_with_encryption() {
        let tmp = TempDir::new().unwrap();
        let store = AuthProfilesStore::new(tmp.path(), true);

        let mut profile = AuthProfile::new_oauth(
            "openai-codex",
            "default",
            TokenSet {
                access_token: "access-123".into(),
                refresh_token: Some("refresh-123".into()),
                id_token: None,
                expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
                token_type: Some("Bearer".into()),
                scope: Some("openid offline_access".into()),
            },
        );
        profile.account_id = Some("acct_123".into());

        store.upsert_profile(profile.clone(), true).await.unwrap();

        let data = store.load().await.unwrap();
        let loaded = data.profiles.get(&profile.id).unwrap();

        assert_eq!(loaded.provider, "openai-codex");
        assert_eq!(loaded.profile_name, "default");
        assert_eq!(loaded.account_id.as_deref(), Some("acct_123"));
        assert_eq!(
            loaded
                .token_set
                .as_ref()
                .and_then(|t| t.refresh_token.as_deref()),
            Some("refresh-123")
        );

        let raw = tokio::fs::read_to_string(store.path()).await.unwrap();
        assert!(raw.contains("enc2:"));
        assert!(!raw.contains("refresh-123"));
        assert!(!raw.contains("access-123"));
    }

    #[tokio::test]
    async fn atomic_write_replaces_file() {
        let tmp = TempDir::new().unwrap();
        let store = AuthProfilesStore::new(tmp.path(), false);

        let profile = AuthProfile::new_token("anthropic", "default", "token-abc".into());
        store.upsert_profile(profile, true).await.unwrap();

        let path = store.path().to_path_buf();
        assert!(path.exists());

        let contents = tokio::fs::read_to_string(path).await.unwrap();
        // PR #79 bumped CURRENT_SCHEMA_VERSION 1 → 2 for oauth_client_* fields.
        assert!(contents.contains("\"schema_version\": 2"));
    }

    // ── PR #79 OAuth foundations ─────────────────────────────────────

    /// V1 files on disk MUST load cleanly under V2 code without losing
    /// data — the two new fields (`oauth_client_id` / `oauth_client_secret`)
    /// are `#[serde(default)]` so absent fields become `None`. This is
    /// the trust-root regression test: existing logged-in users
    /// (OpenAI Codex, Gemini, Anthropic) must NOT break on next plaw
    /// boot after the schema bump.
    #[tokio::test]
    async fn schema_v1_file_loads_into_v2_with_oauth_fields_defaulted() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("auth-profiles.json");
        // Hand-write a v1-shaped file by hand — no oauth_client_id, no
        // oauth_client_secret. Use kind="token" so we don't need to
        // exercise the encrypted token_set path.
        let v1_json = r#"{
            "schema_version": 1,
            "updated_at": "2026-01-01T00:00:00Z",
            "active_profiles": {},
            "profiles": {
                "anthropic:default": {
                    "provider": "anthropic",
                    "profile_name": "default",
                    "kind": "token",
                    "token": "legacy-token-value",
                    "created_at": "2026-01-01T00:00:00Z",
                    "updated_at": "2026-01-01T00:00:00Z"
                }
            }
        }"#;
        tokio::fs::write(&path, v1_json).await.unwrap();

        let store = AuthProfilesStore::new(tmp.path(), false);
        let data = store.load().await.unwrap();
        let loaded = data
            .profiles
            .get("anthropic:default")
            .expect("v1 profile must survive load");

        assert_eq!(loaded.provider, "anthropic");
        assert_eq!(loaded.token.as_deref(), Some("legacy-token-value"));
        // New v2 fields default to None — no data loss, no fabricated value.
        assert!(loaded.oauth_client_id.is_none());
        assert!(loaded.oauth_client_secret.is_none());
    }

    /// V2 file with a v3 schema_version must be REJECTED with a clear
    /// "unsupported schema version" error — the existing read_persisted_locked
    /// invariant. Locks in the rollback story: a user on V2 who rolls
    /// back to V1 code MUST get this error and know to upgrade.
    #[tokio::test]
    async fn future_schema_version_rejected_with_clear_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("auth-profiles.json");
        let future_json = r#"{
            "schema_version": 999,
            "updated_at": "2026-01-01T00:00:00Z",
            "active_profiles": {},
            "profiles": {}
        }"#;
        tokio::fs::write(&path, future_json).await.unwrap();

        let store = AuthProfilesStore::new(tmp.path(), false);
        let err = store.load().await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Unsupported auth profile schema version 999"),
            "got: {msg}"
        );
    }

    /// Round-trip an OAuth profile with `oauth_client_id` (plaintext per
    /// RFC 7591) + `oauth_client_secret` (encrypted at rest). Asserts:
    /// (a) the raw file does NOT contain the plaintext secret, and
    /// (b) reload decrypts back to the original. This is the security
    /// invariant that justifies encrypt_optional treating the secret
    /// the same as refresh_token.
    #[tokio::test]
    async fn oauth_client_credentials_round_trip_encrypted_secret() {
        let tmp = TempDir::new().unwrap();
        let store = AuthProfilesStore::new(tmp.path(), true);

        let mut profile = AuthProfile::new_oauth(
            "mcp:plaw_workspace",
            "default",
            TokenSet {
                access_token: "access-xyz".into(),
                refresh_token: Some("refresh-xyz".into()),
                id_token: None,
                expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
                token_type: Some("Bearer".into()),
                scope: None,
            },
        );
        profile.oauth_client_id = Some("Iv1.public-id".into());
        profile.oauth_client_secret = Some("super-secret-client-secret".into());

        store.upsert_profile(profile.clone(), true).await.unwrap();
        let data = store.load().await.unwrap();
        let loaded = data.profiles.get(&profile.id).unwrap();

        // Plaintext client_id round-trips literal.
        assert_eq!(loaded.oauth_client_id.as_deref(), Some("Iv1.public-id"));
        // Secret round-trips literal AFTER decrypt.
        assert_eq!(
            loaded.oauth_client_secret.as_deref(),
            Some("super-secret-client-secret")
        );

        // Raw on-disk file: plaintext secret MUST NOT appear; the
        // `enc2:` prefix MUST appear (proves encrypt path fired).
        let raw = tokio::fs::read_to_string(store.path()).await.unwrap();
        assert!(
            !raw.contains("super-secret-client-secret"),
            "client_secret leaked to disk in plaintext: {raw}"
        );
        assert!(raw.contains("enc2:"), "expected encrypt marker");
        // Plaintext client_id should appear (it's a public identifier).
        assert!(raw.contains("Iv1.public-id"));
    }

    /// Debug-format omits oauth_client_secret even when populated.
    /// Same invariant as access/refresh tokens — prevents
    /// `tracing::debug!(?profile)` from leaking credentials.
    #[test]
    fn debug_omits_oauth_client_secret() {
        let mut profile = AuthProfile::new_oauth(
            "mcp:plaw_workspace",
            "default",
            TokenSet {
                access_token: "redacted-too".into(),
                refresh_token: Some("also-redacted".into()),
                id_token: None,
                expires_at: None,
                token_type: None,
                scope: None,
            },
        );
        profile.oauth_client_secret = Some("must-not-leak".into());
        let dbg = format!("{profile:?}");
        assert!(!dbg.contains("must-not-leak"), "secret leaked: {dbg}");
        assert!(!dbg.contains("redacted-too"), "access_token leaked: {dbg}");
        assert!(
            !dbg.contains("also-redacted"),
            "refresh_token leaked: {dbg}"
        );
    }
}
