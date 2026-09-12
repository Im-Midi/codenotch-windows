//! The API-keys window. The notch never takes focus (WS_EX_NOACTIVATE), so nothing can be typed
//! into it; keys are entered here instead, in an ordinary focusable window opened from the tray.

use crate::secrets;
use serde::Serialize;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};

pub fn open(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("Codenotch — API keys")
        .inner_size(480.0, 700.0)
        .resizable(false)
        .center()
        .build();
    if let Err(e) = built {
        crate::applog(&format!("settings window: {e}"));
    }
}

#[tauri::command]
pub fn open_settings(app: AppHandle) {
    open(&app);
}

#[derive(Serialize)]
pub struct Status {
    cc_source: Option<String>,
    cc_masked: Option<String>,
    r9_url: String,
    r9_url_custom: bool,
    r9_token_source: Option<String>,
    r9_local_available: bool,
    r9_cf_access: bool,
}

/// Enough of the key to recognise it, never enough to use it
fn mask(k: &str) -> String {
    let n = k.chars().count();
    if n <= 8 {
        return "•".repeat(n);
    }
    let tail: String = k.chars().skip(n - 4).collect();
    format!("••••••••{tail}")
}

#[tauri::command]
pub fn settings_status() -> Status {
    let cc = crate::commandcode::key_source();
    Status {
        cc_source: cc.as_ref().map(|(s, _)| s.to_string()),
        cc_masked: cc.as_ref().map(|(_, k)| mask(k)),
        r9_url: crate::router9::base_url(),
        r9_url_custom: secrets::get(secrets::ROUTER9_URL).is_some(),
        r9_token_source: crate::router9::token_source().map(String::from),
        r9_local_available: crate::router9::local_token().is_some(),
        r9_cf_access: crate::router9::cf_access().is_some(),
    }
}

#[derive(Serialize)]
pub struct Outcome {
    ok: bool,
    saved: bool,
    message: String,
    /// The URL answered with a Cloudflare Access sign-in: the window opens the service-token fields
    access_blocked: bool,
}

fn outcome(ok: bool, saved: bool, message: impl Into<String>) -> Outcome {
    Outcome { ok, saved, message: message.into(), access_blocked: false }
}

/// Tested against Command Code before it is kept: a key the server rejects is not stored at all,
/// while one that merely could not be checked (offline) is kept and the poller retries it.
#[tauri::command]
pub fn save_commandcode_key(key: String) -> Outcome {
    let key = key.trim().to_string();
    if key.is_empty() {
        return outcome(false, false, "Paste a key first");
    }
    let store = |msg: String, ok: bool| match secrets::set(secrets::COMMANDCODE, &key) {
        Ok(()) => {
            crate::commandcode::request_refresh();
            outcome(ok, true, msg)
        }
        Err(e) => outcome(false, false, format!("Could not store the key: {e}")),
    };
    match crate::commandcode::test_key(&key) {
        Ok(user) => store(format!("Connected as {user}"), true),
        Err(crate::commandcode::TestErr::Rejected(m)) => outcome(false, false, m),
        Err(crate::commandcode::TestErr::Other(m)) => store(format!("Saved, but could not verify it yet ({m})"), false),
    }
}

#[tauri::command]
pub fn clear_commandcode_key() {
    secrets::delete(secrets::COMMANDCODE);
    crate::commandcode::request_refresh();
}

/// Characters a URL needs; anything else (spaces, quotes, & | ^ < >) is refused outright rather
/// than escaped, since the dashboard link is later handed to the shell to open.
fn url_is_plain(u: &str) -> bool {
    u.chars().all(|c| c.is_ascii_alphanumeric() || ":/.-_~%?=[]".contains(c))
}

/// The URL field is filled with what is saved, so emptying it means "back to this PC's 9Router".
/// Secret fields left empty keep what is already stored: the window clears them after every save,
/// so "empty means delete" silently threw away a saved token the next time Save was pressed.
/// Removing those is Reset's job.
#[tauri::command]
pub fn save_router9(url: String, token: String, cf_id: String, cf_secret: String) -> Outcome {
    let url = crate::router9::normalize_base(&url);
    if !url.is_empty() && !((url.starts_with("http://") || url.starts_with("https://")) && url_is_plain(&url)) {
        return outcome(false, false, "Enter a plain URL starting with http:// or https://");
    }
    let cf_id = crate::router9::clean_cf_value(&cf_id, "CF-Access-Client-Id");
    let cf_secret = crate::router9::clean_cf_value(&cf_secret, "CF-Access-Client-Secret");
    if cf_id.is_empty() != cf_secret.is_empty() {
        return outcome(false, false, "A Cloudflare Access service token needs both the Client ID and the Client Secret");
    }
    if !cf_id.is_empty() && !cf_id.ends_with(".access") {
        return outcome(false, false, "That isn't a Client ID — Cloudflare's end in “.access”. Check the two fields aren't swapped");
    }
    if token.trim().starts_with("sk-") {
        return outcome(false, false,
            "That's one of 9Router's proxy API keys (sk-…). Its usage API only accepts the CLI token — 16 hex characters worked out on the machine running 9Router");
    }
    let token = token.trim().to_string();
    let mut writes: Vec<(&str, &str)> = Vec::new();
    if url.is_empty() {
        secrets::delete(secrets::ROUTER9_URL);
    } else {
        writes.push((secrets::ROUTER9_URL, &url));
    }
    if !token.is_empty() {
        writes.push((secrets::ROUTER9_TOKEN, &token));
    }
    if !cf_id.is_empty() {
        writes.push((secrets::ROUTER9_CF_ID, &cf_id));
        writes.push((secrets::ROUTER9_CF_SECRET, &cf_secret));
    }
    for (target, value) in writes {
        if let Err(e) = secrets::set(target, value) {
            return outcome(false, false, format!("Could not store it: {e}"));
        }
    }
    crate::router9::request_refresh();
    match crate::router9::test() {
        Ok(m) => outcome(true, true, m),
        Err(f) => Outcome { ok: false, saved: true, message: f.message, access_blocked: f.access_blocked },
    }
}

/// Reset: back to reading this PC's own 9Router, with nothing remote remembered
#[tauri::command]
pub fn clear_router9() {
    for t in [secrets::ROUTER9_URL, secrets::ROUTER9_TOKEN, secrets::ROUTER9_CF_ID, secrets::ROUTER9_CF_SECRET] {
        secrets::delete(t);
    }
    crate::router9::request_refresh();
}

/// This PC's own token, for pasting into Codenotch on another computer that should read this 9Router
#[tauri::command]
pub fn router9_local_token() -> Option<String> {
    crate::router9::local_token()
}

/// Opens one of a fixed set of pages. explorer.exe takes the URL as a plain argument, so unlike
/// `cmd /C start` nothing in it is ever parsed as a shell command.
#[tauri::command]
pub fn open_link(which: String) {
    let url = match which.as_str() {
        "commandcode_keys" => "https://commandcode.ai/studio/".to_string(),
        "router9_dashboard" => format!("{}/dashboard", crate::router9::base_url()),
        "cf_service_tokens" => "https://one.dash.cloudflare.com/".to_string(),
        _ => return,
    };
    if !url_is_plain(&url) {
        return;
    }
    let mut cmd = std::process::Command::new("explorer");
    cmd.arg(url);
    let _ = cmd.spawn();
}
