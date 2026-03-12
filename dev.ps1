# Plaw Desktop - Development launcher
# Usage: .\dev.ps1

Write-Host "=== Plaw Desktop Dev ===" -ForegroundColor Cyan

# Hint: if you need proxy for external API access, set before running:
#   $env:HTTPS_PROXY = "http://127.0.0.1:8118"
if ($env:HTTPS_PROXY) {
    Write-Host "Proxy: $env:HTTPS_PROXY" -ForegroundColor DarkGray
}

# Check pnpm dependencies
if (-not (Test-Path "web/node_modules")) {
    Write-Host "Installing frontend dependencies..." -ForegroundColor Yellow
    Push-Location web
    pnpm install
    Pop-Location
}

# Launch Tauri dev — override resources to empty to skip glob validation in dev mode
Write-Host "Starting Tauri dev server..." -ForegroundColor Green
Push-Location src-tauri
cargo tauri dev
Pop-Location
