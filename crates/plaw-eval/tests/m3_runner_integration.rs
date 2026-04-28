//! M3 integration test — drive the full runner end-to-end against a mock
//! WebSocket server that mimics plaw's gateway protocol.
//!
//! This is the "can run a 1-case minimal suite, case completes → SQLite has
//! full record" acceptance bullet from `tasks.md` M3.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use plaw_eval::runner::{execute, PlawClient, RunnerConfig};
use plaw_eval::storage::EvalRepo;
use plaw_eval::suite::{Case, CaseInput, ChatMsg, ChatRole, JudgeMode, JudgeSpec, Suite};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::protocol::Message;

/// Mock plaw gateway. For each connection it expects exactly one message,
/// then emits canned `chunk` → `done` events. Behaviour is parameterised by
/// `script` so individual tests can simulate failures and per-frame delays.
async fn spawn_mock(script: MockScript) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let script = Arc::new(script);

    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(p) => p,
                Err(_) => break,
            };
            let script = script.clone();
            tokio::spawn(async move {
                let ws = tokio_tungstenite::accept_async(stream).await;
                let mut ws = match ws {
                    Ok(w) => w,
                    Err(_) => return,
                };
                // Drain the inbound request frame.
                if let Some(Ok(Message::Text(_))) = ws.next().await {
                    if !script.delay.is_zero() {
                        tokio::time::sleep(script.delay).await;
                    }
                    for frame in script.frames.iter() {
                        let _ = ws.send(Message::Text(frame.clone())).await;
                    }
                }
                let _ = ws.close(None).await;
            });
        }
    });
    addr
}

#[derive(Clone)]
struct MockScript {
    frames: Vec<String>,
    /// Delay before sending the first frame. Useful for cancellation tests.
    delay: Duration,
}

impl MockScript {
    fn ok(text: &str) -> Self {
        Self {
            frames: vec![
                format!(r#"{{"type":"chunk","content":"{text}"}}"#),
                format!(
                    r#"{{"type":"done","full_response":"{text}","usage":{{"input_tokens":42,"output_tokens":7}}}}"#
                ),
            ],
            delay: Duration::ZERO,
        }
    }

    fn ok_slow(text: &str, delay: Duration) -> Self {
        let mut s = Self::ok(text);
        s.delay = delay;
        s
    }

    fn error(message: &str) -> Self {
        Self {
            frames: vec![format!(r#"{{"type":"error","message":"{message}"}}"#)],
            delay: Duration::ZERO,
        }
    }
}

fn minimal_suite(name: &str, n_cases: usize) -> Suite {
    let cases = (0..n_cases)
        .map(|i| Case {
            id: format!("c{i}"),
            input: CaseInput::Chat {
                messages: vec![ChatMsg {
                    role: ChatRole::User,
                    content: format!("hi #{i}"),
                }],
            },
            expected: None,
            tags: vec![],
            cluster_id: None,
            source: "authored".into(),
            promoted_at: None,
        })
        .collect();
    Suite {
        name: name.into(),
        version: "1.0.0".into(),
        description: "integration".into(),
        default_judge: JudgeSpec {
            model: "kimi-k2.5".into(),
            provider: "kimi".into(),
            temperature: 0.0,
            mode: JudgeMode::default(),
        },
        metrics: vec![],
        cases,
    }
}

#[tokio::test]
async fn single_case_run_writes_results_to_sqlite() {
    let addr = spawn_mock(MockScript::ok("hello world")).await;
    let plaw = PlawClient::new(format!("ws://{addr}")).with_timeout(Duration::from_secs(5));

    let repo = Arc::new(EvalRepo::open_in_memory().unwrap());
    let cfg = RunnerConfig::new(minimal_suite("smoke", 1), plaw, repo.clone());

    let summary = execute(cfg).await.expect("run should succeed");
    assert_eq!(summary.n_total, 1);
    assert_eq!(summary.n_completed, 1);
    assert_eq!(summary.n_failed, 0);
    assert!(!summary.cancelled);

    // SQLite should have one run + one case_result.
    let run = repo.load_run(&summary.run_id).unwrap().unwrap();
    assert_eq!(run.suite_name, "smoke");
    assert!(run.finished_at.is_some());

    let results = repo.load_case_results(&summary.run_id).unwrap();
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r.plaw_response, "hello world");
    assert_eq!(r.tokens_in, 42);
    assert_eq!(r.tokens_out, 7);
    assert!(r.error.is_none());
}

#[tokio::test]
async fn errors_are_isolated_per_case_and_recorded() {
    // Server that always replies with an error event.
    let addr = spawn_mock(MockScript::error("simulated failure")).await;
    let plaw = PlawClient::new(format!("ws://{addr}")).with_timeout(Duration::from_secs(5));
    let repo = Arc::new(EvalRepo::open_in_memory().unwrap());
    let cfg = RunnerConfig::new(minimal_suite("err", 3), plaw, repo.clone());

    let summary = execute(cfg).await.unwrap();
    assert_eq!(summary.n_total, 3);
    assert_eq!(summary.n_completed, 0);
    assert_eq!(summary.n_failed, 3);

    let results = repo.load_case_results(&summary.run_id).unwrap();
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r.error.is_some()));
    assert!(results
        .iter()
        .all(|r| r.error.as_deref().unwrap().contains("simulated failure")));
}

#[tokio::test]
async fn concurrency_runs_multiple_cases_in_parallel() {
    let addr = spawn_mock(MockScript::ok("ok")).await;
    let plaw = PlawClient::new(format!("ws://{addr}")).with_timeout(Duration::from_secs(5));
    let repo = Arc::new(EvalRepo::open_in_memory().unwrap());
    let mut cfg = RunnerConfig::new(minimal_suite("concurrent", 12), plaw, repo.clone());
    cfg.concurrency = 4;

    let started = std::time::Instant::now();
    let summary = execute(cfg).await.unwrap();
    let elapsed = started.elapsed();

    assert_eq!(summary.n_completed, 12);
    // 12 cases at concurrency 4 should comfortably finish in <2s on the
    // mock — anything way over that means we accidentally serialised.
    assert!(
        elapsed < Duration::from_secs(5),
        "12 cases at concurrency 4 should be fast, took {:?}",
        elapsed
    );
}

#[tokio::test]
async fn cancellation_skips_remaining_cases() {
    // Mock holds each case for 200ms so that when we cancel after 100ms,
    // only the first batch (concurrency=2) is in flight; the rest get the
    // cancellation token and bail out immediately.
    let addr = spawn_mock(MockScript::ok_slow("ok", Duration::from_millis(200))).await;
    let plaw = PlawClient::new(format!("ws://{addr}")).with_timeout(Duration::from_secs(5));
    let repo = Arc::new(EvalRepo::open_in_memory().unwrap());
    let mut cfg = RunnerConfig::new(minimal_suite("cancel", 50), plaw, repo.clone());
    cfg.concurrency = 2;
    let cancel = cfg.cancel.clone();

    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });

    let summary = execute(cfg).await.unwrap();
    assert!(summary.cancelled);
    // Most cases should be cancelled — only the first batch had a chance
    // to start. We allow some slack but assert the cancellation at least
    // halted the run early.
    assert!(
        summary.n_completed < 50,
        "expected fewer than 50 to complete, got {}",
        summary.n_completed
    );
    assert!(
        summary.n_completed <= 4,
        "concurrency=2 should mean at most ~2 cases ran before cancel propagated, got {}",
        summary.n_completed
    );
}

#[tokio::test]
async fn sample_n_subsets_the_suite() {
    let addr = spawn_mock(MockScript::ok("ok")).await;
    let plaw = PlawClient::new(format!("ws://{addr}")).with_timeout(Duration::from_secs(5));
    let repo = Arc::new(EvalRepo::open_in_memory().unwrap());
    let mut cfg = RunnerConfig::new(minimal_suite("subset", 10), plaw, repo.clone());
    cfg.sample_n = Some(3);
    cfg.sample_seed = Some(7);

    let summary = execute(cfg).await.unwrap();
    assert_eq!(summary.n_total, 3);
    assert_eq!(summary.n_completed, 3);

    let results = repo.load_case_results(&summary.run_id).unwrap();
    assert_eq!(results.len(), 3);
}
