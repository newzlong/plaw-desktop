# Phase 1: Eval Foundation — Requirements

> 阶段目标：plaw 拥有行业级严谨度的评估体系。任何后续改动都能被科学量化判断"是更好还是更差"。
>
> 周期估计：6-8 周 · Owner：plaw 主开发者 · 状态：未开始
>
> 依赖：无（这是地基）

---

## 一、为什么必须先做 Eval

回到 vision：**没有测量就没有精英。** 凭感觉的"打磨"不算打磨。Phase 2 要重写 prompt / memory / RAG 三大子系统，每一项都需要量化判断"新版是否真的比旧版好"。没有 Phase 1 的 eval 体系，Phase 2 就是宗教辩论。

**反例**：很多团队的 LLM 应用迭代靠"我感觉 v3 比 v2 好"，三个月后回看其实是退步了——只是没量化。这不是精英 plaw 能接受的。

---

## 二、必须满足的需求（Functional Requirements）

### FR-1：统计学严谨度

任何 eval 报告必须包含：

- **F1.1** 均值 + 95% 置信区间（不仅是点值）
- **F1.2** Cluster-robust standard errors（当 case 之间相关时——例如同一段对话的多轮）
- **F1.3** Paired difference analysis（A/B 对比时报告 `mean(A−B)` + `SE(A−B)`，不是两个独立 mean）
- **F1.4** Power analysis 工具：给定效应量和显著性，输出所需样本量
- **F1.5** Bradley-Terry MLE 用于 win-rate 聚合（不用简单计数）
- **F1.6** Bootstrap CI（1000 次重采样标准）

**基线**：Anthropic [《A Statistical Approach to Language Model Evaluations》](https://arxiv.org/abs/2411.00640)（Miller, Nov 2024）。

### FR-2：质量指标库

至少实现以下指标，每个都有可调用的 Rust API：

#### 通用质量指标
- **F2.1** G-Eval（CoT judge + 自动 evaluation steps + log-prob 加权评分）
- **F2.2** Pairwise judge with mandatory position swap（dual-pass，否则丢弃）
- **F2.3** Multi-judge jury aggregator（majority vote / 置信度加权）

#### RAG 专用指标
- **F2.4** Faithfulness（声明 → 文档 verification rate）
- **F2.5** Answer Relevancy（embedding-based + judge-based 双路）
- **F2.6** Context Precision（召回内容中真正有用的比例）
- **F2.7** Context Recall（应召回内容的覆盖率）

#### Agent 专用指标
- **F2.8** Task Success Rate（终态正确性，由 grader 判断）
- **F2.9** Tool-call accuracy 拆分：selection F1 / arg validity rate / redundant-call rate
- **F2.10** Step Success Rate + Plan Quality（trajectory grader）
- **F2.11** Repeatability / pass^k（同任务跑 k 次都成功的比例，τ-bench 模式）
- **F2.12** Error Recovery Rate（注入故障 → agent 恢复完成的比例）

### FR-3：Eval 数据格式

- **F3.1** 数据集格式：TOML（与 plaw config 风格一致）
- **F3.2** Suite 目录结构：`evals/<suite_name>/{cases.toml, golden.toml, judge.toml}`
- **F3.3** Case schema 含：`id`, `input`, `expected_output`（可选）, `tags`, `cluster_id`（用于 cluster SE）
- **F3.4** 数据集版本化：git-tracked，每次修改写 `CHANGELOG.md`
- **F3.5** 支持 JSONL 格式互转（与 RAGAS / DeepEval 数据集互通）

### FR-4：CLI

- **F4.1** `plaw eval run --suite <name> [--n <samples>] [--judge <model>]`
- **F4.2** `plaw eval list` 列出所有 suite
- **F4.3** `plaw eval compare --baseline <run_id> --candidate <run_id>` 输出 paired diff + CI
- **F4.4** `plaw eval power --effect <pp> --sigma <stdev>` 计算所需样本量
- **F4.5** `plaw eval promote --trace <id>` 把生产 trace 升级为 eval case
- **F4.6** 输出：JSON（机器读）+ Markdown 总结（人读）+ 可选 SARIF（CI 标准）

### FR-5：CI 集成

- **F5.1** GitHub Actions workflow：per-PR smoke test（n≈30，1-3 分钟）
- **F5.2** Nightly full eval（n≈300）→ artifact + dashboard
- **F5.3** Weekly extended eval（n≥1000）→ regression detection
- **F5.4** **Gate 逻辑**：fail PR if `lower_CI_bound(new_metric) < mean(baseline_metric) - 1pp`
- **F5.5** PR 自动评论：上一版 vs 当前的指标 diff + CI（Markdown 表格）
- **F5.6** 缓存：按 `(prompt_hash, input_hash, model_version)` 缓存 LLM 响应，避免重跑

### FR-6：Production Trace Flywheel

- **F6.1** 生产 trace 自动采样（默认 1-10%，可配置）
- **F6.2** 异步 LLM-judge 后台运行（不阻塞主流程）
- **F6.3** 失败 / 低置信 trace 进 review queue
- **F6.4** 用户审核 UI：approve / reject / edit
- **F6.5** Approved 的 trace 升级为新 eval case，自动 versioning

### FR-7：初始 Eval Suites

至少 5 个 suite，每个 30-100 例：

- **F7.1** `chat_quality` — 通用对话质量（多场景）
- **F7.2** `tool_routing` — 工具选择准确性
- **F7.3** `rag_grounded_qa` — RAG 检索 + 回答质量
- **F7.4** `agent_multi_step` — 多步 agent 任务完成率
- **F7.5** `error_recovery` — 故障注入恢复
- **F7.6**（可选）`adversarial` — prompt injection 抗性

---

## 三、非功能需求（Non-Functional Requirements）

### NFR-1：性能
- **NF1.1** Smoke eval（n=30）必须在 3 分钟内完成
- **NF1.2** Full eval（n=300）必须在 30 分钟内完成（可并发）
- **NF1.3** 单个 case 的 grader latency p95 ≤ 30s

### NFR-2：成本
- **NF2.1** Smoke eval 单次成本 ≤ $0.50（按 Kimi K2.5 定价）
- **NF2.2** Full eval 单次成本 ≤ $5
- **NF2.3** 缓存命中率 ≥ 60%（重复跑同一 PR 应该几乎免费）

### NFR-3：可移植性
- **NF3.1** 纯 Rust 实现，不依赖 Python runtime
- **NF3.2** 所有数据本地存储（与 vision 一致）
- **NF3.3** Eval 跑在用户机器上，不强制云服务

### NFR-4：可观测性
- **NF4.1** Eval 运行本身也要发 OTel span（meta-observability）
- **NF4.2** 详细日志：每个 case 的输入、输出、grader 中间状态
- **NF4.3** 失败 case 自动保留完整 trace

### NFR-5：可扩展性
- **NF5.1** 新增 metric ≤ 200 LOC
- **NF5.2** 新增 suite 不需要改 plaw 核心代码
- **NF5.3** Judge model 可替换（Kimi → Claude → GPT）

---

## 四、约束（Constraints）

### C-1：技术栈
- 必须 Rust 实现（核心库）
- 允许的 Rust 依赖：`statrs`, `tokio`, `serde`, `serde_json`, `serde_toml`, `reqwest`, `tracing`
- 不允许：Python 运行时依赖、外部数据库 service（仅 SQLite）

### C-2：模型成本
- 默认 judge 用 Kimi K2.5（成本可控）
- 高风险 eval（pairwise jury）允许配置 Anthropic Sonnet 或 OpenAI GPT-4o-mini 作为 cross-judge（cross-family 必要）
- 成本预算：单次 PR 触发的 smoke eval 成本必须 ≤ $0.50

### C-3：和现有 plaw 的兼容
- 不修改 plaw 核心 agent loop
- 通过现有 WS 协议（`{"type": "message", ...}` / 流式响应）调用 plaw
- Eval 是外挂在 plaw 之上的工具，不是 plaw 的核心组件

### C-4：项目阶段
- 必须先于任何 Phase 2 工作完成
- 不引入会阻塞 Phase 2 启动的依赖

---

## 五、验收标准（Acceptance Criteria）

Phase 1 视为完成，需要全部满足：

- **A-1** 5 条 vision 量化指标都能在本地跑出来并报告 95% CI
- **A-2** GitHub Actions 跑通 per-PR smoke + nightly full
- **A-3** 至少 5 个 eval suite 实装，每个 ≥ 30 例
- **A-4** 文档：`docs/eval/methodology.md` 解释每个 metric 的统计基础（≥ 2000 字，引用论文）
- **A-5** 内部用 plaw 自己跑一次 nightly eval，记录基线数字到 `docs/eval/baseline-2026-Q2.md`
- **A-6** Production trace flywheel 跑通端到端（采样 → judge → review → 升级）至少一次
- **A-7** Statistical 库单元测试覆盖 ≥ 90%（关键：CI 计算、Bradley-Terry MLE 数值正确性）
- **A-8** 任何 PR 触发的 eval gate 能正确 fail / pass（手动验证至少 3 个故意改坏的 PR）

---

## 六、风险与对策

| 风险 | 概率 | 影响 | 对策 |
|------|------|------|------|
| Kimi K2.5 作为 judge 自我偏好严重 | 高 | 中 | 强制 cross-family judge（至少加 Anthropic 或 OpenAI 一家） |
| Eval 跑得太慢，开发者不愿意跑 | 中 | 高 | 严格执行 NF1.1 的 3 分钟 smoke 上限；缓存 |
| Eval suite 设计不好，golden 答案有偏 | 高 | 中 | 用 pairwise + 多 judge jury，减少对单一 golden 的依赖 |
| 统计代码 bug 导致结论错误 | 中 | 严重 | 90% 单元测试覆盖；与 Python `scipy.stats` cross-check 数值 |
| 生产 trace 飞轮污染 eval 数据集 | 中 | 中 | 强制人工 review，approve 才能升级为 eval case |
| 成本失控（judge 费用） | 中 | 中 | 缓存 + 采样率配置 + 月度预算硬上限 |

---

## 七、明确不在 Phase 1 范围（Out of Scope）

为了限定边界、避免 phase 拖延：

- ❌ Trace viewer UI（Phase 3）
- ❌ Hallucination detector / HHEM 集成（Phase 3）
- ❌ PII 过滤层（Phase 3）
- ❌ Real-time output guard（Phase 3）
- ❌ Replay 功能（Phase 3）
- ❌ 任何对 plaw agent loop 的改动（Phase 2）
- ❌ Prompt registry / GEPA 编译（Phase 2A）
- ❌ Memory v2 / RAG v2 系统（Phase 2B / 2C）
- ❌ Embedding model 迁移（Phase 2C）
- ❌ 中文 eval suite 单独建库（先一并放在 chat_quality 里，未来再分）

---

## 八、本 requirements 的版本管理

- v1.0（2026-04-26）：初版
- 修改需写 ADR（`../decisions/000X-*.md`）说明原因
- 进入 design.md 阶段后，需求若变更必须回到这里更新

---

## 九、下一步

`design.md` 将基于这份 requirements 设计具体架构：
- 模块划分（`plaw-eval` crate 结构）
- 接口契约（CLI / library API）
- 关键数据结构（Suite / Case / Run / Report）
- 与 plaw 主程序的集成点
- 文件存储 schema（SQLite tables）
