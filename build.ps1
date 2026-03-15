# Plaw Desktop - Production build
# Usage: .\build.ps1            — full build (compile Plaw + package)
#        .\build.ps1 -NoPlaw    — skip Plaw compilation (use your own plaw.exe)
param([switch]$NoPlaw)

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

# ---- Build Plaw engine (skip with -NoPlaw) ----
if ($NoPlaw) {
    Write-Host "Skipping Plaw engine build (-NoPlaw)" -ForegroundColor DarkGray
    if (-not (Test-Path "plaw-data/bin/plaw.exe")) {
        Write-Host "WARNING: plaw-data/bin/plaw.exe not found! Place your own build there." -ForegroundColor Red
    }
} else {
    Write-Host "Building Plaw engine (release)..." -ForegroundColor Yellow
    Push-Location plaw
    cargo build --release
    Pop-Location

    $builtExe = "plaw/target/release/plaw.exe"
    if (Test-Path $builtExe) {
        Remove-Item -Force "plaw-data/bin/plaw.exe" -ErrorAction SilentlyContinue
        Copy-Item $builtExe "plaw-data/bin/plaw.exe"
        Write-Host "Plaw engine deployed to plaw-data/bin/plaw.exe" -ForegroundColor Green
    } else {
        Write-Host "ERROR: plaw.exe not found after build!" -ForegroundColor Red
        exit 1
    }
}

# ---- (Re)generate all tar.gz bundles every time ----
# Tauri consumes/moves these during packaging, so they must be recreated before each build.
$bundles = @{
    "agent-browser-bundle"  = "agent-browser"
    "browsers-bundle"       = "browsers"
    "python-bundle"         = "python"
    "pandoc-bundle"         = "pandoc"
    "libreoffice-bundle"    = "libreoffice"
    "poppler-bundle"        = "poppler"
    "node-modules-bundle"   = "node_modules_global"
    "pwsh-bundle"           = "pwsh"
    "cli-bundle"            = "bin/cli"
}

Write-Host "Generating resource bundles..." -ForegroundColor Yellow
Push-Location plaw-data

foreach ($name in $bundles.Keys) {
    $tarFile = "$name.tar.gz"
    $srcDir  = $bundles[$name]
    if (Test-Path $srcDir) {
        Write-Host "  $tarFile <- $srcDir" -ForegroundColor DarkGray
        tar -czf $tarFile --exclude='__pycache__' $srcDir
    } else {
        Write-Host "  WARNING: $srcDir not found, skipping $tarFile" -ForegroundColor Red
    }
}

Pop-Location

# Skills bundle (different source path)
$skillsSrc = "plaw-data/.plaw/workspace/skills"
if (Test-Path $skillsSrc) {
    Write-Host "  skills-bundle.tar.gz <- .plaw/workspace/skills" -ForegroundColor DarkGray
    tar -czf "plaw-data/skills-bundle.tar.gz" -C "plaw-data/.plaw/workspace" skills
} else {
    Write-Host "  WARNING: skills directory not found, skipping skills-bundle.tar.gz" -ForegroundColor Red
}

Write-Host "Bundles ready." -ForegroundColor Green

# Build with resources config
Write-Host "Building Tauri app with bundled resources..." -ForegroundColor Green
Push-Location src-tauri
cargo tauri build --config tauri.build.conf.json
Pop-Location
