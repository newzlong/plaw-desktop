# 跑 Baseline — T11.7 工作流

> 给 plaw 的 `chat_quality` + `tool_routing` 两个 suite 跑一次 baseline，
> 数字写到 `docs/eval/baseline-2026-Q2.md`，作为后续 PR 回归 gate 的对照。
>
> 估计时间：30-40 分钟（人工干预 < 5 分钟）。
> 估计成本：~¥3-5（按 Kimi Coder 计费）。

## 前提

- plaw-desktop 已经过 Setup Wizard，能正常对话（说明 plaw + Kimi 链路通）
- 工作目录在仓库根：`d:\work\develop\plaw\plaw-desktop\`

## 流程

### 1. 启动 plaw（plaw-desktop）

```powershell
.\dev.ps1
```

等到看到 plaw-desktop 窗口出来 + 能正常对话。这时候 `plaw-data/port-state.json`
已经有 plaw 的实际端口了。**保持它运行，下面的命令在另一个 powershell 里跑。**

验证：

```powershell
cargo run --release -p plaw-eval-cli -- doctor
```

应当看到：
- `suites directory : evals — ok (5 suite(s))`
- `plaw WS endpoint : ws://127.0.0.1:NNNN/ws/chat`（NNNN 是动态分配的端口，**不是 5800**）

### 2. 准备 Kimi API Key

plaw-desktop 把 API key 加密存在 `plaw-data/.plaw/config.toml`，
plaw-eval 没法直接读。最简单的办法：用同一个 key 设环境变量。

```powershell
$env:KIMI_API_KEY = "sk-你的-kimi-coder-api-key"
```

如果你 Setup Wizard 时记不清填的哪个，去 Kimi 的开发者后台重新建一个：
https://platform.moonshot.cn/console/api-keys

### 3. 跑 baseline

n=30 是 smoke 规模，跑得快、能看出大趋势：

```powershell
cargo run --release -p plaw-eval-cli -- run `
  --suite chat_quality `
  --suite tool_routing `
  --n 30 `
  --output target/reports/baseline-smoke.json
```

进度条会显示 case 完成情况。卡某个 case 的话 Ctrl+C 中断，已完成的会写库。

### 4. 看结果

```powershell
cargo run --release -p plaw-eval-cli -- list --detail --limit 5
```

每个 metric 应该有：
- `mean` — 平均分（0-1 区间，越高越好）
- `ci_lower` / `ci_upper` — 95% 置信区间
- `n` — 样本数

**审什么数字**：

| 看到这个 | 想想是不是 |
|---------|----------|
| `mean = 0.95+` 接近满分 | suite 太简单了，gate 不上灵敏度 |
| `mean = 0.30-` 异常低 | suite 设计偏了（不是 plaw 烂） |
| `ci_upper - ci_lower > 0.3` | 样本太小，加大 `--n` |
| 某个 metric 全部 case 都是 1.0 | metric 阈值太宽，没区分度 |
| `n_failed > 0` | plaw 调用 / judge 调用有错 — 看日志 |

### 5. 如果数字合理，跑完整 baseline

把 `--n 30` 改成 `--n 300`（每个 suite 各 300 case，**总共需要 plaw 完整跑 600 次**，约 20-30 分钟）：

```powershell
cargo run --release -p plaw-eval-cli -- run `
  --suite chat_quality `
  --suite tool_routing `
  --n 300 `
  --output target/reports/baseline-full.json
```

实际上我们的 chat_quality 只有 30 个 case，tool_routing 32 个 —— 当 `--n` 超过实际 case 数时，runner 会跑全部 case 并重复 k 次（每个 case 跑 ⌈300/N⌉ 次以填满 n=300，给 repeatability 信号）。

### 6. 写 baseline 文档

```powershell
cargo run --release -p plaw-eval-cli -- list --detail --limit 2 > docs/eval/baseline-2026-Q2.md
```

然后手动编辑这个文件，加：
- 跑的 git commit hash
- 跑的日期
- plaw 版本（plaw 仓库的 commit）
- 你对每个 metric 数字的简短判断（"这个分数 0.78 看起来合理 / 偏高 / 偏低"）

提交：

```powershell
git add docs/eval/baseline-2026-Q2.md
git commit -m "docs(eval): T11.7 — 2026-Q2 baseline numbers"
git push origin main
```

## 排错

### "plaw WS connection refused"

- plaw-desktop 没起 → `.\dev.ps1`
- 或起了但端口对不上 → 检查 `plaw-data/port-state.json`，再跑 `plaw-eval doctor` 看自动检测对不对

### "KIMI_API_KEY not set"

- 当前 powershell session 没设环境变量 → `$env:KIMI_API_KEY = "sk-..."`
- 重新打开 powershell 后会丢 → 想持久化用 `setx KIMI_API_KEY "sk-..."`

### "judge call failed: 401 Unauthorized"

- API key 写错了 / 过期了 → 去 Moonshot 平台重置
- 或用错 base URL：`kimi-coder` 走 `https://api.kimi.com/coding`，`kimi` 走 `https://api.moonshot.cn`，两个 key 不一定通用

### Judge 回的内容看着不对（分数都是 0 或都是 5）

- 大概率 judge 没正确解析 prompt → 看 `plaw-eval` 的日志（加 `-vv` 详细日志）
- 或者 case 设计的 expected_keywords 太苛刻 / 太宽松

## Cross-family judge（高质量但成本高）

上面默认用 `kimi-coder` 当 judge —— 跟被测的 plaw 同 provider，存在自我偏好风险。
更严格的方法：跑同一份 case，但 judge 用 Anthropic Claude：

```powershell
$env:ANTHROPIC_API_KEY = "sk-ant-..."
cargo run --release -p plaw-eval-cli -- run `
  --suite chat_quality `
  --judge "anthropic:claude-haiku-4-5" `
  --n 30 `
  --output target/reports/baseline-cross-judge.json
```

如果两份结果数字差距 > 5pp，说明同 family 自我偏好严重，应该上 jury。
方法见 `docs/eval/judge-selection.md`。
