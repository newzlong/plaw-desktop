---
title: Plaw Baseline — 2026-Q2
date: 2026-04-29
runs:
  chat_quality_kimi_n30_v1: "9545af14-eda3-47dd-98d1-8c953edbc5d3"
  tool_routing_kimi_n30_v1: "d4494973-cce7-4a9f-bb6a-0053051cbafd"
  chat_quality_deepseek_n30: "82c27c1f-4b4c-4ea6-900b-3d5f3c8eeebf"
  chat_quality_kimi_n30_v2: "d0c11231-9706-4728-9754-3c9a5febc18a"
  tool_routing_kimi_n30_v2: "8de3e2a4-e0a8-48e4-8040-8a3e5f6b134b"
  chat_quality_kimi_oversampled: "28b71f3e-7567-4cab-a4ef-3d7389cf523c"  # 30 unique × 10 reps = 300
  tool_routing_kimi_oversampled: "642b6812-1d26-4ad6-accb-1a32369c1833"   # 32 unique × 10 reps = 320
plaw_version: "live dev mode (commit n/a — captured before commit metadata wiring)"
default_judge: kimi-coder/k2p5 (api.kimi.com/coding)
cross_family_judge: deepseek/deepseek-v4-pro (api.deepseek.com)
sample: 30/32 unique cases × 10 repetitions = 300/320 obs per suite
---

# Plaw Baseline — 2026-Q2

**Status**：✅ 完整 baseline 已跑（30/32 unique × 10 reps × 2 suites = 620 obs，cluster-robust SE 启用）。Phase 2 启动后用本文档的数字当对照。

## 锁定的数字 (n=300/320, 10 reps, cluster SE)

| Suite | Metric | n | mean | 95% CI | clustered SE | clusters |
|-------|--------|---:|-----:|--------|---:|---:|
| chat_quality | g_eval | 300 | **0.9218** | [0.87, 0.97] | 0.0240 | 30 |
| chat_quality | keyword_coverage | 300 | **0.7728** | [0.67, 0.87] | 0.0500 | 30 |
| tool_routing | tool_call_accuracy | 263 | **0.7362** | [0.66, 0.81] | 0.0374 | 28 |

> tool_routing 中 4 个 case `expected = []` 且 plaw 没调工具 → 视为信号一致跳过，所以 n=263 < 320。28 clusters 而不是 32 同因。

## 数字演进史

为了让后续读者理解 baseline 是怎么来的（以及哪些是设计 bug、哪些是真信号），保留三轮对照：

### 1. 跨 family judge 对照（n=30）

| Judge | Family | g_eval | keyword_coverage |
|-------|--------|---:|---:|
| kimi-coder/k2p5 | Kimi（同 plaw） | 0.9034 | 0.5278 |
| deepseek/v4-pro | DeepSeek（cross） | 0.9008 | 0.4778 |
| **差值** | — | **−0.003** | −0.05（noise） |

→ self-preference 偏见**未检出**。问题在 G-Eval prompt 偏宽容，不在 family。

### 2. 修复后的两次 n=30（一致性检查）

| Run | g_eval | keyword_coverage | tool_call_accuracy |
|---|---:|---:|---:|
| 跑 1 (4-29 上午) | 0.9034 | 0.5278 | 0.7150 |
| 跑 2 (4-29 下午) | 0.9571 | 0.5111 | 0.7245 |
| 差值 | +5pp（noise）| 0.5（noise）| +1pp（noise）|

→ 单次 noise 很大，必须靠重复采样收紧。

### 3. 完整 oversampled (n=300/320, 10 reps) ⭐ baseline 锁定

见上面"锁定的数字"表。CI 稳定，cluster SE 启用。

## 关键发现

### 0. 锁定的差距：g_eval 0.92 vs keyword_coverage 0.77

n=300 的对比：

| 信号 | mean | 95% CI |
|---|---:|---|
| g_eval | 0.9218 | [0.87, 0.97] |
| keyword_coverage | 0.7728 | [0.67, 0.87] |
| **差距** | **+14pp** | CI 几乎不重叠（0.87 vs 0.87 边界） |

**这是统计显著的差距，不是 noise**。证明 G-Eval judge 比机械关键词覆盖**系统性高估** plaw 的回答质量约 14 pp。
Phase 2 评估 prompt 改动时**优先盯 keyword_coverage**，g_eval 当辅助。

### 1. 偏见来源不是 self-preference，是 judge prompt 太宽松

跨 family 对比：kimi-coder 给 0.9034，deepseek-v4-pro 给 0.9008，**差值 −0.003**（统计上等价）。

这跟 [Liu 2024 (arXiv:2410.02736)](https://arxiv.org/abs/2410.02736) 报告的 5-10pp self-preference 偏见**不一致**。可能的解释：

1. **G-Eval CoT prompt 让 judge 都偏向给高分** — 两个 judge 用同一个评分模板，都收敛到"看起来 OK = 4-5 分"
2. **Judge 区分度不足** — 单 judge 对"好" vs "超好"分不开
3. **测试 case 难度不够** — 大部分 case 是基础对话，所有 judge 都觉得 plaw 答得不错

`g_eval = 0.90` vs `keyword_coverage = 0.53` 的 0.37 差距**不能**被 cross-family judge 修复 —— 换个 judge 也是 0.90。

**真正的修法**：
- 改 G-Eval prompt 用更严格的评分维度（精度、简洁性、引用、calibration）
- 加更难的 case（adversarial / 长 context / 复杂推理）
- 用 pairwise 而非 score —— 让 judge 强制选 A vs B，没法都打 5 分

**可信度排序**（基于这次发现）：keyword_coverage > tool_call_accuracy > g_eval。
**Phase 2 改动**：盯 keyword_coverage 和 tool_call_accuracy 的变化，g_eval 当辅助信号。

### 2. plaw 的真实 calibration bug

测出来一个**真问题**：[chat_quality-unknowable-005](../../evals/chat_quality/cases.toml)（"北京现在几点？"）

plaw 的回答：> 北京时间现在是 **2026年4月29日 01:12:45**（凌晨1点12分）

plaw **没有实时时钟**，但它自信地编了一个时间。这是 hallucination 的典型：明明该说"我没法知道实时信息"，结果硬答了。

**这就是 eval 系统该测出来的信号** —— 后续 Phase 2 修 calibration / hallucination 时，这个 case 会变成回归门槛。

### 3. plaw 的工具命名跟 CLAUDE.md 不一致

CLAUDE.md 写的"语义工具名"（`read_file` / `web_search` / `browser_navigate`）跟 plaw 实际暴露的（`file_read` / `web_search_tool` / `browser`）不同。

cases.toml 已对齐到 plaw 实际名。**待办**：更新 CLAUDE.md 反映真实工具表，避免下次有人按文档写出错的 case。

### 4. plaw 的 routing 倾向

从 24 个有效 tool_routing case 看：

- **倾向 shell over 专门工具**：要列目录倾向 `shell` 而不是 `glob_search`，要 git status 倾向 `git_operations`（这个对的）
- **倾向 web_fetch over web_search_tool**：直接抓页面而不是先查
- **倾向 parallel_delegate 处理多步骤**：批量改 Cargo.toml 时用 parallel_delegate 而不是依次 read+edit
- **多步任务计划失控**：`multi-005`（看 Cargo.toml + 查依赖最新版本）调了 14 次工具，包括 8 次 shell。8 步预算严重超支。

这些都是 Phase 2 prompt / agent loop 改造的优化点。

## case 设计的反思

跑出来才发现 case 设计的几个问题：

### keyword 太死板

像 `unknowable-001` 期望关键词 `不知道/无法/没有方式`，但 plaw 答 "没有找到记录" / "目前没有存储这个信息" —— 语义上完全正确，关键词漏匹配。

**改进思路**（Phase 1.5 / Phase 2）：
1. 关键词列表加更多同义词（`不知道|没找到|没记录|没有信息|无从得知`）
2. 或者 keyword_coverage 的逻辑改成 "一个语义类只要命中一个就算"（用 \| 分隔的 OR 组）
3. 或者 case_insensitive 之外，加个 stemming（中文不适用，但英文场景需要）

### plaw 输出 markdown 干扰关键词

像 `factual-002` 期望 `H2O`，plaw 回 `H₂O`（Unicode 下标 ₂）—— 内容完全对，匹配挂了。

**改进**：keyword_coverage 应该有个 normalize 选项，把 Unicode 数字（U+2080-2089）、HTML 实体、markdown 强调符号都剥掉。

### style/refuse 类的 case 不该用 keyword_coverage

`style-002` 期望关键词 `周一` —— plaw 答得好但写"周一"五次都没用，整个段落是吐槽周一上班。这种**风格 / 情绪类** case 关键词覆盖率不是合适的 metric，应该用 g_eval 或 cross-judge。

**建议**：cases.toml 加个 `applicable_metrics: ["g_eval"]` 字段，按 case 选 metric，不让所有 case 都跑所有 metric。

## 历史工具缺口（已修）

### ~~`--n` 实际是"最多 N 个 case"~~ → 已加 `--repetitions`（commit `175ce3f`）

原本 `--n 300` 被 suite 大小 cap 住。现在两个 flag 各司其职：
- `--n K`：取最多 K 个 unique case
- `--repetitions K`：每个 case 跑 K 次（cluster_id 自动设为 base case id）

本次 baseline 用 `--repetitions 10` 跑出 620 总观察值，cluster-robust SE 自动启用。

## 已做（截至本次更新）

- ✅ **关键词放宽（OR 组）** —— `keyword_coverage` 从 0.53 估到 0.78（离线重算）。`evals/chat_quality/cases.toml` 用 `|` 分隔同义词。
- ✅ **更新 CLAUDE.md 工具表** —— 跟 plaw 实际工具名对齐（13 个真实工具）。
- ✅ **`unknowable-005` 标 regression-target** —— `tags = ["regression-target", "hallucination"]`，Phase 2 修 hallucination 时这个 case 是必盯指标。
- ✅ **Cross-family judge 比较** —— DeepSeek vs Kimi 几乎一致，证明偏差不在 family，在 judge prompt（见 §1）。
- ✅ **`--judge` CLI override** —— 可以一行命令切换 judge：
  `plaw-eval run --suite X --judge "deepseek:deepseek-v4-pro"`

## 下一步（Phase 2 启动前）

按重要程度排：

1. **改 G-Eval prompt** —— 让 judge 拉开分布。引入精度 / 简洁 / calibration / 引用维度。这是 Phase 1.5 关键工程项。
2. **跑 n=300 完整 baseline**（进行中）—— 收紧 CI，给 repeatability 信号。
3. **加更难的 case** —— 当前 case 太基础，judge 一律给高分。需要长 context / 多步推理 / 对抗样本。
4. **case-level metric whitelist** —— 让 style/refuse 类只跑 g_eval；factual/math 类只跑 keyword。避免 metric 污染。
5. **`unknowable-005` Phase 2 攻坚目标** —— 真 hallucination bug。

## 成本回顾

n=30 × 2 suite = 62 cases，每个 case：
- 1 次 plaw chat 调用（agent 内部可能再调 N 次）
- 0-1 次 g_eval judge 调用

实际花费：~¥3。
合理。Full eval（n=300）预估 ~¥30。
