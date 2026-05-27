[English](README_en.md) | **中文**

# Plaw Desktop

**Plaw Desktop** — 开源、自托管的桌面 AI Agent，内嵌 [Plaw](plaw/) 自主智能体引擎。

填入你自己的 API Key，即可拥有一个能操作电脑、浏览网页、读写文件、执行代码的 AI 助手——全程本地运行，密钥直连模型厂商，不经任何中转。

> 自带 API Key（BYO-key），所有运行时依赖打包内置。引擎到前端全部开源、可审计、可改造；同时保留一键安装的开箱即用体验。

## 致谢

Plaw 引擎基于 [ZeroClaw](https://github.com/ZeroClaw-AI/ZeroClaw) 改造而来，感谢 [OpenClaw](https://github.com/OpenClaw-AI) 和 ZeroClaw 的开源贡献。

虽然改到现在代码已经面目全非（内嵌桌面端、并行子 Agent、浏览器自动化、安全防护体系、Skills 热加载……），但没有 ZeroClaw 打下的基础就没有 Plaw。开源精神万岁。

本项目的绝大部分代码由 [Claude](https://claude.ai/)（Anthropic）辅助编写，包括 Rust 后端、Vue 前端、安全策略、Skills 系统等。感谢 Claude 作为全程 AI 结对编程伙伴的贡献。

---

## 为什么做这个？

大多数 AI Agent 要么是云端 SaaS（你的数据和密钥都要过它的服务器），要么是面向终端的开发者工具（Claude Code、Cursor、Cline）。

Plaw Desktop 想做的是另一种形态——**你自己机器上的、用你自己密钥的、可审计可改造的桌面 Agent**：

- **开源、可审计**：引擎到前端全部开源，安全策略和工具实现都能自己读、自己改
- **自托管、BYO-key**：用你自己的 API Key，请求直连模型厂商，不经任何中转
- **全工具权限**：Shell、文件、浏览器、HTTP、Office、Cron——不是阉割版沙盒
- **零外部依赖**：Chromium、Python、Office、Embedding 模型全部打包内置，不用自己装
- **一键安装**：保留开箱即用体验，不强迫你配环境、改配置、敲命令行

## 核心特性

### AI 能力

| 特性 | 说明 |
|------|------|
| **自主 Agent** | AI 自主规划、调用工具、多步推理，直到任务完成 |
| **实时打断** | 随时发消息打断 AI，用户优先，AI 立即停下响应你 |
| **文件理解** | 发送图片、PDF、Excel、Word 等附件，AI 直接分析 |
| **并行执行** | 复杂任务自动拆分为多个子 Agent 并行处理（parallel_delegate） |
| **上下文压缩** | 长对话自动压缩，不丢关键信息，节省 Token |
| **胶囊记忆** | 对话自动沉淀为胶囊，AI 跨会话记住你的偏好和历史 |
| **主动回忆** | AI 遇到似曾相识的问题时，主动搜索历史胶囊辅助回答 |
| **语义 Skill 路由** | 每轮对话根据语义匹配最相关的 Skills，而非全量注入 |
| **Skills 扩展** | Markdown 技能包扩展 AI 能力，热加载，无需重启 |
| **流式输出** | SSE 流式传输，AI 思考和回复实时可见，工具执行带进度提示 |

### 工具箱

| 工具 | 能力 |
|------|------|
| **Shell 执行** | PowerShell / Bash / Cmd，自动选择最优 Shell |
| **文件操作** | 读、写、编辑、搜索、目录浏览 |
| **网络搜索** | Bing 搜索，国内直连无需代理 |
| **网页抓取** | 抓取任意网页内容，自动转 Markdown |
| **浏览器自动化** | 内置 Chromium，AI 可自主浏览和操作网页 |
| **API 调用** | HTTP 请求，对接任意 REST API |
| **胶囊记忆搜索** | 向量 + 关键词混合搜索，跨会话知识检索 |
| **Office 文档** | AI 生成 PPTX / DOCX / XLSX / PDF（内置 Office CLI） |
| **定时任务** | Cron 表达式，AI 定时自动执行 |
| **Git 操作** | 代码版本管理 |
| **PDF / 图片** | 读取 PDF、分析图片内容 |

### 安全防护

| 层级 | 机制 |
|------|------|
| **输入防护** | PromptGuard 提示词注入检测 |
| **外部内容扫描** | 自动标记网页/搜索结果中的注入攻击 |
| **反死循环** | 同名工具重复调用检测 + 迭代上限 |
| **凭证加密** | 所有 token/secret 以 `Secret` 类型加密落盘（ChaCha20-Poly1305），仅在使用时解密，日志自动脱敏 |
| **命令沙箱** | Shell 命令在沙箱内执行；Windows 用 Job Object 隔离，进程随主程序退出一并终止 |
| **Webhook 签名校验** | 所有 webhook 端点默认安全：非回环地址且未配密钥时一律拒绝 |
| **Skills 安全审计** | 自动扫描 Skill 文件，拦截可疑脚本和注入模式 |
| **隐私优先** | 所有数据本地存储，API Key 直连厂商，不经第三方 |

### 桌面体验

| 特性 | 说明 |
|------|------|
| **毛玻璃 UI** | Glassmorphism 风格，深色主题，纯自定义组件 |
| **Portable 模式** | 数据存在安装目录旁，U 盘拷走即用 |
| **中英双语** | 界面支持中文和英文切换 |
| **Setup Wizard** | 首次启动引导配置，傻瓜式操作 |
| **会话管理** | 多会话、历史记录、工具执行步骤持久化 |
| **实时进度** | 工具执行实时显示进度，不是黑盒 |

## 支持的 AI 模型

| Provider | 格式 | 推荐模型 | 备注 |
|----------|------|----------|------|
| **DeepSeek** | OpenAI 兼容 | deepseek-v4-pro | 国内直连，**默认推荐** |
| Anthropic | 原生 | Claude Sonnet/Opus | 需代理 |
| OpenAI | 原生 | GPT-4o | 需代理 |
| Kimi Coder | Anthropic 兼容 | k2p5 | 国内直连 |
| Gemini | 原生 | Gemini Pro | 需代理 |
| Ollama | 本地 | 任意本地模型 | 离线可用 |
| GLM (智谱) | OpenAI 兼容 | GLM-4 | 国内直连 |
| Qwen (通义) | OpenAI 兼容 | Qwen-Max | 国内直连 |
| Moonshot | OpenAI 兼容 | Moonshot-v1 | 国内直连 |
| OpenRouter | OpenAI 兼容 | 多模型路由 | 需代理 |
| 自定义 | OpenAI/Anthropic 兼容 | - | 支持任意端点 |

> **Provider 无关设计**：Plaw 不为任何特定模型特化。切换默认只是改 `config.toml` 的 `default_provider` + `default_model`，不需要改代码、不需要重新编译。当前默认推荐 DeepSeek V4 Pro（国内可直连、质量在国产模型里领先），随模型迭代会更新。

## 架构

```
+---------------------------------------------------+
|                 Plaw Desktop                       |
|  +-----------+-----------+-----------+----------+  |
|  |   Chat    | Capsules  | Settings  |  Cron    |  |
|  |           |  (记忆)   |           |  (定时)  |  |
|  +-----------+-----------+-----------+----------+  |
|  |           Vue 3 + Glassmorphism UI            |  |
|  +---------------------+------------------------+  |
|                        | WebSocket (SSE 流式)       |
|  +---------------------+------------------------+  |
|  |              Plaw Agent Engine                |  |
|  |  +---------+ +--------+ +------------------+ |  |
|  |  | Provider| | Tools  | | Memory & Skills  | |  |
|  |  | 10+ LLM | | Shell  | | Capsule (SQLite) | |  |
|  |  | DeepSeek| | File   | | Embedding 向量   | |  |
|  |  | OpenAI  | | Web    | | Semantic Routing | |  |
|  |  | Anthropic| | Browser| | Skills 热加载   | |  |
|  |  | Custom  | | Office | | Agentic Recall   | |  |
|  |  +---------+ +--------+ +------------------+ |  |
|  +-----------------------------------------------+  |
|  +-----------------------------------------------+  |
|  |          Tauri 2.0 (Rust Backend)             |  |
|  |  Process Mgmt / Config / Embedding Server     |  |
|  +-----------------------------------------------+  |
+---------------------------------------------------+
```

| 层级 | 技术 | 说明 |
|------|------|------|
| 桌面框架 | **Tauri 2.0** | Rust 后端 + 系统 WebView，安装包 ~50MB |
| AI 引擎 | **Plaw** | 纯 Rust 自主智能体，16MB 单二进制 |
| 前端 | **Vue 3** | 毛玻璃 UI，纯自定义组件，零 UI 库依赖 |
| 通信 | **WebSocket** | 前端与 Plaw 实时双向通信，SSE 流式输出 |
| 记忆 | **SQLite + Embedding** | 胶囊记忆 + FTS5 关键词 + 向量混合搜索 |
| Embedding | **llama-server** | 本地 768 维向量，GemmaEmbedding 300M 量化模型 |
| 配置 | **TOML** | 人类可读，应用内可视化编辑 |

## 快速开始

### 直接使用（下载安装包）

从 [Releases](https://github.com/newzlong/plaw-desktop/releases) 下载安装包：

- **Windows**: `plaw-desktop_x.x.x_x64-setup.exe`

安装后启动 → Setup Wizard 引导你选择模型、填入 API Key → 完成，开始对话。

> **安装包为什么这么大？（~800MB）** Plaw 的目标是"安装即用，零配置"，所以把所有运行时环境全部内置打包，用户不需要自己装任何东西：
>
> | 内置组件 | 大小 | 用途 |
> |----------|------|------|
> | Chromium Headless | ~260MB | 浏览器自动化（AI 浏览网页） |
> | Python 3 + 常用库 | ~130MB | Office 文档生成、数据处理 |
> | LibreOffice Portable | ~210MB | DOCX/XLSX/PDF 转换 |
> | Node.js | ~80MB | PPTX 生成、浏览器 daemon |
> | Embedding 模型 | ~315MB | 本地语义搜索（胶囊记忆、Skill 路由） |
> | Plaw 引擎 | ~16MB | AI Agent 核心 |
> | Skills 技能包 | ~14MB | 30+ 预装 AI 技能 |
>
> 这些都是为了开箱即用、零配置。如果你从源码构建，可以按需裁剪。

### 开发者

```powershell
git clone https://github.com/newzlong/plaw-desktop.git
cd plaw-desktop
.\setup.ps1     # 一键安装所有依赖 + 编译 Plaw 引擎
.\dev.ps1       # 启动开发（Vite + Tauri 热重载）
```

`setup.ps1` 自动完成 7 件事：

| 步骤 | 内容 | 安装方式 |
|------|------|----------|
| 1 | Rust 工具链 | rustup |
| 2 | Node.js | winget |
| 3 | pnpm | npm |
| 4 | Tauri CLI | cargo |
| 5 | 前端依赖 | pnpm install |
| 6 | plaw-data 目录 + 默认配置 | 自动创建 |
| 7 | 编译 Plaw 引擎 + 部署 | cargo build --release |

脚本幂等，可反复运行，已完成的步骤自动跳过。

启动后打开应用 → Setup Wizard 填 API Key → 就能用了。

## 项目结构

```
plaw-desktop/
├── src-tauri/              # Tauri Rust 后端
│   └── src/
│       ├── lib.rs          # Portable 数据目录、端口分配、config 读写
│       └── plaw.rs         # Plaw 进程管理
├── web/                    # Vue 3 前端
│   └── src/
│       ├── views/          # Chat, Dashboard, Settings, SetupWizard 等
│       ├── components/     # GlassDialog, SettingsPanel 等自定义组件
│       ├── composables/    # usePlawState, useI18n 等
│       └── api/            # Tauri 命令 + WebSocket 封装
├── plaw/                   # Plaw AI 引擎（完整 Rust 源码）
│   └── src/
│       ├── agent/          # Agent 循环、工具调用、上下文压缩
│       ├── providers/      # 10+ LLM Provider 实现
│       ├── tools/          # 50+ 工具（Shell、文件、浏览器、搜索等）
│       ├── security/       # 安全防护（PromptGuard、审计、沙箱）
│       ├── gateway/        # WebSocket 网关
│       ├── memory/         # 胶囊记忆（SQLite + FTS5 + Embedding 向量混合搜索）
│       └── skills/         # Skills 加载、安全审计、语义路由
├── plaw-data/              # 运行时数据（Portable，git 忽略）
│   ├── bin/                # Plaw 二进制 + Office CLI
│   ├── embedding/          # 本地 Embedding 模型（llama-server + GGUF）
│   ├── browsers/           # 内置 Chromium headless
│   └── .plaw/              # config.toml、会话数据库、Skills
├── setup.ps1               # 一键环境安装
├── dev.ps1                 # 开发启动
└── build.ps1               # 打包构建
```

## 编译打包

```powershell
# 完整构建（编译 Plaw 引擎 + 打包安装程序）
.\build.ps1

# 跳过 Plaw 编译（自己编译 Plaw 的用户）
.\build.ps1 -NoPlaw
```

`build.ps1` 自动完成：编译 Plaw 引擎 → 部署到 plaw-data/bin/ → 生成资源包 → Tauri 打包。

`-NoPlaw` 适合自行编译 Plaw 引擎的用户，手动将 `plaw.exe` 放到 `plaw-data/bin/` 后直接打包。

输出：`src-tauri/target/release/bundle/nsis/plaw-desktop_x.x.x_x64-setup.exe`

## 配置

应用内 Settings 面板可视化配置所有选项。底层配置文件位于 `plaw-data/.plaw/config.toml`：

```toml
api_key = "sk-xxx"                  # 你自己的 Provider API Key
default_provider = "deepseek"       # 默认推荐，改这一行即可切换 Provider
default_model = "deepseek-v4-pro"

[web_search]
enabled = true
provider = "bing"

[browser]
enabled = true
backend = "rust_native"

[cron]
enabled = true
```

## 常见问题

**Q: API Key 安全吗？会上传到第三方吗？**
不会。Key 以 `Secret` 类型加密落盘（ChaCha20-Poly1305），仅在发请求时解密；请求直连你选的模型厂商（DeepSeek / OpenAI / Anthropic 等），不经过任何中转。

**Q: 支持离线吗？**
AI 对话需要网络（调用云端 API）。文件操作、命令执行等工具离线可用。支持 Ollama 本地模型。

**Q: 怎么迁移到另一台电脑？**
拷贝整个安装目录（含 `plaw-data/`）即可，所有配置、会话、知识库都在里面。

**Q: AI 执行命令安全吗？**
Plaw 内置多层安全：PromptGuard 注入检测、外部内容扫描、反死循环、敏感操作拦截。但 Agent 确实能执行系统命令，请注意审查。

**Q: 国内用户推荐什么模型？**
默认推荐 DeepSeek（deepseek-v4-pro），国内直连无需代理。Kimi、智谱 GLM、通义千问也可直连。切换只改 `config.toml` 一行，不需要改代码。

## 技术栈

- [Tauri 2.0](https://v2.tauri.app/) — 轻量桌面框架（Rust + WebView）
- [Vue 3](https://vuejs.org/) — 前端框架
- [Plaw](plaw/) — Rust AI Agent 引擎
- [Tailwind CSS v4](https://tailwindcss.com/) — 原子化 CSS
- [marked](https://marked.js.org/) — Markdown 渲染
- [Lucide](https://lucide.dev/) — 图标库

## Plaw Elite — 严谨度路线图

Plaw 在做一个长期工程：把自己打磨成桌面 AI agent 的参考实现。完整的设计、调研、阶段计划存在 [`.kiro/specs/plaw-elite/`](.kiro/specs/plaw-elite/)。

**Phase 1: Eval Foundation** 已完成核心实装。每次代码改动都能被科学评估，不靠感觉判断"是更好还是更差"。

- [`plaw-eval`](crates/plaw-eval/) crate — Anthropic 级统计严谨度（95% CI、cluster-robust SE、paired diff、Bradley-Terry MLE）+ 多 judge cross-family jury + G-Eval / 工具调用 / 关键词覆盖等指标。
- [`plaw-eval` CLI](crates/plaw-eval-cli/) — `run / list / compare / power / promote / cache / flywheel / doctor`。
- GitHub Actions [workflow](.github/workflows/plaw-eval.yml) — PR 触发的 smoke eval（n=30）+ 每日 nightly（n=300），全部统计严谨地比对 baseline。
- 完整方法论文档：
  - [methodology.md](docs/eval/methodology.md) — 为什么每个指标这么算
  - [suite-design.md](docs/eval/suite-design.md) — 怎么设计好 case
  - [judge-selection.md](docs/eval/judge-selection.md) — 怎么选 judge
  - [troubleshooting.md](docs/eval/troubleshooting.md) — 常见问题
  - [ci-secrets.md](docs/eval/ci-secrets.md) — CI 密钥配置

后续阶段（Phase 2: Substance Rewrite，Phase 3: Observability）见 [`.kiro/specs/plaw-elite/01-roadmap.md`](.kiro/specs/plaw-elite/01-roadmap.md)。

## License

MIT
