# Phase 1: Eval Foundation — Design

> 基于 [requirements.md](./requirements.md) 设计的具体技术方案。
>
> 写作日期：2026-04-26 · 设计目标：在 6-8 周内交付一个 Anthropic 级别严谨度的 eval 体系

---

## 一、系统总览

```
┌────────────────────────────────────────────────────────────────────┐
│  plaw-eval（新增 crate）                                             │
│                                                                      │
│  ┌──────────────┐  ┌─────────────┐  ┌─────────────┐                │
│  │ stats lib    │  │ metrics lib │  │ judges lib  │                │
│  │ - CI / SE    │  │ - G-Eval    │  │ - pairwise  │                │
│  │ - Bradley-T  │  │ - faithful  │  │ - jury      │                │
│  │ - bootstrap  │  │ - tool acc  │  │ - cross-fam │                │
│  └──────────────┘  └─────────────┘  └─────────────┘                │
│         │                  │                │                        │
│         └──────────────────┴────────────────┘                        │
│                            │                                          │
│  ┌─────────────────────────▼────────────────────────────┐           │
│  │  Runner（核心调度）                                    │           │
│  │  · 加载 suite TOML                                     │           │
│  │  · 并发跑 cases（rate-limited）                        │           │
│  │  · 缓存 LLM 响应                                        │           │
│  │  · 结果落地 SQLite                                      │           │
│  └─────────────────────────┬────────────────────────────┘           │
│                            │                                          │
│  ┌─────────────────────────▼────────────────────────────┐           │
│  │  Reporter（输出层）                                    │           │
│  │  · JSON / Markdown / SARIF                             │           │
│  │  · paired diff with CI                                 │           │
│  │  · PR comment generator                                │           │
│  └────────────────────────────────────────────────────────┘           │
└────────────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────────────────┐
│  plaw-eval-cli（新增 binary）                                        │
│  · plaw eval run / list / compare / power / promote                  │
└────────────────────────────────────────────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────────────────┐
│  存储 & 集成                                                          │
│  · evals/<suite>/{cases.toml, golden.toml, judge.toml}              │
│  · plaw-data/.plaw/eval/runs.db (SQLite, 单文件)                     │
│  · GitHub Actions：plaw-eval.yml workflow                            │
│  · plaw 主程序：通过 WS 协议被调用（不改主程序）                          │
└────────────────────────────────────────────────────────────────────┘
```

---

## 二、Crate 结构

新建 workspace member：`crates/plaw-eval/`

```
crates/plaw-eval/
├── Cargo.toml
├── src/
│   ├── lib.rs              # 库入口，re-export
│   ├── stats/              # 统计学库
│   │   ├── mod.rs
│   │   ├── ci.rs           # 置信区间（t-distribution / bootstrap）
│   │   ├── cluster_se.rs   # Cluster-robust standard errors
│   │   ├── paired.rs       # Paired difference analysis
│   │   ├── power.rs        # Power analysis
│   │   └── bradley_terry.rs # B-T MLE for win-rate
│   ├── metrics/            # 指标库
│   │   ├── mod.rs
│   │   ├── g_eval.rs       # G-Eval CoT 评分
│   │   ├── faithfulness.rs # RAG faithfulness
│   │   ├── relevancy.rs    # Answer relevancy
│   │   ├── context.rs      # Context precision/recall
│   │   ├── tool.rs         # Tool-call accuracy
│   │   ├── trajectory.rs   # Step success / plan quality
│   │   └── repeatability.rs # pass^k
│   ├── judges/             # Judge 实现
│   │   ├── mod.rs
│   │   ├── pairwise.rs     # 强制 dual-pass 的 pairwise judge
│   │   ├── jury.rs         # Multi-judge aggregator
│   │   └── client.rs       # LLM client 抽象（kimi / anthropic / openai）
│   ├── suite/              # 数据集结构
│   │   ├── mod.rs
│   │   ├── loader.rs       # TOML 加载
│   │   ├── case.rs         # Case 数据模型
│   │   └── version.rs      # 版本检查
│   ├── runner/             # 调度
│   │   ├── mod.rs
│   │   ├── executor.rs     # 并发执行
│   │   ├── cache.rs        # LLM 响应缓存
│   │   └── plaw_client.rs  # 通过 WS 调用 plaw 主程序
│   ├── storage/            # SQLite 存储
│   │   ├── mod.rs
│   │   ├── schema.rs       # 表结构
│   │   └── repo.rs         # 数据访问层
│   ├── report/             # 输出
│   │   ├── mod.rs
│   │   ├── markdown.rs
│   │   ├── json.rs
│   │   └── pr_comment.rs
│   └── flywheel/           # 生产 trace 飞轮
│       ├── mod.rs
│       ├── sampler.rs
│       ├── reviewer.rs
│       └── promoter.rs
└── tests/
    ├── stats_correctness.rs  # 与 scipy.stats cross-check
    ├── metrics_smoke.rs
    └── e2e_smoke.rs

crates/plaw-eval-cli/
├── Cargo.toml
└── src/main.rs              # clap-based CLI
```

**依赖**（精选，不臃肿）：
```toml
[dependencies]
statrs = "0.18"        # 统计分布（t / chi2 / normal）
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.9"
reqwest = { version = "0.12", features = ["json", "rustls-tls"] }
rusqlite = { version = "0.32", features = ["bundled"] }
tracing = "0.1"
clap = { version = "4", features = ["derive"] }
anyhow = "1"
thiserror = "2"
sha2 = "0.10"          # 缓存键 hashing
indicatif = "0.17"     # 进度条
```

---

## 三、关键数据模型

### 3.1 Suite / Case（输入）

```rust
// suite/case.rs
#[derive(Debug, Deserialize, Clone)]
pub struct Suite {
    pub name: String,
    pub version: String,           // semver, e.g. "1.0.3"
    pub description: String,
    pub default_judge: JudgeSpec,
    pub metrics: Vec<MetricSpec>,
    pub cases: Vec<Case>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Case {
    pub id: String,
    pub input: CaseInput,
    pub expected: Option<CaseExpected>,
    pub tags: Vec<String>,
    pub cluster_id: Option<String>,  // 用于 cluster-robust SE
}

#[derive(Debug, Deserialize, Clone)]
pub enum CaseInput {
    Chat { messages: Vec<ChatMsg> },
    Agent { task: String, max_steps: usize },
    Rag { question: String, ground_truth_doc: Option<String> },
}

#[derive(Debug, Deserialize, Clone)]
pub struct CaseExpected {
    pub answer: Option<String>,           // for grader
    pub answer_keywords: Vec<String>,     // for keyword check
    pub tool_sequence: Vec<String>,       // for agent tasks
    pub final_state: Option<JsonValue>,   // for agent tasks
}

#[derive(Debug, Deserialize, Clone)]
pub struct JudgeSpec {
    pub model: String,                    // "kimi-k2.5" / "claude-sonnet-4.5" / ...
    pub provider: String,                 // "kimi" / "anthropic" / "openai"
    pub temperature: f32,
    pub mode: JudgeMode,                  // Pairwise / Score / Jury
}

#[derive(Debug, Deserialize, Clone)]
pub enum JudgeMode {
    Pairwise { dual_pass: bool },         // 强制 dual_pass=true
    Score { scale: u8 },                  // 1-5 / 1-10
    Jury { models: Vec<JudgeSpec>, aggregator: JuryAggregator },
}
```

### 3.2 Run / Result（输出）

```rust
// storage/schema.rs
#[derive(Debug, Serialize)]
pub struct Run {
    pub id: String,                       // UUID
    pub suite_name: String,
    pub suite_version: String,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub plaw_commit: String,              // git SHA at run time
    pub model_version: String,
    pub config_hash: String,
    pub n_total: usize,
    pub n_completed: usize,
    pub n_failed: usize,
}

#[derive(Debug, Serialize)]
pub struct CaseResult {
    pub run_id: String,
    pub case_id: String,
    pub case_cluster: Option<String>,
    pub plaw_response: String,
    pub plaw_trace_id: Option<String>,    // 关联到 Phase 3 的 trace
    pub metric_scores: HashMap<String, MetricScore>,
    pub latency_ms: u64,
    pub tokens_in: u32,
    pub tokens_out: u32,
    pub cache_read_tokens: u32,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MetricScore {
    pub value: f64,                       // 标准化到 [0, 1] 或 [-1, 1]
    pub raw: JsonValue,                   // 原始 judge 输出
    pub judge_model: String,
}

#[derive(Debug, Serialize)]
pub struct AggregateReport {
    pub run_id: String,
    pub metrics: HashMap<String, MetricAggregate>,
    pub config: ReportConfig,
}

#[derive(Debug, Serialize)]
pub struct MetricAggregate {
    pub mean: f64,
    pub stderr: f64,
    pub stderr_clustered: Option<f64>,    // when cluster_id present
    pub ci_lower: f64,                    // 95% CI
    pub ci_upper: f64,
    pub n: usize,
    pub n_clusters: Option<usize>,
}
```

### 3.3 SQLite Schema

```sql
-- plaw-data/.plaw/eval/runs.db

CREATE TABLE runs (
    id TEXT PRIMARY KEY,
    suite_name TEXT NOT NULL,
    suite_version TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER,
    plaw_commit TEXT NOT NULL,
    model_version TEXT NOT NULL,
    config_hash TEXT NOT NULL,
    n_total INTEGER NOT NULL,
    n_completed INTEGER DEFAULT 0,
    n_failed INTEGER DEFAULT 0
);

CREATE TABLE case_results (
    run_id TEXT NOT NULL,
    case_id TEXT NOT NULL,
    case_cluster TEXT,
    plaw_response TEXT,
    plaw_trace_id TEXT,
    metric_scores TEXT NOT NULL,  -- JSON
    latency_ms INTEGER NOT NULL,
    tokens_in INTEGER NOT NULL,
    tokens_out INTEGER NOT NULL,
    cache_read_tokens INTEGER NOT NULL,
    error TEXT,
    PRIMARY KEY (run_id, case_id),
    FOREIGN KEY (run_id) REFERENCES runs(id)
);

CREATE TABLE judge_cache (
    cache_key TEXT PRIMARY KEY,    -- SHA256(prompt + input + model_version)
    judge_response TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE flywheel_queue (
    id TEXT PRIMARY KEY,
    trace_id TEXT NOT NULL,
    sampled_at INTEGER NOT NULL,
    judge_score REAL,
    review_status TEXT NOT NULL,   -- 'pending' | 'approved' | 'rejected'
    reviewed_at INTEGER,
    promoted_to_suite TEXT,
    promoted_case_id TEXT
);

CREATE INDEX idx_runs_suite ON runs(suite_name, started_at);
CREATE INDEX idx_results_run ON case_results(run_id);
CREATE INDEX idx_flywheel_status ON flywheel_queue(review_status);
```

---

## 四、关键算法决策

### 4.1 Confidence Intervals

**默认 t-distribution CI**（小样本友好）：
```
mean ± t_{n-1, 0.975} · SE
```

**Bootstrap CI**（非参数化，对偏态分布友好）：
- 1000 次 resample with replacement
- Percentile method（取 2.5% 和 97.5% 分位数）
- 用于 win-rate / Bradley-Terry coefficients

**选择规则**：
- 连续指标（G-Eval score 0-1）→ t-distribution
- 比例指标（pass rate） → Wilson score interval
- 排序聚合（B-T） → bootstrap

### 4.2 Cluster-robust SE

`cluster_id` 在 case 中可选。如果存在：

```
SE_clustered = sqrt(
    (G/(G-1)) · sum_g(sum_i_in_g(x_i - mean))^2 / (n-1)^2 · n
)
```
其中 G 是 cluster 数量，g 遍历 cluster，i 遍历 cluster 内 case。

**判断**：当 `n_clusters < n / 5` 时强制使用 cluster SE，否则用普通 SE。

### 4.3 Bradley-Terry MLE

迭代 MM（Minorization-Maximization）算法：
```
p_i^(t+1) = w_i / sum_{j≠i} (n_ij / (p_i^(t) + p_j^(t)))
```
其中 `w_i` 是模型 i 的获胜次数，`n_ij` 是 i 和 j 对决的次数。

收敛阈值：`max|p^(t+1) - p^(t)| < 1e-8` 或 1000 次迭代。

CI：bootstrap（重采样 pairwise comparisons，重新拟合 1000 次）。

### 4.4 G-Eval

按论文 [Liu et al. EMNLP 2023](https://arxiv.org/abs/2303.16634) 实现：

1. Judge 收到：原始任务描述 + 评分维度（如 "coherence"） + 评分量表 (1-5)
2. Judge 自动生成 evaluation steps（CoT）
3. Judge 输出每个分数的 token logprob（通过 OpenAI logprobs API 或 fallback 解析）
4. 最终 score = `sum(s · p(s))` for s in scale

**Kimi K2.5 不支持 logprobs**：fallback 让 judge 输出 JSON `{score: int, confidence: float}`，用 confidence 加权。

### 4.5 Pairwise Position-Swap

```rust
async fn pairwise_judge(case: &Case, response_a: &str, response_b: &str) -> Decision {
    let dec1 = ask_judge(case, "A", response_a, "B", response_b).await?;
    let dec2 = ask_judge(case, "A", response_b, "B", response_a).await?;
    
    // 必须两次结论一致才算
    match (dec1, dec2) {
        (A, B) => Tie,        // 翻转后选另一个 → 位置偏见，丢弃
        (A, A) => B_wins,     // 一致选了内容 b
        (B, B) => A_wins,     // 一致选了内容 a
        (Tie, _) | (_, Tie) => Tie,
    }
}
```

### 4.6 Multi-judge Jury

3-5 个 judges（必须 cross-family，不能全是同一模型族）：
- Majority vote（≥3 / 5 同意才算）
- 不一致时：升级到 confidence-weighted aggregation（LLM-as-a-Fuser 模式）
- 极端不一致（每个 judge 选不同）：标记 `inconclusive`，进 review 队列

---

## 五、Runner 设计

### 5.1 调用链

```
plaw eval run --suite chat_quality --n 30
  ↓
load suite from evals/chat_quality/cases.toml
  ↓
sample n cases (deterministic by seed if --seed given)
  ↓
spawn N parallel tasks (default N=4, configurable)
  ↓
for each case:
    1. plaw_client.send(case.input) → response
    2. for each metric: judge.score(case, response) → MetricScore
    3. write CaseResult to SQLite
  ↓
aggregate:
    - load all CaseResults from this run
    - per metric: compute mean, SE, CI
    - if cluster_id: compute clustered SE
  ↓
write AggregateReport to SQLite + emit JSON / Markdown
```

### 5.2 plaw 主程序的调用方式

**关键决策**：通过 plaw 现有的 WebSocket 协议调用，不改 plaw 核心。

```rust
// runner/plaw_client.rs
pub struct PlawClient {
    ws_url: String,        // ws://127.0.0.1:{port}/ws/chat
    bearer: String,
}

impl PlawClient {
    pub async fn send(&self, input: &CaseInput) -> Result<PlawResponse> {
        let ws = connect_async(&self.ws_url).await?;
        ws.send(json!({"type": "message", "content": input.to_text()})).await?;
        
        let mut full_response = String::new();
        let mut tool_calls = Vec::new();
        let mut usage = Usage::default();
        
        while let Some(msg) = ws.next().await {
            match parse_event(msg)? {
                Event::Chunk { content } => full_response.push_str(&content),
                Event::ToolCall { name, args } => tool_calls.push(...),
                Event::Done { full_response: fr, usage: u } => {
                    return Ok(PlawResponse { text: fr, tool_calls, usage: u });
                },
                Event::Error { message } => return Err(message.into()),
                _ => {}
            }
        }
        Err("WS closed without done event".into())
    }
}
```

### 5.3 缓存策略

**键**：`SHA256(judge_prompt + input + judge_model_version)`

- LLM 响应缓存（避免重复调用 judge）：永久（直到 invalidate）
- plaw 响应不缓存（每次都重跑，因为 plaw 本身可能改了）
- Cache 命中率监控：写入 metrics

**Invalidation**：
- judge model 版本变更 → 自动 miss
- 手动 `plaw eval cache clear --suite <name>`
- TTL 30 天硬过期

### 5.4 并发控制

- 默认 4 个并发（`PLAW_EVAL_CONCURRENCY` 可调）
- Rate limiter：semaphore + token bucket（避免触发模型 API rate limit）
- Per-judge 失败重试：exponential backoff，最多 3 次
- 任何 case 失败不阻塞其他 case 完成；失败列表汇总在最后

---

## 六、CI 集成

### 6.1 Workflow 文件

`.github/workflows/plaw-eval.yml`：

```yaml
name: plaw-eval

on:
  pull_request:
    branches: [main]
  schedule:
    - cron: '0 2 * * *'    # 每日 02:00 UTC
  workflow_dispatch:

jobs:
  smoke:
    if: github.event_name == 'pull_request'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v5
      - uses: dtolnay/rust-toolchain@stable
      - uses: actions/cache@v4
        with:
          path: |
            ~/.cargo
            target
            plaw-data/.plaw/eval/runs.db
          key: plaw-eval-${{ hashFiles('Cargo.lock', 'evals/**') }}
      - name: Build
        run: cargo build --release -p plaw-eval-cli
      - name: Run smoke eval
        run: cargo run --release -p plaw-eval-cli -- run --all --n 30 --output report.json
        env:
          KIMI_API_KEY: ${{ secrets.KIMI_API_KEY }}
          ANTHROPIC_API_KEY: ${{ secrets.ANTHROPIC_API_KEY }}
      - name: Compare to baseline
        run: cargo run --release -p plaw-eval-cli -- compare \
            --baseline-branch main --candidate report.json \
            --gate "metric:lower_ci_bound >= baseline_mean - 0.01"
      - name: Comment PR
        if: always()
        uses: marocchino/sticky-pull-request-comment@v2
        with:
          path: report.md

  nightly:
    if: github.event_name == 'schedule'
    runs-on: ubuntu-latest
    steps:
      # ... similar but with --n 300 and full suite
```

### 6.2 Gate 逻辑

```rust
// report/gate.rs
pub fn evaluate_gate(baseline: &MetricAggregate, candidate: &MetricAggregate, epsilon: f64) -> GateVerdict {
    if candidate.ci_lower < baseline.mean - epsilon {
        GateVerdict::Fail {
            reason: format!(
                "Lower CI bound {:.4} < baseline mean {:.4} - epsilon {:.4}",
                candidate.ci_lower, baseline.mean, epsilon
            ),
        }
    } else if candidate.mean > baseline.mean {
        GateVerdict::Pass { improvement: candidate.mean - baseline.mean }
    } else {
        GateVerdict::Pass { improvement: 0.0 }
    }
}
```

---

## 七、与现有 plaw 的集成点

### 7.1 启动 plaw（CI 中）

```bash
# CI 启动顺序：
1. cargo run --release -p plaw         # 启动 plaw 主程序，监听 ws://127.0.0.1:5800
2. wait for plaw_health endpoint
3. cargo run --release -p plaw-eval-cli -- run ...
4. teardown plaw
```

CLI 会读 `tauri.conf.json` 找端口；或环境变量 `PLAW_GATEWAY_URL`。

### 7.2 plaw config 注入

Eval 跑 plaw 时需要确定的 config：
- `default_provider = "anthropic-custom:..."`（指向测试 endpoint）
- 关闭某些 hook（避免污染 trace）
- 指定 workspace=`evals-tmp/`（不污染用户数据）

通过环境变量 `PLAW_CONFIG_OVERRIDE` 传入临时 TOML 路径。

### 7.3 Trace 关联

如果 plaw 已经发 OTel span（Phase 3 才做），eval 把 `trace_id` 记录到 `case_results.plaw_trace_id`，方便后续 debug。Phase 1 暂时留空字段。

---

## 八、文档结构（验收 A-4）

`docs/eval/methodology.md`（≥ 2000 字）应包含：

1. **统计基础**
   - 为什么必须报告 CI（举例：mean 0.62 vs 0.65 看起来好，但 CI 重叠就不显著）
   - Cluster-robust SE 的必要性（举例：同一对话的多轮是相关的）
   - Paired diff 的样本效率（举例：4-10× fewer samples）

2. **每个 metric 的定义和论文出处**
   - G-Eval：[Liu et al. EMNLP 2023]
   - Faithfulness：[Es et al. RAGAS]
   - Tool-call accuracy：自定义，引用 Anthropic Demystifying Evals
   - 等等

3. **Judge 选择和偏见**
   - 为什么必须 cross-family
   - Position bias 数据（Shi 2025: 60-75%）
   - 如何 pairwise + jury 减偏

4. **Suite 设计原则**
   - 如何选 case
   - 如何标 `cluster_id`
   - 如何避免 leakage（test in train）

5. **解读报告**
   - 怎么看 CI overlap
   - 怎么看 paired diff
   - 什么时候相信 metric，什么时候必须人工 review

---

## 九、风险点的工程对策

| 风险 | 对策（设计层面） |
|------|------------|
| Self-preference bias | `judges/jury.rs` 强制 cross-family 配置；`Suite::default_judge` 中 family 不能与被测试模型相同 |
| Statistical bug | `tests/stats_correctness.rs` 用 scipy 生成 reference，比较数值差距 < 1e-6；CI runs cross-check |
| Judge 太慢 | 缓存 + 并发；smoke eval 强制 n ≤ 30 |
| Judge 太贵 | per-run cost 估算 + 月度 budget；超阈值告警 |
| Eval 数据被污染 | `flywheel_queue.review_status` 强制人工 approve；自动测试是否有 input/output 重叠 |

---

## 十、不确定点（需要原型验证）

1. **Kimi K2.5 logprobs**：是否支持？如果不支持，G-Eval 的实现需要 fallback。预计需要 1 天原型测试。
2. **Bradley-Terry MLE 的数值稳定性**：当 pairwise 数据稀疏时（某些模型对决次数少），可能发散。可能需要 L2 正则化（Bayesian B-T）。
3. **生产 trace flywheel 的人工 review UI**：v1 可能仅 CLI（list / approve / reject），UI 留 Phase 3。
4. **判断 cluster_id 的阈值**：`n_clusters < n/5` 是经验规则，可能需要在数据上验证。

---

## 十一、本 design 的版本管理

- v1.0（2026-04-26）：初版
- 修改原则：实现过程中如发现设计缺陷，写 ADR + 更新本文档
- 任务执行时（`tasks.md`）若需偏离设计，必须先回这里讨论

---

## 十二、下一步

`tasks.md` 将基于这份 design 列出 ~30-50 个可勾选任务，每个 ≤ 1 天工作量，按依赖顺序排列。
