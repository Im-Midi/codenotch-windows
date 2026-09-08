# Run only on a disposable CI/VM account: this exercises real per-user installation.
$ErrorActionPreference = 'Stop'
if ($env:CI -ne 'true') { throw 'Run this test on a disposable CI account with CI=true.' }
$installer = Get-ChildItem "$PSScriptRoot/../target/release/bundle/nsis/*-setup.exe"
if (@($installer).Count -ne 1) { throw 'Expected one installer.' }
$installDir = Join-Path $env:LOCALAPPDATA 'Codenotch Test Zoë'
$settings = Join-Path $env:USERPROFILE '.claude/settings.json'
if (Test-Path $settings) { throw 'Refusing to overwrite existing Claude settings.' }
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if (Get-ItemProperty $runKey -Name Codenotch -ErrorAction SilentlyContinue) {
    throw 'Refusing to replace existing startup registration.'
}
function Run-Checked($exe, $arguments) {
    $process = Start-Process -FilePath $exe -ArgumentList $arguments -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) { throw "$exe exited with $($process.ExitCode)" }
}
Run-Checked $installer.FullName "/S /D=$installDir"
$app = Join-Path $installDir 'codenotch.exe'
$helper = Join-Path $installDir 'codenotch-hook.exe'
if (!(Test-Path $app) -or !(Test-Path $helper)) { throw 'Installed binaries missing.' }
foreach ($notice in @('LICENSE.txt', 'licenses/provider-marks.md')) {
    if (!(Test-Path (Join-Path $installDir $notice))) { throw "Missing bundled notice: $notice" }
}
New-Item -ItemType Directory -Force (Split-Path $settings) | Out-Null
'{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo keep"}]}]},"keep":true}' | Set-Content $settings
Run-Checked $app 'install-hooks'
$before = Get-Content $settings -Raw
if (!$before.Contains('codenotch-hook.exe')) { throw 'Installed helper was not wired.' }
Run-Checked $app 'autostart on'
$startup = (Get-ItemProperty $runKey).Codenotch

# Tauri's update mode replaces binaries without removing opted-in integrations.
Run-Checked $installer.FullName "/S /UPDATE /D=$installDir"
if ((Get-Content $settings -Raw) -ne $before) { throw 'Update changed hooks.' }
if ((Get-ItemProperty $runKey).Codenotch -ne $startup) { throw 'Update changed startup.' }

# Malformed settings must stop uninstall, leaving the app available for retry.
'{broken' | Set-Content $settings
$uninstaller = Join-Path $installDir 'uninstall.exe'
$failed = Start-Process $uninstaller -ArgumentList "/S _?=$installDir" -WindowStyle Hidden -Wait -PassThru
if ($failed.ExitCode -eq 0 -or !(Test-Path $app)) { throw 'Uninstall ignored invalid settings.' }
if ((Get-Content $settings -Raw).Trim() -ne '{broken') { throw 'Invalid settings were overwritten.' }
$before | Set-Content $settings
Run-Checked $uninstaller "/S _?=$installDir"
if (Test-Path $app) { throw 'Uninstall left the app behind.' }
$after = Get-Content $settings -Raw | ConvertFrom-Json
if (!$after.keep -or $after.hooks.Stop[0].hooks[0].command -ne 'echo keep') {
    throw 'Uninstall changed unrelated Claude settings.'
}
if ((Get-Content $settings -Raw).Contains('codenotch-hook.exe')) { throw 'Uninstall left broken hooks.' }
if (Get-ItemProperty $runKey -Name Codenotch -ErrorAction SilentlyContinue) {
    throw 'Uninstall left startup registration.'
}
Write-Host 'Install, update, failure preservation, and uninstall passed.'
