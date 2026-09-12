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
//! A 9Router published on the internet is often behind Cloudflare Access (Zero Trust), which turns
//! every request — even 9Router's public /api/health — into a redirect to its sign-in page. That is
//! answered with a service token (`CF-Access-Client-Id` / `CF-Access-Client-Secret`), sent alongside
//! the CLI token when one is saved in the API-keys window.
//!
//! Endpoint: GET <base>/api/usage/stats?period=today (src/app/api/usage/stats),
//! default base http://127.0.0.1:20128 (9Router's PORT env, defaulted the same way here).
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

/// Is there a 9Router to read — installed locally, or one set up in the API-keys window?
pub fn present() -> bool {
    crate::secrets::get(crate::secrets::ROUTER9_URL).is_some()
        || crate::secrets::get(crate::secrets::ROUTER9_TOKEN).is_some()
        || data_dir().map(|d| d.is_dir()).unwrap_or(false)
}

/// Base URL of the 9Router to read; a remote one can be named in the API-keys window.
pub fn base_url() -> String {
    crate::secrets::get(crate::secrets::ROUTER9_URL)
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", port()))
}

/// Where the token in use comes from: pasted into the API-keys window, or computed from this PC's files
pub fn token_source() -> Option<&'static str> {
    if crate::secrets::get(crate::secrets::ROUTER9_TOKEN).is_some() {
        Some("saved")
    } else if local_token().is_some() {
        Some("local")
    } else {
        None
    }
}

/// A pasted token wins — the only way to read a 9Router on another machine, whose files are local to it.
fn cli_token() -> Option<String> {
    crate::secrets::get(crate::secrets::ROUTER9_TOKEN).or_else(local_token)
}

/// Cloudflare Access service token (client id, client secret), when one is saved
pub fn cf_access() -> Option<(String, String)> {
    Some((
        crate::secrets::get(crate::secrets::ROUTER9_CF_ID)?,
        crate::secrets::get(crate::secrets::ROUTER9_CF_SECRET)?,
    ))
}

/// The token this PC's own 9Router accepts, derived from the two files it persists to authenticate
/// its CLI (shared/utils/machineId.js); both must exist for it to be computable.
pub fn local_token() -> Option<String> {
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
    /// Cloudflare Access answered instead of 9Router — its sign-in page, or a refusal of the service
    /// token. `with_token` tells the two pieces of advice apart.
    AccessBlocked { with_token: bool },
    Other(String),
}

fn access_url(s: &str) -> bool {
    s.contains("cloudflareaccess.com") || s.contains("/cdn-cgi/access/")
}

/// Host part only: a redirect target can carry a signed token in its query string
fn host_of(url: &str) -> &str {
    url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("")
}

fn access_message(with_token: bool) -> String {
    if with_token {
        "Cloudflare Access refused the service token — check the 9Router application has a Service Auth policy that includes it".into()
    } else {
        "This URL is behind Cloudflare Access, which answered with its sign-in page instead of 9Router. Add a service token under “Behind Cloudflare Access?”".into()
    }
}

fn fetch_stats(token: &str) -> Result<serde_json::Value, FetchErr> {
    fetch_stats_at(&base_url(), token, cf_access())
}

fn fetch_stats_at(base: &str, token: &str, cf: Option<(String, String)>) -> Result<serde_json::Value, FetchErr> {
    let url = format!("{base}/api/usage/stats?period=today");
    // Redirects are not followed. 9Router's API never redirects, so a 3xx means something in front
    // of it answered — and following it lands on a sign-in page that then fails as "bad JSON", which
    // is exactly the unhelpful error this used to show for a 9Router behind Cloudflare Access.
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(10)).build();
    let with_token = cf.is_some();
    let mut req = agent.get(&url).set("x-9r-cli-token", token).set("Accept", "application/json");
    if let Some((id, secret)) = &cf {
        req = req.set("CF-Access-Client-Id", id).set("CF-Access-Client-Secret", secret);
    }
    match req.call() {
        Ok(r) => {
            if (300..400).contains(&r.status()) {
                let loc = r.header("location").unwrap_or("").to_string();
                return Err(if access_url(&loc) {
                    FetchErr::AccessBlocked { with_token }
                } else {
                    FetchErr::Other(format!("it redirects to {} — check the address", host_of(&loc)))
                });
            }
            let is_json = r.content_type().contains("json");
            let body = r.into_string().map_err(|e| FetchErr::Other(format!("read: {e}")))?;
            if !is_json {
                if body.contains("Cloudflare Access") {
                    return Err(FetchErr::AccessBlocked { with_token });
                }
                return Err(FetchErr::Other("it answered with a web page, not 9Router's API — check the address".into()));
            }
            serde_json::from_str(&body).map_err(|e| FetchErr::Other(format!("unexpected reply: {e}")))
        }
        // 9Router's own refusals are JSON; an HTML 401/403 comes from Cloudflare in front of it
        Err(ureq::Error::Status(401 | 403, r)) => {
            if r.content_type().contains("json") {
                Err(FetchErr::NeedsAuth)
            } else {
                Err(FetchErr::AccessBlocked { with_token })
            }
        }
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
        snap.note = "No CLI token: 9Router has not run on this PC, and none is saved in the API-keys window".into();
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
            snap.note = "9Router rejected the CLI token".into();
            snap
        }
        Err(FetchErr::AccessBlocked { with_token }) => {
            snap.status = "needsAuth".into();
            snap.note = access_message(with_token);
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
            // Most common cause locally: 9Router installed but its server is not currently running
            snap.status = "none".into();
            snap.note = format!("Could not read 9Router at {}: {msg}", base_url());
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

pub struct TestFail {
    pub message: String,
    /// Cloudflare Access answered: the window opens its service-token fields
    pub access_blocked: bool,
}

/// One read on demand, for the API-keys window's "Save & test"
pub fn test() -> Result<String, TestFail> {
    let fail = |message: String| TestFail { message, access_blocked: false };
    let Some(token) = cli_token() else {
        return Err(fail("No CLI token — paste one, or open this PC's 9Router dashboard once".into()));
    };
    match fetch_stats(&token) {
        Ok(v) => {
            let n = v.get("totalRequests").and_then(|x| x.as_i64()).unwrap_or(0);
            Ok(format!("Connected to {} · {n} request{} today", base_url(), if n == 1 { "" } else { "s" }))
        }
        Err(FetchErr::NeedsAuth) => Err(fail("Reached 9Router, but it rejected this CLI token".into())),
        Err(FetchErr::AccessBlocked { with_token }) => {
            Err(TestFail { message: access_message(with_token), access_blocked: true })
        }
        Err(FetchErr::RateLimited(s)) => Err(fail(format!("9Router is rate limiting — retry in {s}s"))),
        Err(FetchErr::Other(e)) => Err(fail(format!("Saved, but could not read {}: {e}", base_url()))),
    }
}

/// For doctor: contains no secrets (the token itself is never printed)
pub fn probe() -> String {
    let dir = data_dir();
    let dir_ok = dir.as_ref().map(|d| d.is_dir()).unwrap_or(false);
    format!(
        "9Router: url {} | data dir {} ({}), CLI token {}, Cloudflare Access token {}",
        base_url(),
        dir.map(|d| d.display().to_string()).unwrap_or_else(|| "?".into()),
        if dir_ok { "found" } else { "not found" },
        match token_source() {
            Some("saved") => "saved in the API-keys window",
            Some(_) => "computed from local files",
            None => "unavailable (9Router has not run here; paste a token in the API-keys window for a remote one)",
        },
        if cf_access().is_some() { "saved" } else { "none" }
    )
}

#[cfg(test)]
mod tests {
    /// Set CODENOTCH_TEST_R9_URL to a 9Router published behind Cloudflare Access (no service token).
    #[test]
    #[ignore = "needs CODENOTCH_TEST_R9_URL: a 9Router behind Cloudflare Access"]
    fn access_sign_in_is_recognised_not_parsed_as_json() {
        let url = std::env::var("CODENOTCH_TEST_R9_URL").expect("set CODENOTCH_TEST_R9_URL");
        let r = super::fetch_stats_at(url.trim_end_matches('/'), "0000000000000000", None);
        assert!(matches!(r, Err(super::FetchErr::AccessBlocked { with_token: false })));
    }

    #[test]
    fn redirect_host_drops_the_signed_query() {
        assert_eq!(super::host_of("https://team.cloudflareaccess.com/cdn-cgi/access/login/x?meta=eyJhbGci"), "team.cloudflareaccess.com");
    }
}
