**English** | [中文](README.md)

# Plaw Desktop

**Plaw Desktop** — An open-source, self-hosted desktop AI Agent with the built-in [Plaw](plaw/) autonomous agent engine.

Bring your own API Key and get an AI assistant that can control your computer, browse the web, read/write files, and execute code — running entirely on your machine, with requests going straight to your model provider, no relay in between.

> BYO-key, all runtime dependencies bundled. Engine and frontend are fully open-source, auditable, and hackable — while keeping the one-click, ready-to-use install.

## Acknowledgments

The Plaw engine is built upon [ZeroClaw](https://github.com/ZeroClaw-AI/ZeroClaw). Thanks to [OpenClaw](https://github.com/OpenClaw-AI) and ZeroClaw for their open-source contributions. The codebase has diverged significantly (embedded desktop app, parallel sub-agents, browser automation, security system, hot-reloading skills, etc.), but Plaw wouldn't exist without ZeroClaw's foundation. Long live open source.

The vast majority of code in this project was co-authored with [Claude](https://claude.ai/) (Anthropic), including the Rust backend, Vue frontend, security policies, skills system, and more. Thanks to Claude as the full-time AI pair-programming partner.

---

## Why?

Most AI Agents are either cloud SaaS (your data and keys flow through their servers) or terminal-oriented developer tools (Claude Code, Cursor, Cline).

Plaw Desktop aims for a different shape — **a desktop agent on your own machine, with your own keys, that you can audit and modify**:

- **Open-source & auditable**: engine through frontend are all open — read and change the security policies and tool implementations yourself
- **Self-hosted & BYO-key**: use your own API Key, requests go straight to the provider, no relay
- **Full tool access**: Shell, files, browser, HTTP, Office, Cron — not a stripped-down sandbox
- **Zero external deps**: Chromium, Python, Office, embedding model all bundled — nothing to install
- **One-click install**: keeps the ready-to-use experience; no env setup, config editing, or command line required

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
| **Credential Encryption** | All tokens/secrets stored as a `Secret` type, encrypted at rest (ChaCha20-Poly1305), decrypted only on use, auto-redacted from logs |
| **Command Sandbox** | Shell commands run inside a sandbox; on Windows a Job Object isolates them and terminates child processes when the app exits |
| **Webhook Signatures** | All webhook endpoints are secure-by-default: requests from non-loopback addresses are rejected when no secret is configured |
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
| **DeepSeek** | OpenAI-compatible | deepseek-v4-pro | Direct access in China, **default** |
| Anthropic | Native | Claude Sonnet/Opus | Proxy required |
| OpenAI | Native | GPT-4o | Proxy required |
| Kimi Coder | Anthropic-compatible | k2p5 | Direct access in China |
| Gemini | Native | Gemini Pro | Proxy required |
| Ollama | Local | Any local model | Works offline |
| GLM (Zhipu) | OpenAI-compatible | GLM-4 | Direct access in China |
| Qwen (Tongyi) | OpenAI-compatible | Qwen-Max | Direct access in China |
| Moonshot | OpenAI-compatible | Moonshot-v1 | Direct access in China |
| OpenRouter | OpenAI-compatible | Multi-model router | Proxy required |
| Custom | OpenAI/Anthropic-compatible | - | Any endpoint |

> **Provider-agnostic by design**: Plaw is not specialized for any single model. Switching the default is just editing `default_provider` + `default_model` in `config.toml` — no code changes, no recompile. The current default is DeepSeek V4 Pro (directly reachable in China, strongest among domestic models); this recommendation will evolve as models do.

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
|  |  | DeepSeek| | File   | | Embedding Vector | |  |
|  |  | OpenAI  | | Web    | | Semantic Routing | |  |
|  |  | Anthropic| | Browser| | Skills Hot-load | |  |
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

### Direct Use (download installer)

Download the installer from [Releases](https://github.com/newzlong/plaw-desktop/releases):

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
git clone https://github.com/newzlong/plaw-desktop.git
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
api_key = "sk-xxx"                  # your own provider API Key
default_provider = "deepseek"       # default; change this one line to switch provider
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

## FAQ

**Q: Is my API Key safe? Does it get uploaded to third parties?**
No. Keys are stored as a `Secret` type encrypted at rest (ChaCha20-Poly1305) and decrypted only when a request is made. Requests go directly to your chosen provider (DeepSeek / OpenAI / Anthropic, etc.) — no third-party relay.

**Q: Does it work offline?**
AI chat requires internet (cloud API calls). File operations and shell commands work offline. Ollama local models are supported.

**Q: How to migrate to another PC?**
Copy the entire install directory (including `plaw-data/`). All configs, sessions, and knowledge are inside.

**Q: Is it safe for AI to run commands?**
Plaw has multi-layer security: PromptGuard injection detection, external content scanning, anti-loop, sensitive operation blocking. But the agent can execute system commands — review with care.

**Q: Recommended model for China users?**
DeepSeek (deepseek-v4-pro) by default — direct access in China, no proxy needed. Kimi, GLM, and Qwen also work without proxy. Switching is a one-line change in `config.toml`, no code edits.

## Tech Stack

- [Tauri 2.0](https://v2.tauri.app/) — Lightweight desktop framework (Rust + WebView)
- [Vue 3](https://vuejs.org/) — Frontend framework
- [Plaw](plaw/) — Rust AI Agent engine
- [Tailwind CSS v4](https://tailwindcss.com/) — Utility-first CSS
- [marked](https://marked.js.org/) — Markdown rendering
- [Lucide](https://lucide.dev/) — Icon library

## License

MIT
