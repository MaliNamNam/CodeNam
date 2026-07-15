<#
.SYNOPSIS
    Uninstall jcode on Windows.
.DESCRIPTION
    Removes the per-user launcher at %LOCALAPPDATA%\jcode\bin\jcode.exe,
    installed build binaries, and the jcode launcher directory from the user PATH.
    By default user data under %USERPROFILE%\.jcode is kept.

    One-liner uninstall:
      irm https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/uninstall.ps1 | iex
.PARAMETER InstallDir
    Override the launcher directory (default: $env:LOCALAPPDATA\jcode\bin)
.PARAMETER Purge
    Also delete user data in $env:JCODE_HOME or %USERPROFILE%\.jcode.
.PARAMETER DryRun
    Print what would be removed without deleting anything.
.PARAMETER Yes
    Skip the confirmation prompt.
#>
param(
    [string]$InstallDir,
    [switch]$Purge,
    [switch]$DryRun,
    [switch]$Yes
)

$ErrorActionPreference = 'Stop'

function Write-Info($msg) { Write-Host $msg -ForegroundColor Blue }
function Write-Err($msg) { Write-Host "error: $msg" -ForegroundColor Red; exit 1 }
function Write-Warn($msg) { Write-Host "warning: $msg" -ForegroundColor Yellow }

function Get-JcodeLocalAppDataDir {
    if ($env:LOCALAPPDATA) { return $env:LOCALAPPDATA }

    $localAppData = [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)
    if ($localAppData) { return $localAppData }

    if ($env:USERPROFILE) { return (Join-Path $env:USERPROFILE "AppData\Local") }
    return (Join-Path ([Environment]::GetFolderPath("UserProfile")) "AppData\Local")
}

function Get-DefaultJcodeInstallDir {
    return (Join-Path (Get-JcodeLocalAppDataDir) "jcode\bin")
}

function ConvertTo-JcodePathKey([string]$PathValue) {
    if (-not $PathValue) { return "" }
    $clean = [Environment]::ExpandEnvironmentVariables($PathValue.Trim().Trim('"'))
    if (-not $clean) { return "" }
    try { $clean = [System.IO.Path]::GetFullPath($clean) } catch {}
    $clean = $clean.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)
    return $clean.ToUpperInvariant()
}

function Split-JcodePathList([string]$PathValue) {
    if (-not $PathValue) { return @() }
    $entries = @()
    foreach ($entry in ($PathValue -split ';')) {
        $clean = $entry.Trim().Trim('"')
        if ($clean) { $entries += $clean }
    }
    return $entries
}

function Join-JcodePathList([string[]]$Entries) {
    if (-not $Entries -or $Entries.Count -eq 0) { return "" }
    return ($Entries -join ';')
}

function Get-JcodeManagedPathKeys([string]$InstallDir) {
    $keys = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($candidate in @($InstallDir, (Get-DefaultJcodeInstallDir))) {
        $key = ConvertTo-JcodePathKey $candidate
        if ($key) { [void]$keys.Add($key) }
    }
    return $keys
}

function Resolve-JcodePathRemoval {
    param(
        [Parameter(Mandatory = $true)][string]$InstallDir,
        [AllowNull()][string]$CurrentPath
    )

    $managedKeys = Get-JcodeManagedPathKeys -InstallDir $InstallDir
    $nextEntries = @()
    $removedManaged = 0

    foreach ($entry in (Split-JcodePathList $CurrentPath)) {
        $key = ConvertTo-JcodePathKey $entry
        if (-not $key) { continue }
        if ($managedKeys.Contains($key)) {
            $removedManaged += 1
            continue
        }
        $nextEntries += $entry
    }

    $nextPath = Join-JcodePathList $nextEntries
    return [pscustomobject]@{
        Path = $nextPath
        Changed = ($nextPath -ne ([string]$CurrentPath))
        RemovedManagedEntries = $removedManaged
        InstallDir = $InstallDir
    }
}

function Send-JcodeEnvironmentChangedBroadcast {
    if ($env:JCODE_DISABLE_ENV_BROADCAST -eq "1") { return $false }
    if (-not ("Jcode.EnvironmentBroadcast" -as [type])) {
        Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
namespace Jcode {
    public static class EnvironmentBroadcast {
        [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
        public static extern IntPtr SendMessageTimeout(
            IntPtr hWnd,
            UInt32 Msg,
            UIntPtr wParam,
            string lParam,
            UInt32 fuFlags,
            UInt32 uTimeout,
            out UIntPtr lpdwResult);
    }
}
"@
    }
    $result = [UIntPtr]::Zero
    [Jcode.EnvironmentBroadcast]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, "Environment", 0x0002, 5000, [ref]$result) | Out-Null
    return $true
}

function Remove-JcodeUserPath {
    param(
        [Parameter(Mandatory = $true)][string]$InstallDir,
        [AllowNull()][string]$CurrentPath,
        [scriptblock]$SetUserPathAction,
        [scriptblock]$BroadcastAction,
        [bool]$Broadcast = $true
    )

    if (-not $PSBoundParameters.ContainsKey('CurrentPath')) {
        $CurrentPath = [Environment]::GetEnvironmentVariable("Path", "User")
    }

    $update = Resolve-JcodePathRemoval -InstallDir $InstallDir -CurrentPath $CurrentPath
    $broadcasted = $false
    if ($update.Changed) {
        if ($SetUserPathAction) {
            & $SetUserPathAction $update.Path
        } else {
            [Environment]::SetEnvironmentVariable("Path", $update.Path, "User")
        }

        if ($Broadcast) {
            if ($BroadcastAction) { & $BroadcastAction | Out-Null } else { Send-JcodeEnvironmentChangedBroadcast | Out-Null }
            $broadcasted = $true
        }
    }
    $update | Add-Member -NotePropertyName Broadcasted -NotePropertyValue $broadcasted
    return $update
}

if (-not $InstallDir) { $InstallDir = Get-DefaultJcodeInstallDir }

$localJcodeRoot = Join-Path (Get-JcodeLocalAppDataDir) "jcode"
$launcherPath = Join-Path $InstallDir "jcode.exe"
$buildsDir = Join-Path $localJcodeRoot "builds"
$userDataDir = if ($env:JCODE_HOME) {
    $env:JCODE_HOME
} elseif ($env:USERPROFILE) {
    Join-Path $env:USERPROFILE ".jcode"
} else {
    Join-Path ([Environment]::GetFolderPath("UserProfile")) ".jcode"
}

$targets = @()
if (Test-Path -LiteralPath $launcherPath) { $targets += "$launcherPath (launcher)" }
if (Test-Path -LiteralPath $buildsDir) { $targets += "$buildsDir (installed binaries)" }
if ($Purge -and (Test-Path -LiteralPath $userDataDir)) { $targets += "$userDataDir (user data)" }

$userPathPreview = Resolve-JcodePathRemoval -InstallDir $InstallDir -CurrentPath ([Environment]::GetEnvironmentVariable("Path", "User"))
if ($userPathPreview.RemovedManagedEntries -gt 0) {
    $targets += "$InstallDir (user PATH entry)"
}

if ($targets.Count -eq 0) {
    Write-Info "Nothing to uninstall: no jcode installation found."
    exit 0
}

Write-Info "The following will be removed:"
foreach ($target in $targets) { Write-Host "  - $target" }
if (-not $Purge) {
    Write-Warn "User data in $userDataDir is kept. Run with -Purge for a full wipe."
}

if ($DryRun) {
    Write-Info "Dry run: nothing was deleted."
    exit 0
}

if (-not $Yes) {
    $reply = Read-Host "Proceed? [y/N]"
    if ($reply -notin @("y", "Y", "yes", "YES")) {
        Write-Info "Aborted."
        exit 1
    }
}

try {
    Get-CimInstance Win32_Process -Filter "Name = 'jcode.exe'" -ErrorAction SilentlyContinue |
        ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
} catch {}

if (Test-Path -LiteralPath $launcherPath) {
    Remove-Item -LiteralPath $launcherPath -Force
    Write-Info "Removed $launcherPath"
}

if (Test-Path -LiteralPath $InstallDir) {
    try { Remove-Item -LiteralPath $InstallDir -Force -ErrorAction SilentlyContinue } catch {}
}

if ($Purge) {
    foreach ($path in @($localJcodeRoot, $userDataDir)) {
        if ($path -and (Test-Path -LiteralPath $path)) {
            Remove-Item -LiteralPath $path -Recurse -Force
            Write-Info "Removed $path"
        }
    }
} elseif (Test-Path -LiteralPath $buildsDir) {
    Remove-Item -LiteralPath $buildsDir -Recurse -Force
    Write-Info "Removed $buildsDir"
}

$pathUpdate = Remove-JcodeUserPath -InstallDir $InstallDir
if ($pathUpdate.Changed) {
    Write-Info "Removed $($pathUpdate.RemovedManagedEntries) jcode entr$(if ($pathUpdate.RemovedManagedEntries -eq 1) { 'y' } else { 'ies' }) from user PATH"
}

Write-Info "jcode uninstalled."
Write-Info "Reinstall with: irm https://raw.githubusercontent.com/1jehuang/jcode/master/scripts/install.ps1 | iex"
