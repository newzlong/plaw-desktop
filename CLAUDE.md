# CLAUDE.md — Plaw Desktop (Plaw 桌面 AI Agent)

## 项目概述

Plaw Desktop 是一个独立的桌面 AI Agent 应用，内嵌 Plaw 引擎。面向下沉市场用户，傻瓜式安装、配置、使用。

**定位**：Plaw 的消费级桌面前端，与数字人平台(traexhs)完全独立。

## Tech Stack

| 层级 | 技术 | 备注 |
|------|------|------|
| 桌面框架 | Tauri 2.0 | Rust 后端 + WebView |
| AI 引擎 | Plaw | Rust sidecar 二进制，源码在 `plaw/` |
| 前端 | Vue 3 + Tailwind CSS v4 | **纯自定义组件，不使用 Element Plus** |
| UI 风格 | 毛玻璃 Glassmorphism | 深色背景 + 半透明卡片 |
| 配置格式 | TOML | Plaw 的 config.toml |
| 路由 | Vue Router 4 | Hash 或 History mode |

## 目录结构

```
plaw-desktop/
├── src-tauri/              # Tauri Rust 后端
│   ├── Cargo.toml          # plaw-desktop v0.1.0
│   ├── tauri.conf.json     # 端口 5800
│   └── src/
│       ├── main.rs
│       ├── lib.rs          # Portable 数据目录 + 端口分配 + config 读写
│       └── plaw.rs     # Plaw 进程管理（待实现）
├── web/                    # Vue 3 前端
│   ├── package.json
│   ├── vite.config.js
│   └── src/
│       ├── main.js         # 路由定义
│       ├── style.css       # Glassmorphism CSS (glass, glass-card, glass-btn)
│       ├── App.vue         # 毛玻璃侧边栏布局
│       └── views/          # Dashboard, SetupWizard, ProviderConfig, Logs
├── plaw/               # Plaw 完整源码（从 traexhs 复制，独立管理）
├── dev.ps1                 # 开发启动脚本
└── .gitignore
```

## 关键设计决策

### Portable 模式
- 数据目录：exe 同级的 `plaw-data/`（非系统 AppData）
- 启动 Plaw 时设 `HOME=plaw-data`，使其读取 `$HOME/.plaw/config.toml`
- dev 模式用 `CARGO_MANIFEST_DIR` 的父目录下的 `plaw-data/`
- 支持多实例互不冲突

### 动态端口
- 每次启动 `TcpListener::bind("127.0.0.1:0")` 分配可用端口
- 写入 config.toml 供 Plaw 使用

### Rust 层定位
- **仅做胶水逻辑**：进程管理、端口分配、文件 I/O、日志捕获
- 业务逻辑全在前端
- 子进程用 `tokio::process::Command`（UTF-8 友好）

### UI 规范
- **不使用任何 UI 组件库**，全部自定义
- Glassmorphism 风格：`backdrop-filter: blur()` + 半透明背景 + 细边框
- CSS 类：`.glass`, `.glass-card`, `.glass-btn`, `.glass-btn-primary`, `.glass-input`
- 深色渐变背景：`#0f172a → #1e293b`

## 开发命令

```powershell
# 首次安装依赖
cd web; pnpm install

# 开发模式（启动 Vite + Tauri）
.\dev.ps1
# 或手动
cd src-tauri; cargo tauri dev

# 编译 Plaw
cd plaw; cargo build --release

# 打包
cd src-tauri; cargo tauri build
```

## 开发端口

| 端口 | 服务 |
|------|------|
| 5800 | Vite 前端开发服务器 |
| 动态 | Plaw Gateway（每次启动自动分配） |

## 设计文档

完整的 Kiro 设计规格在 `.kiro/specs/plaw-desktop/`：
- `requirements.md` — 17 条需求（Portable、Setup Wizard、Provider、Channel、Skills、Agents、Cron 等）
- `design.md` — 架构图、启动流程、组件接口、正确性属性

## AI 模型配置（Kimi K2.5 直连）

Plaw Desktop 直连 Kimi Coder 的 Kimi K2.5 API，**不走 ECS 中转**（与数字人平台不同）。
用户在 Setup Wizard 中填入自己的 Kimi API Key。

### Plaw config.toml 关键配置
```toml
api_key = "<用户的 Kimi API Key>"                          # 真实 Key，如 sk-xxx
default_provider = "anthropic-custom:https://api.moonshot.cn"  # Kimi 官方 API（Anthropic 兼容格式）
default_model = "kimi-k2.5"
default_temperature = 0.7

[provider]
reasoning_level = "medium"
```

### 链路（直连，无中转）
```
Plaw (x-api-key: sk-xxx)
  → POST https://api.moonshot.cn/v1/messages
  → Kimi K2.5 (Anthropic 兼容格式)
  → 流式 SSE 回传
```

### 要点
- `api_key` = 用户自己的 Kimi API Key（sk-xxx），直接发给 Kimi
- Provider 格式：`anthropic-custom:<base_url>`，Plaw 自动拼 `/v1/messages`
- Kimi Coder 的 API 兼容 Anthropic Messages 格式
- 模型名 `kimi-k2.5`（Kimi K2.5 Coder）
- Web Search 用 Bing RSS（中国可用，无需 VPN）
- 也可支持其他 Provider（OpenAI、Anthropic、DeepSeek 等），在 Setup Wizard 选择

### Setup Wizard 流程
1. 选择 Provider（默认 Kimi Coder）
2. 填入 API Key（sk-xxx）
3. 可选：测试连接（发一条测试请求验证 Key 有效）
4. 生成 config.toml → 启动 Plaw

## Plaw WebSocket 协议（聊天 + 工具执行状态）

前端直连 Plaw WebSocket：`ws://127.0.0.1:{port}/ws/chat`

### 消息类型（Plaw → 前端）

| type | 含义 | 字段 |
|------|------|------|
| `chunk` | 流式文本片段 | `content: string` |
| `thinking` | AI 正在思考/规划 | `content: string`（思考内容摘要） |
| `tool_call` | 开始执行工具 | `name: string, args: object` |
| `tool_result` | 工具执行完成 | `name: string, output: string` |
| `done` | 回复完成 | `full_response: string, usage: object` |
| `error` | 错误 | `message: string` |

### 前端 → Plaw

```json
{"type": "message", "content": "用户输入的文本"}
{"type": "cancel"}  // 中断当前 agent loop
```

### 工具执行状态 UI 实现要点

1. 收到 `thinking` → 显示"🤔 {hint}"卡片（如"正在分析代码..."）
2. 收到 `tool_call` → 清除 thinking 卡片，添加工具卡片（⏳ 执行中）
3. 收到 `tool_result` → 匹配同名工具卡片，标记 ✅ 完成
4. 工具卡片可折叠，展开后显示参数和结果
5. 收到 `done` → 清除所有工具卡片

### 常见工具名

Plaw 的工具包括：`shell`、`read_file`、`write_file`、`edit_file`、`list_dir`、`search`、`browser_navigate`、`browser_click`、`http_request` 等

### 停止按钮

发送 `{"type": "cancel"}` → Plaw 用 CancellationToken 中断 agent loop → 前端显示"AI 回复被中断"

## Plaw 工具配置（config.toml 各段）

### Web Search（网络搜索）
```toml
[web_search]
enabled = true
provider = "bing"       # Bing RSS，中国可用无需 VPN；备选 "duckduckgo"（需代理）
max_results = 5
timeout_secs = 30
```

### Web Fetch（抓取网页内容）
```toml
[web_fetch]
enabled = true
provider = "fast_html2md"   # HTML → Markdown 转换
allowed_domains = ["*"]     # 允许抓取所有域名
max_response_size = 524288  # 512KB
timeout_secs = 30
```

### HTTP Request（API 调用）
```toml
[http_request]
enabled = true
allowed_domains = ["localhost", "127.0.0.1"]  # 默认只允许本地
allow_local = true
max_response_size = 1048576  # 1MB
timeout_secs = 120
```

### Browser（浏览器自动化）
```toml
[browser]
enabled = true
backend = "rust_native"     # Plaw 内置 Rust 浏览器引擎
native_headless = true      # 无头模式（用户看不到浏览器窗口）
browser_open = "disable"    # 禁止自动打开浏览器
native_chrome_path = "..."  # 可选：指定 Chrome/Chromium 路径
```
注意：browser 需要 `--features browser-native` 编译 Plaw

### Proxy（代理）
```toml
[proxy]
enabled = true/false        # 自动从环境变量检测
all_proxy = "http://..."    # HTTPS_PROXY / HTTP_PROXY / ALL_PROXY
scope = "environment"
```
自动检测优先级：HTTPS_PROXY → https_proxy → ALL_PROXY → HTTP_PROXY → http_proxy

### 其他配置段
```toml
[skills]
open_skills_enabled = false
prompt_injection_mode = "full"

[hooks]
enabled = true
[hooks.builtin]
command_logger = true

[scheduler]
enabled = true
max_tasks = 100
max_concurrent = 1

[cron]
enabled = true
max_run_history = 50
```

## 与数字人项目(traexhs)的关系

- **完全独立**：独立 Git 仓库，独立版本号
- Plaw 源码从 traexhs 复制过来，之后独立演进
- 不共享 XHS MCP、Go 后端、Electron 等组件
- Tauri 经验可复用（tauri.conf.json 格式、进程管理模式、打包流程）

## 代理配置

Claude Code 使用 HTTP 代理：`http://127.0.0.1:8118`

## 注意事项

- Windows 平台开发，shell 用 bash 语法（Git Bash）
- 写大文件时分段写（代理对大响应不稳定，超 100 行要拆分）
- `beforeDevCommand` 用对象格式 `{ "script": "pnpm dev", "cwd": "../web" }`
- Windows 进程管理用 `powershell Stop-Process` 而非 bash `kill`
- 打包前需要 `$env:HTTPS_PROXY = "http://127.0.0.1:8118"`（NSIS/WiX 从 GitHub 下载）
- release 模式无 console 输出（`windows_subsystem = "windows"`），需文件日志调试
