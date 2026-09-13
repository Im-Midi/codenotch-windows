//! DeepSeek API balance (https://api-docs.deepseek.com/api/get-user-balance).
//!
//! DeepSeek bills prepaid credit, not a quota window, so there is no vendor "share used". The ring
//! shows how much of the last top-up is gone instead: the highest balance seen (per currency) is
//! remembered as the reference and a new top-up raises it again. That number is ours, so it is
//! marked derived (~). Peak/off-peak pricing is a fixed timetable and is worked out in the page,
//! which needs it every minute anyway for its countdown and the switch-over toasts.
//!
//! Keys: the one saved from the API-keys window (Credential Manager), else `DEEPSEEK_API_KEY`.

use crate::usage::{LimitWindow, UsageSnapshot};
use crate::AppState;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

const POLL_SECS: u64 = 300;
const BALANCE: &str = "https://api.deepseek.com/user/balance";

static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn store_path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("deepseek.json")
}

fn ref_path() -> std::path::PathBuf {
    crate::config::config_path().with_file_name("deepseek-ref.json")
}

pub fn load_persisted() -> UsageSnapshot {
    std::fs::read_to_string(store_path())
        .ok()
        .and_then(|t| serde_json::from_str::<UsageSnapshot>(&t).ok())
        .map(|mut s| {
            if !s.windows.is_empty() {
                s.status = "stale".into();
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

/// (source, key): saved here first, then the env var
pub fn key() -> Option<(&'static str, String)> {
    if let Some(k) = crate::secrets::get(crate::secrets::DEEPSEEK) {
        return Some(("saved", k));
    }
    std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .map(|k| ("env", k))
}

pub enum FetchErr {
    Rejected,
    Other(String),
}

fn fetch(key: &str) -> Result<serde_json::Value, FetchErr> {
    match ureq::get(BALANCE)
        .set("Authorization", &format!("Bearer {key}"))
        .set("Accept", "application/json")
        .timeout(Duration::from_secs(15))
        .call()
    {
        Ok(r) => r.into_json().map_err(|e| FetchErr::Other(format!("parse: {e}"))),
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => Err(FetchErr::Rejected),
        Err(ureq::Error::Status(code, _)) => Err(FetchErr::Other(format!("HTTP {code}"))),
        Err(e) => Err(FetchErr::Other(format!("{e}"))),
    }
}

/// DeepSeek sends the amounts as strings ("110.00"); accept numbers too
fn amount(v: Option<&serde_json::Value>) -> f64 {
    match v {
        Some(serde_json::Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        Some(x) => x.as_f64().unwrap_or(0.0),
        None => 0.0,
    }
}

fn money(currency: &str, v: f64) -> String {
    match currency {
        "USD" => format!("${v:.2}"),
        "CNY" => format!("¥{v:.2}"),
        c => format!("{v:.2} {c}"),
    }
}

/// Turns a balance response into windows, raising the remembered top-up reference as needed
fn windows_from(v: &serde_json::Value, refs: &mut HashMap<String, f64>) -> (Vec<LimitWindow>, bool) {
    let available = v.get("is_available").and_then(|x| x.as_bool()).unwrap_or(true);
    let mut out = Vec::new();
    for info in v.get("balance_infos").and_then(|x| x.as_array()).into_iter().flatten() {
        let currency = info.get("currency").and_then(|x| x.as_str()).unwrap_or("USD").to_string();
        let total = amount(info.get("total_balance"));
        let granted = amount(info.get("granted_balance"));
        let topped = amount(info.get("topped_up_balance"));
        let r = refs.entry(currency.clone()).or_insert(0.0);
        if total > *r {
            *r = total;
        }
        let used = if !available || total <= 0.0 { 1.0 } else if *r > 0.0 { (1.0 - total / *r).clamp(0.0, 1.0) } else { 0.0 };
        out.push(LimitWindow {
            id: format!("balance-{}", currency.to_ascii_lowercase()),
            label: format!("Balance ({currency})"),
            used,
            derived: true,
            amount: Some(format!(
                "{} left · {} topped up · {} granted",
                money(&currency, total),
                money(&currency, topped),
                money(&currency, granted)
            )),
            ..Default::default()
        });
    }
    (out, available)
}

/// Short text for the cell under the ring ("$4.2")
pub fn short_balance(v: &serde_json::Value) -> Option<String> {
    let info = v.get("balance_infos")?.as_array()?.first()?;
    let currency = info.get("currency").and_then(|x| x.as_str()).unwrap_or("USD");
    let total = amount(info.get("total_balance"));
    Some(money(currency, total))
}

fn load_refs() -> HashMap<String, f64> {
    std::fs::read_to_string(ref_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn read_once() -> UsageSnapshot {
    let Some((_, k)) = key() else {
        return UsageSnapshot { status: "needsAuth".into(), note: "No DeepSeek API key".into(), ..Default::default() };
    };
    match fetch(&k) {
        Ok(v) => {
            let mut refs = load_refs();
            let (windows, available) = windows_from(&v, &mut refs);
            if let Ok(t) = serde_json::to_string(&refs) {
                let _ = std::fs::write(ref_path(), t);
            }
            UsageSnapshot {
                status: if windows.is_empty() { "none" } else { "ok" }.into(),
                note: if available {
                    short_balance(&v).unwrap_or_default()
                } else {
                    "Balance too low for API calls — top up at platform.deepseek.com".into()
                },
                windows,
                fetched_at: now_ms(),
                ..Default::default()
            }
        }
        Err(FetchErr::Rejected) => UsageSnapshot {
            status: "needsAuth".into(),
            note: "DeepSeek rejected the saved API key".into(),
            ..Default::default()
        },
        Err(FetchErr::Other(m)) => {
            crate::applog(&format!("deepseek: read failed ({m})"));
            let mut s = load_persisted();
            s.status = if s.windows.is_empty() { "error".into() } else { "stale".into() };
            s
        }
    }
}

/// Checks a key before it is stored: Ok(message) or Err((rejected, message))
pub fn test_key(key: &str) -> Result<String, (bool, String)> {
    match fetch(key) {
        Ok(v) => Ok(match short_balance(&v) {
            Some(b) => format!("Connected · {b} balance"),
            None => "Connected".into(),
        }),
        Err(FetchErr::Rejected) => Err((true, "DeepSeek rejected this key — check it was copied in full".into())),
        Err(FetchErr::Other(m)) => Err((false, m)),
    }
}

fn broadcast(app: &AppHandle, snap: UsageSnapshot) {
    let st = app.state::<AppState>();
    *st.deepseek.lock().unwrap() = snap.clone();
    persist(&snap);
    let _ = app.emit("deepseek", &snap);
}

pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        let snap = if key().is_some() {
            read_once()
        } else {
            UsageSnapshot { status: "absent".into(), ..Default::default() }
        };
        let was_absent = app.state::<AppState>().deepseek.lock().unwrap().status == "absent";
        let now_absent = snap.status == "absent";
        broadcast(&app, snap);
        // The cell count changes the open window's height: re-place when DeepSeek appears or goes
        if was_absent != now_absent {
            crate::place_notch(&app);
        }
        for _ in 0..POLL_SECS {
            if REFRESH.swap(false, Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn balance_strings_and_top_up_reference() {
        let v = serde_json::json!({
            "is_available": true,
            "balance_infos": [{"currency":"USD","total_balance":"7.50","granted_balance":"0.00","topped_up_balance":"7.50"}]
        });
        let mut refs = std::collections::HashMap::from([("USD".to_string(), 10.0)]);
        let (w, ok) = super::windows_from(&v, &mut refs);
        assert!(ok);
        assert!((w[0].used - 0.25).abs() < 1e-9, "a quarter of the 10.00 reference is gone");
        assert_eq!(super::short_balance(&v).as_deref(), Some("$7.50"));

        let topped = serde_json::json!({"is_available": true, "balance_infos": [{"currency":"USD","total_balance":"20"}]});
        let (w, _) = super::windows_from(&topped, &mut refs);
        assert_eq!(w[0].used, 0.0);
        assert_eq!(refs["USD"], 20.0, "a top-up raises the reference");

        let empty = serde_json::json!({"is_available": false, "balance_infos": [{"currency":"CNY","total_balance":"0"}]});
        let (w, ok) = super::windows_from(&empty, &mut refs);
        assert!(!ok);
        assert_eq!(w[0].used, 1.0);
    }
}
