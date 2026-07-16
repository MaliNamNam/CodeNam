param()

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$installScript = Join-Path $repoRoot 'scripts\install.ps1'

$originalLocalAppData = $env:LOCALAPPDATA
$originalImportOnly = $env:JCODE_INSTALL_PS1_IMPORT_ONLY
$testRoot = Join-Path $env:TEMP ("jcode-windows-launcher-tests-{0}" -f ([guid]::NewGuid().ToString('N')))
New-Item -ItemType Directory -Path $testRoot -Force | Out-Null

function Assert-True($Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}

function Assert-Equal($Expected, $Actual, [string]$Message) {
    if ($Expected -ne $Actual) {
        throw "$Message`nExpected: $Expected`nActual:   $Actual"
    }
}

function Assert-PathCount([string]$PathValue, [string]$Entry, [int]$ExpectedCount, [string]$Message) {
    $entryKey = ConvertTo-JcodePathKey $Entry
    $count = 0
    foreach ($candidate in (Split-JcodePathList $PathValue)) {
        if ((ConvertTo-JcodePathKey $candidate) -eq $entryKey) { $count += 1 }
    }
    Assert-Equal $ExpectedCount $count $Message
}

try {
    $env:LOCALAPPDATA = Join-Path $testRoot 'LocalAppData'
    $env:JCODE_INSTALL_PS1_IMPORT_ONLY = '1'
    . $installScript

    $installDir = Join-Path $env:LOCALAPPDATA 'jcode\bin'
    $launcherPath = Join-Path $installDir 'jcode.exe'

    Write-Host 'test_launcher_path_localappdata'
    Assert-Equal $installDir (Get-DefaultJcodeInstallDir) 'default installer path should live under LOCALAPPDATA\jcode\bin'
    Assert-Equal $launcherPath (Join-Path (Get-DefaultJcodeInstallDir) 'jcode.exe') 'launcher path should be LOCALAPPDATA\jcode\bin\jcode.exe'

    Write-Host 'test_path_add_idempotent_dedupes_case_and_slashes'
    $installVariant = ($installDir.ToUpperInvariant() + '\')
    $currentPath = "C:\Tools;$installVariant;$installDir;C:\Tools\;C:\Other"
    $pathUpdate = Resolve-JcodePathUpdate -InstallDir $installDir -CurrentPath $currentPath
    Assert-Equal "$installDir;C:\Tools;C:\Other" $pathUpdate.Path 'install path update should prepend canonical launcher dir and remove stale managed/duplicate entries'
    Assert-PathCount $pathUpdate.Path $installDir 1 'updated PATH should contain exactly one jcode launcher dir'
    Assert-Equal 2 $pathUpdate.RemovedManagedEntries 'path update should remove both stale jcode launcher entries before re-adding one'
    Assert-Equal 1 $pathUpdate.RemovedDuplicateEntries 'path update should remove duplicate non-jcode entries during install'
    $secondUpdate = Resolve-JcodePathUpdate -InstallDir $installDir -CurrentPath $pathUpdate.Path
    Assert-Equal $false $secondUpdate.Changed 'second install path update should be idempotent'
    Assert-PathCount $secondUpdate.Path $installDir 1 'idempotent update should still contain exactly one launcher dir'

    Write-Host 'test_env_broadcast_called_once_when_path_changes'
    $script:setCalls = 0
    $script:broadcastCalls = 0
    $appliedPath = $null
    $setPathAction = { param($value) $script:setCalls += 1; $script:appliedPath = $value }
    $broadcastAction = { $script:broadcastCalls += 1 }
    $mockUpdate = Set-JcodeUserPath -InstallDir $installDir -CurrentPath 'C:\Tools' -SetUserPathAction $setPathAction -BroadcastAction $broadcastAction
    Assert-Equal 1 $script:setCalls 'user PATH setter should be called once when PATH changes'
    Assert-Equal 1 $script:broadcastCalls 'environment broadcast should be called once when PATH changes'
    Assert-Equal $true $mockUpdate.Broadcasted 'path update should report broadcast when changed'
    $noChangeUpdate = Set-JcodeUserPath -InstallDir $installDir -CurrentPath $script:appliedPath -SetUserPathAction $setPathAction -BroadcastAction $broadcastAction
    Assert-Equal 1 $script:setCalls 'user PATH setter should not be called when PATH is already correct'
    Assert-Equal 1 $script:broadcastCalls 'environment broadcast should not be called when PATH is unchanged'
    Assert-Equal $false $noChangeUpdate.Broadcasted 'unchanged path update should not report broadcast'

    Write-Host 'test_local_binary_version_output_parsing'
    Assert-Equal 'v0.47.0' (ConvertFrom-JcodeVersionOutput 'jcode v0.47.0 (f7f5898c)') 'local artifact version parser should accept normal jcode --version output'
    Assert-Equal $null (ConvertFrom-JcodeVersionOutput 'not a jcode binary') 'local artifact version parser should reject unrelated output'

    Write-Host 'test_release_checksum_validation'
    $checksumFile = Join-Path $testRoot 'checksum.bin'
    Set-Content -LiteralPath $checksumFile -Value 'known-content' -NoNewline
    $digest = (Get-FileHash -LiteralPath $checksumFile -Algorithm SHA256).Hash.ToLowerInvariant()
    $manifest = "$digest  nested/path/jcode-windows-x86_64.exe"
    Assert-Equal $digest (Get-JcodeSha256FromManifest -ManifestText $manifest -AssetName 'jcode-windows-x86_64.exe') 'checksum parser should match release assets by file name'
    Assert-Equal $digest (Assert-JcodeFileChecksum -FilePath $checksumFile -ManifestText $manifest -AssetName 'jcode-windows-x86_64.exe') 'checksum validation should accept the matching digest'
    $checksumThrew = $false
    try {
        Assert-JcodeFileChecksum -FilePath $checksumFile -ManifestText (('0' * 64) + '  jcode-windows-x86_64.exe') -AssetName 'jcode-windows-x86_64.exe' | Out-Null
    } catch {
        $checksumThrew = $true
    }
    Assert-Equal $true $checksumThrew 'checksum validation should reject a mismatched digest'

    Write-Host 'test_optional_setup_and_source_build_are_opt_in'
    Assert-Equal $false ([bool]$ConfigureAlacritty) 'core install should not install an optional terminal by default'
    Assert-Equal $false ([bool]$ConfigureHotkey) 'core install should not add login persistence by default'
    Assert-Equal $false ([bool]$BuildFromSource) 'installer should not start a source build by default'
    $installText = Get-Content -LiteralPath $installScript -Raw
    Assert-True ($installText.Contains('will not start a long source build automatically')) 'missing release assets should produce an explicit source-build opt-in message'

    Write-Host 'test_upgrade_replaces_launcher_no_extra_path'
    $sourceDir = Join-Path $testRoot 'sources'
    New-Item -ItemType Directory -Path $sourceDir -Force | Out-Null
    $sourceV1 = Join-Path $sourceDir 'jcode-v1.exe'
    $sourceV2 = Join-Path $sourceDir 'jcode-v2.exe'
    Set-Content -Path $sourceV1 -Value 'version-one' -NoNewline
    Set-Content -Path $sourceV2 -Value 'version-two' -NoNewline
    Install-JcodeLauncher -SourcePath $sourceV1 -LauncherPath $launcherPath | Out-Null
    Install-JcodeLauncher -SourcePath $sourceV2 -LauncherPath $launcherPath | Out-Null
    Assert-Equal 'version-two' (Get-Content -Path $launcherPath -Raw) 'upgrade should replace launcher contents with the new build'
    $tempLaunchers = @(Get-ChildItem -LiteralPath $installDir -Filter '.jcode-launcher-*.tmp.exe' -Force -ErrorAction SilentlyContinue)
    Assert-Equal 0 $tempLaunchers.Count 'launcher upgrade should clean temporary files'
    $upgradePath = Resolve-JcodePathUpdate -InstallDir $installDir -CurrentPath $pathUpdate.Path
    Assert-Equal $false $upgradePath.Changed 'upgrade should not add another PATH entry when launcher dir is already present'
    Assert-PathCount $upgradePath.Path $installDir 1 'upgrade should preserve exactly one launcher PATH entry'

    Write-Host 'test_uninstall_removes_launcher_and_only_jcode_path'
    $removeCurrentPath = "$installDir;C:\Keep;$installVariant;C:\Keep"
    $removeUpdate = Resolve-JcodePathUpdate -InstallDir $installDir -CurrentPath $removeCurrentPath -RemoveOnly
    Assert-Equal 'C:\Keep;C:\Keep' $removeUpdate.Path 'uninstall path cleanup should remove only jcode-managed entries and preserve unrelated entries'
    Assert-Equal 2 $removeUpdate.RemovedManagedEntries 'uninstall path cleanup should remove all jcode launcher dir variants'
    Assert-PathCount $removeUpdate.Path $installDir 0 'uninstall path cleanup should leave no jcode launcher dir entries'

    Write-Host 'All Windows launcher install tests passed.' -ForegroundColor Green
} finally {
    if ($null -eq $originalLocalAppData) { Remove-Item Env:LOCALAPPDATA -ErrorAction SilentlyContinue } else { $env:LOCALAPPDATA = $originalLocalAppData }
    if ($null -eq $originalImportOnly) { Remove-Item Env:JCODE_INSTALL_PS1_IMPORT_ONLY -ErrorAction SilentlyContinue } else { $env:JCODE_INSTALL_PS1_IMPORT_ONLY = $originalImportOnly }
    Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue
}
