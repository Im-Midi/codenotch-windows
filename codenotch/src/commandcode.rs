//! Command Code usage adapter, implemented from the upstream Codenotch's documented behaviour
//! (Sources/Providers/CommandCode{Credentials,Provider,Usage}.swift).
//!
//! The key lives in `~/.commandcode/auth.json` (`apiKey`), the same file the Command Code desktop
//! app writes on login — borrowed read-only, never refreshed here. `COMMAND_CODE_API_KEY` overrides
//! it, matching the desktop harness.
//!
//! Four GET calls against the GOAT plan's `/alpha` endpoints, `Authorization: Bearer <apiKey>`:
//!   1. `/alpha/whoami` → `org.id`
//!   2. `/alpha/billing/credits?orgId=…` → `{ credits:{monthlyCredits}, windowLimits:{fiveHour,weekly} }`
//!   3. `/alpha/billing/subscriptions?orgId=…` → `{ data:{planId,currentPeriodStart,currentPeriodEnd} }`
//!   4. `/alpha/usage/summary?orgId=…&since=<periodStart>` → `{ totalCost, … }`
//! The ring is monthly spend over cap (totalCost + monthlyCredits); five-hour/weekly windows ride
//! along when the account has them. A cap of 0 means nothing metered yet: no windows, no numbers invented.

use crate::usage::{LimitWindow, UsageSnapshot};
use crate::AppState;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const POLL_SECS: u64 = 300;
const BACKOFF_MIN_SECS: u64 = 60;
const WHOAMI: &str = "https://api.commandcode.ai/alpha/whoami";
const CREDITS: &str = "https://api.commandcode.ai/alpha/billing/credits";
const SUBSCRIPTIONS: &str = "https://api.commandcode.ai/alpha/billing/subscriptions";
const SUMMARY: &str = "https://api.commandcode.ai/alpha/usage/summary";

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

fn auth_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".commandcode").join("auth.json"))
}

fn store_path() -> PathBuf {
    crate::config::config_path().with_file_name("commandcode.json")
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

/// Is Command Code present (auth file on disk, or the env override set)? If not, no cell is shown.
pub fn present() -> bool {
    std::env::var_os("COMMAND_CODE_API_KEY").is_some()
        || auth_path().map(|p| p.is_file()).unwrap_or(false)
}

fn load_api_key() -> Option<String> {
    if let Some(env) = std::env::var_os("COMMAND_CODE_API_KEY") {
        let s = env.to_string_lossy().trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    let text = std::fs::read_to_string(auth_path()?).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let key = v.get("apiKey")?.as_str()?.trim().to_string();
    if key.is_empty() {
        None
    } else {
        Some(key)
    }
}

enum FetchErr {
    NeedsAuth,
    RateLimited(u64),
    Other(String),
}

fn get(url: &str, token: &str) -> Result<serde_json::Value, FetchErr> {
    let resp = ureq::get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .set("User-Agent", "command-code-desktop")
        .set("x-command-code-version", "desktop")
        .timeout(Duration::from_secs(15))
        .call();
    match resp {
        Ok(r) => r.into_json().map_err(|e| FetchErr::Other(format!("parse: {e}"))),
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => Err(FetchErr::NeedsAuth),
        Err(ureq::Error::Status(429, r)) => {
            let ra = r.header("retry-after").and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
            Err(FetchErr::RateLimited(ra.max(BACKOFF_MIN_SECS)))
        }
        Err(ureq::Error::Status(code, _)) => Err(FetchErr::Other(format!("HTTP {code}"))),
        Err(e) => Err(FetchErr::Other(format!("{e}"))),
    }
}

fn with_query(url: &str, pairs: &[(&str, Option<&str>)]) -> String {
    let items: Vec<String> = pairs
        .iter()
        .filter_map(|(k, v)| v.filter(|s| !s.is_empty()).map(|s| format!("{k}={}", urlenc(s))))
        .collect();
    if items.is_empty() {
        url.to_string()
    } else {
        format!("{url}?{}", items.join("&"))
    }
}

fn urlenc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn num(v: Option<&serde_json::Value>) -> Option<f64> {
    v.and_then(|x| x.as_f64())
}

/// ISO 8601, unix seconds, or unix milliseconds. Zero/absent is "no reset".
fn parse_date(v: Option<&serde_json::Value>) -> Option<u64> {
    if let Some(s) = v.and_then(|x| x.as_str()) {
        return chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis().max(0) as u64);
    }
    let n = num(v)?;
    if n <= 0.0 {
        return None;
    }
    Some(if n > 1_000_000_000_000.0 { n as u64 } else { (n * 1000.0) as u64 })
}

fn window_from(entry: Option<&serde_json::Value>, id: &str, label: &str) -> Option<LimitWindow> {
    let entry = entry?;
    let cap = num(entry.get("cap")).filter(|c| *c > 0.0)?;
    let used = num(entry.get("used")).unwrap_or(0.0);
    Some(LimitWindow {
        id: id.into(),
        label: label.into(),
        used: (used / cap).clamp(0.0, 1.0),
        resets_at: parse_date(entry.get("resetAt")),
        ..Default::default()
    })
}

fn plan_name(plan_id: Option<&str>) -> Option<String> {
    let id = plan_id?;
    if id.is_empty() {
        return None;
    }
    let key = id.to_ascii_lowercase().replace('-', "_");
    Some(if key.contains("goat") { "GOAT".into() } else { id.to_string() })
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
    let Some(token) = load_api_key() else {
        snap.status = "needsAuth".into();
        snap.note = "No Command Code credential found".into();
        return snap;
    };
    let result = (|| -> Result<UsageSnapshot, FetchErr> {
        let whoami = get(WHOAMI, &token)?;
        let org_id = whoami.pointer("/org/id").and_then(|x| x.as_str()).map(String::from);

        let credits_url = with_query(CREDITS, &[("orgId", org_id.as_deref())]);
        let subs_url = with_query(SUBSCRIPTIONS, &[("orgId", org_id.as_deref())]);
        let credits = get(&credits_url, &token)?;
        let subscription = get(&subs_url, &token)?;

        let sub_data = subscription.get("data").cloned().unwrap_or_else(|| subscription.clone());
        let plan = plan_name(sub_data.get("planId").and_then(|x| x.as_str()));
        let period_start = sub_data
            .get("currentPeriodStart")
            .and_then(parse_iso_or_epoch);
        let period_end = parse_date(sub_data.get("currentPeriodEnd"));

        let summary_url = with_query(SUMMARY, &[("orgId", org_id.as_deref()), ("since", period_start.as_deref())]);
        let summary = get(&summary_url, &token)?;

        let credits_obj = credits.get("credits").cloned().unwrap_or_default();
        let limits = credits.get("windowLimits").cloned().unwrap_or_default();
        let used = num(summary.get("totalCost")).unwrap_or(0.0);
        let remaining = num(credits_obj.get("monthlyCredits")).unwrap_or(0.0);
        let cap = if used > 0.0 || remaining > 0.0 { used + remaining } else { 0.0 };

        let mut windows = Vec::new();
        if cap > 0.0 {
            windows.push(LimitWindow {
                id: "monthly".into(),
                label: "Monthly limit".into(),
                used: (used / cap).clamp(0.0, 1.0),
                resets_at: period_end,
                ..Default::default()
            });
        }
        if let Some(w) = window_from(limits.get("fiveHour"), "fiveHour", "5h limit") {
            windows.push(w);
        }
        if let Some(w) = window_from(limits.get("weekly"), "weekly", "Weekly limit") {
            windows.push(w);
        }

        let mut s = UsageSnapshot::default();
        if windows.is_empty() {
            s.status = "none".into();
            s.note = "Command Code has nothing metered on this account yet".into();
        } else {
            s.status = "ok".into();
            s.windows = windows;
            s.fetched_at = now_ms();
            s.note = plan.unwrap_or_default();
        }
        Ok(s)
    })();

    match result {
        Ok(s) => {
            BACKOFF_UNTIL.store(0, std::sync::atomic::Ordering::Relaxed);
            s
        }
        Err(FetchErr::NeedsAuth) => {
            snap.status = "needsAuth".into();
            snap.note = "Command Code rejected its sign-in — sign in to Command Code again".into();
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
            crate::applog(&format!("commandcode: live read failed ({msg})"));
            snap.status = "error".into();
            snap.note = msg;
            snap
        }
    }
}

/// `currentPeriodStart` accepted either as RFC3339 or epoch seconds/ms, re-emitted as RFC3339
/// (`since` on the summary endpoint expects a timestamp string).
fn parse_iso_or_epoch(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
            return Some(s.to_string());
        }
    }
    let ms = parse_date(Some(v))?;
    let secs = (ms / 1000) as i64;
    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.to_rfc3339())
}

fn broadcast(app: &AppHandle, snap: UsageSnapshot) {
    let st = app.state::<AppState>();
    *st.commandcode.lock().unwrap() = snap.clone();
    persist(&snap);
    let _ = app.emit("commandcode", &snap);
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        {
            let st = app.state::<AppState>();
            let snap = st.commandcode.lock().unwrap().clone();
            let _ = app.emit("commandcode", &snap);
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

/// For doctor: contains no secrets
pub fn probe() -> String {
    let auth = match load_api_key() {
        Some(k) => format!("auth usable ({} chars)", k.len()),
        None if auth_path().map(|p| p.is_file()).unwrap_or(false) => "auth.json present but has no apiKey".to_string(),
        None => "auth.json not found".to_string(),
    };
    format!("Command Code: {auth}")
}
