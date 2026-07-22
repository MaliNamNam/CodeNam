<#
.SYNOPSIS
    Build CodeNam from this repo and install it so you can type CodeNam in a terminal.

.DESCRIPTION
    One-shot local install for Windows (source build):

      1. cargo build --release -p jcode --bin jcode  (crate name is still jcode)
      2. install as CodeNam.exe under %LOCALAPPDATA%\CodeNam\
      3. put that bin dir on your user PATH

    Usage (from repo root):

      powershell -ExecutionPolicy Bypass -File .\scripts\install_local.ps1

    Then open a new terminal and run:

      CodeNam

.PARAMETER Fast
    Use --profile selfdev instead of release (faster compile, slower runtime).

.PARAMETER SkipBuild
    Only re-link/install an existing target binary (no cargo build).
#>
param(
    [switch]$Fast,
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

function Write-Info([string]$Message) {
    Write-Host $Message -ForegroundColor Cyan
}

function Write-Ok([string]$Message) {
    Write-Host $Message -ForegroundColor Green
}

function Write-Warn([string]$Message) {
    Write-Host "warning: $Message" -ForegroundColor Yellow
}

function Normalize-PathKey([string]$PathValue) {
    if (-not $PathValue) { return "" }
    return $PathValue.Trim().TrimEnd([char]0x5C).ToLowerInvariant()
}

# Ensure cargo is on PATH for this process
$cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
if (Test-Path -LiteralPath $cargoBin) {
    $pathParts = @($env:Path -split ";" | Where-Object { $_ })
    if ($pathParts -notcontains $cargoBin) {
        $env:Path = $cargoBin + ";" + $env:Path
    }
}

$cargo = Get-Command cargo -ErrorAction SilentlyContinue
if (-not $cargo) {
    throw "cargo not found. Install Rust from https://rustup.rs then re-open the terminal."
}

$repoRoot = Split-Path -Parent $PSScriptRoot
if (-not (Test-Path -LiteralPath (Join-Path $repoRoot "Cargo.toml"))) {
    throw "Could not find repo root Cargo.toml next to scripts\"
}

Set-Location -LiteralPath $repoRoot

$localAppData = if ($env:LOCALAPPDATA) {
    $env:LOCALAPPDATA
} else {
    [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
}

# Product name is CodeNam (crate/binary name remains jcode for now).
$productName = "CodeNam"
$buildsRoot = Join-Path $localAppData ($productName + "\builds")
$currentDir = Join-Path $buildsRoot "current"
$binDir = Join-Path $localAppData ($productName + "\bin")
$currentExe = Join-Path $currentDir ($productName + ".exe")
$launcherExe = Join-Path $binDir ($productName + ".exe")

New-Item -ItemType Directory -Force -Path $currentDir | Out-Null
New-Item -ItemType Directory -Force -Path $binDir | Out-Null

$profileName = if ($Fast) { "selfdev" } else { "release" }
$builtExe = Join-Path $repoRoot ("target\" + $profileName + "\jcode.exe")

if (-not $SkipBuild) {
    Write-Info ("Building CodeNam (" + $profileName + ") - first build can take a while...")
    if ($Fast) {
        & cargo build --profile selfdev -p jcode --bin jcode
    } else {
        & cargo build --release -p jcode --bin jcode
    }
    if ($LASTEXITCODE -ne 0) {
        throw ("cargo build failed with exit code " + $LASTEXITCODE)
    }
}

if (-not (Test-Path -LiteralPath $builtExe)) {
    throw ("Built binary not found: " + $builtExe)
}

Write-Info ("Installing to " + $currentExe)
Copy-Item -LiteralPath $builtExe -Destination $currentExe -Force
Copy-Item -LiteralPath $builtExe -Destination $launcherExe -Force

# Ensure launcher directory is on the user PATH permanently
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$pathParts = @()
if ($userPath) {
    $pathParts = @($userPath -split ";" | Where-Object { $_ -and $_.Trim() -ne "" })
}
$binDirKey = Normalize-PathKey $binDir
$alreadyOnPath = $false
foreach ($part in $pathParts) {
    if ((Normalize-PathKey $part) -eq $binDirKey) {
        $alreadyOnPath = $true
        break
    }
}

if (-not $alreadyOnPath) {
    Write-Info ("Adding " + $binDir + " to your user PATH")
    if ($userPath -and $userPath.Trim()) {
        $newPath = $binDir + ";" + $userPath
    } else {
        $newPath = $binDir
    }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    $env:Path = $binDir + ";" + $env:Path
    Write-Warn "Open a new terminal so PATH updates everywhere."
} else {
    $sessionParts = @($env:Path -split ";" | Where-Object { $_ })
    $onSessionPath = $false
    foreach ($part in $sessionParts) {
        if ((Normalize-PathKey $part) -eq $binDirKey) {
            $onSessionPath = $true
            break
        }
    }
    if (-not $onSessionPath) {
        $env:Path = $binDir + ";" + $env:Path
    }
}

$versionLine = $null
try {
    $versionLine = & $launcherExe --version 2>$null
} catch {
    $versionLine = $null
}
if (-not $versionLine) {
    $versionLine = "(installed; --version not available)"
}

Write-Ok ""
Write-Ok "Installed CodeNam from this repo."
Write-Ok ("  binary : " + $currentExe)
Write-Ok ("  launch : " + $launcherExe)
Write-Ok ("  version: " + $versionLine)
Write-Ok ""
Write-Ok "Run:"
Write-Ok "  CodeNam"
Write-Ok ""
if (-not $alreadyOnPath) {
    Write-Warn "If CodeNam is not found, close this window and open a new terminal."
}
