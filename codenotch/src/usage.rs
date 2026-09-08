//! Claude usage adapter (official), implemented from the upstream Codenotch's documented behaviour.
//!
//! Two data paths, CLI first (mirrors the macOS original's `ClaudeOAuthProvider`):
//!   1. Live: `claude "/usage"`, asked of the installed CLI. It answers off a credential Claude Code
//!      already holds and refreshes on its own — this app never has to know whether the on-disk
//!      token is stale. Costs a subprocess, so an answer is cached for 5 minutes and a failed
//!      attempt is not retried inside that window either.
//!   2. Fallback (CLI not installed, or it declined to answer): GET
//!      https://api.anthropic.com/api/oauth/usage with the token Claude Code keeps in
//!      ~/.claude/.credentials.json. 401/403 → re-read the credential once and retry (Claude Code
//!      may have just refreshed it) → still failing means needsAuth. 429 → back off 60 s × 2^n
//!      capped at 15 min, Retry-After only raises it; the deadline is persisted.
//! Neither path ever invents a percentage on failure: keep the last reading marked stale, and the
//! UI shows how old it is.
//! Endpoint reply (snake_case): { limits:[{kind,percent,resets_at}], five_hour:{utilization,resets_at}, seven_day:{...} }
//! limits is the forward-compatible main shape; five_hour/seven_day are merged in as a fallback (a window that just rolled over disappears from limits).

use crate::AppState;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const POLL_ACTIVE_SECS: u64 = 60;
const POLL_IDLE_SECS: u64 = 300;
const BACKOFF_BASE_SECS: u64 = 60;
const BACKOFF_CAP_SECS: u64 = 900;
const CLI_REFRESH_MS: u64 = 5 * 60 * 1000;
const CLI_TIMEOUT_SECS: u64 = 20;

static REFRESH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Immediate refresh from the tray or a command
pub fn request_refresh() {
    REFRESH.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Sleep in slices so request_refresh can interrupt it
fn sleep_interruptible(total_secs: u64) {
    for _ in 0..total_secs {
        if REFRESH.swap(false, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LimitWindow {
    pub id: String,
    pub label: String,
    /// 0.0–1.0 (fraction used)
    pub used: f64,
    /// Reset time, ms epoch (None = unknown)
    pub resets_at: Option<u64>,
    /// Pure count window (no published denominator, e.g. Antigravity's requests today) — the cell shows ~N and the ring draws only its track
    #[serde(default)]
    pub count: Option<i64>,
    /// The number is ours, not the vendor's (upstream fidelity=.derived) — the card adds a ~ prefix
    #[serde(default)]
    pub derived: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UsageSnapshot {
    /// ok | stale | needsAuth | backoff | error
    pub status: String,
    pub windows: Vec<LimitWindow>,
    pub fetched_at: u64,
    pub note: String,
    #[serde(default)]
    pub backoff_until: u64,
}

fn store_path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("usage.json")
}

pub fn load_persisted() -> UsageSnapshot {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|t| serde_json::from_str::<UsageSnapshot>(&t).ok())
        .map(|mut s| {
            if !s.windows.is_empty() {
                s.status = "stale".into(); // an old reading after a restart is labelled as such
            }
            s
        })
        .unwrap_or_default()
}

fn persist(s: &UsageSnapshot) {
    if let Ok(t) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(store_path(), t);
    }
}

/// Reads Claude Code's OAuth credential. Returns (token, expired hint).
fn read_credentials() -> Option<(String, bool)> {
    let home = dirs::home_dir()?;
    for name in [".credentials.json", "credentials.json"] {
        let p = home.join(".claude").join(name);
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let oauth = v.get("claudeAiOauth").unwrap_or(&v);
        if let Some(tok) = oauth.get("accessToken").and_then(|x| x.as_str()) {
            let expired = oauth
                .get("expiresAt")
                .and_then(|x| x.as_f64())
                .map(|ms| (ms as u64) <= now_ms())
                .unwrap_or(false);
            return Some((tok.to_string(), expired));
        }
    }
    None
}

/// For doctor: credential probe report (prints no secret values)
pub fn probe_credentials() -> String {
    match read_credentials() {
        Some((tok, expired)) => format!(
            "credential: found (token {} chars, {})",
            tok.len(),
            if expired { "expired — Claude Code refreshes it on its next use" } else { "valid" }
        ),
        None => "credential: ~/.claude/.credentials.json not found (needsAuth; the desktop app may use another store — signing in once with the Claude Code CLI creates it)".into(),
    }
}

// ---------------- Live: `claude "/usage"` ----------------

/// Candidates in the order Claude Code installs itself, native installer first (matches the
/// upstream macOS search order, `.exe` swapped in for the Windows layout).
fn find_claude_binary() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        cands.push(home.join(".local").join("bin").join("claude.exe"));
        cands.push(home.join(".claude").join("local").join("claude.exe"));
        cands.push(home.join(".claude").join("local").join("claude.cmd"));
        cands.push(home.join(".bun").join("bin").join("claude.exe"));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            cands.push(dir.join("claude.exe"));
            cands.push(dir.join("claude.cmd"));
        }
    }
    cands.into_iter().find(|p| p.is_file())
}

/// Runs `claude "/usage"` with no stdin, capped at `CLI_TIMEOUT_SECS`. Reading happens on a side
/// thread so a chatty child can't deadlock the pipe against the timeout loop below.
fn run_claude_cli(binary: &Path) -> Option<String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new(binary);
    cmd.arg("/usage").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    let start = std::time::Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if start.elapsed().as_secs() >= CLI_TIMEOUT_SECS {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(_) => break false,
        }
    };
    let out = rx.recv_timeout(Duration::from_secs(2)).ok()?;
    if ok && !out.trim().is_empty() {
        Some(out)
    } else {
        None
    }
}

/// `all models` → `weekly_all`, `Opus` → `weekly_opus` — the endpoint's own vocabulary, so
/// `label_for` can name both the CLI and the endpoint's windows the same way.
fn week_kind(label: &str) -> String {
    let l = label.trim().to_lowercase();
    if l == "all models" {
        "weekly_all".to_string()
    } else {
        format!("weekly_{}", l.replace(' ', "_"))
    }
}

/// `Sep 7 at 2:59pm (Asia/Jakarta)` → ms epoch. Claude Code prints this in the machine's own local
/// zone (this app runs on the same machine), so the bracketed name is dropped rather than resolved
/// against a timezone database this crate does not carry. No year is printed, so the candidate
/// nearest `now` among last/this/next year is kept — the same rule the macOS original uses.
fn parse_cli_reset(text: &str, now_ms: u64) -> Option<u64> {
    let mut s = text.trim();
    if let Some(open) = s.rfind('(') {
        if s.ends_with(')') {
            s = s[..open].trim();
        }
    }
    // Observed format: "Sep 8, 8:09pm" (comma, no "at"); " at " tolerated too in case CLI wording changes.
    let (month_day, time_part) = s.split_once(", ").or_else(|| s.split_once(" at "))?;
    let mut md = month_day.split_whitespace();
    let mon = md.next()?;
    let day: u32 = md.next()?.parse().ok()?;
    let month = match mon {
        "Jan" => 1, "Feb" => 2, "Mar" => 3, "Apr" => 4, "May" => 5, "Jun" => 6,
        "Jul" => 7, "Aug" => 8, "Sep" => 9, "Oct" => 10, "Nov" => 11, "Dec" => 12,
        _ => return None,
    };
    let lower = time_part.trim().to_lowercase();
    let is_pm = lower.ends_with("pm");
    if !is_pm && !lower.ends_with("am") {
        return None;
    }
    let clock = &lower[..lower.len() - 2];
    let (h, m) = match clock.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (clock.parse::<u32>().ok()?, 0),
    };
    let mut hour24 = h % 12;
    if is_pm {
        hour24 += 12;
    }

    use chrono::{Datelike, Local, TimeZone};
    let now_dt = Local::now();
    let this_year = now_dt.year();
    let now_i = now_ms as i64;
    let mut best: Option<i64> = None;
    for y in [this_year - 1, this_year, this_year + 1] {
        if let chrono::LocalResult::Single(dt) = Local.with_ymd_and_hms(y, month, day, hour24, m, 0) {
            let ts = dt.timestamp_millis();
            best = Some(match best {
                None => ts,
                Some(b) => if (ts - now_i).abs() < (b - now_i).abs() { ts } else { b },
            });
        }
    }
    best.map(|ms| ms.max(0) as u64)
}

/// `Current session: 38% used · resets Sep 7 at 2:59pm (Asia/Jakarta)` and
/// `Current week (all models): 4% used · resets Sep 14 at 5:59am (Asia/Jakarta)` — everything else
/// `/usage` prints is prose about what drove the number, and is ignored.
fn parse_cli_text(text: &str, now_ms: u64) -> Option<Vec<LimitWindow>> {
    let mut out: Vec<LimitWindow> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        let (kind, rest) = if let Some(r) = line.strip_prefix("Current session:") {
            ("session".to_string(), r)
        } else if let Some(r) = line.strip_prefix("Current week (") {
            let Some(close) = r.find(')') else { continue };
            let label = &r[..close];
            let Some(r2) = r[close + 1..].trim_start().strip_prefix(':') else { continue };
            (week_kind(label), r2)
        } else {
            continue;
        };
        if out.iter().any(|w: &LimitWindow| w.id == kind) {
            continue; // first mention wins, same as the endpoint path
        }
        let rest = rest.trim();
        let Some(pct_idx) = rest.find('%') else { continue };
        let Ok(pct) = rest[..pct_idx].trim().parse::<f64>() else { continue };
        let Some(after_used) = rest[pct_idx + 1..].trim_start().strip_prefix("used") else { continue };
        let resets_at = after_used
            .trim_start()
            .strip_prefix('·')
            .map(|s| s.trim_start())
            .and_then(|s| s.strip_prefix("resets"))
            .and_then(|s| parse_cli_reset(s.trim(), now_ms));
        out.push(LimitWindow {
            id: kind.clone(),
            label: label_for(&kind),
            used: (pct / 100.0).clamp(0.0, 1.0),
            resets_at,
            ..Default::default()
        });
    }
    // Without the session window there is no headline; better to fall back to the token path
    // than to draw a ring with a hole in it.
    if !out.iter().any(|w| w.id == "session") {
        return None;
    }
    out.sort_by_key(|w| if w.id == "session" { 0 } else { 1 });
    Some(out)
}

static CLI_CACHE: Mutex<Option<(u64, Vec<LimitWindow>)>> = Mutex::new(None);
static CLI_LAST_ATTEMPT: AtomicU64 = AtomicU64::new(0);

/// What `claude "/usage"` last said, or None to mean "use the token path". Deliberately cannot
/// fail loudly: not installed, signed out, wording changed, wedged and killed are all reasons to
/// ask the endpoint instead, not reasons to show an error the endpoint path already words better.
fn cli_windows(binary: &Path) -> Option<Vec<LimitWindow>> {
    let now = now_ms();
    if let Some((at, w)) = CLI_CACHE.lock().unwrap().as_ref() {
        if now.saturating_sub(*at) < CLI_REFRESH_MS {
            return Some(w.clone());
        }
    }
    if now.saturating_sub(CLI_LAST_ATTEMPT.load(Ordering::Relaxed)) < CLI_REFRESH_MS {
        return None; // a recent failed attempt is still recent; don't spawn again this tick
    }
    CLI_LAST_ATTEMPT.store(now, Ordering::Relaxed);
    let text = run_claude_cli(binary)?;
    let windows = parse_cli_text(&text, now)?;
    *CLI_CACHE.lock().unwrap() = Some((now, windows.clone()));
    Some(windows)
}

/// For doctor: is the CLI path available at all.
pub fn probe_cli() -> String {
    match find_claude_binary() {
        Some(p) => format!("claude CLI: {} (used first; the token above is the fallback)", p.display()),
        None => "claude CLI: not found on PATH or the usual install dirs (token path only)".into(),
    }
}

fn parse_reset(v: &serde_json::Value) -> Option<u64> {
    v.as_str()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis().max(0) as u64)
}

fn label_for(kind: &str) -> String {
    match kind {
        "session" => "Current session".into(),
        "seven_day" | "weekly_all" => "Weekly (all models)".into(),
        "seven_day_opus" | "weekly_opus" => "Weekly (Opus)".into(),
        "weekly_scoped" => "Weekly (model-scoped)".into(),
        other => {
            // Forward compatibility: an unknown kind gets a readable label
            let mut s = other.replace('_', " ");
            if let Some(c) = s.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            s
        }
    }
}

fn parse_response(v: &serde_json::Value) -> Vec<LimitWindow> {
    let mut out: Vec<LimitWindow> = Vec::new();
    if let Some(arr) = v.get("limits").and_then(|x| x.as_array()) {
        for l in arr {
            let Some(kind) = l.get("kind").and_then(|x| x.as_str()) else {
                continue;
            };
            let Some(pct) = l.get("percent").and_then(|x| x.as_f64()) else {
                continue;
            };
            let resets = l.get("resets_at").and_then(parse_reset);
            if resets.is_none() {
                continue; // upstream rule: a window without a reset time is not shown
            }
            out.push(LimitWindow {
                id: kind.to_string(),
                label: label_for(kind),
                used: (pct / 100.0).clamp(0.0, 1.0),
                resets_at: resets, ..Default::default()
            });
        }
    }
    // Fallback merge: a window that just rolled over disappears from limits while the named field remains.
    // In practice the kinds in limits are weekly_all/weekly_scoped, not seven_day — deduplicating by id
    // alone would add the seven_day fallback a second time (the card showed "Weekly all" and
    // "Weekly (all models)" as twins). Three dedupe rules: id alias / same resets_at and percentage / same label.
    let aliases: [(&str, &str, &[&str]); 2] = [
        ("five_hour", "session", &["session", "five_hour"]),
        ("seven_day", "seven_day", &["seven_day", "weekly_all", "weekly"]),
    ];
    for (field, id, alias) in aliases {
        let Some(w) = v.get(field) else { continue };
        let Some(u) = w.get("utilization").and_then(|x| x.as_f64()) else { continue };
        let used = (u / 100.0).clamp(0.0, 1.0);
        let resets_at = w.get("resets_at").and_then(parse_reset);
        let label = label_for(id);
        let dup = out.iter().any(|x| {
            alias.contains(&x.id.as_str())
                || x.label == label
                || (resets_at.is_some()
                    && x.resets_at.map(|r| r / 1000) == resets_at.map(|r| r / 1000)
                    && (x.used - used).abs() < 0.005)
        });
        if dup {
            continue;
        }
        out.push(LimitWindow { id: id.into(), label, used, resets_at, ..Default::default() });
    }
    // session always comes first (upstream display order)
    out.sort_by_key(|w| if w.id == "session" { 0 } else { 1 });
    out
}

enum FetchErr {
    NeedsAuth,
    RateLimited(u64), // suggested wait in seconds (the Retry-After before the floor is applied)
    Other(String),
}

fn fetch_once(token: &str) -> Result<Vec<LimitWindow>, FetchErr> {
    let resp = ureq::get(ENDPOINT)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .timeout(Duration::from_secs(15))
        .call();
    match resp {
        Ok(r) => {
            let v: serde_json::Value = r
                .into_json()
                .map_err(|e| FetchErr::Other(format!("parse: {e}")))?;
            Ok(parse_response(&v))
        }
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => {
            Err(FetchErr::NeedsAuth)
        }
        Err(ureq::Error::Status(429, r)) => {
            let ra = r
                .header("retry-after")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0);
            Err(FetchErr::RateLimited(ra))
        }
        Err(ureq::Error::Status(code, _)) => Err(FetchErr::Other(format!("HTTP {code}"))),
        Err(e) => Err(FetchErr::Other(format!("{e}"))),
    }
}

fn backoff_secs(consecutive: u32, retry_after_floor: u64) -> u64 {
    let exp = BACKOFF_BASE_SECS.saturating_mul(1u64 << consecutive.min(4));
    exp.clamp(BACKOFF_BASE_SECS, BACKOFF_CAP_SECS).max(retry_after_floor)
}

fn set_and_broadcast(app: &AppHandle, mutate: impl FnOnce(&mut UsageSnapshot)) {
    let st = app.state::<AppState>();
    let snap = {
        let mut u = st.usage.lock().unwrap();
        mutate(&mut u);
        u.clone()
    };
    persist(&snap);
    let _ = app.emit("usage", &snap);
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || {
        // Broadcast the persisted old reading at startup (stale beats blank)
        {
            let st = app.state::<AppState>();
            let snap = st.usage.lock().unwrap().clone();
            let _ = app.emit("usage", &snap);
        }
        // Resolved once: the answer only changes if someone installs/removes Claude Code, and a
        // relaunch picks that up either way.
        let claude_binary = find_claude_binary();
        let mut consecutive_429: u32 = 0;
        loop {
            if let Some(binary) = &claude_binary {
                if let Some(windows) = cli_windows(binary) {
                    consecutive_429 = 0;
                    set_and_broadcast(&app, |u| {
                        u.status = "ok".into();
                        u.windows = windows;
                        u.fetched_at = now_ms();
                        u.note.clear();
                        u.backoff_until = 0;
                    });
                    let active = {
                        let st = app.state::<AppState>();
                        let store = st.store.lock().unwrap();
                        !store.snapshot("en", "en", false).sessions.is_empty()
                    };
                    sleep_interruptible(if active { POLL_ACTIVE_SECS } else { POLL_IDLE_SECS });
                    continue;
                }
            }
            // No requests inside the backoff window
            let bu = {
                let st = app.state::<AppState>();
                let u = st.usage.lock().unwrap();
                u.backoff_until
            };
            let now = now_ms();
            if bu > now {
                sleep_interruptible(((bu - now) / 1000).clamp(1, 30));
                continue;
            }
            match read_credentials() {
                None => set_and_broadcast(&app, |u| {
                    u.status = "needsAuth".into();
                    u.note = "No Claude Code credential found".into();
                }),
                Some((token, expired)) => {
                    // On 401/403 re-read the credential and retry once (Claude Code may have just refreshed it)
                    let result = match fetch_once(&token) {
                        Err(FetchErr::NeedsAuth) => match read_credentials() {
                            Some((t2, _)) if t2 != token => fetch_once(&t2),
                            _ => Err(FetchErr::NeedsAuth),
                        },
                        other => other,
                    };
                    let auth_note = if expired {
                        "Credential expired — run any claude command (or chat with Claude) to refresh it"
                    } else {
                        "Credential rejected (switched accounts?)"
                    };
                    match result {
                        Ok(windows) => {
                            consecutive_429 = 0;
                            set_and_broadcast(&app, |u| {
                                u.status = "ok".into();
                                u.windows = windows;
                                u.fetched_at = now_ms();
                                u.note.clear();
                                u.backoff_until = 0;
                            });
                        }
                        Err(FetchErr::NeedsAuth) => set_and_broadcast(&app, |u| {
                            u.status = "needsAuth".into();
                            u.note = auth_note.into();
                        }),
                        Err(FetchErr::RateLimited(ra)) => {
                            consecutive_429 += 1;
                            let wait = backoff_secs(consecutive_429 - 1, ra);
                            set_and_broadcast(&app, |u| {
                                if !u.windows.is_empty() {
                                    u.status = "stale".into();
                                }
                                u.note = format!("Rate limited, retrying in {wait}s");
                                u.backoff_until = now_ms() + wait * 1000;
                            });
                        }
                        Err(FetchErr::Other(msg)) => set_and_broadcast(&app, |u| {
                            if u.windows.is_empty() {
                                u.status = "error".into();
                            } else {
                                u.status = "stale".into();
                            }
                            u.note = msg;
                        }),
                    }
                }
            }
            // 60 s while a session is active, 300 s otherwise (upstream throttling discipline)
            let active = {
                let st = app.state::<AppState>();
                let store = st.store.lock().unwrap();
                let s = store.snapshot("en", "en", false);
                !s.sessions.is_empty()
            };
            sleep_interruptible(if active {
                POLL_ACTIVE_SECS
            } else {
                POLL_IDLE_SECS
            });
        }
    });
}
