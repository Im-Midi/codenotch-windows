//! The API-keys window. The notch never takes focus (WS_EX_NOACTIVATE), so nothing can be typed
//! into it; keys are entered here instead, in an ordinary focusable window opened from the tray.

use crate::AppState;
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
        .inner_size(480.0, 640.0)
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
        r9_url_custom: crate::config::load().router9_url.is_some(),
        r9_token_source: crate::router9::token_source().map(String::from),
        r9_local_available: crate::router9::local_token().is_some(),
    }
}

#[derive(Serialize)]
pub struct Outcome {
    ok: bool,
    saved: bool,
    message: String,
}

fn outcome(ok: bool, saved: bool, message: impl Into<String>) -> Outcome {
    Outcome { ok, saved, message: message.into() }
}

/// Tested against Command Code before it is kept: a key the server rejects is not stored at all,
/// while one that merely could not be checked (offline) is kept and the poller retries it.
#[tauri::command]
pub fn save_commandcode_key(key: String) -> Outcome {
    let key = key.trim().to_string();
    if key.is_empty() {
        return outcome(false, false, "Paste a key first");
    }
    let store = |msg: String, ok: bool| match crate::secrets::set(crate::secrets::COMMANDCODE, &key) {
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
    crate::secrets::delete(crate::secrets::COMMANDCODE);
    crate::commandcode::request_refresh();
}

/// Characters a URL needs; anything else (spaces, quotes, & | ^ < >) is refused outright rather
/// than escaped, since the dashboard link is later handed to the shell to open.
fn url_is_plain(u: &str) -> bool {
    u.chars().all(|c| c.is_ascii_alphanumeric() || ":/.-_~%?=[]".contains(c))
}

#[tauri::command]
pub fn save_router9(app: AppHandle, url: String, token: String) -> Outcome {
    let url = url.trim().trim_end_matches('/').to_string();
    if !url.is_empty() && !((url.starts_with("http://") || url.starts_with("https://")) && url_is_plain(&url)) {
        return outcome(false, false, "Enter a plain URL starting with http:// or https://");
    }
    {
        // Through the shared config, or the next drag or tray toggle would write the old value back
        let st = app.state::<AppState>();
        let mut c = st.cfg.lock().unwrap();
        c.router9_url = if url.is_empty() { None } else { Some(url) };
        crate::config::save(&c);
    }
    let token = token.trim().to_string();
    if token.is_empty() {
        crate::secrets::delete(crate::secrets::ROUTER9_TOKEN);
    } else if let Err(e) = crate::secrets::set(crate::secrets::ROUTER9_TOKEN, &token) {
        return outcome(false, false, format!("Could not store the token: {e}"));
    }
    crate::router9::request_refresh();
    match crate::router9::test() {
        Ok(m) => outcome(true, true, m),
        Err(m) => outcome(false, true, m),
    }
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
        _ => return,
    };
    if !url_is_plain(&url) {
        return;
    }
    let mut cmd = std::process::Command::new("explorer");
    cmd.arg(url);
    let _ = cmd.spawn();
}
