# Windows releases

The Windows workflow builds an x64 NSIS installer, runs workspace tests and a
real installation lifecycle test on a disposable Windows runner, then uploads
the installer and SHA-256 checksum as a build artifact. PR jobs have read-only
repository permissions. Only the upstream tag-release job can write releases.

## Publishing

1. Keep `codenotch/Cargo.toml` and `codenotch/tauri.conf.json` versions equal, and
   update `Cargo.lock` when the package version changes.
2. Push a corresponding `v<version>` tag to the upstream repository. The workflow
   rejects mismatches and creates a **draft** release only after checks pass.
3. Download and verify the draft installer on a clean Windows 11 x64 VM using
   the checklist below. Review the release notes, then publish the draft manually.
4. After the first release exists, replace the README's first-release notice with
   a prominent `[Download for Windows](https://github.com/Im-Midi/codenotch-windows/releases/latest)` link.

The workflow intentionally does not overwrite an existing release on rerun.
If a draft already exists, inspect its assets and remove the incomplete draft
before retrying. Do not replace assets in an already published release; publish
a new version instead.

Signing is not configured. Do not promise a warning-free installation. Adding
Authenticode signing requires maintainer-controlled signing credentials and a
separate change; no signing secrets are required for PR builds.

## Automated coverage

`scripts/test-installer.ps1` is restricted to disposable CI accounts. It installs
to a path containing spaces and Unicode, checks that both binaries exist, wires
hooks using the installed app, exercises `/UPDATE`, verifies that malformed
Claude settings stop uninstall without data loss, and checks final uninstall
preserves unrelated hooks while removing startup registration.

Rust tests additionally cover mixed hook groups and another installation's hooks.
The installer uses Tauri's existing lifecycle: `/UPDATE` keeps integrations;
an explicit uninstall removes them. Do not replace this with unconditional
cleanup during upgrades.

## Manual release gate

- Clean Windows 11 x64 VM, no Rust or C++ development tools: install as a standard
  user and launch from Start. Confirm notch and tray render; test with no providers
  installed and then with a signed-in supported provider.
- VM without WebView2: confirm setup downloads the runtime. Repeat with network
  disconnected and confirm the failure is visible and retry succeeds.
- Real Unicode Windows username: install, enable hooks and startup, log out/in,
  and confirm startup and hook events work.
- Upgrade from the previous published version into the same directory. Preserve
  user settings and integrations; check normal interactive upgrade as well as
  `/S /UPDATE`. If the user explicitly chooses uninstall-first, integrations are
  removed and must be re-enabled after installation.
- Uninstall with the app running: cancellation must not remove hooks. Retry,
  permit the app to close, and verify app/shortcuts/startup are removed while
  unrelated Claude settings and Codenotch preferences remain.

Hosted CI is not a substitute for these checks: its account may have elevated
permissions and WebView2 preinstalled.
