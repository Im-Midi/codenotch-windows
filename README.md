# Codenotch for Windows

A Windows 11 x64 desktop widget for AI usage and activity, built with **Rust + Tauri 2** and plain HTML/CSS/JavaScript. Choose a monitor and any of its four edges; the usage card opens inward. Based on the existing Windows port of [vinzdg/codenotch](https://github.com/vinzdg/codenotch).

## Run

Use `published/codenotch.exe`, or install `published/Codenotch_0.4.1_x64-setup.exe` when available. WebView2 is required; the installer checks for it. The installer is not code-signed.

```powershell
.\published\codenotch.exe --settings
# Explicit demo mode, from a separate PowerShell session:
$env:CODENOTCH_DATA_DIR = "$PWD\output\demo-state"
.\published\codenotch.exe --demo --settings
```

Only one instance runs at a time. Quit from the tray before switching between demo and live mode. Settings are available through the small gear beside the notch and the tray. The focusable Settings window also provides keyboard access to usage details. Escape closes it.

Choose the display, edge, along-edge position, size, taskbar avoidance, topmost/full-screen behavior, optional collapse and reduced motion. Enable/reorder providers and choose the Codex headline window. Dragging is opt-in. If the selected display disconnects, the widget uses the primary display until it returns, retaining the preference.

Start with Windows and Claude Code hook installation are opt-in tray actions. Building/running the app does not enable either. No administrator rights are needed for normal use.

## Data

- **Codex:** reads local sign-in credentials for the internal ChatGPT usage endpoint; falls back to timestamped rollout quota records. Weekly usage is preferred when available. Activity comes from the local desktop database and rollout events; heuristic states are labeled inferred and ambiguous tool states remain unknown.
- **Claude:** existing OAuth usage reader, optional Claude Code hooks and transcript watcher.
- **Cursor:** existing local editor session and usage reader.
- **Antigravity:** existing local bridge/Google reader, with a visibly derived request count when quota is unavailable.

Credentials stay in the native backend and are never sent to the WebView. Sign in through the owning application. These internal endpoints and file formats can change independently of this app. Unavailable data is shown as unavailable; cached/expired readings remain visibly stale. Codex refreshes approximately every five minutes; use Refresh readings for an earlier update. Refresh respects rate-limit cooldowns. Disabling a provider stops its periodic reads after any current read finishes.

Codex profile precedence: the absolute folder set in Settings, then `CODEX_HOME`, then `%USERPROFILE%\.codex`. Changing the Settings profile restarts Codenotch. Cache files are separated by profile.

Settings, numeric usage caches and local diagnostics live in `%APPDATA%\codenotch`. `CODENOTCH_DATA_DIR` overrides this location for testing. Run `codenotch.exe doctor` for a local `doctor.log` in that folder. Diagnostics omit credential values but may contain local paths; inspect before sharing. Provider SVG/PNG overrides can be placed in its `glyphs` folder.

## Build and checks

Install Rust stable with the **MSVC** toolchain, Visual Studio C++ build tools, Node.js/npm, and WebView2. Rustup's minimal profile is sufficient. Use PowerShell 7:

```powershell
pwsh -File .\build.ps1 -Test              # Rust + presentation checks, debug build
pwsh -File .\build.ps1 -Release           # release executable and hook helper
pwsh -File .\build.ps1 -Package           # release executable and NSIS installer
```

The packaging script installs the pinned Tauri CLI locally with `npm ci` if missing. It builds the hook helper first and includes it plus license notices in the installer. Output is copied to `published/`; generated build/test files stay in `target/` and `output/`.

`tests/desktop-smoke.cjs` is an additional native WebView check. It requires Playwright's library and an isolated `--demo --settings` instance launched with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9227`. It exercises the real Settings UI on all attached displays, all edges and four widget sizes, then restores the original demo configuration. Never enable remote debugging for routine use.

See [the HTML plan and implementation record](docs/windows-desktop-plan.html) for delivered scope, validation evidence and remaining hardware acceptance checks. App size scaling is separate from physical monitor DPI.

## Source and attribution

Imported the `windows/` subtree of `vinzdg/codenotch` at commit `0a6c6fb62b7fda52e4f8bd1ce7e8c7e7b8595b75`, then extended it locally for configurable monitor/edge placement, settings, native input regions, provider handling and packaging. This folder is part of the Utilities workspace; it is not a separate Git checkout.

MIT: see `LICENSE`. The upstream Windows port credits [Im-Midi/codenotch-windows](https://github.com/Im-Midi/codenotch-windows), and its session engine originated in [Im-Midi/Pac-Man](https://github.com/Im-Midi/Pac-Man). Embedded provider artwork is from [Lobe Icons](https://github.com/lobehub/lobe-icons), MIT; see `codenotch/glyphs/NOTICE.md`. Product names and marks belong to their respective owners.
