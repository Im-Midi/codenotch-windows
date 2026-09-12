//! 9Router usage adapter (github.com/decolua/9router) — no upstream Codenotch provider exists for
//! this one; implemented directly from 9Router's own source (it is open source, MIT-ish, self-hosted).
//!
//! 9Router is a local proxy/dashboard, not a subscription with a published cap, so there is no
//! percentage to show — same rule as Antigravity's "requests today" cell: a count-only window,
//! never a fabricated fraction.
//!
//! Auth: 9Router's own middleware (src/dashboardGuard.js) accepts a `x-9r-cli-token` header equal to
//! `sha256(rawMachineId + "9r-cli-auth" + cliSecret)[:16]` (hex). `rawMachineId` and `cliSecret` are
//! both written by 9Router itself to plain files under its data dir on first run
//! (`<data>/machine-id`, `<data>/auth/cli-secret`) — read only, borrowed the same way every other
//! provider here borrows a credential, never generated or written by this process.
//! Data dir (src/lib/dataDir.js): `%APPDATA%\9router` on Windows, unless `DATA_DIR` is set to a
//! non-Unix-style path.
//!
//! Endpoint: GET http://127.0.0.1:<port>/api/usage/stats?period=today (src/app/api/usage/stats),
//! default port 20128 (9Router's PORT env, defaulted the same way here).
//! Reply: { totalRequests, totalCost, byProvider:{ id: {requests,cost,...} }, ... } (src/lib/db/repos/usageRepo.js).

use crate::usage::{LimitWindow, UsageSnapshot};
use crate::AppState;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const POLL_SECS: u64 = 120; // a local proxy's own counters — cheap to poll more often than a vendor API
const BACKOFF_MIN_SECS: u64 = 30;
const DEFAULT_PORT: u16 = 20128;

static REFRESH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static BACKOFF_UNTIL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn request_refresh() {
    REFRESH.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// `%APPDATA%\9router`, or `DATA_DIR` when set to a path that isn't Unix-style-absolute on Windows
/// (9Router's own dataDir.js falls back the same way when a Linux-targeted .env leaks in).
fn data_dir() -> Option<PathBuf> {
    if let Some(env) = std::env::var_os("DATA_DIR") {
        let p = PathBuf::from(&env);
        let looks_unix = env.to_string_lossy().starts_with('/');
        if !looks_unix && p.is_dir() {
            return Some(p);
        }
    }
    dirs::config_dir().map(|d| d.join("9router"))
}

fn port() -> u16 {
    std::env::var("PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_PORT)
}

fn store_path() -> PathBuf {
    crate::config::config_path().with_file_name("router9.json")
}

pub fn load_persisted() -> UsageSnapshot {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|t| serde_json::from_str::<UsageSnapshot>(&t).ok())
        .map(|mut s| {
            if !s.windows.is_empty() {
                s.status = "stale".into();
            }
            BACKOFF_UNTIL.store(s.backoff_until, std::sync::atomic::Ordering::Relaxed);
            s
        })
        .unwrap_or_default()
}

fn persist(s: &UsageSnapshot) {
    if let Ok(t) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(store_path(), t);
    }
}

/// Is 9Router installed (its data dir has ever been created)? If not, no cell is shown.
pub fn present() -> bool {
    data_dir().map(|d| d.is_dir()).unwrap_or(false)
}

/// The two files 9Router itself persists to authenticate its own CLI (shared/utils/machineId.js);
/// both must exist for a token to be computable.
fn cli_token() -> Option<String> {
    let dir = data_dir()?;
    let raw = std::fs::read_to_string(dir.join("machine-id")).ok()?.trim().to_string();
    let secret = std::fs::read_to_string(dir.join("auth").join("cli-secret")).ok()?.trim().to_string();
    if raw.is_empty() || secret.is_empty() {
        return None;
    }
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hasher.update(b"9r-cli-auth");
    hasher.update(secret.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    Some(hex[..16].to_string())
}

enum FetchErr {
    NeedsAuth,
    RateLimited(u64),
    Other(String),
}

fn fetch_stats(token: &str) -> Result<serde_json::Value, FetchErr> {
    let url = format!("http://127.0.0.1:{}/api/usage/stats?period=today", port());
    let resp = ureq::get(&url)
        .set("x-9r-cli-token", token)
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(8))
        .call();
    match resp {
        Ok(r) => r.into_json().map_err(|e| FetchErr::Other(format!("parse: {e}"))),
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => Err(FetchErr::NeedsAuth),
        Err(ureq::Error::Status(429, r)) => {
            let ra = r.header("retry-after").and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
            Err(FetchErr::RateLimited(ra.max(BACKOFF_MIN_SECS)))
        }
        Err(ureq::Error::Status(code, _)) => Err(FetchErr::Other(format!("HTTP {code}"))),
        // The server not running (connection refused) lands here too — same message either way
        Err(e) => Err(FetchErr::Other(format!("{e}"))),
    }
}

fn read_once() -> UsageSnapshot {
    let mut snap = UsageSnapshot::default();
    let held_until = BACKOFF_UNTIL.load(std::sync::atomic::Ordering::Relaxed);
    let now = now_ms();
    if held_until > now {
        snap.backoff_until = held_until;
        snap.status = "stale".into();
        snap.note = format!("Rate limited — retrying in {}s", (held_until - now) / 1000);
        return snap;
    }
    let Some(token) = cli_token() else {
        snap.status = "needsAuth".into();
        snap.note = "9Router has not run yet on this machine (no machine-id/cli-secret)".into();
        return snap;
    };
    match fetch_stats(&token) {
        Ok(v) => {
            BACKOFF_UNTIL.store(0, std::sync::atomic::Ordering::Relaxed);
            let requests = v.get("totalRequests").and_then(|x| x.as_i64()).unwrap_or(0);
            let cost = v.get("totalCost").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let providers = v
                .get("byProvider")
                .and_then(|x| x.as_object())
                .map(|o| o.len())
                .unwrap_or(0);
            snap.status = "ok".into();
            snap.fetched_at = now_ms();
            snap.windows = vec![LimitWindow {
                id: "today".into(),
                label: "Requests today".into(),
                used: 0.0,
                resets_at: None,
                count: Some(requests),
                derived: true,
            }];
            snap.note = if providers > 0 {
                format!("${cost:.2} today across {providers} provider{}", if providers == 1 { "" } else { "s" })
            } else {
                format!("${cost:.2} today")
            };
            snap
        }
        Err(FetchErr::NeedsAuth) => {
            snap.status = "needsAuth".into();
            snap.note = "9Router rejected the local CLI token — restart 9Router once to regenerate it".into();
            snap
        }
        Err(FetchErr::RateLimited(secs)) => {
            let until = now_ms() + secs * 1000;
            BACKOFF_UNTIL.store(until, std::sync::atomic::Ordering::Relaxed);
            snap.backoff_until = until;
            snap.status = "stale".into();
            snap.note = format!("Rate limited — retrying in {secs}s");
            snap
        }
        Err(FetchErr::Other(msg)) => {
            // Most common cause: 9Router installed but its server is not currently running
            snap.status = "none".into();
            snap.note = format!("9Router not reachable on 127.0.0.1:{} ({msg})", port());
            snap
        }
    }
}

fn broadcast(app: &AppHandle, snap: UsageSnapshot) {
    let st = app.state::<AppState>();
    *st.router9.lock().unwrap() = snap.clone();
    persist(&snap);
    let _ = app.emit("router9", &snap);
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        {
            let st = app.state::<AppState>();
            let snap = st.router9.lock().unwrap().clone();
            let _ = app.emit("router9", &snap);
        }
        if !present() {
            broadcast(&app, UsageSnapshot { status: "absent".into(), ..Default::default() });
            loop {
                for _ in 0..600 {
                    if REFRESH.swap(false, std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
                if present() {
                    break;
                }
            }
        }
        loop {
            let snap = read_once();
            let hold = snap.backoff_until.saturating_sub(now_ms()) / 1000;
            broadcast(&app, snap);
            for _ in 0..POLL_SECS.max(hold) {
                if REFRESH.swap(false, std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
}

/// For doctor: contains no secrets (the token itself is a derived hash, not printed)
pub fn probe() -> String {
    let dir = data_dir();
    let dir_ok = dir.as_ref().map(|d| d.is_dir()).unwrap_or(false);
    let token_ok = cli_token().is_some();
    format!(
        "9Router: data dir {} ({}), CLI token {}",
        dir.map(|d| d.display().to_string()).unwrap_or_else(|| "?".into()),
        if dir_ok { "found" } else { "not found" },
        if token_ok { "computable" } else { "unavailable (machine-id/cli-secret missing)" }
    )
}
