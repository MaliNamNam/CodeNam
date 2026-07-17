param(
    [Parameter(Mandatory = $true)][string]$ArtifactExePath,
    [string]$Version
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$resolvedArtifact = (Resolve-Path -LiteralPath $ArtifactExePath).Path

if (-not $Version) {
    $artifactVersionOutput = & $resolvedArtifact --version
    if ($LASTEXITCODE -ne 0) {
        throw "Local Windows artifact failed to run --version"
    }

    $artifactVersionText = ($artifactVersionOutput -join "`n")
    if ($artifactVersionText -notmatch '(?i)\bjcode\s+v?(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)\b') {
        throw "Could not parse jcode version from local artifact output: $artifactVersionText"
    }
    $Version = 'v' + $Matches[1]
} else {
    $Version = 'v' + (($Version.Trim()) -replace '^[vV]', '')
}

$tempRoot = Join-Path $env:RUNNER_TEMP ("jcode-windows-install-verify-" + [guid]::NewGuid().ToString('N'))
$localAppData = Join-Path $tempRoot 'localappdata'
$appData = Join-Path $tempRoot 'appdata'
$userProfile = Join-Path $tempRoot 'userprofile'
$jcodeHome = Join-Path $tempRoot '.jcode'
$installDir = Join-Path $localAppData 'jcode\bin'

New-Item -ItemType Directory -Force -Path $localAppData, $appData, $userProfile, $jcodeHome | Out-Null

$env:LOCALAPPDATA = $localAppData
$env:APPDATA = $appData
$env:USERPROFILE = $userProfile
$env:JCODE_HOME = $jcodeHome

$installScript = Join-Path $repoRoot 'scripts\install.ps1'

& $installScript `
    -InstallDir $installDir `
    -Version $Version `
    -ArtifactExePath $resolvedArtifact

$launcherPath = Join-Path $installDir 'jcode.exe'
$versionDir = Join-Path $localAppData ('jcode\builds\versions\' + $Version.TrimStart('v') + '\jcode.exe')
$stablePath = Join-Path $localAppData 'jcode\builds\stable\jcode.exe'

foreach ($path in @($launcherPath, $versionDir, $stablePath)) {
    if (-not (Test-Path -LiteralPath $path)) {
        throw "Expected installed file missing: $path"
    }
}

$hotkeyDir = Join-Path $jcodeHome 'hotkey'
$startupShortcut = Join-Path $appData 'Microsoft\Windows\Start Menu\Programs\Startup\jcode-hotkey.lnk'
if (Test-Path -LiteralPath $hotkeyDir) {
    throw "Default install unexpectedly created optional hotkey files: $hotkeyDir"
}
if (Test-Path -LiteralPath $startupShortcut) {
    throw "Default install unexpectedly created an optional startup shortcut: $startupShortcut"
}

$versionOutput = & $launcherPath --version
if ($LASTEXITCODE -ne 0) {
    throw "Installed launcher failed to run --version"
}

if ($versionOutput -notmatch 'jcode') {
    throw "Installed launcher returned unexpected version output: $versionOutput"
}

& $installScript `
    -InstallDir $installDir `
    -Version $Version `
    -ArtifactExePath $resolvedArtifact

if (-not (Test-Path -LiteralPath $launcherPath)) {
    throw "Launcher missing after reinstall: $launcherPath"
}

# Exercise the explicitly requested hotkey path as well. A fake Alacritty
# executable is sufficient because setup only records its path; the hotkey is
# not pressed during this verification.
$fakeAlacritty = Join-Path $localAppData 'Microsoft\WinGet\Links\alacritty.exe'
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $fakeAlacritty) | Out-Null
New-Item -ItemType File -Force -Path $fakeAlacritty | Out-Null

& $installScript `
    -InstallDir $installDir `
    -Version $Version `
    -ArtifactExePath $resolvedArtifact `
    -ConfigureHotkey

if (-not (Test-Path -LiteralPath $startupShortcut)) {
    throw "Explicit hotkey setup did not create the Startup shortcut: $startupShortcut"
}

$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($startupShortcut)
if ($shortcut.TargetPath -notmatch '(?i)powershell\.exe$') {
    throw "Hotkey shortcut has an unexpected target: $($shortcut.TargetPath)"
}
if ($shortcut.Arguments -notmatch '(?i)-ExecutionPolicy\s+RemoteSigned') {
    throw "Hotkey shortcut does not use RemoteSigned: $($shortcut.Arguments)"
}
if ($shortcut.Arguments -match '(?i)\bBypass\b') {
    throw "Hotkey shortcut unexpectedly bypasses PowerShell execution policy"
}

$legacyVbs = Join-Path $hotkeyDir 'jcode-hotkey-launcher.vbs'
if (Test-Path -LiteralPath $legacyVbs) {
    throw "Legacy VBScript hotkey launcher still exists: $legacyVbs"
}

Write-Host "Windows install verification passed for $Version" -ForegroundColor Green
