# Tham khảo lệnh Plaw

Dựa trên CLI hiện tại (`plaw --help`).

Xác minh lần cuối: **2026-02-20**.

## Lệnh cấp cao nhất

| Lệnh | Mục đích |
|---|---|
| `onboard` | Khởi tạo workspace/config nhanh hoặc tương tác |
| `agent` | Chạy chat tương tác hoặc chế độ gửi tin nhắn đơn |
| `gateway` | Khởi động gateway webhook và HTTP WhatsApp |
| `daemon` | Khởi động runtime có giám sát (gateway + channels + heartbeat/scheduler tùy chọn) |
| `service` | Quản lý vòng đời dịch vụ cấp hệ điều hành |
| `doctor` | Chạy chẩn đoán và kiểm tra trạng thái |
| `status` | Hiển thị cấu hình và tóm tắt hệ thống |
| `cron` | Quản lý tác vụ định kỳ |
| `models` | Làm mới danh mục model của provider |
| `providers` | Liệt kê ID provider, bí danh và provider đang dùng |
| `channel` | Quản lý kênh và kiểm tra sức khỏe kênh |
| `integrations` | Kiểm tra chi tiết tích hợp |
| `skills` | Liệt kê/cài đặt/gỡ bỏ skills |
| `migrate` | Nhập dữ liệu từ runtime khác (hiện hỗ trợ OpenClaw) |
| `config` | Xuất schema cấu hình dạng máy đọc được |
| `completions` | Tạo script tự hoàn thành cho shell ra stdout |
| `hardware` | Phát hiện và kiểm tra phần cứng USB |
| `peripheral` | Cấu hình và nạp firmware thiết bị ngoại vi |

## Nhóm lệnh

### `onboard`

- `plaw onboard`
- `plaw onboard --interactive`
- `plaw onboard --channels-only`
- `plaw onboard --api-key <KEY> --provider <ID> --memory <sqlite|lucid|markdown|none>`
- `plaw onboard --api-key <KEY> --provider <ID> --model <MODEL_ID> --memory <sqlite|lucid|markdown|none>`

### `agent`

- `plaw agent`
- `plaw agent -m "Hello"`
- `plaw agent --provider <ID> --model <MODEL> --temperature <0.0-2.0>`
- `plaw agent --peripheral <board:path>`

### `gateway` / `daemon`

- `plaw gateway [--host <HOST>] [--port <PORT>] [--new-pairing]`
- `plaw daemon [--host <HOST>] [--port <PORT>]`

`--new-pairing` sẽ xóa toàn bộ token đã ghép đôi và tạo mã ghép đôi mới khi gateway khởi động.

### `service`

- `plaw service install`
- `plaw service start`
- `plaw service stop`
- `plaw service restart`
- `plaw service status`
- `plaw service uninstall`

### `cron`

- `plaw cron list`
- `plaw cron add <expr> [--tz <IANA_TZ>] [--timeout-secs <N>] <command>`
- `plaw cron add-at <rfc3339_timestamp> [--timeout-secs <N>] <command>`
- `plaw cron add-every <every_ms> [--timeout-secs <N>] <command>`
- `plaw cron once <delay> [--timeout-secs <N>] <command>`
- `plaw cron update <id> [--expression <expr>] [--tz <IANA_TZ>] [--command <cmd>] [--name <name>] [--timeout-secs <N>]`
- `plaw cron remove <id>`
- `plaw cron pause <id>`
- `plaw cron resume <id>`

> `--timeout-secs`: per-job shell timeout (`1..=86400`s; default `120`). Shell jobs only.

### `models`

- `plaw models refresh`
- `plaw models refresh --provider <ID>`
- `plaw models refresh --force`

`models refresh` hiện hỗ trợ làm mới danh mục trực tiếp cho các provider: `openrouter`, `openai`, `anthropic`, `groq`, `mistral`, `deepseek`, `xai`, `together-ai`, `gemini`, `ollama`, `astrai`, `venice`, `fireworks`, `cohere`, `moonshot`, `glm`, `zai`, `qwen` và `nvidia`.

### `channel`

- `plaw channel list`
- `plaw channel start`
- `plaw channel doctor`
- `plaw channel bind-telegram <IDENTITY>`
- `plaw channel add <type> <json>`
- `plaw channel remove <name>`

Lệnh trong chat khi runtime đang chạy (Telegram/Discord):

- `/models`
- `/models <provider>`
- `/model`
- `/model <model-id>`

Channel runtime cũng theo dõi `config.toml` và tự động áp dụng thay đổi cho:
- `default_provider`
- `default_model`
- `default_temperature`
- `api_key` / `api_url` (cho provider mặc định)
- `reliability.*` cài đặt retry của provider

`add/remove` hiện chuyển hướng về thiết lập có hướng dẫn / cấu hình thủ công (chưa hỗ trợ đầy đủ mutator khai báo).

### `integrations`

- `plaw integrations info <name>`

### `skills`

- `plaw skills list`
- `plaw skills install <source>`
- `plaw skills remove <name>`

`<source>` chấp nhận git remote (`https://...`, `http://...`, `ssh://...` và `git@host:owner/repo.git`) hoặc đường dẫn cục bộ.

Skill manifest (`SKILL.toml`) hỗ trợ `prompts` và `[[tools]]`; cả hai được đưa vào system prompt của agent khi chạy, giúp model có thể tuân theo hướng dẫn skill mà không cần đọc thủ công.

### `migrate`

- `plaw migrate openclaw [--source <path>] [--dry-run]`

### `config`

- `plaw config schema`

`config schema` xuất JSON Schema (draft 2020-12) cho toàn bộ hợp đồng `config.toml` ra stdout.

### `completions`

- `plaw completions bash`
- `plaw completions fish`
- `plaw completions zsh`
- `plaw completions powershell`
- `plaw completions elvish`

`completions` chỉ xuất ra stdout để script có thể được source trực tiếp mà không bị lẫn log/cảnh báo.

### `hardware`

- `plaw hardware discover`
- `plaw hardware introspect <path>`
- `plaw hardware info [--chip <chip_name>]`

### `peripheral`

- `plaw peripheral list`
- `plaw peripheral add <board> <path>`
- `plaw peripheral flash [--port <serial_port>]`
- `plaw peripheral setup-uno-q [--host <ip_or_host>]`
- `plaw peripheral flash-nucleo`

## Kiểm tra nhanh

Để xác minh nhanh tài liệu với binary hiện tại:

```bash
plaw --help
plaw <command> --help
```
