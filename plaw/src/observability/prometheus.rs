use super::traits::{Observer, ObserverEvent, ObserverMetric};
use prometheus::{
    Encoder, GaugeVec, Histogram, HistogramOpts, HistogramVec, IntCounterVec, Registry, TextEncoder,
};

/// Prometheus-backed observer — exposes metrics for scraping via `/metrics`.
pub struct PrometheusObserver {
    registry: Registry,

    // Counters
    agent_starts: IntCounterVec,
    llm_requests: IntCounterVec,
    tokens_input_total: IntCounterVec,
    tokens_output_total: IntCounterVec,
    /// Prefix-cache write tokens billed at 1.25× input (Anthropic + Bedrock).
    /// `tokens_input_total` already includes these — the breakdown counter
    /// lets dashboards compute the cache hit ratio as
    /// `cache_read / (cache_read + cache_creation + uncached)` where
    /// `uncached = input - cache_read - cache_creation`.
    tokens_cache_creation_total: IntCounterVec,
    /// Prefix-cache read tokens billed at 0.1× input. The metric users
    /// watch for cost savings — high values mean the cache is paying off.
    tokens_cache_read_total: IntCounterVec,
    tool_calls: IntCounterVec,
    channel_messages: IntCounterVec,
    heartbeat_ticks: prometheus::IntCounter,
    errors: IntCounterVec,

    // Histograms
    agent_duration: HistogramVec,
    tool_duration: HistogramVec,
    request_latency: Histogram,

    // Gauges
    tokens_used: prometheus::IntGauge,
    active_sessions: GaugeVec,
    queue_depth: GaugeVec,
}

impl PrometheusObserver {
    pub fn new() -> Self {
        let registry = Registry::new();

        let agent_starts = IntCounterVec::new(
            prometheus::Opts::new("plaw_agent_starts_total", "Total agent invocations"),
            &["provider", "model"],
        )
        .expect("valid metric");

        let llm_requests = IntCounterVec::new(
            prometheus::Opts::new("plaw_llm_requests_total", "Total LLM provider requests"),
            &["provider", "model", "success"],
        )
        .expect("valid metric");

        let tokens_input_total = IntCounterVec::new(
            prometheus::Opts::new("plaw_tokens_input_total", "Total input tokens consumed"),
            &["provider", "model"],
        )
        .expect("valid metric");

        let tokens_output_total = IntCounterVec::new(
            prometheus::Opts::new("plaw_tokens_output_total", "Total output tokens consumed"),
            &["provider", "model"],
        )
        .expect("valid metric");

        let tokens_cache_creation_total = IntCounterVec::new(
            prometheus::Opts::new(
                "plaw_tokens_cache_creation_total",
                "Tokens billed at the prefix-cache WRITE rate (1.25x input). \
                 Populated by Anthropic + Bedrock w/ Claude only.",
            ),
            &["provider", "model"],
        )
        .expect("valid metric");

        let tokens_cache_read_total = IntCounterVec::new(
            prometheus::Opts::new(
                "plaw_tokens_cache_read_total",
                "Tokens billed at the prefix-cache HIT rate (0.1x input). \
                 The cost-savings metric — higher is better.",
            ),
            &["provider", "model"],
        )
        .expect("valid metric");

        let tool_calls = IntCounterVec::new(
            prometheus::Opts::new("plaw_tool_calls_total", "Total tool calls"),
            &["tool", "success"],
        )
        .expect("valid metric");

        let channel_messages = IntCounterVec::new(
            prometheus::Opts::new("plaw_channel_messages_total", "Total channel messages"),
            &["channel", "direction"],
        )
        .expect("valid metric");

        let heartbeat_ticks =
            prometheus::IntCounter::new("plaw_heartbeat_ticks_total", "Total heartbeat ticks")
                .expect("valid metric");

        let errors = IntCounterVec::new(
            prometheus::Opts::new("plaw_errors_total", "Total errors by component"),
            &["component"],
        )
        .expect("valid metric");

        let agent_duration = HistogramVec::new(
            HistogramOpts::new(
                "plaw_agent_duration_seconds",
                "Agent invocation duration in seconds",
            )
            .buckets(vec![0.1, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0]),
            &["provider", "model"],
        )
        .expect("valid metric");

        let tool_duration = HistogramVec::new(
            HistogramOpts::new(
                "plaw_tool_duration_seconds",
                "Tool execution duration in seconds",
            )
            .buckets(vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]),
            &["tool"],
        )
        .expect("valid metric");

        let request_latency = Histogram::with_opts(
            HistogramOpts::new("plaw_request_latency_seconds", "Request latency in seconds")
                .buckets(vec![0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0]),
        )
        .expect("valid metric");

        let tokens_used =
            prometheus::IntGauge::new("plaw_tokens_used_last", "Tokens used in the last request")
                .expect("valid metric");

        let active_sessions = GaugeVec::new(
            prometheus::Opts::new("plaw_active_sessions", "Number of active sessions"),
            &[],
        )
        .expect("valid metric");

        let queue_depth = GaugeVec::new(
            prometheus::Opts::new("plaw_queue_depth", "Message queue depth"),
            &[],
        )
        .expect("valid metric");

        // Register all metrics
        registry.register(Box::new(agent_starts.clone())).ok();
        registry.register(Box::new(llm_requests.clone())).ok();
        registry.register(Box::new(tokens_input_total.clone())).ok();
        registry
            .register(Box::new(tokens_output_total.clone()))
            .ok();
        registry
            .register(Box::new(tokens_cache_creation_total.clone()))
            .ok();
        registry
            .register(Box::new(tokens_cache_read_total.clone()))
            .ok();
        registry.register(Box::new(tool_calls.clone())).ok();
        registry.register(Box::new(channel_messages.clone())).ok();
        registry.register(Box::new(heartbeat_ticks.clone())).ok();
        registry.register(Box::new(errors.clone())).ok();
        registry.register(Box::new(agent_duration.clone())).ok();
        registry.register(Box::new(tool_duration.clone())).ok();
        registry.register(Box::new(request_latency.clone())).ok();
        registry.register(Box::new(tokens_used.clone())).ok();
        registry.register(Box::new(active_sessions.clone())).ok();
        registry.register(Box::new(queue_depth.clone())).ok();

        Self {
            registry,
            agent_starts,
            llm_requests,
            tokens_input_total,
            tokens_output_total,
            tokens_cache_creation_total,
            tokens_cache_read_total,
            tool_calls,
            channel_messages,
            heartbeat_ticks,
            errors,
            agent_duration,
            tool_duration,
            request_latency,
            tokens_used,
            active_sessions,
            queue_depth,
        }
    }

    /// Encode all registered metrics into Prometheus text exposition format.
    pub fn encode(&self) -> String {
        let encoder = TextEncoder::new();
        let families = self.registry.gather();
        let mut buf = Vec::new();
        encoder.encode(&families, &mut buf).unwrap_or_default();
        String::from_utf8(buf).unwrap_or_default()
    }
}

impl Observer for PrometheusObserver {
    fn record_event(&self, event: &ObserverEvent) {
        match event {
            ObserverEvent::AgentStart { provider, model } => {
                self.agent_starts
                    .with_label_values(&[provider, model])
                    .inc();
            }
            ObserverEvent::AgentEnd {
                provider,
                model,
                duration,
                tokens_used,
                cost_usd: _,
            } => {
                // Agent duration is recorded via the histogram with provider/model labels
                self.agent_duration
                    .with_label_values(&[provider, model])
                    .observe(duration.as_secs_f64());
                if let Some(t) = tokens_used {
                    self.tokens_used.set(i64::try_from(*t).unwrap_or(i64::MAX));
                }
            }
            ObserverEvent::LlmResponse {
                provider,
                model,
                success,
                input_tokens,
                output_tokens,
                cache_creation_input_tokens,
                cache_read_input_tokens,
                ..
            } => {
                let success_str = if *success { "true" } else { "false" };
                self.llm_requests
                    .with_label_values(&[provider.as_str(), model.as_str(), success_str])
                    .inc();
                if let Some(input) = input_tokens {
                    self.tokens_input_total
                        .with_label_values(&[provider.as_str(), model.as_str()])
                        .inc_by(*input);
                }
                if let Some(output) = output_tokens {
                    self.tokens_output_total
                        .with_label_values(&[provider.as_str(), model.as_str()])
                        .inc_by(*output);
                }
                if let Some(create) = cache_creation_input_tokens {
                    self.tokens_cache_creation_total
                        .with_label_values(&[provider.as_str(), model.as_str()])
                        .inc_by(*create);
                }
                if let Some(read) = cache_read_input_tokens {
                    self.tokens_cache_read_total
                        .with_label_values(&[provider.as_str(), model.as_str()])
                        .inc_by(*read);
                }
            }
            ObserverEvent::ToolCallStart { tool: _ }
            | ObserverEvent::TurnComplete
            | ObserverEvent::LlmRequest { .. } => {}
            ObserverEvent::ToolCall {
                tool,
                duration,
                success,
            } => {
                let success_str = if *success { "true" } else { "false" };
                self.tool_calls
                    .with_label_values(&[tool.as_str(), success_str])
                    .inc();
                self.tool_duration
                    .with_label_values(&[tool.as_str()])
                    .observe(duration.as_secs_f64());
            }
            ObserverEvent::ChannelMessage { channel, direction } => {
                self.channel_messages
                    .with_label_values(&[channel, direction])
                    .inc();
            }
            ObserverEvent::HeartbeatTick => {
                self.heartbeat_ticks.inc();
            }
            ObserverEvent::Error {
                component,
                message: _,
            } => {
                self.errors.with_label_values(&[component]).inc();
            }
        }
    }

    fn record_metric(&self, metric: &ObserverMetric) {
        match metric {
            ObserverMetric::RequestLatency(d) => {
                self.request_latency.observe(d.as_secs_f64());
            }
            ObserverMetric::TokensUsed(t) => {
                self.tokens_used.set(i64::try_from(*t).unwrap_or(i64::MAX));
            }
            ObserverMetric::ActiveSessions(s) => {
                self.active_sessions
                    .with_label_values(&[] as &[&str])
                    .set(*s as f64);
            }
            ObserverMetric::QueueDepth(d) => {
                self.queue_depth
                    .with_label_values(&[] as &[&str])
                    .set(*d as f64);
            }
        }
    }

    fn name(&self) -> &str {
        "prometheus"
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn prometheus_observer_name() {
        assert_eq!(PrometheusObserver::new().name(), "prometheus");
    }

    #[test]
    fn records_all_events_without_panic() {
        let obs = PrometheusObserver::new();
        obs.record_event(&ObserverEvent::AgentStart {
            provider: "openrouter".into(),
            model: "claude-sonnet".into(),
        });
        obs.record_event(&ObserverEvent::AgentEnd {
            provider: "openrouter".into(),
            model: "claude-sonnet".into(),
            duration: Duration::from_millis(500),
            tokens_used: Some(100),
            cost_usd: None,
        });
        obs.record_event(&ObserverEvent::AgentEnd {
            provider: "openrouter".into(),
            model: "claude-sonnet".into(),
            duration: Duration::ZERO,
            tokens_used: None,
            cost_usd: None,
        });
        obs.record_event(&ObserverEvent::ToolCall {
            tool: "shell".into(),
            duration: Duration::from_millis(10),
            success: true,
        });
        obs.record_event(&ObserverEvent::ToolCall {
            tool: "file_read".into(),
            duration: Duration::from_millis(5),
            success: false,
        });
        obs.record_event(&ObserverEvent::ChannelMessage {
            channel: "telegram".into(),
            direction: "inbound".into(),
        });
        obs.record_event(&ObserverEvent::HeartbeatTick);
        obs.record_event(&ObserverEvent::Error {
            component: "provider".into(),
            message: "timeout".into(),
        });
    }

    #[test]
    fn records_all_metrics_without_panic() {
        let obs = PrometheusObserver::new();
        obs.record_metric(&ObserverMetric::RequestLatency(Duration::from_secs(2)));
        obs.record_metric(&ObserverMetric::TokensUsed(500));
        obs.record_metric(&ObserverMetric::TokensUsed(0));
        obs.record_metric(&ObserverMetric::ActiveSessions(3));
        obs.record_metric(&ObserverMetric::QueueDepth(42));
    }

    #[test]
    fn encode_produces_prometheus_text_format() {
        let obs = PrometheusObserver::new();
        obs.record_event(&ObserverEvent::AgentStart {
            provider: "openrouter".into(),
            model: "claude-sonnet".into(),
        });
        obs.record_event(&ObserverEvent::ToolCall {
            tool: "shell".into(),
            duration: Duration::from_millis(100),
            success: true,
        });
        obs.record_event(&ObserverEvent::HeartbeatTick);
        obs.record_metric(&ObserverMetric::RequestLatency(Duration::from_millis(250)));

        let output = obs.encode();
        assert!(output.contains("plaw_agent_starts_total"));
        assert!(output.contains("plaw_tool_calls_total"));
        assert!(output.contains("plaw_heartbeat_ticks_total"));
        assert!(output.contains("plaw_request_latency_seconds"));
    }

    #[test]
    fn counters_increment_correctly() {
        let obs = PrometheusObserver::new();

        for _ in 0..3 {
            obs.record_event(&ObserverEvent::HeartbeatTick);
        }

        let output = obs.encode();
        assert!(output.contains("plaw_heartbeat_ticks_total 3"));
    }

    #[test]
    fn tool_calls_track_success_and_failure_separately() {
        let obs = PrometheusObserver::new();

        obs.record_event(&ObserverEvent::ToolCall {
            tool: "shell".into(),
            duration: Duration::from_millis(10),
            success: true,
        });
        obs.record_event(&ObserverEvent::ToolCall {
            tool: "shell".into(),
            duration: Duration::from_millis(10),
            success: true,
        });
        obs.record_event(&ObserverEvent::ToolCall {
            tool: "shell".into(),
            duration: Duration::from_millis(10),
            success: false,
        });

        let output = obs.encode();
        assert!(output.contains(r#"plaw_tool_calls_total{success="true",tool="shell"} 2"#));
        assert!(output.contains(r#"plaw_tool_calls_total{success="false",tool="shell"} 1"#));
    }

    #[test]
    fn errors_track_by_component() {
        let obs = PrometheusObserver::new();
        obs.record_event(&ObserverEvent::Error {
            component: "provider".into(),
            message: "timeout".into(),
        });
        obs.record_event(&ObserverEvent::Error {
            component: "provider".into(),
            message: "rate limit".into(),
        });
        obs.record_event(&ObserverEvent::Error {
            component: "channels".into(),
            message: "disconnected".into(),
        });

        let output = obs.encode();
        assert!(output.contains(r#"plaw_errors_total{component="provider"} 2"#));
        assert!(output.contains(r#"plaw_errors_total{component="channels"} 1"#));
    }

    #[test]
    fn gauge_reflects_latest_value() {
        let obs = PrometheusObserver::new();
        obs.record_metric(&ObserverMetric::TokensUsed(100));
        obs.record_metric(&ObserverMetric::TokensUsed(200));

        let output = obs.encode();
        assert!(output.contains("plaw_tokens_used_last 200"));
    }

    #[test]
    fn llm_response_tracks_request_count_and_tokens() {
        let obs = PrometheusObserver::new();

        obs.record_event(&ObserverEvent::LlmResponse {
            provider: "openrouter".into(),
            model: "claude-sonnet".into(),
            duration: Duration::from_millis(200),
            success: true,
            error_message: None,
            input_tokens: Some(100),
            output_tokens: Some(50),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });
        obs.record_event(&ObserverEvent::LlmResponse {
            provider: "openrouter".into(),
            model: "claude-sonnet".into(),
            duration: Duration::from_millis(300),
            success: true,
            error_message: None,
            input_tokens: Some(200),
            output_tokens: Some(80),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });

        let output = obs.encode();
        assert!(output.contains(
            r#"plaw_llm_requests_total{model="claude-sonnet",provider="openrouter",success="true"} 2"#
        ));
        assert!(output.contains(
            r#"plaw_tokens_input_total{model="claude-sonnet",provider="openrouter"} 300"#
        ));
        assert!(output.contains(
            r#"plaw_tokens_output_total{model="claude-sonnet",provider="openrouter"} 130"#
        ));
    }

    #[test]
    fn llm_response_without_tokens_increments_request_only() {
        let obs = PrometheusObserver::new();

        obs.record_event(&ObserverEvent::LlmResponse {
            provider: "ollama".into(),
            model: "llama3".into(),
            duration: Duration::from_millis(100),
            success: false,
            error_message: Some("timeout".into()),
            input_tokens: None,
            output_tokens: None,
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });

        let output = obs.encode();
        assert!(output.contains(
            r#"plaw_llm_requests_total{model="llama3",provider="ollama",success="false"} 1"#
        ));
        // Token counters should not appear (no data recorded)
        assert!(!output.contains("plaw_tokens_input_total{"));
        assert!(!output.contains("plaw_tokens_output_total{"));
    }

    // ── PR #78 prefix-cache observability ────────────────────────────

    /// Anthropic / Bedrock w/ Claude reports cache breakdown — the new
    /// `tokens_cache_creation_total` and `tokens_cache_read_total`
    /// counters must accumulate alongside the existing input/output
    /// totals.
    #[test]
    fn llm_response_with_cache_breakdown_increments_cache_counters() {
        let obs = PrometheusObserver::new();

        obs.record_event(&ObserverEvent::LlmResponse {
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            duration: Duration::from_millis(150),
            success: true,
            error_message: None,
            input_tokens: Some(10_000),
            output_tokens: Some(200),
            cache_creation_input_tokens: Some(8_000),
            cache_read_input_tokens: Some(1_500),
        });
        obs.record_event(&ObserverEvent::LlmResponse {
            provider: "anthropic".into(),
            model: "claude-opus-4-8".into(),
            duration: Duration::from_millis(120),
            success: true,
            error_message: None,
            input_tokens: Some(9_800),
            output_tokens: Some(180),
            cache_creation_input_tokens: Some(0),
            cache_read_input_tokens: Some(9_500),
        });

        let output = obs.encode();
        assert!(output.contains(
            r#"plaw_tokens_cache_creation_total{model="claude-opus-4-8",provider="anthropic"} 8000"#
        ));
        // 1500 + 9500 = 11000 — high cache_read counter means the cache
        // is paying off.
        assert!(output.contains(
            r#"plaw_tokens_cache_read_total{model="claude-opus-4-8",provider="anthropic"} 11000"#
        ));
        // Existing input/output totals continue to reflect the TOTAL
        // prompt tokens (the cache fields are a breakdown, not an
        // alternative — they sum into input).
        assert!(output.contains(
            r#"plaw_tokens_input_total{model="claude-opus-4-8",provider="anthropic"} 19800"#
        ));
    }

    /// Providers that do not report cache breakdown (OpenAI, Ollama,
    /// Gemini, etc.) must NOT create cache counter time-series — empty
    /// label sets clutter dashboards and grafana would error on
    /// missing metric subscripts.
    #[test]
    fn llm_response_without_cache_breakdown_leaves_cache_counters_empty() {
        let obs = PrometheusObserver::new();

        obs.record_event(&ObserverEvent::LlmResponse {
            provider: "ollama".into(),
            model: "llama3".into(),
            duration: Duration::from_millis(50),
            success: true,
            error_message: None,
            input_tokens: Some(100),
            output_tokens: Some(40),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        });

        let output = obs.encode();
        // Input/output do appear; cache counters do NOT (None inputs
        // skip the .inc_by branch entirely).
        assert!(output.contains("plaw_tokens_input_total{"));
        assert!(!output.contains("plaw_tokens_cache_creation_total{"));
        assert!(!output.contains("plaw_tokens_cache_read_total{"));
    }
}
