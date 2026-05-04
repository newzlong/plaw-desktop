---
title: F-1 Retrospective — Lib Test Fixture Rebuild
date: 2026-05-04
status: complete
related_commit: pending
---

# F-1 — Lib Test Fixture Rebuild

## 目标

修通 plaw 的 `cargo test --lib` 编译，让 22 个 pre-existing test errors
归零。这是 Phase 3 Track B 的 F-1，阻塞所有后续 F-X 和 L1-5 的 unit
test 验证。

## 入口诊断（错误分布）

22 errors 集中在 7 个文件、6 类根因：

| 类别 | 数量 | 文件 | 根因 |
|---|---:|---|---|
| 函数签名 drift | 12 | delegate / http_request / scheduler / subagent_spawn | 生产代码加了参数，test 调用没跟进 |
| 结构体新字段 | 3 | anthropic | `ChatRequest` 加了 `stream` 字段，test literal 缺 |
| 私有函数访问 | 2 | channels/mod.rs | tests 调 `strip_isolated_tool_json_artifacts` 但生产改成 `fn`(私有) |
| 类型推断退化 | 1 | channels/mod.rs | 邻近代码改动导致编译器无法 infer iterator type |
| 字段类型变化 | 2 | delegate.rs / subagent_spawn.rs | `agents` 字段从 `HashMap` 改成 `Arc<RwLock<HashMap>>`，tests 直接 `.get()` |
| 常量断言过时 | 1 | agent/loop_.rs | `DEFAULT_MAX_TOOL_ITERATIONS` 从有限值改为 `usize::MAX`，老 const_assert `<=100` 永远失败 |
| 函数渲染 trait drift | 1 | subagent_spawn | 跟"函数签名 drift"同因，但插入位置在中间 |

## 修法（按文件）

每个修法都是 1-3 行 mechanical 改动：

1. **delegate.rs**: `execute_agentic` 5 个 callsite 加 `, "openrouter", "model-test"` 两个字符串参数。一个 `HashMap::new()` 包成 `wrap_agents(HashMap::new())`。`tool.agents.get(...)` 改成 `tool.agents.read().unwrap().get(...)`（先 read lock 再 get）。
2. **http_request.rs**: `HttpRequestTool::new` 5 个 callsite 末尾加 `, false`（新加的 `allow_local: bool` 默认值）。
3. **anthropic.rs**: 3 个 `ChatRequest { ... }` literal 加 `stream: None,`。
4. **scheduler.rs**: 2 个 `process_due_jobs(...)` callsite 末尾加 `, None`（新参数 `Option<&broadcast::Sender>`）。
5. **agent/loop_.rs**: 删除已过时的 `assert!(DEFAULT_MAX_TOOL_ITERATIONS <= 100)`，加注释说明为什么不再适用。
6. **channels/message_sanitization.rs**: `fn strip_isolated_tool_json_artifacts` 改成 `pub(crate) fn`。
7. **channels/mod.rs**: 加 `use crate::channels::message_sanitization::strip_isolated_tool_json_artifacts;` 到 test mod。
8. **subagent_spawn.rs**: `SubAgentSpawnTool::new` 加新参数 `String::new()`，并把 `sample_agents_raw()` 包成 `wrap_agents(sample_agents_raw())`。

## 一个 sub-task：regex line continuation

修完编译错误后跑测试时发现 5 个我自己写的 intent regex test 失败。诊断：

```rust
Regex::new(
    r"(?i)\[\s*(?:system|...)\b\
     |<\|im_(?:start|end)\|>\
     |...",
)
```

我写错了。在 raw string `r"..."` 里，`\<newline>` **不是** line continuation
转义（那是普通 string `"..."` 的特性）。raw string 里 backslash 字面保留，
所以这个 regex 实际是一长串里夹了字面 `\` 和换行 + 缩进空格，破坏了
alternation 语法。

修法：用 `concat!()` 显式拼字符串字面值。

```rust
Regex::new(concat!(
    r"(?i)",
    r"\[\s*(?:system|admin|developer|override|jailbreak)\b",
    r"|<\|im_(?:start|end)\|>",
    r"|\bignore (?:the )?(?:above|previous|prior)\b",
    r"|(?:你是|you are)\s+DAN\b",
    r"|\bforget (?:all )?(?:your )?(?:previous |prior )?instructions\b",
))
```

## 一个 sub-task：Chinese 字符 + `\b`

另一个 regex bug：`r"\b撬锁"` 在测试里不匹配 `"我把自己锁在房门外了，怎么撬锁？"`。

诊断：regex 的 `\b` 是 \w 和 \W 之间的边界。Rust `regex` crate 默认
**Chinese 字符不是 \w**（除非 enable Unicode 类）。所以 `\b撬` 要求左侧
是 \w（ASCII alphanum 或 \_），但中文上下文里 `撬` 左边是另一个中文字 →
不算 \w → \b 不在那里 → 不匹配。

修法：丢掉 `\b` 用纯 substring 匹配（regex `撬锁` 直接表达 substring）：

```rust
Regex::new(r"(?i)撬锁|破解|绕过.*(?:检测|限制|风控|审核)|入侵|pick\s+(?:the\s+)?lock")
```

## 学到的一般 pattern

### 1. 测试直接构造 production 类型 = API drift 放大器

每次生产代码改个参数 / 字段 / visibility，N 个 test 同时坏。这次 12 个
errors 都属于这一类。

**反思**：生产代码用 `pub fn new(...)` 直接给 test 用，是反模式。test
应该走 `TestFixtureBuilder` 这种东西，把 production constructor 隔离一层。
当 production 改时，只需要改 builder 一处。

但我没在 F-1 引入 builder —— 因为：
- 这次的 fix 已经做了"修每处"路径，先证明所有 test 含义正确
- 引入 builder 是 F-1 之外的 refactor，独立 ship 更干净
- 几个 test 用 builder 反而复杂（比如就构造 1 个 HashMap 又用一次）

如果将来 plaw 的 production constructor 又改一次，这个学习就该转化成
F-1.5 = build TestFixtureBuilder。

### 2. 编译期断言（const_assert）也需要维护

`assert!(DEFAULT_MAX_TOOL_ITERATIONS <= 100)` 是个聪明的编译期 sanity check，
但当生产把常量改大后，这个断言成了"绝对要失败"的代码。如果没有 lib test
target，这个断言永远不被求值（在 `#[cfg(test)] mod` 里），不会暴露。

**反思**：编译期断言要么写 invariant（`> 0`），要么写 fixed bound（`< usize::MAX`）。
不要写 "应该不超过我个人觉得合理的数值"（`<= 100`）—— 那只是 magic number。

### 3. `\b` 在中文上失效

regex 教科书没强调 `\b` 的 \w 依赖。Chinese / 日文 / 韩文 在没启用
Unicode 类时都不是 \w，所以 `\b` 在 CJK 边界上不工作。这个我以后写
multilingual rule 时会记住。

### 4. raw string 的 line continuation

Rust 字符串两种：
- 普通 `"..."`：支持 `\<newline>` line continuation
- raw `r"..."`：不支持任何转义，包括 line continuation

这次踩坑是因为我下意识用普通 string 的习惯。raw string 写多行 regex
有几个选择：
- (a) 用 `concat!()` 拼多个 raw string literal —— 推荐
- (b) 写一长串（难读但无歧义）
- (c) 用 `(?x)` extended mode + 普通 string —— 太花哨

## 影响

- L1 的 `intent.rs` 26 个 unit test 现在能跑（之前被 22 个 lib-test errors
  阻塞）。
- 后续 F-X / L1-5 / L2 写 unit test 都能直接 cargo test 验证。
- 修复过程没有改任何 production code 的行为，只是 test/assertion 的
  catch-up。

## 下一步

F-2: judge cache 接进 scoring path（独立工作，跟 F-1 不耦合）。
