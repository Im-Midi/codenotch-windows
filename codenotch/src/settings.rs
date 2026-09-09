use tauri::{AppHandle, Emitter, Manager};
use crate::{config::Config, placement};
use std::sync::atomic::{AtomicBool, Ordering};

pub static HIDDEN: AtomicBool = AtomicBool::new(false);
static SHOW_FULLSCREEN: AtomicBool = AtomicBool::new(false);

pub fn enabled(app: &AppHandle, provider: &str) -> bool {
    app.state::<crate::AppState>().cfg.lock().unwrap().providers.iter().any(|p| p == provider)
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Config { app.state::<crate::AppState>().cfg.lock().unwrap().clone() }

#[tauri::command]
pub fn get_displays(app: AppHandle) -> Vec<placement::Display> { placement::displays(&app) }

#[tauri::command]
pub fn save_settings(app: AppHandle, window: tauri::WebviewWindow, mut settings: Config) -> Result<(), String> {
    if window.label() != "settings" { return Err("Open Settings to change preferences.".into()); }
    settings.validate()?;
    if !settings.monitor_id.is_empty() && !placement::displays(&app).iter().any(|d| d.id == settings.monitor_id) {
        // Keep an unplugged preferred monitor; reject only newly submitted unknown identities.
        if app.state::<crate::AppState>().cfg.lock().unwrap().monitor_id != settings.monitor_id { return Err("That monitor is no longer connected. Choose an available display.".into()); }
    }
    let st = app.state::<crate::AppState>();
    let mut current = st.cfg.lock().unwrap();
    settings.port = current.port;
    let restart = settings.codex_home != current.codex_home;
    crate::config::try_save(&settings)?;
    *current = settings.clone(); drop(current);
    placement::apply(&app);
    let _ = app.emit("settings", &settings);
    crate::refresh_usage(app.clone());
    if restart { app.restart(); }
    Ok(())
}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("settings") { w.show().map_err(|e|e.to_string())?; return w.set_focus().map_err(|e|e.to_string()); }
    tauri::WebviewWindowBuilder::new(&app, "settings", tauri::WebviewUrl::App("settings.html".into()))
        .title("Codenotch settings").inner_size(660.0,780.0).min_inner_size(440.0,420.0)
        .build().map(|_|()).map_err(|e|e.to_string())
}

#[tauri::command]
pub fn close_settings(window: tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "settings" { return Err("Only the settings window can close itself.".into()); }
    window.close().map_err(|e|e.to_string())
}

pub fn show(app: &AppHandle) {
    HIDDEN.store(false, Ordering::Relaxed); SHOW_FULLSCREEN.store(true, Ordering::Relaxed);
    if let Some(w)=app.get_webview_window("notch") { let _=w.show(); crate::noactivate(app); }
}
pub fn toggle(app: &AppHandle) {
    if HIDDEN.load(Ordering::Relaxed) { show(app); } else {
        HIDDEN.store(true, Ordering::Relaxed);
        if let Some(w)=app.get_webview_window("notch") { let _=w.hide(); }
    }
}

pub fn start_display_watch(app: AppHandle) {
    std::thread::spawn(move || {
        let mut previous = placement::displays(&app);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
            let now = placement::displays(&app);
            if now != previous {
                previous = now;
                placement::apply(&app);
                let _ = app.emit("displays_changed", ());
            }
            let cfg = app.state::<crate::AppState>().cfg.lock().unwrap().clone();
            let full = cfg.hide_fullscreen && placement::fullscreen(&app, &cfg);
            if !full { SHOW_FULLSCREEN.store(false, Ordering::Relaxed); }
            let hide = HIDDEN.load(Ordering::Relaxed) || (full && !SHOW_FULLSCREEN.load(Ordering::Relaxed));
            if let Some(w) = app.get_webview_window("notch") {
                if w.is_visible().unwrap_or(false) == hide { if hide { let _=w.hide(); } else { let _=w.show(); crate::noactivate(&app); } }
            }
        }
    });
}
