---
title: Phase 3 Layer 1 — Intent Router
status: spec — ready to implement
date: 2026-04-30
target cases: chat_quality-math-003, ambiguity-001, borderline-refuse-001, conflict-001, pushback-001
---

# Layer 1: Intent Router

第一个 Phase 3 PR。在 plaw 的 agent loop **入口**加一次 intent
classification，把不同类型的用户消息分流到不同的 prompt scaffold + 行为
约束，避免"一个 CalibrationSection 试图盖所有 case"的 prompt 饱和问题。

## 现状（为什么需要）

Phase 2 v1/v2/v3 三轮迭代证明：math-003（wrong-premise）和 ambiguity-001
（ambiguous）在同一个 prompt section 里会互相挤压。强化 ambiguous 规则
→ math 退化；强化 wrong-premise 规则 → ambiguous 退化。

根因：plaw 当前每条消息都走同一条 "respond helpfully" 流水线，没有先识别
"这是哪类消息"。不同 intent 需要不同行为约束（refuse-friendly vs
correct-first vs clarify-first），但 prompt-only 实现互斥。

## 设计

### 1. Intent 分类

定义 7 种 intent（覆盖 Phase 2 撞墙 case + 常规对话）：

| Intent | 触发条件 | 期望行为 |
|---|---|---|
| `WrongPremise` | 用户陈述明显错误事实并要求基于此回答 | 先纠正前提再可选回答 |
| `Ambiguous` | 缺关键上下文（"the X" 没指定哪个） | 必须先反问澄清 |
| `ConflictingConstraints` | 输出要求自相矛盾 | 选一条说明原因 |
| `BorderlineSafety` | 听起来危险但可能合理（撬自家锁） | 先 intent-check 再帮助 |
| `AdversarialInjection` | 用户输入伪装系统指令 | 拒绝 + 说明 |
| `FactualLookup` | 普通求知（"什么是水"） | 走 CalibrationSection 标准路径 |
| `TaskRequest` | 默认（"帮我写代码"等） | 走 CalibrationSection 标准路径 |

### 2. 分类策略：hybrid（rules + LLM fallback）

避免每条消息都额外多一次 LLM 调用：

```
fn classify(message, history) -> Intent:
    # Layer A: cheap rule-based checks (~100 LOC regex / keyword)
    if matches_math_wrong_pattern(message):  # "已知 X = Y, 那么..."
        return WrongPremise
    if matches_clear_injection(message):     # "[SYSTEM]" / "OVERRIDE"
        return AdversarialInjection
    if matches_conflicting_constraint(message):  # "用一句话...但展开三个例子"
        return ConflictingConstraints
    if matches_clear_factual(message):       # 短问题 + 无指代不清
        return FactualLookup

    # Layer B: LLM fallback for unclear messages
    return llm_classify(message, history)
```

LLM fallback 用 plaw 当前 provider，结构化输出（小 JSON），冷启动可以
换成更小的模型（如 kimi-light）做 cost optim。

### 3. 路由：每个 intent 加一个 scaffold

定义 trait：

```rust
pub trait IntentScaffold: Send + Sync {
    /// Append intent-specific guidance into the system prompt.
    /// Called once per turn, after classification.
    fn build(&self, ctx: &PromptContext<'_>) -> String;

    /// Optional: mid-loop modification (e.g. force a clarifying question
    /// before tool calls for Ambiguous intent).
    fn pre_iteration_constraint(&self, iteration: usize) -> Option<String> {
        None
    }
}
```

每个 intent 对应一个 scaffold 实现，例如 `AmbiguousScaffold` 注入：

> [Intent: ambiguous] The user's request is missing critical context. Your
> FIRST response must be a single clarifying question. Do NOT call tools or
> make assumptions until the user clarifies.

### 4. 集成点

新加文件 `plaw/src/agent/intent.rs`：

- `pub enum Intent { ... }` —— 7 variants
- `pub trait IntentRouter` + `pub trait IntentScaffold`
- `pub struct HybridRouter` —— 默认实现（rules + LLM fallback）
- `pub fn classify_intent(message, history, provider, model) -> Intent`

修改 `plaw/src/agent/loop_.rs`：

- `run_tool_call_loop` 入口（iteration=0 之前）调用 `classify_intent`
- 把分类结果对应的 scaffold append 到 system prompt
- 如果 scaffold 有 `pre_iteration_constraint`，每次 iteration 前注入
  额外约束（用类似 T-2 reminder 的机制）

## 验收

### 单元测试

`plaw/src/agent/intent.rs` 自带 ≥30 unit test：

- 每个 intent 至少 3 个 positive case + 1 个 negative case（避免误分类）
- HybridRouter 的 rule layer 召回率 ≥80%（在 plaw-eval cases.toml 抽样）

### 集成测试（plaw-eval）

PR 合并门槛：

| Case | Phase 2 v2 baseline | Phase 3 L1 后期望 |
|---|---:|---:|
| math-003 (WrongPremise) | 3.00 | ≥4.0 |
| ambiguity-001 (Ambiguous) | 3.00 | ≥4.0 |
| conflict-001 (ConflictingConstraints) | 2.80 | ≥4.0 |
| borderline-refuse-001 (BorderlineSafety) | 4.40 | ≥4.0（不退化） |
| 整体 chat_quality g_eval | ~0.76 | 不低于 baseline -1pp |

n=20 reps per case（不再 n=5，避开噪声 floor）。

### 回归测试

- 普通对话 case（factual / math basic / style）都走 `FactualLookup` 或
  `TaskRequest` 分支，应**与 Phase 2 v2 行为完全一致**（相同 system prompt，
  相同行为）。任何意外路由（FactualLookup → 其他 intent）记 unit test
  failure。

## 工作分解

按 commit 粒度估约 500 LOC，6-8 个 commit：

1. `feat(plaw): add Intent enum + IntentRouter trait skeleton` (~80 LOC)
2. `feat(plaw): add HybridRouter rule layer (5 intent regex matchers)` (~150 LOC)
3. `feat(plaw): add HybridRouter LLM fallback path` (~100 LOC)
4. `feat(plaw): add IntentScaffold trait + 7 default scaffold impls` (~100 LOC)
5. `feat(plaw): wire intent classification into run_tool_call_loop` (~60 LOC)
6. `test(plaw): unit tests for HybridRouter` (~150 LOC test code)
7. `docs(eval): Phase 3 L1 verification — n=20 per target case`
8. `feat(plaw): tune scaffold wording based on n=20 results`（如需）

每 commit 独立 PR-able。中间任何一步失败都可以单独回退。

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| Hybrid router 误分类（factual 被分到 ambiguous） | 单元测试覆盖；LLM fallback 给 confidence，confidence < 0.7 时 fallback 到 TaskRequest（保守路径） |
| 每条消息多一次 LLM 调用 → 延迟 | rule layer 命中率 ≥80% 跳过 LLM；LLM 用最小模型（kimi-light 或 cache） |
| Scaffold prompt 自身有 prompt 饱和问题（一个 scaffold 改坏另一个） | 每个 scaffold 独立 prompt，不共享。改 scaffold A 不影响 scaffold B 的 system prompt |
| L1 引入 bug 影响所有用户消息 | 加 feature flag `[experimental] intent_router = false`，默认关；plaw-eval 显式开。验证稳定后再默认开 |

## 实施完成的标志

- [ ] `plaw/src/agent/intent.rs` 存在
- [ ] HybridRouter ≥30 unit test 全过
- [ ] plaw-eval 跑 n=20 per target case，4 个 case ≥4.0
- [ ] PR 描述里贴 Phase 2 v2 vs Phase 3 L1 的 paired-diff 表
- [ ] 整体 chat_quality g_eval CI 不低于 v2 baseline -1pp
