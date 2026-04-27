# Plaw Eval System — SOTA Research (April 2026)

Research scope: design plaw's evaluation system without compromise. Source priority: 2025–2026 papers, Anthropic/OpenAI engineering posts, top OSS frameworks. Stack constraint: Rust (Plaw core) + Tauri + Vue, Kimi K2.5 default model, embedding RAG via Qwen3-Embedding-0.6B.

---

## 1. Offline Eval Frameworks

The 2026 landscape splits into **eval frameworks** (DeepEval, RAGAS, PromptFoo, OpenAI Evals, lm-eval-harness) and **eval+observability platforms** (Phoenix, Langfuse, Braintrust). MTEB is its own thing — a leaderboard for embedding models.

**Top 3 SOTA:**

1. **DeepEval (Confident AI)** — strongest open-source metric library: 50+ metrics including G-Eval, DAG, conversational metrics, multimodal. Pytest-style API, CI/CD-native, vendor-agnostic. Methodology is the most rigorous in OSS: each metric has a documented prompt template, calibration target, and human-agreement benchmark. Weakness: open-source layer is local-only — no production loop without paying for Confident AI cloud.
2. **RAGAS** — gold standard for RAG-specific reference-free metrics (faithfulness, answer relevancy, context precision/recall). Backed by published research (Es et al.); methodology is the most peer-reviewed of any RAG framework. Narrow scope: RAG only, weak for agents.
3. **Braintrust** — best end-to-end loop (offline eval + production tracing + release gates) on a single platform. Closed-source SaaS; methodology less transparent than DeepEval but UX/CI integration is unmatched.

**Honorable mentions:** PromptFoo (best for red-teaming + prompt A/B; Node.js native), Phoenix (best OSS OTel-native tracing), Langfuse (best self-hostable observability), lm-eval-harness (academic standard for static benchmarks like MMLU/GSM8K), MTEB/MMTEB (only credible embedding benchmark — Qwen3-Embedding-8B leads at 70.58 multilingual).

**Tradeoffs:** Frameworks like DeepEval/RAGAS require Python — Plaw is Rust. Three integration paths: (a) sidecar Python eval-runner subprocess, (b) port metrics to Rust (only viable for deterministic ones), (c) call the same judge LLM via plaw's own Anthropic-compat HTTP client and reimplement scoring math in Rust. Path (c) is cleanest for plaw.

**Implementation effort (Rust+Tauri):**
- G-Eval, faithfulness, answer relevancy: ~400 LOC each in Rust (prompts + JSON parsing + log-probability weighted sum).
- RAGAS-style context precision/recall: needs ground-truth-free claim decomposition — ~600 LOC.
- DeepEval's DAG metric (multi-step decision-tree judge): not worth porting; spawn Python sidecar if needed.
- Phoenix/Langfuse OTel ingestion: emit OTel traces from Rust via `opentelemetry-otlp` crate, point to self-hosted Langfuse — minimal effort, maximum leverage.

**Must-read:**
- Braintrust — DeepEval Alternatives 2026: https://www.braintrust.dev/articles/deepeval-alternatives-2026
- DeepEval docs (metric methodology): https://deepeval.com/blog
- RAGAS metrics ref: https://docs.ragas.io/en/stable/concepts/metrics/available_metrics/

---

## 2. LLM-as-Judge

**SOTA techniques:**

1. **G-Eval (Liu et al., EMNLP 2023)** — chain-of-thought judge with auto-generated evaluation steps + form-filling + log-probability-weighted scoring. Achieves Spearman 0.514 with humans on summarization (vs ~0.3 for prior methods). Still the canonical scoring-judge baseline in 2026.
2. **Pairwise + position-swap (LLM-Jury / dual-pass)** — single-pass pairwise comparison shows 60–75% position bias across judges (Shi et al., 2025). The 2025–2026 consensus: always run both orderings and only count agreement; use multi-judge jury (3–5 judges, majority vote) for high-stakes evals. MAJ-Eval and ChatEval show jurys beat single judges by 10–15% human-agreement.
3. **Confidence-calibrated ensembles (LLM-as-a-Fuser, 2025)** — combines multiple judges with regression-based calibration on a small human-labeled set; reports +47% accuracy and –54% Expected Calibration Error vs single judge on JudgeBench. This is the new ceiling.

**Best practices (2026 consensus):**
- Validate every judge against a 50–200 item golden set; require ≥0.80 Spearman or ≥85% agreement before deployment.
- Pairwise > absolute scoring for relative quality; absolute scoring only for compliance/safety pass-fail.
- Mitigate self-preference bias: never let model X judge its own outputs in production gating; use a different family (e.g., Kimi K2.5 grading; Claude or GPT judging — or vice versa).
- Constitutional-AI-style judges: encode rubric as explicit principles; works well for safety, weakly for quality.
- Multi-judge jury > single judge for any decision that ships to users.

**Limitations:** Judges inherit base model biases (length, verbosity, sycophancy). Model-graded evals saturate as judge model gets stronger relative to evaluatee — eventually judge can't tell good from great.

**Implementation effort:** All of the above are prompt + aggregation logic. ~800 LOC Rust for: pairwise judge with swap, jury aggregator, golden-set validator, calibration regression. No external deps beyond Plaw's existing HTTP client.

**Must-read:**
- G-Eval paper: https://arxiv.org/abs/2303.16634
- Position bias systematic study: https://arxiv.org/pdf/2406.07791 (ACL 2025)
- LLM-as-a-Judge survey: https://arxiv.org/abs/2411.15594
- Overconfidence in LLM judges: https://arxiv.org/html/2508.06225v2

---

## 3. Statistical Rigor

The single most important paper of the last 18 months: **Anthropic's "A Statistical Approach to Language Model Evaluations"** (Miller, Nov 2024 / arxiv 2411.00640). It establishes 2026's baseline expectations.

**Key requirements:**

1. **Always report SEM and 95% CI** — `mean ± 1.96·SEM`. Reporting a bare accuracy number is now considered amateur.
2. **Cluster-correct standard errors** — many evals contain correlated questions (multiple Qs per passage, multiple turns per conversation). Naive SEM under-estimates true variance by **>3×**. Use cluster-robust SE grouped by passage/conversation/task.
3. **Paired difference analysis for A vs B** — when comparing two prompts/models, report `mean(A−B)`, `SE(A−B)`, and CI on the difference. Variance shrinks because the same question's noise correlates across A and B; this can require 4–10× fewer samples than two independent runs.
4. **Power analysis** — to detect a 2-point absolute improvement at α=0.05, β=0.2, with σ≈0.4 → need ~250 paired samples. Most teams run 20–50 and call it done; this is statistical theater.
5. **Win-rate** is preferred over absolute score for subjective tasks. Aggregate via **Bradley-Terry MLE** (LMSYS Arena standard) — more stable than Elo for static models. Report BT confidence intervals via bootstrap (1000 resamples standard).

**Tradeoffs:** Rigor costs samples → costs $. A 250-sample paired eval at Kimi K2.5 prices is trivial; at Claude Opus prices it's $5–20/run, fine for nightly CI but not per-PR. Solution: tiered eval — small (n=30) per-PR smoke test, large (n=300) nightly, full (n=1000+) weekly.

**Implementation effort:** Trivial in Rust — `statrs` crate has t-distribution, bootstrap CIs, and Welch's t-test. Bradley-Terry MLE is ~150 LOC (iterative MM algorithm). Cluster-robust SE: ~80 LOC. Total <500 LOC.

**Must-read:**
- Anthropic statistical approach: https://www.anthropic.com/research/statistical-approach-to-model-evals
- Paper: https://arxiv.org/abs/2411.00640
- Cameron Wolfe deep-dive: https://cameronrwolfe.substack.com/p/stats-llm-evals
- LMSYS Bradley-Terry methodology: https://www.lmsys.org/blog/2023-12-07-leaderboard/

---

## 4. Production Monitoring

**What top teams actually do (2025–2026):**

- **Anthropic (per "Demystifying evals" + Code Review post, 2026):** combine 20–50 carefully-curated task evals + production A/B tests + transcript review. They explicitly run Code Review on nearly every internal PR; eval results feed back into training. Petri (2025) is their open-source auditing tool for safety eval.
- **Cursor / Cognition:** publish CursorBench scores for Anthropic releases — implies internal eval suite of multi-step coding tasks scored on resolution rate, tool-error rate, token efficiency. Opus 4.7 vs 4.6: +12pp resolution, –67% tool errors.
- **OpenAI Evals + production traces:** sample-based grading at runtime, plus offline replay of failure traces.

**Top 3 production techniques:**

1. **Token-level hallucination gating (HaluGate, vLLM blog Dec 2025)** — verify claims at decode time against retrieved sources; intercept before SSE flush. Sub-100ms overhead.
2. **Trace sampling + async judge** — sample 1–10% of production traces, route to LLM-as-judge in background, alert on score drops via PSI/CUSUM. Phoenix + Langfuse both support this.
3. **Drift detection via embedding distribution shift** — track input/output embedding distributions, alert on KL/Wasserstein deltas vs reference. Catches silent prompt drift, model upgrade regressions.

**Implementation in plaw:** Plaw already emits structured events (chunk/thinking/tool_call/tool_result/done). Add: (a) trace export to OTel/Langfuse, (b) async sampling judge running locally on Kimi K2.5, (c) embedding-drift detector using Qwen3-Embedding-0.6B (already in stack), (d) hallucination check via RAG-claim-verification when retrieved context is present. ~1500 LOC Rust + 1 dashboard view in Vue.

**Must-read:**
- Anthropic Demystifying Evals: https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents
- HaluGate (token-level): https://blog.vllm.ai/2025/12/14/halugate.html
- Petri (Anthropic auditing): https://alignment.anthropic.com/2025/petri/

---

## 5. Continuous Eval Infrastructure

**SOTA pattern (PromptFoo + Braintrust + LangSmith all converged on this):**

1. **Treat prompts as code** — versioned in repo, reviewed in PR.
2. **Per-PR smoke test (n≈30)** — runs in 1–3 min via GitHub Actions; blocks merge if regression beyond CI.
3. **Nightly full eval (n≈300)** — produces dashboard, files issues for regressions.
4. **Golden dataset versioning** — datasets are git-tracked YAML/JSONL or DVC-tracked; production failures auto-promote into the dataset (with human review) — this is the "production-trace flywheel."
5. **Regression gate logic** — fail PR if `lower_CI_bound(new) < mean(baseline) − ε`, not just point-mean comparison.

**Tradeoffs:** Per-PR LLM evals create a $ floor proportional to PR rate. Mitigate with caching by `(prompt_hash, input_hash, model_version)`.

**Implementation in plaw:** Dataset format = TOML files (consistent with plaw's config style), one dir per suite. CI = `cargo test --features eval` runs the suite; results → JSON → posted as PR comment via `gh`. ~1000 LOC Rust + a `plaw eval` CLI subcommand.

**Must-read:**
- Promptfoo CI/CD: https://www.promptfoo.dev/docs/integrations/ci-cd/
- Braintrust CI/CD article: https://www.braintrust.dev/articles/best-ai-evals-tools-cicd-2025

---

## 6. Agent-Specific Evaluation

Chat-quality metrics don't transfer to agents. The 2026 SOTA explicitly separates **trajectory metrics** (how the agent reasoned) from **outcome metrics** (did it complete the task).

**Top metrics that matter:**

1. **Task Success Rate (SR)** — final-state correctness. Reference: τ-bench (Sierra, 2024), SWE-bench Verified, Terminal-Bench (May 2025), SWE-EVO (Dec 2025). Best models hit 65% SWE-bench Verified but only 21% on SWE-EVO multi-step — the long-horizon gap is huge.
2. **Tool-call accuracy** — correct tool, correct args, correct order. Decompose into: tool-selection F1, arg-validity rate, redundant-call rate. Anthropic's Opus 4.7 metric "tool errors / task" is the right unit.
3. **Step Success Rate + Plan Quality** — fraction of plan steps that execute successfully + LLM-judge score on plan coherence/efficiency before execution. Catches "right answer, wrong reasoning."
4. **Repeatability / pass^k** (τ-bench) — run same task k times; report fraction where ALL k succeed. Exposes flaky agents that average well but fail catastrophically.
5. **Error recovery quality** — inject failures (tool timeout, bad output) mid-trajectory, score whether agent recovers. Underrated.

**Anthropic's framing (Demystifying Evals, 2026):** task → trial → grader → transcript → outcome. Always read transcripts; graders that look right on aggregate often reject creative valid solutions (Opus 4.5 finding loopholes in τ²-bench).

**Tradeoffs:** Outcome-only graders miss reasoning failures that will bite under distribution shift. Trajectory-only graders penalize creative solutions. Run both; weight by deployment risk.

**Implementation in plaw:** plaw is itself an agent — eval harness must spawn isolated plaw instances against fixture environments (sandboxed `plaw-data/` per trial), capture full trajectory (already streamed via WebSocket protocol — reuse), run graders. Suites: (a) coding sandbox tasks (mini-SWE-bench-style with local git repos), (b) tool-routing micro-tasks, (c) RAG-grounded QA, (d) error-injection variants. ~2000 LOC for harness + suites.

**Must-read:**
- Anthropic Demystifying Evals: https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents
- τ-bench: https://sierra.ai/blog/benchmarking-ai-agents
- Beyond Task Completion (Dec 2025): https://arxiv.org/html/2512.12791v1
- LLM Agent Eval survey (Jul 2025): https://arxiv.org/html/2507.21504v1
- SWE-EVO (Dec 2025): https://arxiv.org/pdf/2512.18470v1

---

## Build vs Integrate Recommendation for plaw

| Component | Decision | Rationale |
|---|---|---|
| Core metrics (G-Eval, faithfulness, pairwise judge) | **Build in Rust** | ~2000 LOC total, removes Python dep, runs in-process |
| Statistical layer (CI, Bradley-Terry, paired diff) | **Build in Rust** (`statrs`) | <500 LOC, table-stakes rigor |
| OTel tracing → Langfuse self-hosted | **Integrate** | Free, OSS, dashboards solved |
| Embedding drift detection | **Build** | Already have Qwen3-Embedding-0.6B in stack |
| Eval CLI + dataset format | **Build** (TOML, `plaw eval`) | Consistent with plaw idioms |
| Agent harness (sandboxed trial runner) | **Build** | Plaw-specific; reuse WS protocol |
| Production replay / failure flywheel | **Build minimal**, integrate Langfuse for storage | Hybrid keeps data local, dashboards offloaded |
| MTEB embedding benchmark | **Skip building**, use published Qwen3 numbers + small custom retrieval suite | MTEB is for embedding-model authors, not consumers |

Total estimated build: **~6000 LOC Rust + ~1500 LOC Vue dashboards**. Achievable in 2–3 focused weeks. End state: every plaw release auto-evaluated with statistical rigor matching Anthropic's published bar.
