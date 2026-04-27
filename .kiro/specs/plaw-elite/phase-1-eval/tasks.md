# Phase 1: Eval Foundation — Tasks

> 基于 [design.md](./design.md) 拆解的可执行任务。每个任务 ≤ 1 个工作日。
>
> 依赖图按 task ID 顺序排列；并行任务在同一个 milestone 内。
>
> 状态符号：`☐` = todo · `▶` = in progress · `✓` = done · `✗` = abandoned

---

## Milestone 0：脚手架（Week 1，~3 天）

目标：crate 结构搭好，能跑 hello world

- ☐ **T0.1** 在 root `Cargo.toml` 新增 workspace member `crates/plaw-eval` 和 `crates/plaw-eval-cli`
- ☐ **T0.2** 创建 `crates/plaw-eval/Cargo.toml`，加 design.md §二里列的所有依赖
- ☐ **T0.3** 创建 `crates/plaw-eval-cli/Cargo.toml`，依赖 `plaw-eval` + `clap`
- ☐ **T0.4** 在 `crates/plaw-eval/src/lib.rs` 写 module 骨架（空 mod 声明）
- ☐ **T0.5** 在 `crates/plaw-eval-cli/src/main.rs` 写 clap subcommand 骨架（`run`, `list`, `compare`, `power`, `promote`, `cache`）
- ☐ **T0.6** 验证：`cargo build --release -p plaw-eval-cli` 通过
- ☐ **T0.7** 验证：`cargo run --release -p plaw-eval-cli -- --help` 输出所有子命令

**Milestone 0 验收**：CLI 能跑通 `--help`，crate 结构和 design.md §二一致。

---

## Milestone 1：Stats 库（Week 1-2，~5 天）

目标：Anthropic 级别的统计严谨度，纯 Rust 实现，与 scipy 数值匹配

- ☐ **T1.1** `stats/ci.rs`：实现 `t_distribution_ci(mean, sem, n, alpha) -> (low, high)`
- ☐ **T1.2** `stats/ci.rs`：实现 `wilson_score_ci(successes, n, alpha) -> (low, high)` for binary metrics
- ☐ **T1.3** `stats/ci.rs`：实现 `bootstrap_ci<F>(samples, n_resamples, percentile_low, percentile_high, statistic_fn)` 通用 bootstrap
- ☐ **T1.4** `stats/cluster_se.rs`：实现 `cluster_robust_se(values, cluster_ids) -> f64`
- ☐ **T1.5** `stats/cluster_se.rs`：实现自动判定 `should_use_cluster_se(n, n_clusters) -> bool`（阈值 n_clusters < n/5）
- ☐ **T1.6** `stats/paired.rs`：实现 `paired_difference(samples_a, samples_b) -> PairedResult { mean_diff, se, ci }`
- ☐ **T1.7** `stats/power.rs`：实现 `required_sample_size(effect_pp, sigma, alpha, power) -> usize`
- ☐ **T1.8** `stats/bradley_terry.rs`：实现 MM 迭代求解 `bradley_terry_mle(comparisons: &[(i, j, winner)]) -> Vec<f64>`
- ☐ **T1.9** `stats/bradley_terry.rs`：实现 bootstrap CI for B-T 系数
- ☐ **T1.10** `tests/stats_correctness.rs`：用 scipy 生成 reference 值（fixtures/scipy_reference.json），cross-check
  - t-CI：1000 个随机分布，CI 边界差距 < 1e-6
  - Cluster SE：与 [Cameron-Miller 论文公式]() 比对
  - B-T MLE：与 `choix` Python 库比对（差距 < 1e-4）
- ☐ **T1.11** 单元测试覆盖率 ≥ 90%（用 `cargo tarpaulin` 验证）

**Milestone 1 验收**：所有 stats fn 都有 ≥ 1 个 cross-check test 通过；rust-doc 示例能跑通。

---

## Milestone 2：Suite & Storage（Week 2，~3 天）

目标：能加载 TOML suite，能往 SQLite 写数据

- ☐ **T2.1** `suite/case.rs`：定义 `Suite`, `Case`, `CaseInput`, `CaseExpected`, `JudgeSpec` 数据结构（design §3.1）
- ☐ **T2.2** `suite/loader.rs`：实现 `load_suite(path: &Path) -> Result<Suite>`，TOML 反序列化 + schema 验证
- ☐ **T2.3** `suite/version.rs`：实现 semver 解析 + 兼容性检查（major version mismatch 拒绝加载）
- ☐ **T2.4** `storage/schema.rs`：定义 `Run`, `CaseResult`, `MetricScore`, `AggregateReport` 结构
- ☐ **T2.5** `storage/repo.rs`：实现 `EvalRepo::new(db_path)` 自动建表（design §3.3 SQL）
- ☐ **T2.6** `storage/repo.rs`：实现 `insert_run`, `insert_case_result`, `update_run_finished`
- ☐ **T2.7** `storage/repo.rs`：实现 `load_run(id)`, `list_runs(suite, limit)`, `get_baseline(suite)` 
- ☐ **T2.8** `storage/repo.rs`：实现 judge cache `get_cached(key)`, `set_cached(key, response)`, `clear_expired(ttl_days)`
- ☐ **T2.9** 写 `evals/_template/cases.toml`，作为示例文档
- ☐ **T2.10** 集成测试：load template → insert run → assert SQLite 中数据正确

**Milestone 2 验收**：从 TOML 加载 suite，写入 SQLite，再读出来字段全对。

---

## Milestone 3：Plaw Client + Runner 核心（Week 2-3，~4 天）

目标：能调用 plaw 主程序跑一个 case

- ☐ **T3.1** `runner/plaw_client.rs`：实现 `PlawClient::new(ws_url, bearer)`
- ☐ **T3.2** `runner/plaw_client.rs`：实现 `send(input: &CaseInput) -> Result<PlawResponse>`，处理流式 SSE 事件
- ☐ **T3.3** `runner/plaw_client.rs`：处理 `chunk` / `tool_call` / `tool_result` / `done` / `error` 事件
- ☐ **T3.4** `runner/plaw_client.rs`：超时处理（默认 5 分钟）+ 取消支持
- ☐ **T3.5** `runner/cache.rs`：实现 `JudgeCache::get(key) / set(key, value)`，键生成用 SHA256
- ☐ **T3.6** `runner/executor.rs`：实现 `Runner::new(suite, judges, plaw_client, repo)` 构造
- ☐ **T3.7** `runner/executor.rs`：实现 `Runner::execute(n: Option<usize>) -> Result<RunSummary>` 串行版本
- ☐ **T3.8** `runner/executor.rs`：升级到并发执行（tokio semaphore，默认 4）
- ☐ **T3.9** `runner/executor.rs`：失败重试 + 不阻塞其他 case
- ☐ **T3.10** 进度条（indicatif）：每个 case 完成更新
- ☐ **T3.11** Cancellation：Ctrl-C 优雅退出，写入部分结果

**Milestone 3 验收**：能跑一个 1-case 的最小 suite，case 完成后 SQLite 里有完整记录。

---

## Milestone 4：Judges 实现（Week 3-4，~5 天）

目标：可用的 LLM-as-Judge，包括 pairwise + jury

- ☐ **T4.1** `judges/client.rs`：实现 `JudgeClient` trait（async fn `complete(prompt) -> Result<String>`）
- ☐ **T4.2** `judges/client.rs`：实现 `KimiJudgeClient`（调 Kimi K2.5 OpenAI-compat endpoint）
- ☐ **T4.3** `judges/client.rs`：实现 `AnthropicJudgeClient`（调 Anthropic Messages API）
- ☐ **T4.4** `judges/client.rs`：实现 `OpenAIJudgeClient`（调 OpenAI chat completions）
- ☐ **T4.5** `judges/pairwise.rs`：实现 `PairwiseJudge::compare(case, response_a, response_b) -> Decision`
  - 强制 dual-pass（两次调用，位置交换）
  - 不一致时返回 `Tie`（剔除位置偏见）
- ☐ **T4.6** `judges/pairwise.rs`：实现 prompt template（参考 LMSYS Arena 标准）
- ☐ **T4.7** `judges/jury.rs`：实现 `Jury::new(judges, aggregator)` + `decide(case, response_a, response_b)`
- ☐ **T4.8** `judges/jury.rs`：实现 `MajorityVote` aggregator（≥3/5 一致）
- ☐ **T4.9** `judges/jury.rs`：实现 `ConfidenceWeighted` aggregator（LLM-as-a-Fuser 模式）
- ☐ **T4.10** `judges/jury.rs`：实现 cross-family 强制（同 family 拒绝构造 Jury）
- ☐ **T4.11** Self-preference bias 防御：当被测模型与 judge 同 family 时记录 warning
- ☐ **T4.12** 集成测试：用固定 fixture（人工标注的 100 case）验证 jury 与人类一致率 ≥ 0.80

**Milestone 4 验收**：jury 跑通，cross-family 强制有效，与 fixture 人类标注 Spearman ≥ 0.80。

---

## Milestone 5：Metrics 实现（Week 4-5，~6 天）

目标：实现 design §4 列出的所有 metric

### 通用质量

- ☐ **T5.1** `metrics/g_eval.rs`：实现 G-Eval scoring 流程
  - 自动生成 evaluation steps（CoT prompt）
  - 强制 JSON 输出 `{score: int, confidence: float}`
  - Logprobs fallback：confidence 加权
- ☐ **T5.2** `metrics/g_eval.rs`：with self-test fixture（n=20 已知答案，验证分数稳定）

### RAG 专用

- ☐ **T5.3** `metrics/faithfulness.rs`：实现 claim 提取 + 文档 verification
- ☐ **T5.4** `metrics/relevancy.rs`：实现 answer ↔ question embedding sim + judge backup
- ☐ **T5.5** `metrics/context.rs`：实现 context precision（召回的有用率）
- ☐ **T5.6** `metrics/context.rs`：实现 context recall（应召回的覆盖率）

### Agent 专用

- ☐ **T5.7** `metrics/tool.rs`：实现 tool selection F1（vs expected_tools）
- ☐ **T5.8** `metrics/tool.rs`：实现 arg validity rate（schema 检查）
- ☐ **T5.9** `metrics/tool.rs`：实现 redundant call rate（重复同 tool+args 比例）
- ☐ **T5.10** `metrics/trajectory.rs`：实现 step success rate（每步是否完成）
- ☐ **T5.11** `metrics/trajectory.rs`：实现 plan quality judge（计划合理性 LLM-judge）
- ☐ **T5.12** `metrics/repeatability.rs`：实现 pass^k（同 case 跑 k 次都通过的比例）
- ☐ **T5.13** Error recovery rate：在 `error_recovery` suite 中通过特殊 metric 实现

**Milestone 5 验收**：每个 metric 都有单元测试 + 至少 1 个 fixture case 通过。

---

## Milestone 6：Aggregation & Reports（Week 5，~3 天）

目标：把 case 结果聚合成统计严谨的报告

- ☐ **T6.1** `runner/executor.rs::aggregate()`：从 SQLite 读 case results → 计算 per-metric mean / SE / CI
- ☐ **T6.2** `runner/executor.rs::aggregate()`：自动启用 cluster SE（当 cluster_id 数量符合阈值）
- ☐ **T6.3** `runner/executor.rs::compare()`：实现 `compare_runs(baseline_id, candidate_id) -> ComparisonReport`
  - Paired diff（如 case_id 匹配）
  - 独立 diff（fallback）
  - Gate verdict
- ☐ **T6.4** `report/json.rs`：序列化 `AggregateReport` → JSON
- ☐ **T6.5** `report/markdown.rs`：渲染人类可读的 Markdown 表格
  - 每个 metric 一行：mean ± CI / N / vs baseline
  - 高亮 gate-failing metrics
- ☐ **T6.6** `report/pr_comment.rs`：构造 PR comment Markdown（含折叠的详细 case 失败列表）
- ☐ **T6.7** `report/sarif.rs`（可选）：输出 SARIF 格式（GitHub Code Scanning）

**Milestone 6 验收**：能跑 `plaw eval compare --baseline X --candidate Y` 输出 Markdown，含 paired diff + gate verdict。

---

## Milestone 7：CLI 完整化（Week 5-6，~3 天）

目标：design §四列出的所有 CLI 命令可用

- ☐ **T7.1** `plaw eval run --suite <name> [--n <samples>] [--judge <model>] [--seed <int>] [--output <path>]`
- ☐ **T7.2** `plaw eval list [--detail]`
- ☐ **T7.3** `plaw eval compare --baseline <run_id> --candidate <run_id|json_path> [--gate <expr>]`
- ☐ **T7.4** `plaw eval power --effect <pp> --sigma <stdev> [--alpha 0.05] [--power 0.8]`
- ☐ **T7.5** `plaw eval promote --trace <id> --suite <name> [--review-status approved]`
- ☐ **T7.6** `plaw eval cache {clear|stats}`
- ☐ **T7.7** `plaw eval doctor`：环境自检（API keys、plaw 端点、SQLite 写权限）
- ☐ **T7.8** 全局 flags：`--config <path>`, `--quiet`, `--verbose`, `--no-color`
- ☐ **T7.9** Shell completion 脚本（bash/zsh/fish via clap_complete）

**Milestone 7 验收**：所有 design §四列出的命令都能跑通，`--help` 文档完整。

---

## Milestone 8：CI 集成（Week 6，~2 天）

目标：GitHub Actions 跑通 smoke + nightly

- ☐ **T8.1** `.github/workflows/plaw-eval.yml` smoke job（按 design §6.1）
- ☐ **T8.2** `.github/workflows/plaw-eval.yml` nightly job
- ☐ **T8.3** Secrets 配置文档：`docs/eval/ci-secrets.md`（如何加 KIMI_API_KEY 等）
- ☐ **T8.4** PR comment 模板（marocchino/sticky-pull-request-comment）
- ☐ **T8.5** Cache 配置（actions/cache）减少重复构建
- ☐ **T8.6** 测试：手动开 PR，故意改坏一个 prompt，验证 gate fail
- ☐ **T8.7** 测试：手动开 PR，正常改动，验证 gate pass
- ☐ **T8.8** Nightly artifact 上传（GitHub Releases 或 actions artifacts）

**Milestone 8 验收**：3 个手动测试 PR 都符合预期。

---

## Milestone 9：初始 Eval Suites（Week 6-7，~5 天）

目标：5 个 suite 实装，每个 ≥ 30 case

- ☐ **T9.1** `evals/chat_quality/`：30+ 通用对话 case，cluster_id 按对话主题
  - 闲聊、技术问答、创意写作、复杂推理 各 10 例
- ☐ **T9.2** `evals/tool_routing/`：30+ tool 选择 case
  - 涵盖 plaw 现有所有工具（shell, read_file, web_search, ...）
  - 标注 expected_tool_sequence
- ☐ **T9.3** `evals/rag_grounded_qa/`：30+ RAG case
  - 用 plaw 的现有知识库或测试 fixture
  - 包含 in-distribution / out-of-distribution / 对抗（信息不在文档里）
- ☐ **T9.4** `evals/agent_multi_step/`：30+ 多步任务
  - 简单（2-3 步）/ 中等（4-6 步）/ 复杂（7+ 步）
  - final_state 可机器验证
- ☐ **T9.5** `evals/error_recovery/`：30+ 故障注入 case
  - 工具超时、bad output、permission denied、网络断
  - Expected：agent 应识别并尝试恢复
- ☐ **T9.6**（可选）`evals/adversarial/`：20+ prompt injection case
  - 文档里包含恶意指令
  - Expected：agent 不执行恶意指令

**Milestone 9 验收**：5 个 suite 都能跑通 smoke eval；每个有 README 说明设计意图。

---

## Milestone 10：Production Trace Flywheel（Week 7-8，~3 天）

目标：生产 trace → eval case 自动化

- ☐ **T10.1** `flywheel/sampler.rs`：实现 trace 采样（默认 5%，可配）
- ☐ **T10.2** `flywheel/sampler.rs`：将采样的 trace 写入 `flywheel_queue` 表
- ☐ **T10.3** `flywheel/reviewer.rs`：实现 `list_pending() / approve(id) / reject(id)` API
- ☐ **T10.4** CLI：`plaw eval flywheel list-pending [--limit N]`
- ☐ **T10.5** CLI：`plaw eval flywheel review <id> {approve|reject}`
- ☐ **T10.6** `flywheel/promoter.rs`：approved trace → 转换为 Case → 追加到 suite TOML
- ☐ **T10.7** Promotion 自动 git commit（`feat(eval): promote trace <id> to <suite>`）
- ☐ **T10.8** Backwards compatibility：promoted case 标 `source: "flywheel"` + `promoted_at`
- ☐ **T10.9** 端到端测试：从 plaw 跑一个对话 → 采样进队列 → 手动 approve → 进入 suite → 下次 eval 跑到

**Milestone 10 验收**：完整飞轮端到端跑通至少一次。

---

## Milestone 11：文档 & Baseline（Week 8，~3 天）

目标：完成验收 A-4 / A-5

- ☐ **T11.1** `docs/eval/methodology.md`（≥ 2000 字，design §八的内容）
- ☐ **T11.2** `docs/eval/ci-secrets.md`（CI 配置教程）
- ☐ **T11.3** `docs/eval/suite-design.md`（如何写 suite + cluster_id 用法）
- ☐ **T11.4** `docs/eval/judge-selection.md`（judge model 选择指南，含 cross-family 解释）
- ☐ **T11.5** `docs/eval/troubleshooting.md`（常见问题）
- ☐ **T11.6** `crates/plaw-eval/README.md`（快速上手）
- ☐ **T11.7** 跑 nightly eval，记录 baseline → `docs/eval/baseline-2026-Q2.md`
  - 5 个 suite 各跑 n=300
  - 报告每个 metric 的 mean ± CI
  - 标注 "this is the baseline; future PRs gate against these"
- ☐ **T11.8** 更新 plaw 主 README，添加 "Plaw Elite Phase 1: Eval Foundation" 章节

**Milestone 11 验收**：所有文档完成；baseline 数字写入 git。

---

## Milestone 12：收尾 & 验收（Week 8）

- ☐ **T12.1** 全部 8 个验收标准（requirements.md §五）逐项 check
- ☐ **T12.2** 单元测试覆盖率 ≥ 90% 验证（`cargo tarpaulin --out Html`）
- ☐ **T12.3** Cargo.lock 提交，确保依赖固定
- ☐ **T12.4** 运行完整 nightly eval 一次，验证 < 30 min 完成
- ☐ **T12.5** 写 `phase-1-eval/retrospective.md`：做对了什么、做错了什么、下个 phase 怎么调
- ☐ **T12.6** Git tag：`elite-phase-1-complete`
- ☐ **T12.7** 更新 `00-vision.md` 如果 vision 在实战中需要修订
- ☐ **T12.8** 通知：Phase 2 可以启动

**Phase 1 完成判定**：T12.1 全勾 + git tag 已打。

---

## 总览

| Milestone | 周 | 任务数 | 关键交付 |
|-----------|----|----|---------|
| M0 脚手架 | 1 | 7 | crate 编译通过，CLI 骨架 |
| M1 Stats 库 | 1-2 | 11 | scipy cross-check 通过 |
| M2 Suite & Storage | 2 | 10 | TOML 加载 + SQLite 写入 |
| M3 Plaw Client + Runner | 2-3 | 11 | 能跑 1-case suite |
| M4 Judges | 3-4 | 12 | Cross-family jury Spearman ≥ 0.80 |
| M5 Metrics | 4-5 | 13 | 所有 metric 实现 + 测试 |
| M6 Aggregation & Reports | 5 | 7 | Paired diff + Markdown 报告 |
| M7 CLI 完整化 | 5-6 | 9 | 所有子命令可用 |
| M8 CI 集成 | 6 | 8 | Smoke + nightly 跑通 |
| M9 Eval Suites | 6-7 | 6 | 5 个 suite × 30+ case |
| M10 Flywheel | 7-8 | 9 | 端到端飞轮跑通 |
| M11 文档 & Baseline | 8 | 8 | 文档 + baseline 数字 |
| M12 收尾 | 8 | 8 | 验收 + tag |
| **总计** | **8 周** | **~120 任务** | Anthropic 级 eval 体系 |

---

## 并行机会

以下 milestone 可以并行（多人或多 session）：

- M1 Stats + M2 Suite/Storage（独立）
- M4 Judges + M5 Metrics（M4 完成 T4.1-T4.4 后并行）
- M9 Suites 可在 M8 完成后并行写
- M11 文档可在 M10 完成后并行

---

## 进度追踪

每周末更新本文档：
- 把完成的任务从 `☐` 改为 `✓`
- 当前在做的标 `▶`
- 估计偏差 > 50% 的任务加注释解释
- Milestone 完成时在 retrospective.md 记录

---

## 本 tasks 的版本管理

- v1.0（2026-04-26）：初版
- 实现过程中发现遗漏的任务可以补加（在对应 milestone 末尾）
- 不能删除已经定义的任务，只能标 `✗ abandoned` + 说明原因
