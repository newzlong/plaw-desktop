# Plaw Desktop

**Plaw** — 开箱即用的桌面 AI Agent，内嵌 [Plaw](plaw/) 自主智能体引擎。

一键安装，填入 API Key，即可拥有一个能操作电脑、浏览网页、读写文件、执行代码的 AI 助手。

> 面向普通用户，不需要 Docker、Python、命令行知识。安装即用。

**Plaw** — A ready-to-use desktop AI Agent with the built-in [Plaw](plaw/) autonomous agent engine.

One-click install, enter your API Key, and you have an AI assistant that can control your computer, browse the web, read/write files, and execute code.

> For everyday users — no Docker, Python, or command-line knowledge required. Install and go.

## 致谢 / Acknowledgments

Plaw 引擎基于 [ZeroClaw](https://github.com/ZeroClaw-AI/ZeroClaw) 改造而来，感谢 [OpenClaw](https://github.com/OpenClaw-AI) 和 ZeroClaw 的开源贡献。

虽然改到现在代码已经面目全非（内嵌桌面端、并行子 Agent、浏览器自动化、安全防护体系、Skills 热加载……），但没有 ZeroClaw 打下的基础就没有 Plaw。开源精神万岁。

The Plaw engine is built upon [ZeroClaw](https://github.com/ZeroClaw-AI/ZeroClaw). Thanks to [OpenClaw](https://github.com/OpenClaw-AI) and ZeroClaw for their open-source contributions. The codebase has diverged significantly (embedded desktop app, parallel sub-agents, browser automation, security system, hot-reloading skills, etc.), but Plaw wouldn't exist without ZeroClaw's foundation. Long live open source.

本项目的绝大部分代码由 [Claude](https://claude.ai/)（Anthropic）辅助编写，包括 Rust 后端、Vue 前端、安全策略、Skills 系统等。感谢 Claude 作为全程 AI 结对编程伙伴的贡献。

The vast majority of code in this project was co-authored with [Claude](https://claude.ai/) (Anthropic), including the Rust backend, Vue frontend, security policies, skills system, and more. Thanks to Claude as the full-time AI pair-programming partner.

---

## 为什么做这个？ / Why?

市面上的 AI Agent 工具（Claude Code、Cursor、Cline 等）都面向开发者，普通用户根本用不了。

Plaw 要解决的问题：**让不懂技术的人也能用上 AI Agent**。

- 不需要装 Python / Docker / WSL
- 不需要配环境变量、改配置文件
- 不需要懂命令行
- 安装 → 填 Key → 直接用

Existing AI Agent tools (Claude Code, Cursor, Cline, etc.) are all developer-oriented. Regular users simply can't use them.

Plaw's mission: **bring AI Agents to everyone, not just developers.**

- No Python / Docker / WSL installation needed
- No environment variables or config files to set up
- No command-line knowledge required
- Install → enter API Key → start using

## 核心特性 / Key Features

### AI 能力 / AI Capabilities

| 特性 Feature | 说明 Description |
|------|------|
| **自主 Agent** Autonomous Agent | AI 自主规划、调用工具、多步推理，直到任务完成 / AI plans, invokes tools, and reasons through multi-step tasks autonomously |
| **实时打断** Real-time Interrupt | 随时发消息打断 AI，用户优先 / Interrupt AI anytime, user messages take priority |
| **文件理解** File Understanding | 发送图片、PDF、Excel、Word 等附件，AI 直接分析 / Send images, PDFs, Excel, Word — AI analyzes them directly |
| **并行执行** Parallel Execution | 复杂任务自动拆分为多个子 Agent 并行处理 / Complex tasks auto-split into parallel sub-agents |
| **上下文压缩** Context Compaction | 长对话自动压缩，不丢关键信息，节省 Token / Long conversations auto-compressed, saving tokens without losing key info |
| **胶囊记忆** Capsule Memory | 对话自动沉淀为胶囊，AI 跨会话记住你的偏好和历史 / Conversations distilled into capsules, AI remembers across sessions |
| **主动回忆** Agentic Recall | AI 遇到似曾相识的问题时，主动搜索历史胶囊 / AI proactively searches past capsules for relevant context |
| **语义 Skill 路由** Semantic Skill Routing | 每轮对话根据语义匹配最相关的 Skills / Each turn semantically matches the most relevant skills |
| **Skills 扩展** Skills Extension | Markdown 技能包扩展 AI 能力，热加载 / Markdown skill packs extend AI capabilities, hot-reloaded |
| **流式输出** Streaming Output | SSE 流式传输，AI 思考和回复实时可见 / SSE streaming, AI thinking and responses visible in real-time |

### 工具箱 / Toolbox

| 工具 Tool | 能力 Capability |
|------|------|
| **Shell 执行** Shell | PowerShell / Bash / Cmd, auto-selects optimal shell |
| **文件操作** File Ops | Read, write, edit, search, directory browsing |
| **网络搜索** Web Search | Bing search, direct access in China (no proxy needed) |
| **网页抓取** Web Fetch | Fetch any webpage, auto-convert to Markdown |
| **浏览器自动化** Browser | Built-in Chromium, AI can browse and interact with web pages |
| **API 调用** HTTP Request | HTTP requests, connect to any REST API |
| **胶囊记忆搜索** Capsule Search | Vector + keyword hybrid search, cross-session knowledge retrieval |
| **Office 文档** Office Docs | AI generates PPTX / DOCX / XLSX / PDF (built-in Office CLI) |
| **定时任务** Cron | Cron expressions, AI runs tasks on schedule |
| **Git 操作** Git | Version control |
| **PDF / 图片** PDF/Images | Read PDFs, analyze image content |

### 安全防护 / Security

| 层级 Layer | 机制 Mechanism |
|------|------|
| **输入防护** Input Guard | PromptGuard prompt injection detection |
| **外部内容扫描** External Scan | Auto-flags injection attacks in web/search results |
| **反死循环** Anti-Loop | Repeated tool call detection + iteration limits |
| **配置保护** Config Protection | API Key encrypted locally, sensitive operations blocked |
| **Skills 安全审计** Skills Audit | Auto-scans skill files, blocks suspicious scripts and injection patterns |
| **隐私优先** Privacy First | All data stored locally, API Key connects directly to provider, no third-party relay |

### 桌面体验 / Desktop Experience

| 特性 Feature | 说明 Description |
|------|------|
| **毛玻璃 UI** Glassmorphism | Dark theme, custom components, no UI library dependencies |
| **Portable 模式** Portable | Data stored alongside install directory, USB-portable |
| **中英双语** Bilingual | Chinese and English UI |
| **Setup Wizard** | First-launch guided setup, beginner-friendly |
| **会话管理** Sessions | Multi-session, history, tool execution steps persisted |
| **实时进度** Live Progress | Tool execution progress shown in real-time |

## 支持的 AI 模型 / Supported AI Models

| Provider | 格式 | 推荐模型 | 备注 |
|----------|------|----------|------|
| **Kimi Coder** | Anthropic 兼容 | k2p5 | 国内直连，推荐 |
| Anthropic | 原生 | Claude Sonnet/Opus | 需代理 |
| OpenAI | 原生 | GPT-4o | 需代理 |
| DeepSeek | OpenAI 兼容 | DeepSeek-V3 | 国内直连 |
| Gemini | 原生 | Gemini Pro | 需代理 |
| Ollama | 本地 | 任意本地模型 | 离线可用 |
| GLM (智谱) | OpenAI 兼容 | GLM-4 | 国内直连 |
| Qwen (通义) | OpenAI 兼容 | Qwen-Max | 国内直连 |
| Moonshot | OpenAI 兼容 | Moonshot-v1 | 国内直连 |
| OpenRouter | OpenAI 兼容 | 多模型路由 | 需代理 |
| 自定义 | OpenAI/Anthropic 兼容 | - | 支持任意端点 |

> **注意**：开发者目前只用 **Kimi Coder (k2p5)** 做过完整测试。其他 Provider 理论上可用但未经验证——如果遇到网络连接或消息格式问题，欢迎自行查看源码修复并提 PR。
>
> **Note**: Only **Kimi Coder (k2p5)** has been fully tested by the developer. Other providers should work in theory but are unverified — if you encounter network or format issues, feel free to check the source code and submit a PR.

## 架构 / Architecture

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
|  |  | Anthropic| | File  | | Embedding 向量   | |  |
|  |  | OpenAI  | | Web    | | Semantic Routing | |  |
|  |  | Kimi    | | Browser| | Skills 热加载    | |  |
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

## 快速开始 / Quick Start

### 普通用户 / End Users

从 [Releases](https://github.com/gfisrubbish/plaw-desktop/releases) 下载安装包：
Download the installer from [Releases](https://github.com/gfisrubbish/plaw-desktop/releases):

- **Windows**: `plaw-desktop_x.x.x_x64-setup.exe`

安装后启动 → Setup Wizard 引导你选择模型、填入 API Key → 完成，开始对话。

Install → launch → Setup Wizard guides you to choose a model and enter your API Key → done, start chatting.

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
> 这些都是为了让普通用户开箱即用。如果你是开发者，可以按需裁剪。
>
> All bundled for a zero-config experience. Developers can trim as needed.

### 开发者 / Developers

```powershell
git clone https://github.com/gfisrubbish/plaw-desktop.git
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

## 项目结构 / Project Structure

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

## 编译打包 / Building

```powershell
# 完整构建（编译 Plaw 引擎 + 打包安装程序）
.\build.ps1

# 跳过 Plaw 编译（自己编译 Plaw 的用户）
.\build.ps1 -NoPlaw
```

`build.ps1` 自动完成：编译 Plaw 引擎 → 部署到 plaw-data/bin/ → 生成资源包 → Tauri 打包。

`-NoPlaw` 适合自行编译 Plaw 引擎的用户，手动将 `plaw.exe` 放到 `plaw-data/bin/` 后直接打包。

输出：`src-tauri/target/release/bundle/nsis/plaw-desktop_x.x.x_x64-setup.exe`

## 配置 / Configuration

应用内 Settings 面板可视化配置所有选项。底层配置文件位于 `plaw-data/.plaw/config.toml`：

```toml
api_key = "sk-xxx"
default_provider = "anthropic-custom:https://api.kimi.com/coding"
default_model = "k2p5"

[web_search]
enabled = true
provider = "bing"

[browser]
enabled = true
backend = "rust_native"

[cron]
enabled = true
```

## 常见问题 / FAQ

**Q: API Key 安全吗？会上传到第三方吗？ / Is my API Key safe?**
不会。Key 加密存储在本地，请求直连你选的模型厂商（Kimi / OpenAI / Anthropic），不经过任何中转。
No. Keys are encrypted locally. Requests go directly to your chosen provider — no third-party relay.

**Q: 支持离线吗？ / Does it work offline?**
AI 对话需要网络（调用云端 API）。文件操作、命令执行等工具离线可用。支持 Ollama 本地模型。
AI chat requires internet (cloud API calls). File operations and shell commands work offline. Ollama local models are supported.

**Q: 怎么迁移到另一台电脑？ / How to migrate to another PC?**
拷贝整个安装目录（含 `plaw-data/`）即可，所有配置、会话、知识库都在里面。
Copy the entire install directory (including `plaw-data/`). All configs, sessions, and knowledge are inside.

**Q: AI 执行命令安全吗？ / Is it safe for AI to run commands?**
Plaw 内置多层安全：PromptGuard 注入检测、外部内容扫描、反死循环、敏感操作拦截。但 Agent 确实能执行系统命令，请注意审查。
Plaw has multi-layer security: PromptGuard injection detection, external content scanning, anti-loop, sensitive operation blocking. But the agent can execute system commands — review with care.

**Q: 国内用户推荐什么模型？ / Recommended model for China users?**
推荐 Kimi Coder（k2p5），国内直连无需代理，API 兼容 Anthropic 格式。DeepSeek、智谱 GLM、通义千问也可直连。
Kimi Coder (k2p5) — direct access in China, no proxy needed, Anthropic-compatible API. DeepSeek, GLM, Qwen also work without proxy.

## 技术栈 / Tech Stack

- [Tauri 2.0](https://v2.tauri.app/) — 轻量桌面框架（Rust + WebView）
- [Vue 3](https://vuejs.org/) — 前端框架
- [Plaw](plaw/) — Rust AI Agent 引擎
- [Tailwind CSS v4](https://tailwindcss.com/) — 原子化 CSS
- [marked](https://marked.js.org/) — Markdown 渲染
- [Lucide](https://lucide.dev/) — 图标库

## License

MIT
