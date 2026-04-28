---
title: Plaw Baseline — 2026-Q2
date: 2026-04-29
runs:
  chat_quality: "9545af14-eda3-47dd-98d1-8c953edbc5d3"
  tool_routing: "d4494973-cce7-4a9f-bb6a-0053051cbafd"
plaw_version: "live dev mode (commit n/a — captured before commit metadata wiring)"
judge: kimi-coder/k2p5 (api.kimi.com/coding)
sample_n: 30 per suite
---

# Plaw Baseline — 2026-Q2

**Status**：smoke baseline，n=30。下一步是用 cross-family judge（Anthropic）跑 n=300 验证 self-preference 偏见有多大。

## 数字

| Suite | Metric | n | mean | 95% CI | 说明 |
|-------|--------|---:|-----:|--------|------|
| chat_quality | g_eval | 30 | 0.9034 | [0.83, 0.98] | judge 给的整体分数 — 同 family 偏高，**信号被污染** |
| chat_quality | keyword_coverage | 30 | 0.5278 | [0.38, 0.67] | 机械关键词命中 — **信号干净** |
| tool_routing | tool_call_accuracy | 24 | 0.7150 | [0.63, 0.80] | F1 + redundancy + arg validity 复合分 |

## 关键发现

### 1. judge 的自我偏好膨胀

`g_eval = 0.90` vs `keyword_coverage = 0.53` 在同一批响应上，差距 0.37 几乎全部来自：

- judge 是 plaw 同 family（kimi-coder 评 kimi-coder）—— Liu 2024 [arXiv:2410.02736](https://arxiv.org/abs/2410.02736) 报告这种情况下偏好方向能差 5-10pp
- judge 看 plaw 的 markdown 排版漂亮就给高分，不严格检查内容覆盖
- keyword_coverage 是确定性的，没有偏袒空间

**可信度排序**：keyword_coverage > tool_call_accuracy > g_eval。Phase 2 改动用 g_eval 单独跑数字时务必加 cross-family judge 复核。

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

## 下一步

按重要程度排：

1. **加 cross-family judge**（高）— 当前 self-preference 污染严重。下次跑 baseline 加个 Anthropic Claude 当 jury 第二票。
2. **关键词放宽**（中）— 用 `\|` OR 组扩同义词，重新跑 chat_quality。
3. **case-level metric whitelist**（中）— 让 style/refuse 类只跑 g_eval。
4. **更新 CLAUDE.md 工具表**（低）— 跟 plaw 实际暴露的名字对齐。
5. **`unknowable-005` 单独追**（高）— 这是发现的真 calibration bug，Phase 2 修 hallucination 时优先处理。
6. **跑 n=300 完整 baseline**（中）— 当前 n=30 只是 smoke，CI 太宽。完整 baseline 才能锁数字给 Phase 2 当 gate。

## 成本回顾

n=30 × 2 suite = 62 cases，每个 case：
- 1 次 plaw chat 调用（agent 内部可能再调 N 次）
- 0-1 次 g_eval judge 调用

实际花费：~¥3。
合理。Full eval（n=300）预估 ~¥30。
