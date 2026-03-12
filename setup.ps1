# Plaw Desktop - Development Environment Setup
# Usage: .\setup.ps1
# Checks and installs: Rust, Node.js, pnpm, Tauri CLI, frontend deps, plaw-data dirs

$ErrorActionPreference = "Stop"

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host "  Plaw Desktop - Environment Setup"  -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# --- Helper functions ---

function Test-CommandExists($cmd) {
    $null -ne (Get-Command $cmd -ErrorAction SilentlyContinue)
}

function Write-Step($msg) {
    Write-Host "[*] $msg" -ForegroundColor Yellow
}

function Write-Ok($msg) {
    Write-Host "[OK] $msg" -ForegroundColor Green
}

function Write-Skip($msg) {
    Write-Host "[--] $msg" -ForegroundColor DarkGray
}

function Write-Fail($msg) {
    Write-Host "[!!] $msg" -ForegroundColor Red
}

$allGood = $true

# ==============================
# 1. Rust toolchain
# ==============================
Write-Host "--- 1/7  Rust toolchain ---" -ForegroundColor Magenta

if (Test-CommandExists "rustc") {
    $rustVer = (rustc --version) -replace "rustc ",""
    Write-Ok "Rust already installed: $rustVer"
} else {
    Write-Step "Rust not found. Installing via rustup..."
    try {
        $rustupInit = "$env:TEMP\rustup-init.exe"
        Invoke-WebRequest -Uri "https://win.rustup.rs/x86_64" -OutFile $rustupInit -UseBasicParsing
        & $rustupInit -y --default-toolchain stable
        # Refresh PATH for current session
        $env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
        if (Test-CommandExists "rustc") {
            Write-Ok "Rust installed: $(rustc --version)"
        } else {
            Write-Fail "Rust installation may require restarting your terminal"
            $allGood = $false
        }
    } catch {
        Write-Fail "Failed to install Rust: $_"
        Write-Host "  Please install manually: https://rustup.rs/" -ForegroundColor DarkYellow
        $allGood = $false
    }
}

# ==============================
# 2. Node.js
# ==============================
Write-Host ""
Write-Host "--- 2/7  Node.js ---" -ForegroundColor Magenta

if (Test-CommandExists "node") {
    $nodeVer = (node --version)
    $nodeMajor = [int]($nodeVer -replace "v(\d+)\..*",'$1')
    if ($nodeMajor -ge 18) {
        Write-Ok "Node.js already installed: $nodeVer"
    } else {
        Write-Fail "Node.js version $nodeVer is too old (need >= 18)"
        Write-Host "  Please upgrade: https://nodejs.org/" -ForegroundColor DarkYellow
        $allGood = $false
    }
} else {
    Write-Step "Node.js not found. Attempting install via winget..."
    try {
        winget install OpenJS.NodeJS.LTS --accept-source-agreements --accept-package-agreements
        # Refresh PATH
        $env:PATH = [System.Environment]::GetEnvironmentVariable("PATH", "Machine") + ";" + [System.Environment]::GetEnvironmentVariable("PATH", "User")
        if (Test-CommandExists "node") {
            Write-Ok "Node.js installed: $(node --version)"
        } else {
            Write-Fail "Node.js installed but not in PATH yet. Restart your terminal."
            $allGood = $false
        }
    } catch {
        Write-Fail "Failed to install Node.js: $_"
        Write-Host "  Please install manually: https://nodejs.org/" -ForegroundColor DarkYellow
        $allGood = $false
    }
}

# ==============================
# 3. pnpm
# ==============================
Write-Host ""
Write-Host "--- 3/7  pnpm ---" -ForegroundColor Magenta

if (Test-CommandExists "pnpm") {
    Write-Ok "pnpm already installed: $(pnpm --version)"
} else {
    Write-Step "pnpm not found. Installing via npm..."
    try {
        npm install -g pnpm
        if (Test-CommandExists "pnpm") {
            Write-Ok "pnpm installed: $(pnpm --version)"
        } else {
            Write-Fail "pnpm installed but not in PATH. Restart your terminal."
            $allGood = $false
        }
    } catch {
        Write-Fail "Failed to install pnpm: $_"
        Write-Host "  Try: npm install -g pnpm" -ForegroundColor DarkYellow
        $allGood = $false
    }
}

# ==============================
# 4. Tauri CLI
# ==============================
Write-Host ""
Write-Host "--- 4/7  Tauri CLI ---" -ForegroundColor Magenta

# Check if cargo-tauri is installed
$tauriInstalled = $false
if (Test-CommandExists "cargo") {
    $cargoTauri = cargo install --list 2>$null | Select-String "^tauri-cli"
    if ($cargoTauri) {
        $tauriInstalled = $true
        Write-Ok "Tauri CLI already installed: $($cargoTauri.ToString().Trim())"
    }
}

if (-not $tauriInstalled) {
    if (Test-CommandExists "cargo") {
        Write-Step "Installing Tauri CLI (cargo install tauri-cli)... This may take a few minutes."
        try {
            cargo install tauri-cli
            Write-Ok "Tauri CLI installed"
        } catch {
            Write-Fail "Failed to install Tauri CLI: $_"
            Write-Host "  Try manually: cargo install tauri-cli" -ForegroundColor DarkYellow
            $allGood = $false
        }
    } else {
        Write-Fail "Cannot install Tauri CLI without Rust/cargo"
        $allGood = $false
    }
}

# ==============================
# 5. Frontend dependencies
# ==============================
Write-Host ""
Write-Host "--- 5/7  Frontend dependencies ---" -ForegroundColor Magenta

$webDir = Join-Path $PSScriptRoot "web"
$nodeModules = Join-Path $webDir "node_modules"

if (Test-Path $nodeModules) {
    Write-Skip "web/node_modules already exists, skipping pnpm install"
} else {
    if (Test-CommandExists "pnpm") {
        Write-Step "Installing frontend dependencies (pnpm install)..."
        Push-Location $webDir
        try {
            pnpm install
            Write-Ok "Frontend dependencies installed"
        } catch {
            Write-Fail "pnpm install failed: $_"
            $allGood = $false
        } finally {
            Pop-Location
        }
    } else {
        Write-Fail "pnpm not available, cannot install frontend dependencies"
        $allGood = $false
    }
}

# ==============================
# 6. plaw-data directory structure
# ==============================
Write-Host ""
Write-Host "--- 6/7  plaw-data directory ---" -ForegroundColor Magenta

$plawData = Join-Path $PSScriptRoot "plaw-data"

# Create essential directories
$dirs = @(
    $plawData,
    (Join-Path $plawData "bin"),
    (Join-Path $plawData ".plaw"),
    (Join-Path $plawData ".plaw" "workspace"),
    (Join-Path $plawData ".plaw" "workspace" "skills"),
    (Join-Path $plawData ".plaw" "knowledge"),
    (Join-Path $plawData "sessions"),
    (Join-Path $plawData "uploads")
)

foreach ($d in $dirs) {
    if (-not (Test-Path $d)) {
        New-Item -ItemType Directory -Path $d -Force | Out-Null
    }
}
Write-Ok "plaw-data directory structure created"

# ==============================
# 7. Build Plaw engine & deploy
# ==============================
Write-Host ""
Write-Host "--- 7/7  Build Plaw engine ---" -ForegroundColor Magenta

$plawExe = Join-Path $plawData "bin" "plaw.exe"
if (Test-Path $plawExe) {
    Write-Ok "Plaw binary already exists: bin/plaw.exe"
    Write-Host "       To rebuild: cd plaw; cargo build --release" -ForegroundColor DarkGray
} else {
    if (Test-CommandExists "cargo") {
        Write-Step "Building Plaw engine (cargo build --release)... This will take several minutes on first run."
        $plawDir = Join-Path $PSScriptRoot "plaw"
        Push-Location $plawDir
        try {
            cargo build --release
            Pop-Location
            # Deploy binary
            $builtExe = Join-Path $plawDir "target" "release" "plaw.exe"
            if (Test-Path $builtExe) {
                Copy-Item -Path $builtExe -Destination $plawExe -Force
                Write-Ok "Plaw engine built and deployed to plaw-data/bin/plaw.exe"
            } else {
                Write-Fail "Build succeeded but plaw.exe not found at expected path"
                $allGood = $false
            }
        } catch {
            Pop-Location
            Write-Fail "Plaw build failed: $_"
            Write-Host "  Try manually: cd plaw && cargo build --release" -ForegroundColor DarkYellow
            $allGood = $false
        }
    } else {
        Write-Fail "Cannot build Plaw without Rust/cargo"
        $allGood = $false
    }
}

# Check if config.toml exists
$configToml = Join-Path $plawData ".plaw" "config.toml"
if (Test-Path $configToml) {
    Write-Skip "config.toml already exists"
} else {
    Write-Step "Creating minimal config.toml (fill in your API Key later)..."
    $configContent = @"
# Plaw Configuration
# Fill in your API Key to get started

api_key = ""
default_provider = "anthropic-custom:https://api.kimi.com/coding"
default_model = "k2p5"
default_temperature = 0.7

[provider]
reasoning_level = "medium"

[web_search]
enabled = true
provider = "bing"
max_results = 5
timeout_secs = 30

[web_fetch]
enabled = true
provider = "fast_html2md"
allowed_domains = ["*"]
max_response_size = 524288
timeout_secs = 30

[browser]
enabled = false

[cron]
enabled = true
max_run_history = 50

[scheduler]
enabled = true
max_tasks = 100
max_concurrent = 1
"@
    Set-Content -Path $configToml -Value $configContent -Encoding UTF8
    Write-Ok "config.toml created (remember to fill in api_key)"
}

# ==============================
# Summary
# ==============================
Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan

if ($allGood) {
    Write-Host "  All checks passed!" -ForegroundColor Green
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Next steps:" -ForegroundColor White
    Write-Host "  1. Fill in your API Key:" -ForegroundColor Gray
    Write-Host "     Edit plaw-data/.plaw/config.toml" -ForegroundColor White
    Write-Host "     Set api_key = `"sk-your-key-here`"" -ForegroundColor White
    Write-Host ""
    Write-Host "  2. Start development:" -ForegroundColor Gray
    Write-Host "     .\dev.ps1" -ForegroundColor White
} else {
    Write-Host "  Some steps failed (see above)" -ForegroundColor Red
    Write-Host "========================================" -ForegroundColor Cyan
    Write-Host ""
    Write-Host "Fix the issues above, then run .\setup.ps1 again." -ForegroundColor Yellow
    Write-Host "The script is idempotent - it will skip already completed steps." -ForegroundColor DarkGray
}

Write-Host ""
