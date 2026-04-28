---
phase: 1
title: Eval Foundation — Phase 1 Retrospective
date: 2026-04-28
status: complete
---

# Phase 1: Eval Foundation — Retrospective

> 阶段定位：Plaw Elite 计划的第 1 阶段，目标是给 plaw 装上 Anthropic 级别的评估底座。
> Phase 2 起的所有 prompt / memory / RAG 改动都要靠这一层做"是更好还是更差"的科学判断。
>
> 本文件不是验收报告（验收见 [requirements.md §五](./requirements.md)），而是事后复盘：
> 做对了什么、做错了什么、Phase 2 怎么调。

---

## 一、最终交付盘点

### 1.1 代码

| 模块 | 路径 | 测试数 | 备注 |
|------|------|------|------|
| Stats 库 | [crates/plaw-eval/src/stats/](../../../crates/plaw-eval/src/stats/) | 25 | t-CI / Wilson / bootstrap / cluster SE / paired diff / power / Bradley-Terry MLE |
| Suite & Storage | [crates/plaw-eval/src/suite/](../../../crates/plaw-eval/src/suite/) + [storage/](../../../crates/plaw-eval/src/storage/) | 19 | TOML 加载 + SQLite 持久化 + 运行时 migration |
| Plaw Client + Runner | [crates/plaw-eval/src/runner/](../../../crates/plaw-eval/src/runner/) | 11 | WS 客户端 + 缓存 + 限流执行器 + cancellation |
| Judges | [crates/plaw-eval/src/judges/](../../../crates/plaw-eval/src/judges/) | 21 | Kimi / Anthropic / OpenAI 客户端 + 强制 dual-pass + cross-family jury |
| Metrics | [crates/plaw-eval/src/metrics/](../../../crates/plaw-eval/src/metrics/) | 23 | G-Eval / 关键词覆盖 / 工具调用 F1 / 参数有效性 / 重复调用率 |
| Aggregation & Reports | [crates/plaw-eval/src/report/](../../../crates/plaw-eval/src/report/) | 9 | Paired diff + Gate verdict + JSON / Markdown / PR-comment 渲染 |
| Flywheel | [crates/plaw-eval/src/flywheel/](../../../crates/plaw-eval/src/flywheel/) | 8 + 3 集成 | sampler / reviewer / promoter，端到端跑通 |
| CLI | [crates/plaw-eval-cli/src/main.rs](../../../crates/plaw-eval-cli/src/main.rs) | — | 8 个子命令 `run / list / compare / power / promote / cache / flywheel / doctor` |
| **合计** | | **145** | 全部通过（lib 133 + 集成 12） |

### 1.2 文档

| 文件 | 字数 | 内容 |
|------|------|------|
| [docs/eval/methodology.md](../../../docs/eval/methodology.md) | ~3500 | 13 节统计与方法学说明，每条都引用论文 |
| [docs/eval/suite-design.md](../../../docs/eval/suite-design.md) | ~1900 | 如何写 case + cluster_id 用法 |
| [docs/eval/judge-selection.md](../../../docs/eval/judge-selection.md) | ~1700 | judge 选型、bias 防御、成本表 |
| [docs/eval/troubleshooting.md](../../../docs/eval/troubleshooting.md) | ~1900 | 6 类常见问题 |
| [docs/eval/ci-secrets.md](../../../docs/eval/ci-secrets.md) | ~700 | GitHub Actions secrets 配置 |
| [crates/plaw-eval/README.md](../../../crates/plaw-eval/README.md) | ~600 | 快速上手 |

### 1.3 CI

[.github/workflows/plaw-eval.yml](../../../.github/workflows/plaw-eval.yml) 三个 job：

- `lint-and-test` — 永远跑（fmt、clippy `-D warnings`、unit + integration 测试）
- `smoke-eval` — PR 上跑（仅在 secrets 存在时进入下游步骤），sticky PR comment
- `nightly-eval` — cron `03:17 UTC` 跑

---

## 二、做对了什么

### 2.1 先拿统计学严谨度立 baseline

**决定**：把 [Miller 2024 (arXiv:2411.00640)](https://arxiv.org/abs/2411.00640) 列出的所有方法（t-CI、Wilson、cluster SE、paired diff、power、Bradley-Terry）都写成纯 Rust 单元，再去做"判官"。

**收益**：

- 后续每个 metric / judge / runner 都直接调用这层，不再有"自己计算均值忘了 SE"的隐患
- Cluster SE 的自动启用（`should_use_cluster_se` 阈值 `n_clusters * 5 < n`）让聚合层没有手动开关
- Paired diff 在 [report/gate.rs](../../../crates/plaw-eval/src/report/gate.rs) 中默认启用，A/B 比较直接享受 4-10× 样本效率

**反例（如果反过来做）**：先写 metric 再补统计。我们会得到一堆 mean-only 报告，回头改全部接口的代价巨大。

### 2.2 Cross-family jury 是硬约束，不是配置项

**决定**：[judges/jury.rs](../../../crates/plaw-eval/src/judges/jury.rs) 的 `Jury::new` 直接拒绝构造同 family 的 jury，必须满足 `min_distinct_families ≥ 2`。

**为什么硬约束**：[Liu 2024](https://arxiv.org/abs/2410.02736) 证明 LLM-as-judge 的 self-preference bias 普遍存在；[Panickssery 2024](https://arxiv.org/abs/2404.13076) 在 ELO arena 上观测到同 family 平均给自己加 2-5%。这不是一个"高级用户的可选优化"，而是"做对的最低要求"。

**收益**：用户写 `judge.toml` 时如果手抖把三个 judge 都填成 OpenAI，CLI 会直接拒绝运行，而不是默默给出有偏的结论。

### 2.3 Flywheel 的"鉴权链路"设计

**决定**：[flywheel/promoter.rs](../../../crates/plaw-eval/src/flywheel/promoter.rs) 拒绝任何 `review_status != "approved"` 的入队条目；promoted case 必须带 `source: "flywheel"` + `promoted_at` 字段（[suite/case.rs](../../../crates/plaw-eval/src/suite/case.rs)）。

**为什么**：飞轮最危险的失败模式是"低质量响应被采样回来变成新 case，然后 plaw 被 train 去拟合自己的烂输出"。我们用三道闸门挡：

1. 采样器只挑低分 / 失败 case，而不是高分（避免分数虚高）
2. 必须人审 approve（CLI 的 `flywheel review`）
3. promoted case 永远带血统标签，未来可以一键审计 / 移除

### 2.4 SQLite migration 用 idempotent ALTER

**决定**：[storage/repo.rs::apply_runtime_migrations](../../../crates/plaw-eval/src/storage/repo.rs) 用 `ALTER TABLE ADD COLUMN` + 容忍 "duplicate column name"，而不是 schema diff 工具或 sqlx 的 migrate macro。

**收益**：legacy DB 升级测试 ([m10_flywheel_integration.rs::migration_upgrades_old_dbs](../../../crates/plaw-eval/tests/m10_flywheel_integration.rs#L173)) 直接通过；不需要额外迁移文件管理；rollback 不需要——column 加了就加了，旧代码不读它就不影响。

### 2.5 不写抽象到位的 trait，先写够用的 enum

**决定**：[sampler.rs](../../../crates/plaw-eval/src/flywheel/sampler.rs) 的 `SampleStrategy` 是 enum，4 个 variant 直接 match。没有 `trait Sampler { fn sample(&self, ...) }` 这种"未来可扩展"的抽象。

**收益**：第 5 种采样策略要加的时候——加一个 enum variant + 一个 match arm，5 分钟。比起 trait 的话还要写 dyn 装箱、文档、测试 fixture，至少省一倍工。

**反例**：M4 的 `JudgeClient` 是 trait，因为 Kimi / Anthropic / OpenAI 三家 API 不同——这种情况 trait 是合理的。区分 "真有 polymorphism 需要" vs "只是想象未来需要"。

---

## 三、做错了什么 / 没做完

### 3.1 M9 的 5 个 suite 没写

**事实**：[evals/_template/cases.toml](../../../evals/_template/cases.toml) 仅作为格式示例。`chat_quality / tool_routing / rag_grounded_qa / agent_multi_step / error_recovery` 五个 suite 各 30+ case 都没写。

**为什么**：写 case 不是工程工作，是产品工作——要做 case 选型、cluster 划分、对抗样本设计，需要的是对 plaw 主程序日常输入的判断力，而不是 Rust 代码能力。Session 内塞进去会得到一堆"看起来像样但没设计意图"的 case，最后还要全删重来。

**Phase 2 调整**：把 M9 单独拆出来，找一两个真实使用 plaw 的 session，从 trace 里采样 30 个有代表性的 case 起步，再用 [docs/eval/suite-design.md](../../../docs/eval/suite-design.md) 的方法论扩到 30+。

### 3.2 M11.T11.7 baseline 数字没跑

**事实**：[docs/eval/baseline-2026-Q2.md] 不存在；nightly eval 没跑过一次真实的 5 × n=300。

**为什么**：跑 baseline 需要 ① M9 的 suite 已就位 ② 真的 plaw 进程可连 ③ Kimi / Anthropic API key 充值。这是个"前面三件事都做完才能做的事"，session 内做不了。

**风险**：没有 baseline 数字意味着 Phase 2 的第一个 PR 没有 ground truth 对照。Mitigation：Phase 2 启动前先单独花 1 天补这件事，作为 Phase 1 的延续而不是 Phase 2 的开头。

### 3.3 单元测试覆盖率没数字

**事实**：[T12.2](./tasks.md#L268) 要求 ≥ 90% 覆盖率，但 Phase 1 没有跑出最终覆盖率报告。

**为什么**：Windows + cargo-tarpaulin 不兼容（tarpaulin 用 ptrace，仅 Linux）；cargo-llvm-cov 跨平台但需要安装。本次 session 启动了安装但优先把 retrospective + tag 写完。

**Phase 2 调整**：把 `cargo llvm-cov` 加到 `.github/workflows/plaw-eval.yml` 的 `lint-and-test` job 里，每次 PR 都报告差值。Windows 本地不再纠结。

### 3.4 部分次要 metric 推迟

**推迟的**：

- [tasks.md M5.T5.3-T5.6](./tasks.md#L126) RAG 专用指标（faithfulness / relevancy / context precision / recall）
- [M5.T5.10-T5.13](./tasks.md#L136) trajectory + repeatability + error recovery
- [M6.T6.7](./tasks.md#L160) SARIF 输出
- [M7.T7.9](./tasks.md#L178) shell completion

**为什么这么排**：M5 已实现的 G-Eval + 关键词 + 工具调用 F1/validity/redundancy 已经覆盖 [00-vision.md](../00-vision.md) 的 5 条量化目标。RAG / trajectory 是"plaw 还没有 RAG 主链路"和"agent 多步任务的 ground truth 难标"的现实问题，先实装会写出 tests-pass-but-meaningless 的代码。

**Phase 2 调整**：RAG 指标在 [00-vision.md] 定的 RAG 子系统重写时一并补；trajectory 在 agent loop 改造时补；SARIF / shell completion 是 nice-to-have，永远可以补。

### 3.5 M4.T4.12 与人类标注 fixture 没建

**事实**：[tasks.md M4.T4.12](./tasks.md#L106) 要求"用 100 个人工标注 case 验证 jury 与人类一致率 ≥ 0.80"，没做。

**为什么**：标 100 个 case 需要 1-2 人天的纯人力，并且需要先有 plaw 跑出来的 100 个真实响应。Session 内不可能。

**风险**：我们对 jury 质量的信心来自 [Verga 2024](https://arxiv.org/abs/2404.18796) 的论文结论而不是 plaw-specific 验证。
**Mitigation**：Phase 2 第一周，从 baseline 跑结果里抽 100 个，三个人独立标，再算 Spearman。算是 Phase 1.5 的事情。

---

## 四、技术债清单

按 [tasks.md](./tasks.md) 标记 `pending` / 未完成的任务汇总：

| ID | 任务 | 优先级 | 适合时机 |
|---|------|------|---------|
| T1.10 | scipy cross-check fixture | 中 | Phase 2 开始前 |
| T1.11 | tarpaulin 90% 覆盖率验证 | 中 | 转用 cargo-llvm-cov，加进 CI |
| T4.11 | 同 family judge 警告 | 低 | 已硬约束 family 区分，warning 实际上是 nice-to-have |
| T4.12 | 与人类标注 100 case 一致率 | 高 | 见 §3.5 |
| T5.3-T5.6 | RAG 专用指标 4 项 | 高 | 跟 RAG 子系统重写绑定 |
| T5.10-T5.13 | trajectory / repeatability / error recovery | 中 | 跟 agent loop 改造绑定 |
| T6.7 | SARIF 输出 | 低 | 永远可以补 |
| T7.9 | shell completion | 低 | 永远可以补 |
| T8.6/T8.7 | 真实 PR 测试 gate | 中 | Phase 2 第一个 PR 即顺手验证 |
| T9.1-T9.6 | 5 个 suite 30+ case | 高 | 见 §3.1 |
| T10.7 | promote 自动 git commit | 低 | 当前需要人审完才 promote，自动 commit 反而风险 |
| T11.7 | baseline 数字 | 高 | 见 §3.2 |

---

## 五、Phase 2 启动前的清单

进入 Phase 2 之前，必须完成：

1. **写 5 × 30+ case suite**（M9，~5 天）—— 没这个，Phase 2 没东西可比
2. **跑出 baseline 数字**（T11.7，~1 天）—— 没这个，Phase 2 第一 PR 是宗教辩论
3. **建 100 case 人审 fixture**（T4.12，~2 天）—— 没这个，jury 的可信度只能靠论文背书
4. **CI 接入 cargo-llvm-cov**（T1.11，~半天）—— 没这个，覆盖率回归不可见

合计 ~8.5 人天。建议作为 Phase 1.5 单独立项，而不是混进 Phase 2。

进入 Phase 2 之前，应当 **不必** 完成（可以延后）：

- RAG / trajectory 指标 → 跟着 RAG / agent 改造一起做
- SARIF / shell completion → 永远是 nice-to-have
- 自动 commit / 警告类的低优先项

---

## 六、对 vision 的修订建议

[00-vision.md](../00-vision.md) 的 5 条原则在 Phase 1 实战中没有被推翻，但有 2 条需要细化：

### 6.1 「Measurable Excellence」要细化"可度量"的范围

原文："任何改动必须能被量化判断好坏"。

实战发现：plaw 的某些行为（如长对话中的人格连贯性、tool 使用的"得体感"）很难写成单一 metric。强行写会得到 Goodhart 风险。

**修订建议**：在 [00-vision.md] 中加一段说明——对于某些维度，pairwise jury 的人类一致率本身就是度量，不要求每件事都有 0-1 的 score。

### 6.2 「Self-Evolving Memory」需要等 Phase 2 才有具象

原文：plaw 的记忆应当持续学习并改进。

实战发现：Phase 1 完全没碰记忆系统。Self-evolving 在 Phase 1 退化为 "flywheel 把生产 case 升级为 eval case"——这是 eval 系统的进化，不是记忆系统的进化。

**修订建议**：Phase 2 启动时回头确认这条原则的适用范围（是 plaw 的记忆，还是 eval 的 case 库，还是两者）。

---

## 七、关键数字

- **代码**：~6500 LOC（不含 tests / docs / fixtures）
- **测试**：145 全过（133 lib unit + 12 integration）
- **文档**：~10000 字（5 篇主文档 + crate README + 根 README 章节）
- **commit**：~12 个里程碑提交（M0 → M11 串行 + M10 在 M11 之后）
- **持续时间**：~1 个高强度 session（vs 计划的 8 周）—— 严重压缩，意味着 case 设计 / baseline 等"非工程"工作必须独立排期

---

## 八、给 Phase 2 实施者的话

你接手时，你拥有：

- 一个**测得准**的工具箱：Phase 2 的 prompt 改动、memory 重写、RAG 替换都能直接调 [crates/plaw-eval](../../../crates/plaw-eval/) 测出 paired diff。
- 一个**会自动喊停**的 CI：[.github/workflows/plaw-eval.yml] 的 gate 会在你回归时拒 PR。
- 一个**可信的判官**：jury 强制 cross-family，不会出现"自己评自己分高"的情况。

你**没有**的（要先补）：

- 5 个真正的 suite——见 §五
- baseline 数字——见 §五
- RAG / trajectory 指标——你做哪个子系统就补哪个

不要做的事：

- 不要绕过 paired diff 直接比 mean——会被 noise 淹没
- 不要把 jury 退化成单 judge"图省事"——self-preference bias 会让你对自己的改动评分虚高
- 不要把 promoted case 当作 ground truth——它是采样回来的低分案例，加入 suite 是为了让 plaw 在这类输入上变好，不是 hold-out test set

---

## 九、感谢清单

- [Miller 2024](https://arxiv.org/abs/2411.00640) — 提供了整个 stats 库的范本
- [Liu 2023](https://arxiv.org/abs/2303.16634) — G-Eval 的原始论文
- [Verga 2024](https://arxiv.org/abs/2404.18796) — 多 judge jury 的依据
- [Hunter 2004](https://www.jstor.org/stable/3448522) — Bradley-Terry MM 迭代算法

—— Phase 1 完
