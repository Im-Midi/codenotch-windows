//! 9Router usage adapter (github.com/decolua/9router) — no upstream Codenotch provider exists for
//! this one; implemented directly from 9Router's own source (it is open source, MIT-ish, self-hosted).
//!
//! Two readings, from the two pages of 9Router's dashboard:
//!   - Usage: today's request count and cost (`/api/usage/stats`) — a count, never a fabricated fraction.
//!   - Quota Tracker: every usage-eligible account (`/api/providers/client`, whose response is an
//!     explicit allowlist — no tokens) and each one's quotas (`/api/usage/<connectionId>`), reshaped
//!     by a port of the dashboard's own `parseQuotaData` so the rows read exactly as they do there.
//!     Quotas hidden in the dashboard (`quotaVisibility` in `/api/settings`) stay hidden here.
//!
//! Each `/api/usage/<id>` call makes 9Router query that vendor's quota API, so accounts are read no
//! more than every 5 minutes, and Claude every 10 — the dashboard itself throttles Claude that way.
//!
//! Auth: 9Router's own middleware (src/dashboardGuard.js) accepts a `x-9r-cli-token` header equal to
//! `sha256(rawMachineId + "9r-cli-auth" + cliSecret)[:16]` (hex). `rawMachineId` and `cliSecret` are
//! both written by 9Router itself to plain files under its data dir on first run
//! (`<data>/machine-id`, `<data>/auth/cli-secret`) — read only, borrowed the same way every other
//! provider here borrows a credential, never generated or written by this process.
//! Data dir (src/lib/dataDir.js): `%APPDATA%\9router` on Windows, unless `DATA_DIR` is set to a
//! non-Unix-style path. The `sk-…` API keys 9Router issues are for its LLM proxy and are refused here.
//!
//! A 9Router published on the internet is often behind Cloudflare Access (Zero Trust), which turns
//! every request — even 9Router's public /api/health — into a redirect to its sign-in page. That is
//! answered with a service token (`CF-Access-Client-Id` / `CF-Access-Client-Secret`), sent alongside
//! the CLI token when one is saved in the API-keys window.

use crate::usage::{LimitWindow, QuotaGroup, UsageSnapshot};
use crate::AppState;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const POLL_SECS: u64 = 120; // the request counter is 9Router's own and cheap to read
const ACCOUNT_POLL_SECS: u64 = 300; // each account read makes 9Router call that vendor's quota API
const CLAUDE_POLL_SECS: u64 = 600; // 9Router's dashboard throttles Claude to every 10 minutes too
const BACKOFF_MIN_SECS: u64 = 30;
const DEFAULT_PORT: u16 = 20128;

static REFRESH: AtomicBool = AtomicBool::new(false);
/// A manual refresh re-reads every account now instead of waiting for its interval
static FORCE: AtomicBool = AtomicBool::new(false);
static BACKOFF_UNTIL: AtomicU64 = AtomicU64::new(0);

struct Account {
    id: String,
    fetched_at: u64,
    group: QuotaGroup,
}
/// Last reading per account, in 9Router's own order
static ACCOUNTS: Mutex<Vec<Account>> = Mutex::new(Vec::new());

pub fn request_refresh() {
    FORCE.store(true, Ordering::Relaxed);
    REFRESH.store(true, Ordering::Relaxed);
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
            BACKOFF_UNTIL.store(s.backoff_until, Ordering::Relaxed);
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
        .map(|u| normalize_base(&u))
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", port()))
}

/// 9Router's dashboard shows its LLM endpoint as `<host>/v1`, and that is the URL people copy —
/// but the usage API lives at `<host>/api/usage/...`, so `/v1/api/usage/stats` is simply the wrong
/// path. The proxy suffixes are dropped rather than rejected.
pub fn normalize_base(u: &str) -> String {
    let mut s = u.trim().trim_end_matches('/').to_string();
    for suffix in ["/api/v1beta", "/api/v1", "/v1beta", "/v1"] {
        if s.len() > suffix.len() && s.to_ascii_lowercase().ends_with(suffix) {
            s.truncate(s.len() - suffix.len());
            break;
        }
    }
    s.trim_end_matches('/').to_string()
}

/// Cloudflare's copy button on a service token copies the whole header line
/// (`CF-Access-Client-Id: <value>`), and sending that as the value is refused like a wrong token.
pub fn clean_cf_value(v: &str, header: &str) -> String {
    let v = v.trim();
    match v.get(..header.len()) {
        Some(head) if head.eq_ignore_ascii_case(header) => v[header.len()..].trim_start().trim_start_matches(':').trim().to_string(),
        _ => v.to_string(),
    }
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

/// The easy mistake: pasting one of the `sk-…` keys the dashboard lists under "API Keys"
fn saved_token_is_proxy_key() -> bool {
    crate::secrets::get(crate::secrets::ROUTER9_TOKEN)
        .map(|t| t.trim().starts_with("sk-"))
        .unwrap_or(false)
}

/// Cloudflare Access service token (client id, client secret), when one is saved. Cleaned on the way
/// out as well as on the way in, so a header line saved before that was fixed still works.
pub fn cf_access() -> Option<(String, String)> {
    let id = clean_cf_value(&crate::secrets::get(crate::secrets::ROUTER9_CF_ID)?, "CF-Access-Client-Id");
    let secret = clean_cf_value(&crate::secrets::get(crate::secrets::ROUTER9_CF_SECRET)?, "CF-Access-Client-Secret");
    Some((id, secret))
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

// ---------------- HTTP ----------------

enum FetchErr {
    /// A JSON 401/403 from 9Router, with its own explanation when it gave one
    NeedsAuth(String),
    RateLimited(u64),
    /// Cloudflare Access answered instead of 9Router — its sign-in page, or a refusal of the service
    /// token. `with_token` tells the two pieces of advice apart.
    AccessBlocked { with_token: bool },
    NotFound,
    Other(String),
}

fn err_text(e: &FetchErr) -> String {
    match e {
        FetchErr::NeedsAuth(m) if !m.is_empty() => m.clone(),
        FetchErr::NeedsAuth(_) => "refused".into(),
        FetchErr::RateLimited(s) => format!("rate limited, retrying in {s}s"),
        FetchErr::AccessBlocked { .. } => "blocked by Cloudflare Access".into(),
        FetchErr::NotFound => "not found".into(),
        FetchErr::Other(m) => m.clone(),
    }
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

fn rejected_message() -> String {
    if saved_token_is_proxy_key() {
        "Reached 9Router, but the saved token is one of its proxy API keys (sk-…). The usage API only accepts the CLI token".into()
    } else {
        "Reached 9Router, but it rejected this CLI token".into()
    }
}

fn get_json(path: &str, token: &str) -> Result<Value, FetchErr> {
    get_json_at(&base_url(), path, token, cf_access())
}

fn get_json_at(base: &str, path: &str, token: &str, cf: Option<(String, String)>) -> Result<Value, FetchErr> {
    let url = format!("{base}{path}");
    // Redirects are not followed. 9Router's API never redirects, so a 3xx means something in front
    // of it answered — and following it lands on a sign-in page that then fails as "bad JSON", which
    // is exactly the unhelpful error this used to show for a 9Router behind Cloudflare Access.
    let agent = ureq::AgentBuilder::new().redirects(0).timeout(Duration::from_secs(30)).build();
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
                let msg = r
                    .into_json::<Value>()
                    .ok()
                    .and_then(|v| v.get("error").or_else(|| v.get("message")).and_then(|x| x.as_str()).map(String::from))
                    .unwrap_or_default();
                Err(FetchErr::NeedsAuth(msg))
            } else {
                Err(FetchErr::AccessBlocked { with_token })
            }
        }
        Err(ureq::Error::Status(404, _)) => Err(FetchErr::NotFound),
        Err(ureq::Error::Status(429, r)) => {
            let ra = r.header("retry-after").and_then(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
            Err(FetchErr::RateLimited(ra.max(BACKOFF_MIN_SECS)))
        }
        Err(ureq::Error::Status(code, _)) => Err(FetchErr::Other(format!("HTTP {code}"))),
        // The server not running (connection refused) lands here too — same message either way
        Err(e) => Err(FetchErr::Other(format!("{e}"))),
    }
}

fn fetch_stats(token: &str) -> Result<Value, FetchErr> {
    get_json("/api/usage/stats?period=today", token)
}

// ---------------- Quota Tracker: a port of the dashboard's parseQuotaData ----------------
// src/app/(dashboard)/dashboard/usage/components/ProviderLimits/utils.js

struct Raw {
    /// Visibility key: the dashboard's `modelKey || name`
    key: String,
    name: String,
    used: f64,
    total: f64,
    reset: Option<u64>,
    /// Treated as a 0–100 percentage by the dashboard (getRemainingPercentage), whatever it is called
    remaining: Option<f64>,
    remaining_pct: Option<f64>,
    unlimited: bool,
}

fn num(v: Option<&Value>) -> f64 {
    v.and_then(|x| x.as_f64()).unwrap_or(0.0)
}

fn parse_reset(v: Option<&Value>) -> Option<u64> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return chrono::DateTime::parse_from_rfc3339(s).ok().map(|d| d.timestamp_millis().max(0) as u64);
    }
    let n = v.as_f64().filter(|n| *n > 0.0)?;
    Some(if n > 1e12 { n as u64 } else { (n * 1000.0) as u64 })
}

fn raw_from(key: &str, name: String, q: &Value, fwd_pct: bool, fwd_remaining: bool, fwd_unlimited: bool) -> Raw {
    Raw {
        key: key.to_string(),
        name,
        used: num(q.get("used")),
        total: num(q.get("total")),
        reset: parse_reset(q.get("resetAt")),
        remaining: if fwd_remaining { q.get("remaining").and_then(|x| x.as_f64()) } else { None },
        remaining_pct: if fwd_pct { q.get("remainingPercentage").and_then(|x| x.as_f64()) } else { None },
        unlimited: fwd_unlimited && q.get("unlimited").and_then(|x| x.as_bool()).unwrap_or(false),
    }
}

/// calculatePercentage: remaining share of used/total, 0 when there is no total
fn calc_pct(used: f64, total: f64) -> f64 {
    if total == 0.0 {
        return 0.0;
    }
    if used <= 0.0 {
        return 100.0;
    }
    if used >= total {
        return 0.0;
    }
    ((total - used) / total * 100.0).round()
}

/// getRemainingPercentage
fn remaining_percent(r: &Raw) -> f64 {
    if let Some(x) = r.remaining {
        x.round().max(0.0)
    } else if let Some(p) = r.remaining_pct {
        p.round()
    } else {
        calc_pct(r.used, r.total)
    }
}

fn fmt_num(x: f64) -> String {
    if (x - x.round()).abs() > 1e-9 {
        return format!("{x:.2}");
    }
    let n = x.round() as i64;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

fn to_window(r: Raw) -> LimitWindow {
    let rem = remaining_percent(&r).clamp(0.0, 100.0);
    let amount = if r.unlimited {
        format!("{} used · Unlimited", fmt_num(r.used))
    } else {
        format!("{} / {}", fmt_num(r.used), if r.total > 0.0 { fmt_num(r.total) } else { "∞".into() })
    };
    LimitWindow {
        id: r.key,
        label: r.name,
        used: if r.unlimited { 0.0 } else { 1.0 - rem / 100.0 },
        resets_at: r.reset,
        amount: Some(amount),
        ..Default::default()
    }
}

fn display_name(key: &str, q: &Value) -> String {
    q.get("displayName").and_then(|x| x.as_str()).unwrap_or(key).to_string()
}

/// Rows for one account, and the account-level message when there are none
fn parse_quotas(provider: &str, data: &Value) -> (Vec<LimitWindow>, Option<String>) {
    let message = data.get("message").and_then(|x| x.as_str()).map(String::from);
    let Some(q) = data.get("quotas").and_then(|x| x.as_object()) else {
        return (Vec::new(), message);
    };
    let mut rows: Vec<Raw> = Vec::new();
    match provider {
        "antigravity" => {
            // The dashboard folds the per-model buckets into families, each shown at its most-used model
            const WEEKLY: [&str; 2] = ["gemini_weekly", "claude_gpt_weekly"];
            let is_gem = |k: &str| k.starts_with("gemini-") && !k.contains("image");
            let is_cla = |k: &str| k.starts_with("claude-");
            let is_img = |k: &str| k.contains("image");
            let is_wk = |k: &str| WEEKLY.contains(&k);
            let pct_of = |v: &Value| v.get("remainingPercentage").and_then(|x| x.as_f64()).unwrap_or(100.0);
            let rep = |f: &dyn Fn(&str) -> bool| {
                let mut best: Option<&Value> = None;
                for (k, v) in q {
                    if f(k) && best.map(|b| pct_of(v) < pct_of(b)).unwrap_or(true) {
                        best = Some(v);
                    }
                }
                best
            };
            if let Some(v) = rep(&is_gem) {
                rows.push(raw_from("gemini", "Gemini (Flash / Pro)".into(), v, true, false, false));
            }
            if let Some(v) = rep(&is_cla) {
                rows.push(raw_from("claude", "Claude (Sonnet / Opus)".into(), v, true, false, false));
            }
            for (k, v) in q {
                if !is_gem(k) && !is_cla(k) && !is_img(k) && !is_wk(k) {
                    rows.push(raw_from(k, display_name(k, v), v, true, false, false));
                }
            }
            for (k, v) in q {
                if is_img(k) {
                    rows.push(raw_from(k, display_name(k, v), v, true, false, false));
                }
            }
            // Named explicitly: serde_json keeps keys sorted, which would put Claude & GPT first
            for k in WEEKLY {
                if let Some(v) = q.get(k) {
                    rows.push(raw_from(k, display_name(k, v), v, true, false, false));
                }
            }
        }
        "codex" => {
            const ORDER: [(&str, &str); 6] = [
                ("session", "5h"),
                ("weekly", "Weekly"),
                ("spark_session", "Spark (5h)"),
                ("spark_weekly", "Spark (Weekly)"),
                ("review_session", "Review (5h)"),
                ("review_weekly", "Review (Weekly)"),
            ];
            for (k, name) in ORDER {
                if let Some(v) = q.get(k) {
                    rows.push(raw_from(k, name.into(), v, false, true, false));
                }
            }
            for (k, v) in q {
                if !ORDER.iter().any(|(o, _)| o == k) {
                    rows.push(raw_from(k, k.clone(), v, false, true, false));
                }
            }
        }
        "claude" => {
            if message.is_some() {
                return (Vec::new(), message);
            }
            for (k, v) in q {
                let (used, total) = (num(v.get("used")), num(v.get("total")));
                let remaining = v
                    .get("remaining")
                    .and_then(|x| x.as_f64())
                    .unwrap_or_else(|| ((if total > 0.0 { total } else { 100.0 }) - used).max(0.0));
                let pct = v.get("remainingPercentage").and_then(|x| x.as_f64()).unwrap_or_else(|| calc_pct(used, total));
                rows.push(Raw {
                    key: k.clone(),
                    name: k.clone(),
                    used,
                    total,
                    reset: parse_reset(v.get("resetAt")),
                    remaining: Some(remaining),
                    remaining_pct: Some(pct),
                    unlimited: false,
                });
            }
            const ORDER: [&str; 5] = ["session (5h)", "weekly (7d)", "weekly fable (7d)", "weekly opus (7d)", "weekly sonnet (7d)"];
            rows.sort_by_key(|r| ORDER.iter().position(|o| *o == r.name).unwrap_or(99));
        }
        "qoder" => {
            for (k, v) in q {
                // A personal account has no organisation bucket; "0 / 0" would only mislead
                if k == "organization" && num(v.get("total")) == 0.0 {
                    continue;
                }
                let name = match k.as_str() {
                    "user" => "Personal".to_string(),
                    "organization" => "Organization".to_string(),
                    _ => k.clone(),
                };
                rows.push(raw_from(k, name, v, false, false, false));
            }
        }
        _ => {
            // Credit balances and percentage-only windows state remainingPercentage; plain used/total
            // providers (github, kiro, groq…) have it derived, exactly as the dashboard does
            let fwd_pct = matches!(provider, "vercel-ai-gateway" | "grok-cli" | "kimi" | "deepseek" | "ollama" | "zed");
            for (k, v) in q {
                rows.push(raw_from(k, k.clone(), v, fwd_pct, false, provider == "zed"));
            }
        }
    }
    let rows: Vec<LimitWindow> = rows.into_iter().map(to_window).collect();
    let message = if rows.is_empty() { message } else { None };
    (rows, message)
}

fn provider_title(provider: &str) -> String {
    match provider {
        "antigravity" => "Antigravity".into(),
        "claude" => "Claude".into(),
        "codex" => "Codex".into(),
        "github" => "GitHub Copilot".into(),
        "kiro" => "Kiro".into(),
        "qoder" => "Qoder".into(),
        "grok-cli" => "Grok CLI".into(),
        "kimi" => "Kimi".into(),
        "deepseek" => "DeepSeek".into(),
        "groq" => "Groq".into(),
        "ollama" => "Ollama".into(),
        "zed" => "Zed".into(),
        "vercel-ai-gateway" => "Vercel AI Gateway".into(),
        "codebuddy-cn" => "CodeBuddy CN".into(),
        other => other.to_string(),
    }
}

/// getConnectionLabel: name, else email, else displayName
fn connection_label(c: &Value) -> String {
    ["name", "email", "displayName"]
        .iter()
        .filter_map(|f| c.get(*f).and_then(|x| x.as_str()).map(str::trim))
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string()
}

fn build_group(c: &Value, provider: &str, data: &Value) -> QuotaGroup {
    let (rows, message) = parse_quotas(provider, data);
    QuotaGroup {
        provider: provider.to_string(),
        title: provider_title(provider),
        account: connection_label(c),
        plan: data.get("plan").and_then(|x| x.as_str()).map(String::from),
        message,
        rows,
    }
}

/// Quotas hidden with the eye icon in the dashboard (`quotaVisibility[provider].hidden`) stay hidden
fn apply_visibility(mut g: QuotaGroup, visibility: &Value) -> QuotaGroup {
    if let Some(hidden) = visibility.get(&g.provider).and_then(|p| p.get("hidden")).and_then(|h| h.as_array()) {
        let hidden: Vec<&str> = hidden.iter().filter_map(|x| x.as_str()).map(str::trim).collect();
        g.rows.retain(|r| !hidden.contains(&r.id.trim()));
    }
    g
}

/// Connection ids are UUIDs; anything else is percent-encoded rather than trusted into a path
fn path_segment(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Every active usage-eligible account and its quotas. Accounts not yet due are served from the
/// last reading, so each vendor sees at most one quota call per interval from here.
fn read_accounts(token: &str) -> Result<Vec<QuotaGroup>, FetchErr> {
    let list = get_json("/api/providers/client?page=1&pageSize=100&accountStatus=active&sort=priority", token)?;
    let conns: Vec<Value> = list.get("connections").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    // Only the one field is kept; the rest of 9Router's settings never leave this function
    let visibility = get_json("/api/settings", token)
        .ok()
        .and_then(|s| s.get("quotaVisibility").cloned())
        .unwrap_or(Value::Null);
    let force = FORCE.swap(false, Ordering::Relaxed);
    let now = now_ms();
    let mut prev = std::mem::take(&mut *ACCOUNTS.lock().unwrap());
    let mut next: Vec<Account> = Vec::new();
    for c in &conns {
        let Some(id) = c.get("id").and_then(|x| x.as_str()) else { continue };
        let provider = c.get("provider").and_then(|x| x.as_str()).unwrap_or("").to_ascii_lowercase();
        let old = prev.iter().position(|a| a.id == id).map(|i| prev.swap_remove(i));
        let every = if provider == "claude" { CLAUDE_POLL_SECS } else { ACCOUNT_POLL_SECS } * 1000;
        if old.as_ref().map(|a| !force && now.saturating_sub(a.fetched_at) < every).unwrap_or(false) {
            next.push(old.unwrap());
            continue;
        }
        let group = match get_json(&format!("/api/usage/{}", path_segment(id)), token) {
            Ok(data) => build_group(c, &provider, &data),
            Err(FetchErr::NotFound) => continue, // removed in 9Router since the list was read
            Err(e) => {
                // Keep the last rows and say why they are old; a 401 here is the *account's* expired
                // sign-in inside 9Router (the list call already proved the CLI token)
                let mut g = old.map(|a| a.group).unwrap_or_else(|| QuotaGroup {
                    provider: provider.clone(),
                    title: provider_title(&provider),
                    account: connection_label(c),
                    ..Default::default()
                });
                g.message = Some(match &e {
                    FetchErr::NeedsAuth(m) if !m.is_empty() => m.clone(),
                    FetchErr::NeedsAuth(_) => "This account needs re-authorising in 9Router".into(),
                    other => format!("Couldn't refresh: {}", err_text(other)),
                });
                g
            }
        };
        next.push(Account { id: id.to_string(), fetched_at: now, group });
    }
    let out = next.iter().map(|a| apply_visibility(a.group.clone(), &visibility)).collect();
    *ACCOUNTS.lock().unwrap() = next;
    Ok(out)
}

fn cached_groups() -> Vec<QuotaGroup> {
    ACCOUNTS.lock().unwrap().iter().map(|a| a.group.clone()).collect()
}

/// The ring shows the one quota that will stop you first, across every account (Command Code's
/// multi-account card uses it too)
pub fn headline(groups: &[QuotaGroup]) -> Option<LimitWindow> {
    let mut best: Option<(&QuotaGroup, &LimitWindow)> = None;
    for g in groups {
        for r in &g.rows {
            if best.map(|(_, b)| r.used > b.used).unwrap_or(true) {
                best = Some((g, r));
            }
        }
    }
    best.map(|(g, r)| LimitWindow {
        id: "tightest".into(),
        label: format!("{} · {}", g.title, r.label),
        used: r.used,
        resets_at: r.resets_at,
        amount: r.amount.clone(),
        ..Default::default()
    })
}

// ---------------- Putting it together ----------------

fn read_once() -> UsageSnapshot {
    let mut snap = UsageSnapshot::default();
    let held_until = BACKOFF_UNTIL.load(Ordering::Relaxed);
    let now = now_ms();
    if held_until > now {
        snap.backoff_until = held_until;
        snap.status = "stale".into();
        snap.note = format!("Rate limited — retrying in {}s", (held_until - now) / 1000);
        snap.groups = cached_groups();
        return snap;
    }
    let Some(token) = cli_token() else {
        snap.status = "needsAuth".into();
        snap.note = "No CLI token: 9Router has not run on this PC, and none is saved in the API-keys window".into();
        return snap;
    };
    match fetch_stats(&token) {
        Ok(v) => {
            BACKOFF_UNTIL.store(0, Ordering::Relaxed);
            let requests = v.get("totalRequests").and_then(|x| x.as_i64()).unwrap_or(0);
            let cost = v.get("totalCost").and_then(|x| x.as_f64()).unwrap_or(0.0);
            let providers = v.get("byProvider").and_then(|x| x.as_object()).map(|o| o.len()).unwrap_or(0);
            snap.status = "ok".into();
            snap.fetched_at = now_ms();
            snap.windows = vec![LimitWindow {
                id: "today".into(),
                label: "Requests today".into(),
                count: Some(requests),
                derived: true,
                ..Default::default()
            }];
            snap.note = format!(
                "{requests} request{} · ${cost:.2} today{}",
                if requests == 1 { "" } else { "s" },
                if providers > 0 { format!(" across {providers} provider{}", if providers == 1 { "" } else { "s" }) } else { String::new() }
            );
            // An older 9Router without the Quota Tracker API still gets the request count
            snap.groups = match read_accounts(&token) {
                Ok(g) => g,
                Err(e) => {
                    crate::applog(&format!("9router: quota tracker read failed ({})", err_text(&e)));
                    cached_groups()
                }
            };
            if let Some(h) = headline(&snap.groups) {
                snap.windows.insert(0, h);
            }
            snap
        }
        Err(FetchErr::NeedsAuth(_)) => {
            snap.status = "needsAuth".into();
            snap.note = rejected_message();
            snap
        }
        Err(FetchErr::AccessBlocked { with_token }) => {
            snap.status = "needsAuth".into();
            snap.note = access_message(with_token);
            snap
        }
        Err(FetchErr::RateLimited(secs)) => {
            let until = now_ms() + secs * 1000;
            BACKOFF_UNTIL.store(until, Ordering::Relaxed);
            snap.backoff_until = until;
            snap.status = "stale".into();
            snap.note = format!("Rate limited — retrying in {secs}s");
            snap.groups = cached_groups();
            snap
        }
        Err(FetchErr::NotFound) => {
            snap.status = "none".into();
            snap.note = format!("No 9Router usage API at {}", base_url());
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
                    if REFRESH.swap(false, Ordering::Relaxed) {
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
                if REFRESH.swap(false, Ordering::Relaxed) {
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
        Err(FetchErr::NeedsAuth(_)) => Err(fail(rejected_message())),
        Err(FetchErr::AccessBlocked { with_token }) => {
            Err(TestFail { message: access_message(with_token), access_blocked: true })
        }
        Err(FetchErr::RateLimited(s)) => Err(fail(format!("9Router is rate limiting — retry in {s}s"))),
        Err(FetchErr::NotFound) => Err(fail(format!("Saved, but there is no 9Router usage API at {}", base_url()))),
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
            Some("saved") if saved_token_is_proxy_key() => "saved, but it is a proxy API key (sk-…), not the CLI token",
            Some("saved") => "saved in the API-keys window",
            Some(_) => "computed from local files",
            None => "unavailable (9Router has not run here; paste a token in the API-keys window for a remote one)",
        },
        if cf_access().is_some() { "saved" } else { "none" }
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    /// Set CODENOTCH_TEST_R9_URL to a 9Router published behind Cloudflare Access (no service token).
    #[test]
    #[ignore = "needs CODENOTCH_TEST_R9_URL: a 9Router behind Cloudflare Access"]
    fn access_sign_in_is_recognised_not_parsed_as_json() {
        let url = std::env::var("CODENOTCH_TEST_R9_URL").expect("set CODENOTCH_TEST_R9_URL");
        let r = super::get_json_at(&super::normalize_base(&url), "/api/usage/stats?period=today", "0000000000000000", None);
        assert!(matches!(r, Err(super::FetchErr::AccessBlocked { with_token: false })));
    }

    #[test]
    fn redirect_host_drops_the_signed_query() {
        assert_eq!(super::host_of("https://team.cloudflareaccess.com/cdn-cgi/access/login/x?meta=eyJhbGci"), "team.cloudflareaccess.com");
    }

    #[test]
    fn proxy_suffix_is_dropped_from_the_base() {
        assert_eq!(super::normalize_base("https://r.example.net/v1/"), "https://r.example.net");
        assert_eq!(super::normalize_base("https://r.example.net/API/v1"), "https://r.example.net");
        assert_eq!(super::normalize_base("https://r.example.net/v1beta"), "https://r.example.net");
        assert_eq!(super::normalize_base("http://127.0.0.1:20128"), "http://127.0.0.1:20128");
        assert_eq!(super::normalize_base(""), "");
    }

    #[test]
    fn copied_header_line_is_reduced_to_its_value() {
        assert_eq!(super::clean_cf_value("CF-Access-Client-Id: abc.access", "CF-Access-Client-Id"), "abc.access");
        assert_eq!(super::clean_cf_value("  cf-access-client-secret:cfast_x ", "CF-Access-Client-Secret"), "cfast_x");
        assert_eq!(super::clean_cf_value("abc.access", "CF-Access-Client-Id"), "abc.access");
        assert_eq!(super::clean_cf_value("é", "CF-Access-Client-Id"), "é"); // shorter than the header: no slicing panic
    }

    fn q(used: f64, total: f64, pct: f64, name: &str) -> serde_json::Value {
        json!({"used": used, "total": total, "resetAt": "2026-09-13T06:48:48.000Z", "remainingPercentage": pct, "unlimited": false, "displayName": name})
    }

    /// Shape of a real Antigravity reply: 19 per-model buckets fold into the dashboard's 6 rows
    #[test]
    fn antigravity_folds_like_the_quota_tracker() {
        let data = json!({"plan": "Antigravity", "quotas": {
            "gemini-3.8-flash-high": q(0.0, 1000.0, 100.0, "Gemini 3.8 Flash (High)"),
            "gemini-pro-agent": q(0.0, 1000.0, 100.0, "Gemini 3.1 Pro (High)"),
            "claude-sonnet-4-6": q(0.0, 1000.0, 100.0, "Claude Sonnet 4.6 (Thinking)"),
            "claude-opus-4-6-thinking": q(0.0, 1000.0, 100.0, "Claude Opus 4.6 (Thinking)"),
            "gpt-oss-120b-medium": q(0.0, 1000.0, 100.0, "GPT-OSS 120B (Medium)"),
            "gemini-3.1-flash-image": q(0.0, 1000.0, 100.0, "Gemini 3.1 Flash Image"),
            "gemini_weekly": q(35.0, 1000.0, 96.519464, "Gemini (Weekly)"),
            "claude_gpt_weekly": q(0.0, 1000.0, 100.0, "Claude & GPT (Weekly)"),
        }});
        let (rows, msg) = super::parse_quotas("antigravity", &data);
        let names: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(names, ["Gemini (Flash / Pro)", "Claude (Sonnet / Opus)", "GPT-OSS 120B (Medium)", "Gemini 3.1 Flash Image", "Gemini (Weekly)", "Claude & GPT (Weekly)"]);
        let weekly = &rows[4];
        assert_eq!(weekly.amount.as_deref(), Some("35 / 1,000"));
        assert!((weekly.used - 0.03).abs() < 1e-9, "97% left, as the dashboard rounds it");
        assert_eq!(rows[0].id, "gemini");
        assert!(msg.is_none());
    }

    #[test]
    fn codex_and_claude_rows_are_named_and_ordered() {
        let codex = json!({"plan": "plus", "quotas": {
            "weekly": {"used": 15, "total": 100, "remaining": 85, "resetAt": "2026-09-19T09:51:28.000Z"},
            "session": {"used": 0, "total": 100, "remaining": 100, "resetAt": "2026-09-13T06:48:50.000Z"}}});
        let (rows, _) = super::parse_quotas("codex", &codex);
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["5h", "Weekly"]);
        assert!((rows[1].used - 0.15).abs() < 1e-9);

        let claude = json!({"quotas": {
            "weekly (7d)": {"used": 26, "total": 100, "remaining": 74, "remainingPercentage": 74},
            "session (5h)": {"used": 17, "total": 100, "remaining": 83, "remainingPercentage": 83}}});
        let (rows, _) = super::parse_quotas("claude", &claude);
        assert_eq!(rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(), ["session (5h)", "weekly (7d)"]);

        let (rows, msg) = super::parse_quotas("claude", &json!({"message": "Token expired"}));
        assert!(rows.is_empty());
        assert_eq!(msg.as_deref(), Some("Token expired"));
    }

    #[test]
    fn hidden_quotas_stay_hidden_and_the_ring_takes_the_tightest() {
        let g = super::QuotaGroup {
            provider: "antigravity".into(),
            rows: super::parse_quotas("antigravity", &json!({"quotas": {
                "gemini-3.8-flash-high": q(0.0, 1000.0, 100.0, "x"),
                "gemini_weekly": q(35.0, 1000.0, 96.5, "Gemini (Weekly)")}})).0,
            ..Default::default()
        };
        let g = super::apply_visibility(g, &json!({"antigravity": {"hidden": ["gemini"]}}));
        assert_eq!(g.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["gemini_weekly"]);
        let h = super::headline(&[g]).unwrap();
        assert!(h.label.ends_with("Gemini (Weekly)"));
    }

    #[test]
    fn amounts_read_like_the_dashboard() {
        assert_eq!(super::fmt_num(1000.0), "1,000");
        assert_eq!(super::fmt_num(1234567.0), "1,234,567");
        assert_eq!(super::fmt_num(12.5), "12.50");
        assert_eq!(super::calc_pct(0.0, 0.0), 0.0);
        assert_eq!(super::calc_pct(0.0, 100.0), 100.0);
    }
}
