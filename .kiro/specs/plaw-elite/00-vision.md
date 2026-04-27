# Plaw Elite — 北极星 Vision

> 这份文档定义"精英 plaw"是什么样子。它是工程决策的最终裁判：当我们不知道是否该做某件事时，回到这里看是否符合 vision。
>
> 写作日期：2026-04-26 · 基于 6 份 SOTA 调研报告综合（research/）

---

## 一、定位

**plaw 不追求市场份额。它追求成为一个参考实现——证明在 2026 年，一个本地优先的桌面 AI agent 可以做到什么程度。**

具体地说：

- **不是 ChatGPT desktop 的克隆**：那是一个云端对话外壳，plaw 是本地优先的 agent runtime
- **不是 Claude Code 的复制**：Claude Code 是 CLI 工具，plaw 是有图形界面的桌面应用，承担 Claude Code 不做的事（持久化记忆、可视化追踪、本地推理 fallback、桌面级 computer use）
- **不是另一个 LangGraph/AutoGen**：那些是框架，plaw 是产品 + runtime，开箱即用

plaw 的存在意义：**当一个内行人（AI 工程师、研究者）打开 plaw，看到的每一个细节都让他点头。**

---

## 二、五条不妥协原则

这五条原则是评判 plaw 任何设计、任何代码的标尺。违反任何一条 = 不是精英 plaw。

### 1. 可测量的卓越（Measurable Excellence）

**没有测量，就没有精英。** 凭感觉的"打磨"不算打磨。

- 每一次模型调用都被评估（异步采样 + 离线 eval suite）
- 每一次改动都有统计显著性判断（非点值比较，需要 95% CI）
- 每一个 skill / prompt 都有自己的 golden set（≥100 例）
- 每一次发布都跑完整 eval（n≥300），结果公开记录
- **基线**：Anthropic [《A Statistical Approach to Language Model Evaluations》](https://arxiv.org/abs/2411.00640) 的统计严谨度

### 2. 本地优先 + 用户主权（Local-first, User-sovereign）

**plaw 默认从不联网。所有数据、追踪、记忆、eval 结果都在用户机器上。**

- 默认零遥测，零数据回传
- 用户可主动开启 OTLP 端点（指向自己的 Langfuse/Phoenix）
- PII 在持久化前过滤（OpenAI Privacy Filter / Presidio）
- GDPR 友好：tombstone 删除 + 级联 `requires_reverification`
- 本地推理 fallback：embedding（Qwen3-Embedding 0.6B）已是本地，未来可加本地小模型作为云端 fallback

### 3. 自我进化的记忆（Self-evolving Memory）

**记忆不是被动的存储库，是会反思、会蒸馏、会改写自己的活体。**

- 四层架构：working / episodic / semantic / procedural（Letta tier shape）
- 写时链接（A-MEM Zettelkasten 模式）：新记忆自动关联旧记忆，可重写旧记忆的标签
- 双时态边（Zep bi-temporal）：永不删除，仅 invalidate；conflict 自动检测
- Sleep-time consolidation（Letta + Anthropic Auto-Dream 模式）：用户空闲时跑后台反思，蒸馏 episodic → semantic
- Provenance 追溯：每个 derived memory 知道自己来自哪些证据；证据被删则自动标记 `requires_reverification`

### 4. Prompt 是一等公民（Prompt as a First-class Artifact）

**Prompt 永不硬编码在 `.rs` 文件里。它是有版本、被测试、被自动优化的工程产物。**

- 存储格式：`prompts/<skill>/<version>.prompt.toml`
- 每个 prompt 有：`inputs`、`system`、`examples`、`output_schema`、`cache_breakpoint_after`、`metric`、`eval_set`
- 编译时优化：`build.rs` 调用 DSPy + GEPA（[ICLR 2026 oral](https://arxiv.org/abs/2507.19457)），把人写的 prompt 优化成 SOTA 版本
- Cache 严谨：稳定前缀强制排在 `cache_breakpoint` 之前（lint 规则强制）
- 注入防御：所有工具输出包裹 `<untrusted_data>`；Sentinel-class 预过滤
- 结构化输出：用 Anthropic tool-based pattern（Kimi 兼容），永不依赖自由文本 JSON

### 5. 透明可追踪（Radical Transparency）

**plaw 的每一个决策、每一次模型调用、每一次工具执行都是可见、可重放、可对比的。**

- OTel GenAI semconv 从第一天起：每个 LLM call / tool call / retrieval 一个 span
- 本地 SQLite trace DB：默认存储位置 `plaw-data/.plaw/observability/traces.db`
- 内嵌 trace viewer（Vue UI）：时间线、span 树、工具 I/O 可折叠卡片
- Replay 能力：把一段对话用新 prompt / 新模型重跑，diff 对比
- 隐式用户反馈自动捕获（copy / regen / abandon / rephrase）→ 关联 span ID → 进入 eval 数据集

---

## 三、五个量化标准

精英 plaw 在这 5 个维度上必须达到或超过的具体数值：

| 维度 | 指标 | 目标值 | 测量方式 |
|------|------|--------|---------|
| **回答质量** | LLM-Judge Win Rate vs baseline | ≥ 55% (95% CI 下界) | 多 judge jury，pairwise + position swap，n≥300 |
| **检索质量** | Recall@5 on plaw-bench-rag | ≥ 0.85 | Hybrid 检索 + BGE rerank，对照 naive RAG baseline |
| **Agent 鲁棒性** | Tool error recovery rate (ERR) | ≥ 0.7 | 注入故障后 agent 恢复完成的比例 |
| **运行经济性** | Cache hit rate | ≥ 70% | `cache_read_input_tokens / total_input_tokens` |
| **隐私合规** | 网络出站请求（默认配置下） | = 0（除模型 API 外） | 启动时 strace / netstat 监控 |

**严格要求**：任何 PR 不能让以上任意一项指标的 95% CI 下界低于 baseline 的均值减去 ε（ε = 1pp）。这是 CI 自动 gate。

---

## 四、参考架构

```
┌─────────────────────────────────────────────────────────────────┐
│  用户界面（Vue + Tauri）                                          │
│  ┌─────────────┐  ┌──────────────┐  ┌──────────────┐            │
│  │   Chat UI   │  │ Trace Viewer │  │ Eval Reports │            │
│  └─────────────┘  └──────────────┘  └──────────────┘            │
└────────────┬─────────────────┬──────────────┬───────────────────┘
             │                 │              │
┌────────────▼─────────────────▼──────────────▼───────────────────┐
│  plaw 核心 runtime（Rust）                                        │
│                                                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Agent Loop（单线程 ReAct + Task subagent dispatch）      │   │
│  │  · 并行 tool call（tokio::join!）                         │   │
│  │  · 错误即指令（PlawError {code, hint, retry_strategy}）   │   │
│  │  · 每轮 SQLite checkpoint（可恢复）                       │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                   │
│  ┌─────────────────┐  ┌──────────────────┐  ┌────────────────┐  │
│  │  Prompt Registry│  │  Memory System   │  │  RAG Pipeline  │  │
│  │  · TOML files   │  │  · 4-tier Letta  │  │  · CRAG eval   │  │
│  │  · GEPA-编译    │  │  · A-MEM 链接    │  │  · Adaptive    │  │
│  │  · cache 严谨   │  │  · Zep bi-temp   │  │    router      │  │
│  │  · 注入防御     │  │  · sleep-time    │  │  · Hybrid+BGE  │  │
│  └─────────────────┘  └──────────────────┘  └────────────────┘  │
│                                                                   │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │  Observability（OTel GenAI semconv）                      │   │
│  │  · Local SQLite trace store（默认）                        │   │
│  │  · 可选 OTLP export（用户提供端点）                        │   │
│  │  · 5 个 always-on 守护规则（schema/args/loops/length/...） │   │
│  │  · 异步 sampled judge（drift / quality）                   │   │
│  └──────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────┘
             │                                    │
┌────────────▼──────────────────┐  ┌──────────────▼──────────────┐
│  Storage（all SQLite）         │  │  External（按需）             │
│  · plaw-data/<ws>/memory.db   │  │  · Kimi K2.5（默认 LLM）     │
│  · plaw-data/<ws>/vectors.db  │  │  · Anthropic / OpenAI（备用）│
│    （sqlite-vec）              │  │  · 本地 llama.cpp（fallback）│
│  · plaw-data/<ws>/traces.db   │  │  · MCP servers（用户配置）   │
└────────────────────────────────┘  └────────────────────────────┘

边车进程（可选，opt-in）：
  · ort + Qwen3-Embedding-0.6B（替代 llama.cpp embedder）
  · ort + BGE-reranker-v2-m3
  · HHEM-2.1 幻觉检测
  · OpenAI Privacy Filter PII 过滤
  · DSPy + GEPA prompt 编译（build.rs 阶段）
```

---

## 五、技术选型决策（顶层）

| 层 | 选择 | 替代方案 | 决策依据 |
|----|------|---------|---------|
| Agent loop | 单线程 ReAct + Task subagent | 多 agent orchestrator | Cognition + Anthropic 共识：write 任务用单 agent，read fan-out 才用 multi-agent |
| Prompt 编译 | DSPy + GEPA（offline） | MIPROv2 / TextGrad / 手写 | GEPA ICLR 2026 oral，比 MIPROv2 +10%，比 RL +6-20% with 35× fewer rollouts |
| 类型安全 prompt | BAML（Rust 原生 codegen） | f-string in Rust | BAML 是唯一 first-class Rust 支持的类型安全 prompt 框架 |
| 结构化输出 | Anthropic tool-based | 自由文本 JSON / Outlines | Kimi K2.5 兼容；最可靠，原生支持 |
| 检索 | Tantivy(BM25) + LanceDB(dense) + RRF | 纯 dense / 纯 BM25 | Hybrid 把 recall@10 从 65-78% 拉到 91% |
| Reranker | BGE-reranker-v2-m3（ort/ONNX） | LLM listwise / Cohere | 中英多语，Kimi token 成本不允许 listwise |
| Embedding | Qwen3-Embedding-0.6B（ort/ONNX 而非 llama.cpp） | 现有 llama.cpp | llama.cpp issue #19933 慢 5×；ort 快 3-5× |
| Chunking | Late chunking（代码/文档）+ Contextual Retrieval（记忆胶囊） | Naive 固定字数 | 减少 49% 检索失败；Anthropic prompt cache 让成本可承受 |
| 记忆框架 | Letta tier + A-MEM 链接 + Zep bi-temporal（自建 Rust） | 直接用 Letta/Mem0/Zep | 都是 Python；自建可控、零依赖、可深度集成 |
| 存储 | SQLite + sqlite-vec（all in plaw-data/） | Qdrant service / pgvector | 单文件，便携，GDPR 删除友好 |
| Eval 框架 | 自建 Rust（核心指标）+ Langfuse（追踪/仪表板） | DeepEval / RAGAS（Python） | 避免 Python sidecar；Anthropic 统计严谨度可在 Rust 实现 |
| Observability | OTel GenAI + 本地 SQLite + 内嵌 viewer | Langfuse 全量 / Phoenix Docker | 本地优先，零配置，可选高级用户开 OTLP |
| 幻觉检测 | HHEM-2.1-Open（opt-in 边车） | Lynx-70B / SelfCheckGPT | 600MB RAM，CPU 1.5s，可生产部署 |
| Computer Use | Anthropic `computer_20251124`（仅 Anthropic 模型）；纯坐标 fallback | 自建 a11y tree | OSWorld-Verified Apr 2026: Anthropic 79.6%；自建 ROI 低 |

---

## 六、反目标（What Plaw Elite Will Not Be）

明确说"不做什么"和说"做什么"同样重要。

- ❌ **不做云服务**。plaw 是桌面应用，不会有 plaw.com。任何"我们的服务器"模式都背离 vision。
- ❌ **不做付费墙**。这是个人作品，不卖 SaaS。如果将来有商业化，不影响开源核心。
- ❌ **不做插件市场 / skill 商店**（短期）。先把核心做到极致，市场是分心。
- ❌ **不做 Self-RAG / 微调模型**。plaw 是 model-agnostic 的 runtime，不绑定特定模型权重。
- ❌ **不做 GraphRAG**（v1）。LazyGraphRAG 仍然成本不匹配桌面场景；先看 vector RAG 极限。
- ❌ **不做多用户协作**。单用户单工作区是核心定位；多用户是另一个产品。
- ❌ **不做 Python sidecar 运行时**。仅在 build 阶段（DSPy GEPA 编译）允许 Python，runtime 必须纯 Rust。
- ❌ **不做"AI 助手"营销话术**。plaw 是 agent runtime，不是聊天玩具。

---

## 七、成功标准（什么时候算"做到了"）

精英 plaw 的成功不是用户数，是以下事实：

1. **任何 AI 工程师 clone 仓库 5 分钟，能看到至少 3 个让他想截图分享的细节**（GEPA 编译、bi-temporal 记忆、in-app trace viewer 等）
2. **plaw 的 eval suite 能被引用**——其他项目用 plaw 的 benchmarks 评估自己的方案
3. **plaw 的某个组件被独立提取使用**——比如 prompt registry、memory system 被其他 Rust agent 项目引用
4. **plaw 的某个设计决策被其他产品采纳**——这是最高荣誉
5. **作者本人在用 plaw 时不再想"如果...就好了"**——所有内心想要的都做到了

---

## 八、本 vision 的版本管理

- v1.0（2026-04-26）：本文档初版
- 后续修改需在 `decisions/` 下加一个 ADR 说明 why
- vision 不轻易改；如果某个原则被违反 3 次以上，要么修改 vision 要么修复违反点
- 每完成一个 phase（见 `01-roadmap.md`），回头审视 vision 是否仍然 hold
