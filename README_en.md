**English** | [中文](README.md)

# Plaw Desktop

**Plaw** — A ready-to-use desktop AI Agent with the built-in [Plaw](plaw/) autonomous agent engine.

One-click install, enter your API Key, and you have an AI assistant that can control your computer, browse the web, read/write files, and execute code.

> For everyday users — no Docker, Python, or command-line knowledge required. Install and go.

## Acknowledgments

The Plaw engine is built upon [ZeroClaw](https://github.com/ZeroClaw-AI/ZeroClaw). Thanks to [OpenClaw](https://github.com/OpenClaw-AI) and ZeroClaw for their open-source contributions. The codebase has diverged significantly (embedded desktop app, parallel sub-agents, browser automation, security system, hot-reloading skills, etc.), but Plaw wouldn't exist without ZeroClaw's foundation. Long live open source.

The vast majority of code in this project was co-authored with [Claude](https://claude.ai/) (Anthropic), including the Rust backend, Vue frontend, security policies, skills system, and more. Thanks to Claude as the full-time AI pair-programming partner.

---

## Why?

Existing AI Agent tools (Claude Code, Cursor, Cline, etc.) are all developer-oriented. Regular users simply can't use them.

Plaw's mission: **bring AI Agents to everyone, not just developers.**

- No Python / Docker / WSL installation needed
- No environment variables or config files to set up
- No command-line knowledge required
- Install → enter API Key → start using

## Key Features

### AI Capabilities

| Feature | Description |
|---------|-------------|
| **Autonomous Agent** | AI plans, invokes tools, and reasons through multi-step tasks until completion |
| **Real-time Interrupt** | Interrupt AI anytime — user messages take priority, AI stops immediately |
| **File Understanding** | Send images, PDFs, Excel, Word attachments — AI analyzes them directly |
| **Parallel Execution** | Complex tasks auto-split into parallel sub-agents (parallel_delegate) |
| **Context Compaction** | Long conversations auto-compressed, saving tokens without losing key info |
| **Capsule Memory** | Conversations distilled into capsules, AI remembers across sessions |
| **Agentic Recall** | AI proactively searches past capsules when encountering familiar problems |
| **Semantic Skill Routing** | Each turn semantically matches the most relevant skills instead of injecting all |
| **Skills Extension** | Markdown skill packs extend AI capabilities, hot-reloaded without restart |
| **Streaming Output** | SSE streaming — AI thinking and responses visible in real-time with tool progress |

### Toolbox

| Tool | Capability |
|------|-----------|
| **Shell** | PowerShell / Bash / Cmd, auto-selects optimal shell |
| **File Ops** | Read, write, edit, search, directory browsing |
| **Web Search** | Bing search, direct access in China (no proxy needed) |
| **Web Fetch** | Fetch any webpage, auto-convert to Markdown |
| **Browser** | Built-in Chromium, AI can browse and interact with web pages |
| **HTTP Request** | Connect to any REST API |
| **Capsule Search** | Vector + keyword hybrid search, cross-session knowledge retrieval |
| **Office Docs** | AI generates PPTX / DOCX / XLSX / PDF (built-in Office CLI) |
| **Cron** | Cron expressions, AI runs tasks on schedule |
| **Git** | Version control |
| **PDF / Images** | Read PDFs, analyze image content |

### Security

| Layer | Mechanism |
|-------|-----------|
| **Input Guard** | PromptGuard prompt injection detection |
| **External Scan** | Auto-flags injection attacks in web/search results |
| **Anti-Loop** | Repeated tool call detection + iteration limits |
| **Config Protection** | API Key encrypted locally, sensitive operations blocked |
| **Skills Audit** | Auto-scans skill files, blocks suspicious scripts and injection patterns |
| **Privacy First** | All data stored locally, API Key connects directly to provider, no third-party relay |

### Desktop Experience

| Feature | Description |
|---------|-------------|
| **Glassmorphism UI** | Dark theme, custom components, no UI library dependencies |
| **Portable** | Data stored alongside install directory, USB-portable |
| **Bilingual** | Chinese and English UI |
| **Setup Wizard** | First-launch guided setup, beginner-friendly |
| **Sessions** | Multi-session, history, tool execution steps persisted |
| **Live Progress** | Tool execution progress shown in real-time |

## Supported AI Models

| Provider | Format | Recommended Model | Notes |
|----------|--------|-------------------|-------|
| **Kimi Coder** | Anthropic-compatible | k2p5 | Direct access in China, recommended |
| Anthropic | Native | Claude Sonnet/Opus | Proxy required |
| OpenAI | Native | GPT-4o | Proxy required |
| DeepSeek | OpenAI-compatible | DeepSeek-V3 | Direct access in China |
| Gemini | Native | Gemini Pro | Proxy required |
| Ollama | Local | Any local model | Works offline |
| GLM (Zhipu) | OpenAI-compatible | GLM-4 | Direct access in China |
| Qwen (Tongyi) | OpenAI-compatible | Qwen-Max | Direct access in China |
| Moonshot | OpenAI-compatible | Moonshot-v1 | Direct access in China |
| OpenRouter | OpenAI-compatible | Multi-model router | Proxy required |
| Custom | OpenAI/Anthropic-compatible | - | Any endpoint |

> **Note**: Only **Kimi Coder (k2p5)** has been fully tested by the developer. Other providers should work in theory but are unverified — if you encounter network or format issues, feel free to check the source code and submit a PR.

## Architecture

```
+---------------------------------------------------+
|                 Plaw Desktop                       |
|  +-----------+-----------+-----------+----------+  |
|  |   Chat    | Capsules  | Settings  |  Cron    |  |
|  |           | (Memory)  |           | (Sched.) |  |
|  +-----------+-----------+-----------+----------+  |
|  |           Vue 3 + Glassmorphism UI            |  |
|  +---------------------+------------------------+  |
|                        | WebSocket (SSE Stream)      |
|  +---------------------+------------------------+  |
|  |              Plaw Agent Engine                |  |
|  |  +---------+ +--------+ +------------------+ |  |
|  |  | Provider| | Tools  | | Memory & Skills  | |  |
|  |  | 10+ LLM | | Shell  | | Capsule (SQLite) | |  |
|  |  | Anthropic| | File  | | Embedding Vector | |  |
|  |  | OpenAI  | | Web    | | Semantic Routing | |  |
|  |  | Kimi    | | Browser| | Skills Hot-load  | |  |
|  |  | Custom  | | Office | | Agentic Recall   | |  |
|  |  +---------+ +--------+ +------------------+ |  |
|  +-----------------------------------------------+  |
|  +-----------------------------------------------+  |
|  |          Tauri 2.0 (Rust Backend)             |  |
|  |  Process Mgmt / Config / Embedding Server     |  |
|  +-----------------------------------------------+  |
+---------------------------------------------------+
```

| Layer | Technology | Description |
|-------|-----------|-------------|
| Desktop | **Tauri 2.0** | Rust backend + system WebView, ~50MB installer |
| AI Engine | **Plaw** | Pure Rust autonomous agent, 16MB single binary |
| Frontend | **Vue 3** | Glassmorphism UI, custom components, zero UI library deps |
| Communication | **WebSocket** | Bidirectional real-time, SSE streaming |
| Memory | **SQLite + Embedding** | Capsule memory + FTS5 keywords + vector hybrid search |
| Embedding | **llama-server** | Local 768-dim vectors, GemmaEmbedding 300M quantized |
| Config | **TOML** | Human-readable, visual editor in-app |

## Quick Start

### End Users

Download the installer from [Releases](https://github.com/gfisrubbish/plaw-desktop/releases):

- **Windows**: `plaw-desktop_x.x.x_x64-setup.exe`

Install → launch → Setup Wizard guides you to choose a model and enter your API Key → done, start chatting.

> **Why is the installer so large? (~800MB)** Plaw aims for "install and go, zero config", so all runtime environments are bundled — users don't need to install anything:
>
> | Component | Size | Purpose |
> |-----------|------|---------|
> | Chromium Headless | ~260MB | Browser automation (AI browses the web) |
> | Python 3 + libs | ~130MB | Office document generation, data processing |
> | LibreOffice Portable | ~210MB | DOCX/XLSX/PDF conversion |
> | Node.js | ~80MB | PPTX generation, browser daemon |
> | Embedding Model | ~315MB | Local semantic search (capsule memory, skill routing) |
> | Plaw Engine | ~16MB | AI Agent core |
> | Skills Pack | ~14MB | 30+ pre-installed AI skills |
>
> All bundled for a zero-config experience. Developers can trim as needed.

### Developers

```powershell
git clone https://github.com/gfisrubbish/plaw-desktop.git
cd plaw-desktop
.\setup.ps1     # Install all dependencies + compile Plaw engine
.\dev.ps1       # Start dev (Vite + Tauri hot-reload)
```

`setup.ps1` auto-completes 7 steps:

| Step | Task | Method |
|------|------|--------|
| 1 | Rust toolchain | rustup |
| 2 | Node.js | winget |
| 3 | pnpm | npm |
| 4 | Tauri CLI | cargo |
| 5 | Frontend deps | pnpm install |
| 6 | plaw-data dir + default config | auto-create |
| 7 | Compile Plaw engine + deploy | cargo build --release |

Idempotent — run as many times as you want, completed steps are skipped.

Launch the app → Setup Wizard to enter API Key → ready to use.

## Project Structure

```
plaw-desktop/
├── src-tauri/              # Tauri Rust backend
│   └── src/
│       ├── lib.rs          # Portable data dir, port allocation, config R/W
│       └── plaw.rs         # Plaw process management
├── web/                    # Vue 3 frontend
│   └── src/
│       ├── views/          # Chat, Dashboard, Settings, SetupWizard, etc.
│       ├── components/     # GlassDialog, SettingsPanel, custom components
│       ├── composables/    # usePlawState, useI18n, etc.
│       └── api/            # Tauri commands + WebSocket wrapper
├── plaw/                   # Plaw AI engine (full Rust source)
│   └── src/
│       ├── agent/          # Agent loop, tool calls, context compaction
│       ├── providers/      # 10+ LLM provider implementations
│       ├── tools/          # 50+ tools (shell, file, browser, search, etc.)
│       ├── security/       # Security (PromptGuard, audit, sandbox)
│       ├── gateway/        # WebSocket gateway
│       ├── memory/         # Capsule memory (SQLite + FTS5 + embedding vector hybrid)
│       └── skills/         # Skills loading, security audit, semantic routing
├── plaw-data/              # Runtime data (portable, git-ignored)
│   ├── bin/                # Plaw binary + Office CLI
│   ├── embedding/          # Local embedding model (llama-server + GGUF)
│   ├── browsers/           # Built-in Chromium headless
│   └── .plaw/              # config.toml, session DB, skills
├── setup.ps1               # One-click environment setup
├── dev.ps1                 # Dev launcher
└── build.ps1               # Build & package
```

## Building

```powershell
# Full build (compile Plaw engine + package installer)
.\build.ps1

# Skip Plaw compilation (for users who compile Plaw separately)
.\build.ps1 -NoPlaw
```

`build.ps1` auto-completes: compile Plaw → deploy to plaw-data/bin/ → generate bundles → Tauri package.

`-NoPlaw` is for users who compile the Plaw engine themselves — place `plaw.exe` in `plaw-data/bin/` then package.

Output: `src-tauri/target/release/bundle/nsis/plaw-desktop_x.x.x_x64-setup.exe`

## Configuration

All options are configurable via the in-app Settings panel. The underlying config file is at `plaw-data/.plaw/config.toml`:

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

## FAQ

**Q: Is my API Key safe? Does it get uploaded to third parties?**
No. Keys are encrypted locally. Requests go directly to your chosen provider (Kimi / OpenAI / Anthropic) — no third-party relay.

**Q: Does it work offline?**
AI chat requires internet (cloud API calls). File operations and shell commands work offline. Ollama local models are supported.

**Q: How to migrate to another PC?**
Copy the entire install directory (including `plaw-data/`). All configs, sessions, and knowledge are inside.

**Q: Is it safe for AI to run commands?**
Plaw has multi-layer security: PromptGuard injection detection, external content scanning, anti-loop, sensitive operation blocking. But the agent can execute system commands — review with care.

**Q: Recommended model for China users?**
Kimi Coder (k2p5) — direct access in China, no proxy needed, Anthropic-compatible API. DeepSeek, GLM, Qwen also work without proxy.

## Tech Stack

- [Tauri 2.0](https://v2.tauri.app/) — Lightweight desktop framework (Rust + WebView)
- [Vue 3](https://vuejs.org/) — Frontend framework
- [Plaw](plaw/) — Rust AI Agent engine
- [Tailwind CSS v4](https://tailwindcss.com/) — Utility-first CSS
- [marked](https://marked.js.org/) — Markdown rendering
- [Lucide](https://lucide.dev/) — Icon library

## License

MIT
