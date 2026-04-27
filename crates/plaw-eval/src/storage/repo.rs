//! SQLite-backed repository for runs, case results, judge cache, flywheel.
//!
//! All operations are synchronous against a connection wrapped in a
//! `parking_lot`-style mutex (we use `std::sync::Mutex` here to avoid an
//! extra dependency — eval workloads aren't lock-contention-bound).

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use super::schema::{
    AggregateReport, CaseResult, FlywheelEntry, JudgeCacheEntry, MetricAggregate, MetricScore,
    Run, SCHEMA_SQL,
};

/// Repository handle. Cheap to clone via `Arc` if multiple owners are needed.
pub struct EvalRepo {
    conn: Mutex<Connection>,
}

impl EvalRepo {
    /// Open or create the SQLite DB, applying schema migrations.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating parent dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening eval db at {}", path.display()))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;").ok();
        conn.execute_batch(SCHEMA_SQL).context("applying schema")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// In-memory store, useful for tests.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;").ok();
        conn.execute_batch(SCHEMA_SQL).context("applying schema")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // ----- Runs -----

    pub fn insert_run(&self, run: &Run) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO runs (id, suite_name, suite_version, started_at, finished_at,
                               plaw_commit, model_version, config_hash,
                               n_total, n_completed, n_failed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                run.id,
                run.suite_name,
                run.suite_version,
                run.started_at,
                run.finished_at,
                run.plaw_commit,
                run.model_version,
                run.config_hash,
                run.n_total as i64,
                run.n_completed as i64,
                run.n_failed as i64,
            ],
        )?;
        Ok(())
    }

    pub fn update_run_finished(
        &self,
        run_id: &str,
        finished_at: i64,
        n_completed: usize,
        n_failed: usize,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE runs SET finished_at = ?2, n_completed = ?3, n_failed = ?4 WHERE id = ?1",
            params![run_id, finished_at, n_completed as i64, n_failed as i64],
        )?;
        Ok(())
    }

    pub fn load_run(&self, id: &str) -> Result<Option<Run>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, suite_name, suite_version, started_at, finished_at,
                        plaw_commit, model_version, config_hash,
                        n_total, n_completed, n_failed
                 FROM runs WHERE id = ?1",
                params![id],
                row_to_run,
            )
            .optional()?;
        Ok(row)
    }

    pub fn list_runs(&self, suite_name: Option<&str>, limit: usize) -> Result<Vec<Run>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = if suite_name.is_some() {
            conn.prepare(
                "SELECT id, suite_name, suite_version, started_at, finished_at,
                        plaw_commit, model_version, config_hash,
                        n_total, n_completed, n_failed
                 FROM runs WHERE suite_name = ?1 ORDER BY started_at DESC LIMIT ?2",
            )?
        } else {
            conn.prepare(
                "SELECT id, suite_name, suite_version, started_at, finished_at,
                        plaw_commit, model_version, config_hash,
                        n_total, n_completed, n_failed
                 FROM runs ORDER BY started_at DESC LIMIT ?1",
            )?
        };
        let rows = if let Some(name) = suite_name {
            stmt.query_map(params![name, limit as i64], row_to_run)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        } else {
            stmt.query_map(params![limit as i64], row_to_run)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        Ok(rows)
    }

    /// Most recent finished run for a suite, used as the regression baseline.
    pub fn get_baseline(&self, suite_name: &str) -> Result<Option<Run>> {
        let conn = self.conn.lock().unwrap();
        let row = conn
            .query_row(
                "SELECT id, suite_name, suite_version, started_at, finished_at,
                        plaw_commit, model_version, config_hash,
                        n_total, n_completed, n_failed
                 FROM runs WHERE suite_name = ?1 AND finished_at IS NOT NULL
                 ORDER BY started_at DESC LIMIT 1",
                params![suite_name],
                row_to_run,
            )
            .optional()?;
        Ok(row)
    }

    // ----- Case results -----

    pub fn insert_case_result(&self, result: &CaseResult) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let metric_json = serde_json::to_string(&result.metric_scores)
            .context("serialising metric scores")?;
        conn.execute(
            "INSERT INTO case_results (run_id, case_id, case_cluster, plaw_response,
                                       plaw_trace_id, metric_scores, latency_ms,
                                       tokens_in, tokens_out, cache_read_tokens, error)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                result.run_id,
                result.case_id,
                result.case_cluster,
                result.plaw_response,
                result.plaw_trace_id,
                metric_json,
                result.latency_ms as i64,
                result.tokens_in,
                result.tokens_out,
                result.cache_read_tokens,
                result.error,
            ],
        )?;
        Ok(())
    }

    pub fn load_case_results(&self, run_id: &str) -> Result<Vec<CaseResult>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT run_id, case_id, case_cluster, plaw_response, plaw_trace_id,
                    metric_scores, latency_ms, tokens_in, tokens_out,
                    cache_read_tokens, error
             FROM case_results WHERE run_id = ?1",
        )?;
        let rows = stmt
            .query_map(params![run_id], row_to_case_result)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    // ----- Judge cache -----

    pub fn get_cached(&self, cache_key: &str) -> Result<Option<JudgeCacheEntry>> {
        let conn = self.conn.lock().unwrap();
        let entry = conn
            .query_row(
                "SELECT cache_key, judge_response, created_at FROM judge_cache WHERE cache_key = ?1",
                params![cache_key],
                |row| {
                    Ok(JudgeCacheEntry {
                        cache_key: row.get(0)?,
                        judge_response: row.get(1)?,
                        created_at: row.get(2)?,
                    })
                },
            )
            .optional()?;
        Ok(entry)
    }

    pub fn set_cached(&self, cache_key: &str, response: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO judge_cache (cache_key, judge_response, created_at)
             VALUES (?1, ?2, ?3)",
            params![cache_key, response, now_unix()],
        )?;
        Ok(())
    }

    /// Drop cache rows older than `ttl_seconds`. Pass `0` to drop everything
    /// inserted at or before the current second (`<=` so freshly-written
    /// rows in the same second are eligible for purge).
    pub fn clear_expired(&self, ttl_seconds: i64) -> Result<usize> {
        let cutoff = now_unix() - ttl_seconds;
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "DELETE FROM judge_cache WHERE created_at <= ?1",
            params![cutoff],
        )?;
        Ok(n)
    }

    pub fn cache_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM judge_cache", [], |row| row.get(0))?;
        Ok(n as usize)
    }

    // ----- Flywheel queue -----

    pub fn flywheel_enqueue(&self, entry: &FlywheelEntry) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO flywheel_queue (id, trace_id, sampled_at, judge_score,
                                          review_status, reviewed_at,
                                          promoted_to_suite, promoted_case_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                entry.id,
                entry.trace_id,
                entry.sampled_at,
                entry.judge_score,
                entry.review_status,
                entry.reviewed_at,
                entry.promoted_to_suite,
                entry.promoted_case_id,
            ],
        )?;
        Ok(())
    }

    pub fn flywheel_list_pending(&self, limit: usize) -> Result<Vec<FlywheelEntry>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, trace_id, sampled_at, judge_score, review_status,
                    reviewed_at, promoted_to_suite, promoted_case_id
             FROM flywheel_queue WHERE review_status = 'pending'
             ORDER BY sampled_at DESC LIMIT ?1",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], row_to_flywheel)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    pub fn flywheel_set_status(
        &self,
        id: &str,
        status: &str,
        reviewed_at: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE flywheel_queue SET review_status = ?2, reviewed_at = ?3 WHERE id = ?1",
            params![id, status, reviewed_at],
        )?;
        Ok(())
    }

    // ----- Aggregates (no-op storage layer; aggregates live in memory) -----

    /// Convenience helper: compute and return metric means from stored case
    /// results. Real aggregation with CIs is performed by the runner; this
    /// is intended for ad-hoc CLI inspection.
    pub fn quick_summary(&self, run_id: &str) -> Result<AggregateReport> {
        let results = self.load_case_results(run_id)?;
        let mut sums: std::collections::HashMap<String, (f64, usize)> =
            std::collections::HashMap::new();
        for r in &results {
            for (name, score) in &r.metric_scores {
                let entry = sums.entry(name.clone()).or_insert((0.0, 0));
                entry.0 += score.value;
                entry.1 += 1;
            }
        }
        let metrics = sums
            .into_iter()
            .map(|(k, (sum, n))| {
                let mean = if n > 0 { sum / n as f64 } else { 0.0 };
                (
                    k,
                    MetricAggregate {
                        mean,
                        stderr: 0.0,
                        stderr_clustered: None,
                        ci_lower: 0.0,
                        ci_upper: 0.0,
                        n,
                        n_clusters: None,
                    },
                )
            })
            .collect();
        Ok(AggregateReport {
            run_id: run_id.to_string(),
            metrics,
        })
    }
}

fn row_to_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: row.get(0)?,
        suite_name: row.get(1)?,
        suite_version: row.get(2)?,
        started_at: row.get(3)?,
        finished_at: row.get(4)?,
        plaw_commit: row.get(5)?,
        model_version: row.get(6)?,
        config_hash: row.get(7)?,
        n_total: row.get::<_, i64>(8)? as usize,
        n_completed: row.get::<_, i64>(9)? as usize,
        n_failed: row.get::<_, i64>(10)? as usize,
    })
}

fn row_to_case_result(row: &rusqlite::Row<'_>) -> rusqlite::Result<CaseResult> {
    let metric_json: String = row.get(5)?;
    let metric_scores: std::collections::HashMap<String, MetricScore> =
        serde_json::from_str(&metric_json).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?;
    Ok(CaseResult {
        run_id: row.get(0)?,
        case_id: row.get(1)?,
        case_cluster: row.get(2)?,
        plaw_response: row.get(3)?,
        plaw_trace_id: row.get(4)?,
        metric_scores,
        latency_ms: row.get::<_, i64>(6)? as u64,
        tokens_in: row.get::<_, i64>(7)? as u32,
        tokens_out: row.get::<_, i64>(8)? as u32,
        cache_read_tokens: row.get::<_, i64>(9)? as u32,
        error: row.get(10)?,
    })
}

fn row_to_flywheel(row: &rusqlite::Row<'_>) -> rusqlite::Result<FlywheelEntry> {
    Ok(FlywheelEntry {
        id: row.get(0)?,
        trace_id: row.get(1)?,
        sampled_at: row.get(2)?,
        judge_score: row.get(3)?,
        review_status: row.get(4)?,
        reviewed_at: row.get(5)?,
        promoted_to_suite: row.get(6)?,
        promoted_case_id: row.get(7)?,
    })
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_run() -> Run {
        Run {
            id: "run-1".into(),
            suite_name: "smoke".into(),
            suite_version: "1.0.0".into(),
            started_at: 100,
            finished_at: None,
            plaw_commit: "deadbeef".into(),
            model_version: "kimi-k2.5".into(),
            config_hash: "abc".into(),
            n_total: 3,
            n_completed: 0,
            n_failed: 0,
        }
    }

    fn sample_result(run_id: &str, case_id: &str) -> CaseResult {
        let mut scores = HashMap::new();
        scores.insert(
            "g_eval".into(),
            MetricScore {
                value: 0.8,
                raw: serde_json::json!({"score": 4}),
                judge_model: "kimi".into(),
            },
        );
        CaseResult {
            run_id: run_id.into(),
            case_id: case_id.into(),
            case_cluster: Some("cluster-a".into()),
            plaw_response: "hello".into(),
            plaw_trace_id: None,
            metric_scores: scores,
            latency_ms: 1234,
            tokens_in: 50,
            tokens_out: 10,
            cache_read_tokens: 0,
            error: None,
        }
    }

    #[test]
    fn round_trips_run_and_results() {
        let repo = EvalRepo::open_in_memory().unwrap();
        repo.insert_run(&sample_run()).unwrap();
        repo.insert_case_result(&sample_result("run-1", "c1")).unwrap();
        repo.insert_case_result(&sample_result("run-1", "c2")).unwrap();
        repo.update_run_finished("run-1", 200, 2, 0).unwrap();

        let run = repo.load_run("run-1").unwrap().unwrap();
        assert_eq!(run.finished_at, Some(200));
        assert_eq!(run.n_completed, 2);

        let results = repo.load_case_results("run-1").unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].metric_scores.get("g_eval").unwrap().value, 0.8);
    }

    #[test]
    fn list_and_baseline_filter_by_suite() {
        let repo = EvalRepo::open_in_memory().unwrap();
        let mut r1 = sample_run();
        r1.id = "r1".into();
        r1.started_at = 10;
        r1.finished_at = Some(20);
        let mut r2 = sample_run();
        r2.id = "r2".into();
        r2.started_at = 50;
        r2.finished_at = Some(60);
        repo.insert_run(&r1).unwrap();
        repo.insert_run(&r2).unwrap();

        let runs = repo.list_runs(Some("smoke"), 10).unwrap();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].id, "r2"); // most recent first

        let baseline = repo.get_baseline("smoke").unwrap().unwrap();
        assert_eq!(baseline.id, "r2");
    }

    #[test]
    fn judge_cache_set_get_clear() {
        let repo = EvalRepo::open_in_memory().unwrap();
        repo.set_cached("k1", "{\"score\":4}").unwrap();
        let cached = repo.get_cached("k1").unwrap().unwrap();
        assert_eq!(cached.judge_response, "{\"score\":4}");

        // ttl=0 → all rows older than now drop out
        let cleared = repo.clear_expired(0).unwrap();
        assert!(cleared >= 1);
        assert!(repo.get_cached("k1").unwrap().is_none());
    }

    #[test]
    fn flywheel_enqueue_review_promote() {
        let repo = EvalRepo::open_in_memory().unwrap();
        let entry = FlywheelEntry {
            id: "f1".into(),
            trace_id: "trace-x".into(),
            sampled_at: 0,
            judge_score: Some(0.4),
            review_status: "pending".into(),
            reviewed_at: None,
            promoted_to_suite: None,
            promoted_case_id: None,
        };
        repo.flywheel_enqueue(&entry).unwrap();
        assert_eq!(repo.flywheel_list_pending(10).unwrap().len(), 1);

        repo.flywheel_set_status("f1", "approved", Some(123)).unwrap();
        assert!(repo.flywheel_list_pending(10).unwrap().is_empty());
    }

    #[test]
    fn quick_summary_averages_metric() {
        let repo = EvalRepo::open_in_memory().unwrap();
        repo.insert_run(&sample_run()).unwrap();
        let mut r1 = sample_result("run-1", "c1");
        r1.metric_scores.get_mut("g_eval").unwrap().value = 0.6;
        let mut r2 = sample_result("run-1", "c2");
        r2.metric_scores.get_mut("g_eval").unwrap().value = 1.0;
        repo.insert_case_result(&r1).unwrap();
        repo.insert_case_result(&r2).unwrap();

        let agg = repo.quick_summary("run-1").unwrap();
        let m = agg.metrics.get("g_eval").unwrap();
        assert_eq!(m.n, 2);
        assert!((m.mean - 0.8).abs() < 1e-12);
    }
}
