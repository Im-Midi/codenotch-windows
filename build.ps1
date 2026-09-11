param([switch]$Release, [switch]$Package, [switch]$Test)
$ErrorActionPreference = 'Stop'
$env:Path = (Join-Path $env:USERPROFILE '.cargo/bin') + ';' + $env:Path
Push-Location $PSScriptRoot
try {
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Install Rust with the MSVC toolchain first. See README.md.' }
    if ($Test) {
        & cargo test --workspace --locked
        if ($LASTEXITCODE -ne 0) { throw 'Rust checks failed.' }
        & node tests/presentation.test.cjs
        if ($LASTEXITCODE -ne 0) { throw 'Presentation checks failed.' }
    }
    if ($Package) {
        & cargo build --release --locked --package codenotch-hook
        if ($LASTEXITCODE -ne 0) { throw 'Hook build failed.' }
        if (-not (Test-Path node_modules/.bin/tauri.cmd)) {
            & npm ci --ignore-scripts
            if ($LASTEXITCODE -ne 0) { throw 'Tauri CLI installation failed.' }
        }
        Push-Location codenotch
        try { & ../node_modules/.bin/tauri.cmd build --config tauri.package.conf.json --bundles nsis }
        finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw 'Installer build failed.' }
    } else {
        $buildArgs = @('build', '--workspace', '--locked')
        if ($Release) { $buildArgs += '--release' }
        & cargo @buildArgs
        if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }
    }
    if ($Release -or $Package) {
        New-Item -ItemType Directory -Force published | Out-Null
        Copy-Item target/release/codenotch.exe, target/release/codenotch-hook.exe, LICENSE -Destination published
        Copy-Item codenotch/glyphs/NOTICE.md published/ARTWORK-NOTICE.md
        if ($Package) { Copy-Item target/release/bundle/nsis/*-setup.exe -Destination published }
        Write-Output "Published files: $PSScriptRoot\published"
    }
} finally { Pop-Location }
