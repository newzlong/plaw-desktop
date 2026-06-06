pub mod schema;
pub mod traits;

#[allow(unused_imports)]
pub use schema::{
    apply_runtime_proxy_to_builder, build_runtime_proxy_client,
    build_runtime_proxy_client_with_timeouts, runtime_proxy_config, set_runtime_proxy_config,
    AgentConfig, AgentsIpcConfig, AuditConfig, AutonomyConfig, BrowserComputerUseConfig,
    BrowserConfig, BuiltinHooksConfig, ChainOfVerificationConfig, ChannelsConfig,
    ClassificationRule, ComposioConfig, Config, CoordinationConfig, CostConfig, CronConfig,
    DelegateAgentConfig, DiscordConfig,
    DockerRuntimeConfig, EditLinterConfig, EditLinterMode, EmbeddingRouteConfig, EstopConfig, FeishuConfig, GatewayConfig,
    GroupReplyConfig, GroupReplyMode, HardwareConfig, HardwareTransport, HeartbeatConfig,
    HooksConfig, HttpRequestConfig, IMessageConfig, IdentityConfig, LarkConfig, MatrixConfig,
    McpConfig, McpServerConfig, McpTransport, MemoryConfig, ModelRouteConfig, MultimodalConfig,
    NextcloudTalkConfig,
    NonCliNaturalLanguageApprovalMode, ObservabilityConfig, OtpConfig, OtpMethod,
    PeripheralBoardConfig, PeripheralsConfig, PipelineConfig, PipelineErrorPolicy,
    PipelineStage, ProviderConfig, ProxyConfig, ProxyScope,
    QdrantConfig, QueryClassificationConfig, ReliabilityConfig, RepoMapConfig, ResearchPhaseConfig,
    ResearchTrigger, ResourceLimitsConfig, RuntimeConfig, SandboxBackend, SandboxConfig,
    SchedulerConfig, SecretsConfig, SecurityConfig, SkillsConfig, SkillsPromptInjectionMode,
    SlackConfig, StorageConfig, StorageProviderConfig, StorageProviderSection, StreamMode,
    SyscallAnomalyConfig, TelegramConfig, TranscriptionConfig, TunnelConfig,
    WasmCapabilityEscalationMode, WasmModuleHashPolicy, WasmRuntimeConfig, WasmSecurityConfig,
    WebFetchConfig, WebSearchConfig, WebhookConfig,
};

pub fn name_and_presence<T: traits::ChannelConfig>(channel: Option<&T>) -> (&'static str, bool) {
    (T::name(), channel.is_some())
}

/// Build a [`crate::security::SecretStore`] from the same plaw_dir +
/// encryption flag that the gateway uses.
///
/// Replaces a 5-line `let plaw_dir = config.config_path.parent()...;
/// let store = SecretStore::new(&plaw_dir, config.secrets.encrypt);`
/// idiom that had accumulated in 5 production call sites
/// (channels::collect_configured_channels, the 4 cron::scheduler
/// delivery arms). Each call site reduces to one line:
///
/// ```ignore
/// let secret_store = crate::config::secret_store_for(config);
/// let token = some_secret_field.reveal(&secret_store)?;
/// ```
///
/// SecretStore::new is cheap (just a PathBuf + bool), so we don't
/// memoize / share an instance — each Secret-reading scope builds
/// its own. The gateway's AppState still caches an `Arc<SecretStore>`
/// because it serves many concurrent handlers; everyone else uses
/// this helper.
pub fn secret_store_for(config: &Config) -> crate::security::SecretStore {
    let plaw_dir = config
        .config_path
        .parent()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    crate::security::SecretStore::new(&plaw_dir, config.secrets.encrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexported_config_default_is_constructible() {
        let config = Config::default();

        assert!(config.default_provider.is_some());
        assert!(config.default_model.is_some());
        assert!(config.default_temperature > 0.0);
    }

    #[test]
    fn reexported_channel_configs_are_constructible() {
        let telegram = TelegramConfig {
            bot_token: crate::security::Secret::from_wire("token".into()),
            allowed_users: vec!["alice".into()],
            stream_mode: StreamMode::default(),
            draft_update_interval_ms: 1000,
            interrupt_on_new_message: false,
            mention_only: false,
            group_reply: None,
            base_url: None,
        };

        let discord = DiscordConfig {
            bot_token: crate::security::Secret::from_wire("token".into()),
            guild_id: Some("123".into()),
            allowed_users: vec![],
            listen_to_bots: false,
            mention_only: false,
            group_reply: None,
        };

        let lark = LarkConfig {
            app_id: "app-id".into(),
            app_secret: crate::security::Secret::from_wire("app-secret".into()),
            encrypt_key: None,
            verification_token: None,
            allowed_users: vec![],
            mention_only: false,
            group_reply: None,
            use_feishu: false,
            receive_mode: crate::config::schema::LarkReceiveMode::Websocket,
            port: None,
            draft_update_interval_ms: crate::config::schema::default_lark_draft_update_interval_ms(
            ),
            max_draft_edits: crate::config::schema::default_lark_max_draft_edits(),
        };
        let feishu = FeishuConfig {
            app_id: "app-id".into(),
            app_secret: crate::security::Secret::from_wire("app-secret".into()),
            encrypt_key: None,
            verification_token: None,
            allowed_users: vec![],
            group_reply: None,
            receive_mode: crate::config::schema::LarkReceiveMode::Websocket,
            port: None,
            draft_update_interval_ms: crate::config::schema::default_lark_draft_update_interval_ms(
            ),
            max_draft_edits: crate::config::schema::default_lark_max_draft_edits(),
        };

        let nextcloud_talk = NextcloudTalkConfig {
            base_url: "https://cloud.example.com".into(),
            app_token: crate::security::Secret::from_wire("app-token".into()),
            webhook_secret: None,
            allowed_users: vec!["*".into()],
        };

        assert_eq!(telegram.allowed_users.len(), 1);
        assert_eq!(discord.guild_id.as_deref(), Some("123"));
        assert_eq!(lark.app_id, "app-id");
        assert_eq!(feishu.app_id, "app-id");
        assert_eq!(nextcloud_talk.base_url, "https://cloud.example.com");
    }
}
