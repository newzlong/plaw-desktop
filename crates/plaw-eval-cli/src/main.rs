//! plaw-eval — CLI for the plaw-elite eval foundation.
//!
//! Subcommands map 1:1 onto the design in
//! `.kiro/specs/plaw-elite/phase-1-eval/design.md` §四.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use chrono::{TimeZone, Utc};
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use plaw_eval::judges::{api_key_env_var, build_from_spec};
use plaw_eval::report::{
    compare_runs, extract_failing_rows, render_aggregate_md, render_comparison_md,
    render_pr_comment, write_aggregate_json, write_comparison_json, write_comparison_sarif,
    GateVerdict, DEFAULT_EPSILON,
};
use plaw_eval::runner::{
    aggregate, execute, PlawClient, RunnerConfig, DEFAULT_AGGREGATE_ALPHA, DEFAULT_TIMEOUT,
};
use plaw_eval::stats::required_sample_size;
use plaw_eval::storage::{EvalRepo, FlywheelEntry};
use plaw_eval::suite::{discover_suites, load_suite, JudgeMode, JudgeSpec};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "plaw-eval",
    version,
    about = "Evaluate plaw with statistical rigor (plaw-elite Phase 1)",
    long_about = None,
)]
struct Cli {
    /// Path to a config file. Defaults to ~/.plaw/eval/config.toml.
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Reduce output to errors only.
    #[arg(long, global = true)]
    quiet: bool,

    /// Increase verbosity (repeat for more, e.g. -vv).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,

    /// Disable ANSI colors in output.
    #[arg(long, global = true)]
    no_color: bool,

    /// Path to the SQLite eval database. Defaults to
    /// `plaw-data/.plaw/eval/runs.db` under the current working dir.
    #[arg(long, global = true, env = "PLAW_EVAL_DB")]
    db: Option<PathBuf>,

    /// Root directory holding `evals/<suite>/cases.toml`. Defaults to
    /// `./evals` under the current working dir.
    #[arg(long, global = true, env = "PLAW_EVAL_SUITES_DIR")]
    suites_dir: Option<PathBuf>,

    /// WebSocket endpoint for plaw. Default `ws://127.0.0.1:5800/ws/chat`.
    #[arg(long, global = true, env = "PLAW_WS_URL")]
    ws_url: Option<String>,

    /// Optional bearer token for the plaw WS connection.
    #[arg(long, global = true, env = "PLAW_WS_BEARER")]
    ws_bearer: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run an eval suite against a live plaw instance.
    Run {
        /// Suite name (directory under `evals/`). Repeatable.
        #[arg(long)]
        suite: Vec<String>,

        /// Run all discovered suites.
        #[arg(long, conflicts_with = "suite")]
        all: bool,

        /// Number of cases to sample (default: full suite).
        #[arg(long)]
        n: Option<usize>,

        /// Repeat each sampled case this many times. Engages cluster-robust
        /// SE because repeats of the same case are correlated. Default 1.
        #[arg(long, default_value_t = 1)]
        repetitions: usize,

        /// Override the judge model (provider:model, e.g. `kimi:kimi-k2.5`).
        #[arg(long)]
        judge: Option<String>,

        /// Deterministic sampling seed.
        #[arg(long)]
        seed: Option<u64>,

        /// Where to write the JSON report.
        #[arg(long)]
        output: Option<PathBuf>,
    },

    /// List recent eval runs.
    List {
        /// Limit per suite (or overall when `--suite` not set).
        #[arg(long, default_value_t = 20)]
        limit: usize,

        /// Filter by suite name.
        #[arg(long)]
        suite: Option<String>,

        /// Show full per-metric details.
        #[arg(long)]
        detail: bool,
    },

    /// Compare two runs and emit a paired-diff report with gate verdict.
    Compare {
        /// Baseline run ID. Use `latest` for the most recent finished run.
        #[arg(long)]
        baseline: String,

        /// Candidate run ID, or `latest` for the most recent finished run.
        #[arg(long)]
        candidate: String,

        /// Suite to scope `latest` lookups to.
        #[arg(long)]
        suite: Option<String>,

        /// Override gate epsilon (default: 0.01).
        #[arg(long, default_value_t = DEFAULT_EPSILON)]
        epsilon: f64,

        /// Significance level for confidence intervals.
        #[arg(long, default_value_t = DEFAULT_AGGREGATE_ALPHA)]
        alpha: f64,

        /// Write a Markdown PR-comment body to this path.
        #[arg(long)]
        pr_comment: Option<PathBuf>,

        /// Write a JSON comparison report to this path.
        #[arg(long)]
        output: Option<PathBuf>,

        /// Write a SARIF 2.1.0 report to this path (for GitHub Code Scanning).
        #[arg(long)]
        sarif: Option<PathBuf>,
    },

    /// Compute the sample size needed to detect an effect.
    Power {
        /// Effect size in percentage points (e.g. 2.0 for 2pp).
        #[arg(long)]
        effect: f64,

        /// Estimated standard deviation of the metric.
        #[arg(long)]
        sigma: f64,

        /// Significance level. Default 0.05.
        #[arg(long, default_value_t = 0.05)]
        alpha: f64,

        /// Statistical power. Default 0.80.
        #[arg(long, default_value_t = 0.80)]
        power: f64,
    },

    /// Promote a production trace into the flywheel review queue.
    Promote {
        /// Source trace ID.
        #[arg(long)]
        trace: String,

        /// Optional pre-graded score that gets attached to the queue entry.
        #[arg(long)]
        judge_score: Option<f64>,

        /// Pre-mark review status (default: pending).
        #[arg(long, default_value = "pending")]
        review_status: String,
    },

    /// Manage the judge response cache.
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },

    /// Manage the production-trace flywheel.
    Flywheel {
        #[command(subcommand)]
        action: FlywheelAction,
    },

    /// Diagnose the local environment (API keys, plaw endpoint, DB).
    Doctor,

    /// Generate a shell completion script.
    ///
    /// Examples:
    ///   plaw-eval completion bash > /etc/bash_completion.d/plaw-eval
    ///   plaw-eval completion zsh  > ~/.zfunc/_plaw-eval
    ///   plaw-eval completion fish > ~/.config/fish/completions/plaw-eval.fish
    ///   plaw-eval completion powershell | Out-String | Invoke-Expression
    Completion {
        /// Shell flavor to emit.
        shell: Shell,
    },
}

#[derive(Debug, Subcommand)]
enum CacheAction {
    /// Clear cached judge responses older than `--ttl-days`.
    Clear {
        /// TTL in days. 0 means clear everything inserted at or before now.
        #[arg(long, default_value_t = 0)]
        ttl_days: i64,
    },
    /// Show cache statistics.
    Stats,
}

#[derive(Debug, Subcommand)]
enum FlywheelAction {
    /// List traces awaiting review.
    ListPending {
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Approve or reject a queued trace.
    Review {
        id: String,
        #[arg(value_parser = ["approve", "reject"])]
        verdict: String,
    },
    /// Sample case results from a finished run into the queue.
    Sample {
        /// Source run id (use `latest` for the most recent finished run).
        #[arg(long)]
        run: String,

        /// Suite to scope the `latest` lookup to.
        #[arg(long)]
        suite: Option<String>,

        /// Sampling strategy. Default: `random`.
        #[arg(long, default_value = "random",
              value_parser = ["random", "failed", "low-score", "all"])]
        strategy: String,

        /// Sampling rate `[0, 1]` for the `random` strategy.
        #[arg(long, default_value_t = 0.1)]
        rate: f64,

        /// Deterministic sampling seed for the `random` strategy.
        #[arg(long, default_value_t = 0xC0FFEE_u64)]
        seed: u64,

        /// Metric name for the `low-score` strategy.
        #[arg(long, default_value = "g_eval")]
        metric: String,

        /// Threshold for the `low-score` strategy.
        #[arg(long, default_value_t = 0.5)]
        threshold: f64,

        /// Tag every queued entry with this target suite so promote knows
        /// where to write.
        #[arg(long)]
        target_suite: Option<String>,
    },
    /// Promote an approved queue entry into the target suite TOML.
    Promote {
        /// Queue entry id to promote.
        id: String,

        /// Path to the target suite's cases.toml. Falls back to the
        /// queue entry's `target_suite` field when omitted.
        #[arg(long)]
        suite_path: Option<PathBuf>,

        /// Override the default `<suites_dir>/<target_suite>/cases.toml`
        /// resolution by passing a name here.
        #[arg(long)]
        suite: Option<String>,
    },
}

fn init_tracing(verbose: u8, quiet: bool) {
    if quiet {
        return;
    }
    let level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose, cli.quiet);

    let db_path = resolve_db_path(cli.db.clone());
    let suites_dir = cli
        .suites_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from("evals"));

    match cli.command {
        Command::Run {
            suite,
            all,
            n,
            repetitions,
            judge,
            seed,
            output,
        } => {
            cmd_run(
                &db_path,
                &suites_dir,
                cli.ws_url.as_deref(),
                cli.ws_bearer.as_deref(),
                suite,
                all,
                n,
                repetitions,
                judge.as_deref(),
                seed,
                output.as_deref(),
            )
            .await
        }
        Command::List {
            limit,
            suite,
            detail,
        } => cmd_list(&db_path, suite.as_deref(), limit, detail),
        Command::Compare {
            baseline,
            candidate,
            suite,
            epsilon,
            alpha,
            pr_comment,
            output,
            sarif,
        } => cmd_compare(
            &db_path,
            &baseline,
            &candidate,
            suite.as_deref(),
            epsilon,
            alpha,
            pr_comment.as_deref(),
            output.as_deref(),
            sarif.as_deref(),
        ),
        Command::Power {
            effect,
            sigma,
            alpha,
            power,
        } => cmd_power(effect, sigma, alpha, power),
        Command::Promote {
            trace,
            judge_score,
            review_status,
        } => cmd_promote(&db_path, &trace, judge_score, &review_status),
        Command::Cache { action } => cmd_cache(&db_path, action),
        Command::Flywheel { action } => cmd_flywheel(&db_path, &suites_dir, action),
        Command::Doctor => cmd_doctor(&db_path, &suites_dir, cli.ws_url.as_deref()),
        Command::Completion { shell } => cmd_completion(shell),
    }
}

fn cmd_completion(shell: Shell) -> Result<()> {
    let mut cmd = Cli::command();
    let bin = cmd.get_name().to_string();
    clap_complete::generate(shell, &mut cmd, bin, &mut std::io::stdout());
    Ok(())
}

// ---------- helpers ----------

fn resolve_db_path(override_path: Option<PathBuf>) -> PathBuf {
    override_path.unwrap_or_else(|| PathBuf::from("plaw-data/.plaw/eval/runs.db"))
}

/// Parse a `--judge provider:model` override into a [`JudgeSpec`].
/// Mode defaults to score(scale=5) — the suite's mode isn't preserved
/// because the override is meant for ad-hoc cross-family comparison runs
/// (G-Eval / score work for any single judge).
fn parse_judge_override(s: &str) -> Result<JudgeSpec> {
    let (provider, model) = s
        .split_once(':')
        .ok_or_else(|| anyhow!("--judge must be 'provider:model', got '{s}'"))?;
    if provider.is_empty() || model.is_empty() {
        return Err(anyhow!("--judge provider and model must be non-empty"));
    }
    Ok(JudgeSpec {
        model: model.to_string(),
        provider: provider.to_string(),
        temperature: 0.0,
        mode: JudgeMode::Score { scale: 5 },
    })
}

fn resolve_ws_url(override_url: Option<&str>) -> String {
    if let Some(s) = override_url {
        return s.to_string();
    }
    // plaw-desktop allocates a dynamic port at startup and writes it to
    // `plaw-data/port-state.json`. Look there before falling back.
    if let Some(port) = read_plaw_desktop_port() {
        return format!("ws://127.0.0.1:{port}/ws/chat");
    }
    "ws://127.0.0.1:5800/ws/chat".into()
}

fn read_plaw_desktop_port() -> Option<u16> {
    // Walk up from cwd looking for `plaw-data/port-state.json`. The dev
    // build writes it to `<repo>/plaw-data/`, the bundled build to the
    // exe's neighbour. We check the cwd's siblings in both cases.
    let cwd = std::env::current_dir().ok()?;
    for dir in std::iter::once(cwd.as_path()).chain(cwd.ancestors().skip(1)) {
        let p = dir.join("plaw-data").join("port-state.json");
        if p.exists() {
            let content = std::fs::read_to_string(&p).ok()?;
            let v: serde_json::Value = serde_json::from_str(&content).ok()?;
            return v.get("port").and_then(|x| x.as_u64()).map(|n| n as u16);
        }
    }
    None
}

fn open_repo(db_path: &Path) -> Result<EvalRepo> {
    EvalRepo::open(db_path)
        .with_context(|| format!("opening eval database at {}", db_path.display()))
}

/// Decide which `cases.toml` file to operate against during a flywheel
/// promotion. Order of preference:
///   1. Explicit `--suite-path` flag (operator knows best).
///   2. `--suite <name>` flag → `<suites_dir>/<name>/cases.toml`.
///   3. Queue entry's stored `target_suite` field.
fn resolve_suite_path(
    suite_path: Option<&Path>,
    suite_name: Option<&str>,
    queue_target: Option<&str>,
    suites_dir: &Path,
) -> Result<PathBuf> {
    if let Some(p) = suite_path {
        return Ok(p.to_path_buf());
    }
    let name = suite_name.or(queue_target).ok_or_else(|| {
        anyhow!(
            "no target suite specified — pass --suite-path, --suite, or set the queue \
             entry's target_suite at sample time"
        )
    })?;
    Ok(suites_dir.join(name).join("cases.toml"))
}

fn resolve_run_id(repo: &EvalRepo, raw: &str, suite: Option<&str>) -> Result<String> {
    if raw == "latest" {
        let baseline = match suite {
            Some(name) => repo.get_baseline(name)?,
            None => repo.list_runs(None, 1)?.into_iter().next(),
        };
        return baseline
            .map(|r| r.id)
            .ok_or_else(|| anyhow!("no finished runs available for `latest`"));
    }
    Ok(raw.to_string())
}

fn format_unix(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt: chrono::DateTime<Utc>| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| ts.to_string())
}

// ---------- run ----------

#[allow(clippy::too_many_arguments)]
async fn cmd_run(
    db_path: &Path,
    suites_dir: &Path,
    ws_url: Option<&str>,
    ws_bearer: Option<&str>,
    suites: Vec<String>,
    all: bool,
    n: Option<usize>,
    repetitions: usize,
    judge_override: Option<&str>,
    seed: Option<u64>,
    output: Option<&Path>,
) -> Result<()> {
    let judge_spec_override = judge_override
        .map(parse_judge_override)
        .transpose()
        .context("parsing --judge override")?;
    let repo = Arc::new(open_repo(db_path)?);
    let ws = resolve_ws_url(ws_url);

    let targets: Vec<(PathBuf, plaw_eval::suite::Suite)> = if all {
        discover_suites(suites_dir)?
    } else if !suites.is_empty() {
        let mut out = Vec::new();
        for name in &suites {
            let p = suites_dir.join(name).join("cases.toml");
            let suite = load_suite(&p)?;
            out.push((p, suite));
        }
        out
    } else {
        return Err(anyhow!(
            "either --suite <name> or --all is required (no targets specified)"
        ));
    };

    if targets.is_empty() {
        println!("no suites discovered under {}", suites_dir.display());
        return Ok(());
    }

    let cancel = CancellationToken::new();
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                tracing::warn!("Ctrl-C received, cancelling run");
                cancel.cancel();
            }
        });
    }

    for (path, suite) in targets {
        println!(
            "running suite '{}' ({} cases)",
            suite.name,
            suite.cases.len()
        );
        let plaw = PlawClient::new(&ws);
        let plaw = if let Some(b) = ws_bearer {
            plaw.with_bearer(b)
        } else {
            plaw
        }
        .with_timeout(DEFAULT_TIMEOUT);

        // CLI override (--judge provider:model) wins over the suite's
        // default_judge. Useful for cross-family comparison runs:
        //   plaw-eval run --suite chat_quality --judge anthropic:claude-haiku-4-5
        let effective_judge_spec = judge_spec_override
            .clone()
            .unwrap_or_else(|| suite.default_judge.clone());
        let raw_judge = build_from_spec(&effective_judge_spec).with_context(|| {
            format!(
                "building judge for suite '{}' (provider={}, model={})",
                suite.name, effective_judge_spec.provider, effective_judge_spec.model
            )
        })?;
        // Wrap with bounded retry so transient judge errors (timeout,
        // 429 rate limit, connection reset) don't leave cases with
        // empty metric_scores. 3 attempts × 1s/2s backoff catches the
        // common case without doubling cost on permanent failures.
        let retried: std::sync::Arc<dyn plaw_eval::judges::JudgeClient> =
            std::sync::Arc::new(plaw_eval::judges::RetryingJudgeClient::new(raw_judge, 3));
        // Cache wraps the retry layer so cache hits short-circuit before
        // any network call, while misses still benefit from retry. Errors
        // are not cached. Re-running the same suite (same judge model)
        // therefore replays scores from the eval SQLite DB instead of
        // re-paying the judge bill.
        let cache = std::sync::Arc::new(plaw_eval::runner::cache::JudgeCache::new(repo.clone()));
        let judge: std::sync::Arc<dyn plaw_eval::judges::JudgeClient> =
            std::sync::Arc::new(plaw_eval::judges::CachedJudgeClient::new(retried, cache));

        let mut cfg = RunnerConfig::new(suite.clone(), plaw, repo.clone());
        cfg.cancel = cancel.clone();
        cfg.show_progress = true;
        cfg.sample_n = n;
        cfg.sample_seed = seed;
        cfg.repetitions = repetitions.max(1);
        cfg.model_version = effective_judge_spec.model.clone();

        let summary = execute(cfg).await?;
        println!(
            "  suite '{}' run {}: total={} ok={} failed={} cancelled={}",
            summary.suite_name,
            summary.run_id,
            summary.n_total,
            summary.n_completed,
            summary.n_failed,
            summary.cancelled
        );

        if !summary.cancelled {
            let score_summary = plaw_eval::metrics::runner::score_run_with_concurrency_and_progress(
                &repo,
                &summary.run_id,
                &suite,
                &*judge,
                plaw_eval::metrics::runner::DEFAULT_SCORING_CONCURRENCY,
                true,
            )
            .await
            .with_context(|| format!("scoring run {}", summary.run_id))?;
            println!(
                "  scored {} (case, metric) pairs",
                score_summary.pairs_scored
            );
            if score_summary.has_silent_failures() {
                let n_failed = score_summary.cases_all_metrics_failed.len();
                println!(
                    "  ⚠ {n_failed} case(s) had ALL metric attempts error \
                     (judge timeout / rate limit / parse failure). \
                     metric_scores left empty for these cases — investigate \
                     before treating aggregate as authoritative."
                );
                for cid in score_summary.cases_all_metrics_failed.iter().take(5) {
                    println!("     - {cid}");
                }
                if n_failed > 5 {
                    println!("     ... and {} more", n_failed - 5);
                }
            }
        }

        let agg = aggregate(&repo, &summary.run_id, DEFAULT_AGGREGATE_ALPHA)?;
        println!("{}", render_aggregate_md(&agg));

        if let Some(path) = output {
            // When multiple suites run, append the suite name to keep
            // outputs separate.
            let target = if targets_count_hint(&suites, all) > 1 {
                let parent = path.parent().unwrap_or_else(|| Path::new("."));
                parent.join(format!("{}_{}.json", file_stem(path), suite.name))
            } else {
                path.to_path_buf()
            };
            write_aggregate_json(&agg, &target)?;
            println!("  wrote JSON report to {}", target.display());
        }

        if cancel.is_cancelled() {
            tracing::warn!("cancellation requested; stopping run loop");
            break;
        }
        let _ = path;
    }
    Ok(())
}

fn targets_count_hint(suites: &[String], all: bool) -> usize {
    if all {
        2
    } else {
        suites.len()
    }
}

fn file_stem(p: &Path) -> String {
    p.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("report")
        .to_string()
}

// ---------- list ----------

fn cmd_list(db_path: &Path, suite: Option<&str>, limit: usize, detail: bool) -> Result<()> {
    let repo = open_repo(db_path)?;
    let runs = repo.list_runs(suite, limit)?;
    if runs.is_empty() {
        println!("(no runs)");
        return Ok(());
    }
    println!(
        "{:<36}  {:<24}  {:<14}  {:>4}  {:>4}  {:>4}  finished",
        "run_id", "suite", "model", "tot", "ok", "err"
    );
    for r in &runs {
        println!(
            "{:<36}  {:<24}  {:<14}  {:>4}  {:>4}  {:>4}  {}",
            r.id,
            truncate(&r.suite_name, 24),
            truncate(&r.model_version, 14),
            r.n_total,
            r.n_completed,
            r.n_failed,
            r.finished_at
                .map(format_unix)
                .unwrap_or_else(|| "(running)".into()),
        );
    }
    if detail {
        for r in &runs {
            let agg = aggregate(&repo, &r.id, DEFAULT_AGGREGATE_ALPHA)?;
            println!();
            println!("Run {} — {}", r.id, r.suite_name);
            println!("{}", render_aggregate_md(&agg));
        }
    }
    Ok(())
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let mut t: String = s.chars().take(max - 1).collect();
        t.push('…');
        t
    } else {
        s.to_string()
    }
}

// ---------- compare ----------

#[allow(clippy::too_many_arguments)]
fn cmd_compare(
    db_path: &Path,
    baseline_raw: &str,
    candidate_raw: &str,
    suite: Option<&str>,
    epsilon: f64,
    alpha: f64,
    pr_comment_path: Option<&Path>,
    output: Option<&Path>,
    sarif_path: Option<&Path>,
) -> Result<()> {
    let repo = open_repo(db_path)?;
    let baseline_id = resolve_run_id(&repo, baseline_raw, suite)?;
    let candidate_id = resolve_run_id(&repo, candidate_raw, suite)?;
    if baseline_id == candidate_id {
        return Err(anyhow!("baseline and candidate resolve to the same run id"));
    }
    let report = compare_runs(&repo, &baseline_id, &candidate_id, epsilon, alpha)?;
    println!("{}", render_comparison_md(&report));

    if let Some(path) = pr_comment_path {
        let baseline_cases = repo.load_case_results(&baseline_id)?;
        let candidate_cases = repo.load_case_results(&candidate_id)?;
        let rows = extract_failing_rows(&report, &baseline_cases, &candidate_cases, 10);
        let body = render_pr_comment(&report, &rows);
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !parent.exists() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating parent dir for {}", path.display()))?;
            }
        }
        std::fs::write(path, body)
            .with_context(|| format!("writing PR comment to {}", path.display()))?;
        println!("wrote PR comment to {}", path.display());
    }

    if let Some(path) = output {
        write_comparison_json(&report, path)?;
        println!("wrote JSON comparison to {}", path.display());
    }

    if let Some(path) = sarif_path {
        write_comparison_sarif(&report, path)?;
        println!("wrote SARIF report to {}", path.display());
    }

    if matches!(report.verdict, GateVerdict::Fail) {
        // CI gate: surface a non-zero exit so callers can block merges.
        std::process::exit(1);
    }
    Ok(())
}

// ---------- power ----------

fn cmd_power(effect: f64, sigma: f64, alpha: f64, power: f64) -> Result<()> {
    let n = required_sample_size(effect, sigma, alpha, power)
        .ok_or_else(|| anyhow!("invalid power-analysis inputs"))?;
    println!("Required n = {n} for effect={effect} sigma={sigma} alpha={alpha} power={power}");
    Ok(())
}

// ---------- promote ----------

fn cmd_promote(
    db_path: &Path,
    trace_id: &str,
    judge_score: Option<f64>,
    review_status: &str,
) -> Result<()> {
    let repo = open_repo(db_path)?;
    let id = uuid::Uuid::new_v4().to_string();
    let entry = FlywheelEntry {
        id: id.clone(),
        trace_id: trace_id.into(),
        sampled_at: chrono::Utc::now().timestamp(),
        judge_score,
        review_status: review_status.into(),
        reviewed_at: None,
        promoted_to_suite: None,
        promoted_case_id: None,
        source_run_id: None,
        source_case_id: None,
        target_suite: None,
    };
    repo.flywheel_enqueue(&entry)?;
    println!("queued trace {trace_id} as flywheel entry {id} ({review_status})");
    Ok(())
}

// ---------- cache ----------

fn cmd_cache(db_path: &Path, action: CacheAction) -> Result<()> {
    let repo = open_repo(db_path)?;
    match action {
        CacheAction::Clear { ttl_days } => {
            let ttl_secs = ttl_days * 86_400;
            let removed = repo.clear_expired(ttl_secs)?;
            println!("cleared {removed} cache rows older than {ttl_days} day(s)");
        }
        CacheAction::Stats => {
            let n = repo.cache_count()?;
            println!("judge_cache: {n} rows");
        }
    }
    Ok(())
}

// ---------- flywheel ----------

fn cmd_flywheel(db_path: &Path, suites_dir: &Path, action: FlywheelAction) -> Result<()> {
    let repo = open_repo(db_path)?;
    match action {
        FlywheelAction::ListPending { limit } => {
            let pending = repo.flywheel_list_pending(limit)?;
            if pending.is_empty() {
                println!("(no pending traces)");
                return Ok(());
            }
            println!(
                "{:<36}  {:<24}  judge  status   target          sampled",
                "id", "trace_id"
            );
            for p in pending {
                println!(
                    "{:<36}  {:<24}  {:>5}  {:<8} {:<15} {}",
                    p.id,
                    truncate(&p.trace_id, 24),
                    p.judge_score
                        .map(|s| format!("{s:.2}"))
                        .unwrap_or_else(|| "—".into()),
                    p.review_status,
                    truncate(p.target_suite.as_deref().unwrap_or("—"), 15),
                    format_unix(p.sampled_at),
                );
            }
        }
        FlywheelAction::Review { id, verdict } => {
            let status = match verdict.as_str() {
                "approve" => "approved",
                "reject" => "rejected",
                _ => unreachable!("clap validated this"),
            };
            let now = chrono::Utc::now().timestamp();
            repo.flywheel_set_status(&id, status, Some(now))?;
            println!("flywheel entry {id} → {status}");
        }
        FlywheelAction::Sample {
            run,
            suite,
            strategy,
            rate,
            seed,
            metric,
            threshold,
            target_suite,
        } => {
            let run_id = resolve_run_id(&repo, &run, suite.as_deref())?;
            let strategy = match strategy.as_str() {
                "random" => plaw_eval::flywheel::SampleStrategy::Random { rate, seed },
                "failed" => plaw_eval::flywheel::SampleStrategy::FailedOnly,
                "low-score" => plaw_eval::flywheel::SampleStrategy::LowScore {
                    metric: metric.clone(),
                    threshold,
                },
                "all" => plaw_eval::flywheel::SampleStrategy::All,
                _ => unreachable!("clap validated this"),
            };
            let summary =
                plaw_eval::flywheel::sample_run(&repo, &run_id, strategy, target_suite.as_deref())?;
            println!(
                "sampled {} of {} cases from run {} into the flywheel queue",
                summary.queued, summary.total_cases, summary.run_id
            );
        }
        FlywheelAction::Promote {
            id,
            suite_path,
            suite,
        } => {
            let entry = repo
                .flywheel_get(&id)?
                .ok_or_else(|| anyhow!("flywheel entry '{id}' not found"))?;
            let resolved_path = resolve_suite_path(
                suite_path.as_deref(),
                suite.as_deref(),
                entry.target_suite.as_deref(),
                suites_dir,
            )?;
            let result = plaw_eval::flywheel::promote(&repo, &id, &resolved_path)?;
            println!(
                "promoted entry {} → {}::{} ({} bytes appended to {})",
                result.queue_id,
                resolved_path.display(),
                result.new_case_id,
                result.appended_bytes,
                result.target_suite_path,
            );
        }
    }
    Ok(())
}

// ---------- doctor ----------

fn cmd_doctor(db_path: &Path, suites_dir: &Path, ws_url: Option<&str>) -> Result<()> {
    println!("plaw-eval doctor");
    println!("  plaw-eval version: {}", plaw_eval::VERSION);

    // DB
    print!("  database         : {}", db_path.display());
    match EvalRepo::open(db_path) {
        Ok(repo) => {
            let runs = repo.list_runs(None, 1).unwrap_or_default();
            println!(" — ok ({} run(s))", runs.len());
        }
        Err(e) => println!(" — FAIL: {e}"),
    }

    // Suites
    print!("  suites directory : {}", suites_dir.display());
    if !suites_dir.exists() {
        println!(" — missing");
    } else {
        match discover_suites(suites_dir) {
            Ok(suites) => println!(" — ok ({} suite(s))", suites.len()),
            Err(e) => println!(" — FAIL: {e}"),
        }
    }

    // WS
    println!("  plaw WS endpoint : {}", resolve_ws_url(ws_url));

    // API keys
    let providers = ["anthropic", "openai", "kimi", "deepseek", "qwen"];
    println!("  judge API keys:");
    for p in providers {
        let env = api_key_env_var(p);
        let present = std::env::var(env).is_ok();
        println!(
            "    {:<10} ({:<22})  {}",
            p,
            env,
            if present { "set" } else { "missing" }
        );
    }
    Ok(())
}
