//! plaw-eval — CLI for the plaw-elite eval foundation.
//!
//! Subcommands map 1:1 onto the design in
//! `.kiro/specs/plaw-elite/phase-1-eval/design.md` §四.

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
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

        /// Override the judge model.
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
        /// Show full per-metric details.
        #[arg(long)]
        detail: bool,
    },

    /// Compare two runs and emit a paired-diff report with gate verdict.
    Compare {
        /// Baseline run ID, or `main` to fetch baseline-from-branch.
        #[arg(long)]
        baseline: String,

        /// Candidate run ID or path to a JSON report.
        #[arg(long)]
        candidate: String,

        /// Gate expression, e.g. `metric:lower_ci_bound >= baseline_mean - 0.01`.
        #[arg(long)]
        gate: Option<String>,

        /// Override gate epsilon (default: 0.01).
        #[arg(long, default_value_t = 0.01)]
        epsilon: f64,
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

    /// Promote a production trace into an eval suite.
    Promote {
        /// Source trace ID.
        #[arg(long)]
        trace: String,

        /// Target suite name.
        #[arg(long)]
        suite: String,

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
}

#[derive(Debug, Subcommand)]
enum CacheAction {
    /// Clear cached judge responses.
    Clear {
        /// Limit clear to a single suite.
        #[arg(long)]
        suite: Option<String>,
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

    match cli.command {
        Command::Run { .. } => {
            anyhow::bail!("`run` is not yet implemented (M3 / M5).");
        }
        Command::List { .. } => {
            anyhow::bail!("`list` is not yet implemented (M2).");
        }
        Command::Compare { .. } => {
            anyhow::bail!("`compare` is not yet implemented (M6).");
        }
        Command::Power { .. } => {
            anyhow::bail!("`power` is not yet implemented (M1).");
        }
        Command::Promote { .. } => {
            anyhow::bail!("`promote` is not yet implemented (M10).");
        }
        Command::Cache { .. } => {
            anyhow::bail!("`cache` is not yet implemented (M3).");
        }
        Command::Flywheel { .. } => {
            anyhow::bail!("`flywheel` is not yet implemented (M10).");
        }
        Command::Doctor => {
            println!("plaw-eval doctor — implementation pending (M7.T7.7)");
            println!("  plaw-eval version: {}", plaw_eval::VERSION);
            Ok(())
        }
    }
}
