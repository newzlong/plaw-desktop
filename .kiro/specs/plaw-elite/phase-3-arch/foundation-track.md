---
title: Phase 3 Track B — Foundation Hardening
status: roadmap, items ordered by dependency
date: 2026-05-04
---

# Foundation Track — codebase hardening alongside Phase 3 layers

并行 Phase 3 layers (L1-L4) 的工程改进集合。每条独立可 ship，但有自然的
依赖顺序。目的不是"修缝缝补补"——是把每条作为一个 focused 学习项目搞透
（具体技术 + 设计原则 + 实际应用）。

## 顺序与依赖

```
F-1 lib test fixture (#7)        ← 阻塞所有 unit-test 工作
  ├── F-2 judge cache (#2)       ← 可并行，独立功能
  ├── F-3 typed errors (#3)      ← 影响 F-4 / L1-5 的 API 风格
  │     └── F-4 god module split (#4)  ← 大重构
  ├── F-5 progress reporting (#6)        ← 独立 UX
  ├── F-6 evaluation parallelism (#1)    ← 用 F-3 类型 / F-5 progress
  ├── F-7 prompt section DAG (#5)        ← 独立小重构
  └── F-8 property-based tests (#8)      ← 应用到上面所有
```

| 序号 | 项目 | 单 session 工作量 | 关键学到 |
|---|---|---|---|
| **F-1** | plaw lib unit-test target 修通（22 errors）| 1 session | TestFixtureBuilder pattern；how API drift creates test debt；factory + default-overrides |
| **F-2** | judge_cache 真正用上 | 1 session | 内容寻址 cache (SHA256 key)；TTL 策略；layered JudgeClient 装饰器叠加（cache → retry → real） |
| **F-3** | agent loop 错误类型化（anyhow → ToolLoopError enum）| 2 sessions | thiserror；Rust application vs library error 哲学；callsite typed match vs string-contains 反模式 |
| **F-4** | loop_.rs 4000 行 god module 拆 | 3-5 sessions | DDD-style 分层；single-responsibility 在大模块上；trait extraction 时机；纯函数抽取 |
| **F-5** | plaw-eval incremental progress | 1 session | tokio mpsc；indicatif；observer pattern；不污染日志的 UX |
| **F-6** | plaw-eval scoring 并发化（200 min → 25 min）| 1 session | tokio Stream + buffer_unordered；bounded concurrency；rate-limit-friendly fan-out |
| **F-7** | PromptSection 加 ordering DAG | 1 session | 拓扑排序应用；plugin-style 架构；DAG cycle detection |
| **F-8** | proptest 引入 + 覆盖核心 fn | 1 session | property-based testing；shrinking；invariant testing vs example testing |

总共 11-15 sessions 的独立工作。配合 Phase 3 L1-L4 的 4-6 sessions，加起来
~20 sessions 的 Phase 3 整体。

## 每个 F-X 的产出标准

每完成一个 F-X 都包含：

1. **代码改动**：scope 限定，独立 PR，独立 verify
2. **commit 消息**：not just "what changed" 而是 "why + 学到的 principle"
3. **可选：retrospective 文档**：`.kiro/specs/plaw-elite/phase-3-arch/retros/F-X-<name>.md`
   - "我在做这个之前以为 X，做完发现 Y"
   - 关键设计决策的 trade-off
   - 引用的外部资料 / 论文 / 文档

retrospective 是这个项目和普通 ship-and-go 的最大区别 —— 学习成果固化。

## 不在 Foundation Track 范围

排除项（这些是别的方向）:

- 训练 / fine-tune 类工作（Phase 3 L2/L3 触及，但需大计算资源）
- 完全重写 plaw 数据存储层（与 Phase 3 无关）
- 添加新 channel / provider（也不阻 Phase 3）
- UI / Tauri 改进（与 agent runtime 弱相关）

## 当前推荐起点

**F-1 (lib test fixup)** 因为：
- 不大、不险（mechanical refactor，不改 production code）
- 阻塞所有后续 unit test 工作
- 是 "TestFixtureBuilder pattern" 的经典学习场景
- 完成后 L1 的 13 个 intent.rs 测试就能跑了，立即得到 feedback

下一步进入 F-1 实施。
