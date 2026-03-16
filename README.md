# Plaw Desktop

**Plaw** — 开箱即用的桌面 AI Agent，内嵌 [Plaw](plaw/) 自主智能体引擎。

一键安装，填入 API Key，即可拥有一个能操作电脑、浏览网页、读写文件、执行代码的 AI 助手。

> 面向普通用户，不需要 Docker、Python、命令行知识。安装即用。

## 致谢

Plaw 引擎基于 [ZeroClaw](https://github.com/ZeroClaw-AI/ZeroClaw) 改造而来，感谢 [OpenClaw](https://github.com/OpenClaw-AI) 和 ZeroClaw 的开源贡献。

虽然改到现在代码已经面目全非（内嵌桌面端、并行子 Agent、浏览器自动化、安全防护体系、Skills 热加载……），但没有 ZeroClaw 打下的基础就没有 Plaw。开源精神万岁。

---

## 为什么做这个？

市面上的 AI Agent 工具（Claude Code、Cursor、Cline 等）都面向开发者，普通用户根本用不了。

Plaw 要解决的问题：**让不懂技术的人也能用上 AI Agent**。

- 不需要装 Python / Docker / WSL
- 不需要配环境变量、改配置文件
- 不需要懂命令行
- 安装 → 填 Key → 直接用

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
| **配置保护** | API Key 加密存储，敏感操作拦截 |
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

## 快速开始

### 普通用户

从 [Releases](https://github.com/gfisrubbish/plaw-desktop/releases) 下载安装包：

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
> 这些都是为了让普通用户开箱即用。如果你是开发者，可以按需裁剪。

### 开发者

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

## 常见问题

**Q: API Key 安全吗？会上传到第三方吗？**
不会。Key 加密存储在本地，请求直连你选的模型厂商（Kimi / OpenAI / Anthropic），不经过任何中转。

**Q: 支持离线吗？**
AI 对话需要网络（调用云端 API）。文件操作、命令执行等工具离线可用。支持 Ollama 本地模型。

**Q: 怎么迁移到另一台电脑？**
拷贝整个安装目录（含 `plaw-data/`）即可，所有配置、会话、知识库都在里面。

**Q: AI 执行命令安全吗？**
Plaw 内置多层安全：PromptGuard 注入检测、外部内容扫描、反死循环、敏感操作拦截。但 Agent 确实能执行系统命令，请注意审查。

**Q: 国内用户推荐什么模型？**
推荐 Kimi Coder（k2p5），国内直连无需代理，API 兼容 Anthropic 格式。DeepSeek、智谱 GLM、通义千问也可直连。

## 技术栈

- [Tauri 2.0](https://v2.tauri.app/) — 轻量桌面框架（Rust + WebView）
- [Vue 3](https://vuejs.org/) — 前端框架
- [Plaw](plaw/) — Rust AI Agent 引擎
- [Tailwind CSS v4](https://tailwindcss.com/) — 原子化 CSS
- [marked](https://marked.js.org/) — Markdown 渲染
- [Lucide](https://lucide.dev/) — 图标库

## License

MIT
