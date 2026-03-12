# Plaw Desktop - Production build
# Usage: .\build.ps1

Write-Host "=== Plaw Desktop Build ===" -ForegroundColor Cyan

# Ensure proxy is set for NSIS/WiX downloads
if (-not $env:HTTPS_PROXY) {
    $env:HTTPS_PROXY = "http://127.0.0.1:8118"
    Write-Host "Proxy set: $env:HTTPS_PROXY" -ForegroundColor DarkGray
}

# Check pnpm dependencies
if (-not (Test-Path "web/node_modules")) {
    Write-Host "Installing frontend dependencies..." -ForegroundColor Yellow
    Push-Location web
    pnpm install
    Pop-Location
}

# Build with resources config
Write-Host "Building Tauri app with bundled resources..." -ForegroundColor Green
Push-Location src-tauri
cargo tauri build --config tauri.build.conf.json
Pop-Location
