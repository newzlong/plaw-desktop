---
title: Phase 3 — Agent Loop Architecture
status: spec — not yet started
date: 2026-04-30
predecessors: phase-1-eval/, phase-2-targets.md
---

# Phase 3 — Agent Loop Architecture

## 为什么有这一阶段

Phase 2 用 prompt 迭代修了 4 个目标，但发现 3 个目标（T-2 / T-3 / T-7 /
T-10）撞墙 —— 不是 prompt 措辞不到位，是 plaw 的 agent loop 缺少特定
架构层。

具体诊断在 `memory/project_phase3_architecture_gaps.md`。简言之：

| 撞墙 case | 缺的层 |
|---|---|
| T-2 confabulation | 答案前 grounding/citation 验证 + tool 结果 freshness |
| T-3/T-10 wrong-premise vs ambiguous 互斥 | 入口 intent classification / case router |
| T-7 overzealous refuse | 同上：intent router |
| 长 tool 链规则衰减 | rule attention scaffolding（T-2 reminder 是雏形）|

Phase 3 = 把这 4 个层补齐。**全是 Rust 工程**（agent loop 改造），不需要训练
或 fine-tune。

## 4 个层 + 实施顺序

| 顺序 | 层 | 修哪些 case | LOC | 理由 |
|---:|---|---|---:|---|
| **L1** | 入口 intent router | T-3 / T-6 / T-7 / T-10 | ~500 | 最 leverage、最自包含、能验证 Phase 3 整体假设 |
| **L2** | 答案前 grounding/citation 验证 | T-2 主问题 | 500-800 | T-2 单 case 收益最大，前提是 L1 跑通建信心 |
| **L3** | tool 结果 freshness metadata | T-2 子问题 | ~200 | 配合 L2 用，单独价值有限 |
| **L4** | rule attention scaffolding 完整化 | 长 tool 链通用 | ~300 | T-2 reminder 已是雏形；后期再扩展整套规则 |

每层一个独立 PR，独立 plaw-eval 验证，独立 rollback 路径。

## Phase 3 起点

**新 baseline lock**（必须先做）：当前 docs/eval/baseline-2026-Q2.md 的 n=300
数字是 Phase 2 改动**之前**的（run id `28b71f3e`）。Phase 2 的 v2 + T-9
+ T-2 reminder + E-1 都改变了 plaw 行为。Phase 3 任何度量都需要新 baseline
做对照。

跑 `--repetitions 10 --suite chat_quality + tool_routing`，写新文档
`docs/eval/baseline-2026-Q2-post-phase2.md`，作为 Phase 3 的零点。

## 验收标准（整体）

Phase 3 出阶段的标志：

- [ ] L1 / L2 / L3 / L4 各 1 个 PR 已 merge，每个有 plaw-eval 验证
- [ ] T-2 / T-3 / T-10 在 plaw-eval 稳定 ≥4.0（n=20 reps）
- [ ] 整体 chat_quality g_eval CI 不重叠 Phase 2 baseline（即真正提升）
- [ ] 每层有 design 文档 + 回滚说明

## 依赖与风险

**依赖**：
- Phase 1.5 eval foundation 可用（已就位）
- plaw agent loop 现有 trait 架构允许加 hook（已有 `PromptSection` /
  `Tool` trait 可类比；新层会加 `IntentRouter` / `GroundingChecker` 等）

**风险**：
- L1 引入额外 LLM 调用（intent classification）→ 整体 latency +~1s。
  缓解：classification 结果缓存、对短/明确消息跳过分类。
- L2 grounding verifier 设计困难 → 可能本身需要 LLM judge 二次验证。
  缓解：先做规则化 verifier（verbatim string match），不行再加 LLM。

## 子文档

- [layer-1-intent-router.md](layer-1-intent-router.md) — 入口分流（first PR）
- (其余 layer 在 L1 跑通后再写 spec)

## 出 Phase 3 时的状态

plaw 不再是"single LLM + tool registry + simple loop"，而是**有 4 层
hooks 的 agent runtime**。adversarial / calibration-heavy case 在
plaw-eval 上稳定到与 baseline 90% 持平的水平。Phase 4 的 observability
和高级特性可以在这个稳定底座上叠加。
