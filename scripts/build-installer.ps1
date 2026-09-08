$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..')
try {
    # The helper must exist before Tauri resolves bundle resources.
    cargo build --locked --release --package codenotch-hook
    if ($LASTEXITCODE -ne 0) { throw 'Hook build failed.' }
    Push-Location codenotch
    try {
        tauri build --ci --bundles nsis -- --locked
        if ($LASTEXITCODE -ne 0) { throw 'Installer build failed.' }
    } finally { Pop-Location }
} finally { Pop-Location }
