#![cfg_attr(all(not(debug_assertions), windows), windows_subsystem = "windows")]

mod autostart;
mod config;
mod doctor;
mod focus;
mod hooks_install;
mod i18n;
mod server;
mod state;
mod tray;
mod usage;
mod codex;
mod cursor;
mod antigravity;
mod commandcode;
mod router9;
mod glyphs;
mod activity;
mod diag;
mod watcher;

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

/// Logical size of the notch window when open on a side edge: the 70 pt pill column plus room for the hover card.
pub const NOTCH_W: f64 = 340.0;
/// Hand-bumped build tag, written to run.log at startup so a log can always be matched to the exe that wrote it.
pub const BUILD: &str = "r32";
pub const NOTCH_H: f64 = 460.0; // 300 clipped the card once it held three window blocks plus the session list
/// Open size in island form (top/bottom edge, or free-floating): cells run in a row, card sits under them.
/// Wide enough for six cells (6×56 + 5×18 + padding = 466) plus room for the card to sit under an end cell.
pub const ISLAND_W: f64 = 560.0;
pub const ISLAND_H: f64 = 440.0;
/// The island hangs from the top of its own window (16 px inset + ~52 px half-height), so this is
/// where its centre sits. Free-floating placement anchors on that point, not the window centre, or
/// the island would jump upwards the moment the window grew around the handle.
const ISLAND_ANCHOR_Y: f64 = 68.0;
/// Cell metrics, mirroring ui/notch.html: ring 56 + 6 gap + 18 label, stacked with a 14 gap inside 18 padding.
/// Only the *height* of the open window follows the provider count — the width stays fixed so the page's
/// design-width zoom correction (fitZoom) keeps a single number to compare against.
const CELL_H: f64 = 80.0;
const COL_GAP: f64 = 14.0;
const CARD_H: f64 = 280.0;
/// Closed size. The window shrinks to roughly the handle itself so the rest of the screen stays clickable —
/// an always-on-top window swallows clicks over its transparent area, so "hidden" has to mean "small", not "invisible".
pub const HANDLE_LONG: f64 = 132.0;
pub const HANDLE_SHORT: f64 = 18.0;

/// Open (true) or collapsed to the handle (false). The page drives this through `notch_expand`.
static EXPANDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn edge_is_horizontal(edge: &str) -> bool {
    edge == "top" || edge == "bottom"
}

pub struct AppState {
    pub store: Mutex<state::Store>,
    pub cfg: Mutex<config::Config>,
    pub usage: Mutex<usage::UsageSnapshot>,
    /// Codex snapshot (same UsageSnapshot shape; status may also be none/absent)
    pub codex: Mutex<usage::UsageSnapshot>,
    pub cursor: Mutex<usage::UsageSnapshot>,
    pub antigravity: Mutex<usage::UsageSnapshot>,
    pub commandcode: Mutex<usage::UsageSnapshot>,
    pub router9: Mutex<usage::UsageSnapshot>,
    /// Provider glyph cache, collected at launch and again on a tray refresh
    pub glyphs: Mutex<std::collections::HashMap<String, glyphs::Glyph>>,
    /// Working state of the non-Claude providers (Cursor reports it; Codex and Antigravity are inferred from recent writes)
    pub activity: Mutex<Vec<activity::Activity>>,
}

fn resolved_lang(raw: &str) -> String {
    if raw == "auto" {
        i18n::resolve_auto().to_string()
    } else {
        raw.to_string()
    }
}

pub fn broadcast(app: &AppHandle) {
    let st = app.state::<AppState>();
    let snap = {
        let store = st.store.lock().unwrap();
        let cfg = st.cfg.lock().unwrap();
        store.snapshot(&cfg.lang, &resolved_lang(&cfg.lang), false)
    };
    let _ = app.emit("state", &snap);
}

/// Distance from the window's top to the island's own centre: the pill hangs from the top when open,
/// and the handle fills the whole (tiny) window when closed.
fn island_anchor_y(expanded: bool, wh: i32, notch_scale: f64) -> i32 {
    if expanded {
        (ISLAND_ANCHOR_Y * notch_scale).round() as i32
    } else {
        wh / 2
    }
}

/// How many provider cells the pill will draw: Claude always has one, the rest only once they report
/// something other than "absent" (same rule as `providers()` in the page).
fn visible_providers(app: &AppHandle) -> usize {
    let st = app.state::<AppState>();
    let mut n = 1;
    for m in [&st.codex, &st.cursor, &st.antigravity, &st.commandcode, &st.router9] {
        if m.lock().map(|s| s.status != "absent").unwrap_or(false) {
            n += 1;
        }
    }
    n
}

/// Logical window size for the current mode: open or collapsed to the handle, docked on a side edge
/// (tall) or lying along a top/bottom edge or floating free (wide island). A tall column has to fit
/// every cell — six providers need ~586 px, well past the 460 the notch used when there were four.
fn notch_size(edge: &str, free_move: bool, expanded: bool, notch_scale: f64, cells: usize) -> (f64, f64) {
    let horizontal = free_move || edge_is_horizontal(edge);
    let (w, h) = match (expanded, horizontal) {
        (true, true) => (ISLAND_W, ISLAND_H),
        (true, false) => {
            let n = cells.max(1) as f64;
            let pill_h = n * CELL_H + (n - 1.0) * COL_GAP + 36.0;
            (NOTCH_W, pill_h.max(CARD_H) + 48.0)
        }
        (false, true) => (HANDLE_LONG, HANDLE_SHORT),
        (false, false) => (HANDLE_SHORT, HANDLE_LONG),
    };
    (w * notch_scale, h * notch_scale)
}

/// Places the notch: docked against its edge (the pill's centre held at the saved ratio along that
/// edge, so growing and shrinking never makes it wander), or free-floating around the saved centre.
pub fn place_notch(app: &AppHandle) {
    let Some(w) = app.get_webview_window("notch") else {
        return;
    };
    let scale = w.scale_factor().unwrap_or(1.0);
    let (free_move, free_centre, notch_scale, edge, ratio) = {
        let st = app.state::<AppState>();
        let c = st.cfg.lock().unwrap();
        (
            c.drag_enabled,
            c.bar_x.zip(c.bar_y),
            c.scale.clamp(0.7, 1.6),
            c.edge.clone(),
            c.notch_y.clamp(0.0, 1.0),
        )
    };
    let expanded = EXPANDED.load(std::sync::atomic::Ordering::Relaxed);
    let cells = visible_providers(app);
    if let Ok(Some(mon)) = w.primary_monitor() {
        // Two monitors at different scales (150 % and 200 % in practice): the physical size can
        // end up converted with the *other* monitor's scale factor depending on where the window
        // is created and then moved, leaving the WebView ~256 logical px wide instead of 340.
        // So the physical size is pinned straight from mon.scale_factor() before placing the
        // window; if it still reports a different scale afterwards, it is pinned once more.
        let ms = mon.scale_factor();
        let (nw, nh) = notch_size(&edge, free_move, expanded, notch_scale, cells);
        let target = tauri::PhysicalSize::new((nw * ms).round() as u32, (nh * ms).round() as u32);
        let _ = w.set_size(target);
        // Position from the window's measured physical size — deriving it from the scale factor
        // pushed the window past the right edge at 125 % / 150 % (the ring's right side was clipped).
        let (ww, wh) = w
            .outer_size()
            .map(|s| (s.width as i32, s.height as i32))
            .unwrap_or(((nw * scale) as i32, (nh * scale) as i32));
        let (mx, my) = (mon.position().x, mon.position().y);
        let (mw, mh) = (mon.size().width as i32, mon.size().height as i32);
        let (x, y) = if free_move {
            let (cx, cy) = free_centre.unwrap_or((mx + mw - ww / 2, my + mh / 2));
            let anchor = island_anchor_y(expanded, wh, notch_scale);
            clamp_to_virtual_screen(cx - ww / 2, cy - anchor, ww, wh)
        } else {
            match edge.as_str() {
                // Along the edge the centre comes from the saved ratio; across it the window is flush.
                "left" => {
                    let y = (my as f64 + mh as f64 * ratio - wh as f64 / 2.0).round() as i32;
                    (mx, y.clamp(my, my + (mh - wh).max(0)))
                }
                "top" => {
                    let x = (mx as f64 + mw as f64 * ratio - ww as f64 / 2.0).round() as i32;
                    (x.clamp(mx, mx + (mw - ww).max(0)), my)
                }
                "bottom" => {
                    let x = (mx as f64 + mw as f64 * ratio - ww as f64 / 2.0).round() as i32;
                    (x.clamp(mx, mx + (mw - ww).max(0)), my + mh - wh)
                }
                _ => {
                    let y = (my as f64 + mh as f64 * ratio - wh as f64 / 2.0).round() as i32;
                    (mx + mw - ww, y.clamp(my, my + (mh - wh).max(0)))
                }
            }
        };
        let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
        if w.outer_size().map(|s| s.width != target.width).unwrap_or(false) {
            let _ = w.set_size(target);
            let x = match (free_move, edge.as_str()) {
                (false, "right") => mx + mw - target.width as i32,
                _ => x,
            };
            let _ = w.set_position(tauri::PhysicalPosition::new(x, y));
        }
        // Placement log line: the first thing to check when the notch is not visible.
        // Appended, never rewritten — this used to truncate run.log, and now that the notch is placed
        // on every open and close it would wipe every other provider's diagnostics within seconds.
        applog(&format!(
            "notch placed build={BUILD}: edge={edge} free={free_move} open={expanded} pos=({x},{y}) size=({ww}x{wh}) inner={:?} win_scale={scale} mon_scale={ms} monitor=({mx},{my} {mw}x{mh})",
            w.inner_size().map(|s| (s.width, s.height)).unwrap_or((0, 0)),
        ));
    }
}

/// The page opens and closes the notch; the window itself follows so the collapsed handle does not
/// swallow clicks meant for whatever is behind it.
#[tauri::command]
fn notch_expand(app: AppHandle, on: bool) {
    if EXPANDED.swap(on, std::sync::atomic::Ordering::Relaxed) == on {
        return;
    }
    place_notch(&app);
}

/// Keeps a free-floating drag inside the combined bounds of every attached monitor (not just the
/// primary one), so a drag onto a second screen at a different DPI still lands somewhere visible.
#[cfg(windows)]
fn clamp_to_virtual_screen(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    };
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let cx = x.clamp(vx, (vx + vw - w).max(vx));
        let cy = y.clamp(vy, (vy + vh - h).max(vy));
        (cx, cy)
    }
}
#[cfg(not(windows))]
fn clamp_to_virtual_screen(x: i32, y: i32, _w: i32, _h: i32) -> (i32, i32) {
    (x, y)
}

/// Older entry point name still used by tray.rs
pub fn reset_bar(app: &AppHandle) {
    {
        let st = app.state::<AppState>();
        let mut c = st.cfg.lock().unwrap();
        c.notch_y = 0.5;
        c.bar_x = None;
        c.bar_y = None;
        c.edge = "right".into();
        config::save(&c);
    }
    place_notch(app);
    emit_config(app);
}

/// Drag along the right edge. The page calls this once after a press on the pill moves more than
/// 4 px; from then on a Rust thread follows the system cursor (WebView mousemove is unreliable
/// once the window itself starts moving). Releasing the left button ends the drag and the centre
/// ratio is written back to the config.
static DRAGGING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn left_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    unsafe { (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 }
}
#[cfg(not(windows))]
fn left_button_down() -> bool {
    false
}

#[tauri::command]
fn drag_begin(app: AppHandle) {
    if DRAGGING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let free_move = {
        let st = app.state::<AppState>();
        let c = st.cfg.lock().unwrap();
        c.drag_enabled
    };
    std::thread::spawn(move || {
        let Some(w) = app.get_webview_window("notch") else {
            DRAGGING.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        };
        let (Ok(start_cur), Ok(start_pos), Ok(size)) = (app.cursor_position(), w.outer_position(), w.outer_size())
        else {
            DRAGGING.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        };
        let (ww, wh) = (size.width as i32, size.height as i32);
        let mut moved = false;
        // Both modes drag freely across every monitor; only the release differs — docked snaps to
        // the nearest edge (AssistiveTouch), free keeps the island wherever it was let go.
        let mut last = (start_pos.x, start_pos.y);
        loop {
            if !left_button_down() {
                break;
            }
            if let Ok(cur) = app.cursor_position() {
                let nx = (start_pos.x as f64 + (cur.x - start_cur.x)).round() as i32;
                let ny = (start_pos.y as f64 + (cur.y - start_cur.y)).round() as i32;
                let (nx, ny) = clamp_to_virtual_screen(nx, ny, ww, wh);
                if (nx, ny) != last {
                    last = (nx, ny);
                    moved = true;
                    let _ = w.set_position(tauri::PhysicalPosition::new(nx, ny));
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
        if moved {
            let centre = (last.0 + ww / 2, last.1 + wh / 2);
            if free_move {
                // Saved against the island's own anchor point, matching how place_notch reads it back
                let notch_scale = {
                    let st = app.state::<AppState>();
                    let c = st.cfg.lock().unwrap();
                    c.scale.clamp(0.7, 1.6)
                };
                let anchor = island_anchor_y(EXPANDED.load(std::sync::atomic::Ordering::Relaxed), wh, notch_scale);
                let st = app.state::<AppState>();
                let mut c = st.cfg.lock().unwrap();
                c.bar_x = Some(centre.0);
                c.bar_y = Some(last.1 + anchor);
                config::save(&c);
                applog(&format!("notch drag (island): anchor=({},{})", centre.0, last.1 + anchor));
            } else if let Ok(Some(mon)) = w.primary_monitor() {
                let (mx, my) = (mon.position().x, mon.position().y);
                let (mw, mh) = (mon.size().width as i32, mon.size().height as i32);
                // Nearest edge by gap, then the centre's position along that edge becomes the ratio
                let gaps = [
                    ("left", centre.0 - mx),
                    ("right", mx + mw - centre.0),
                    ("top", centre.1 - my),
                    ("bottom", my + mh - centre.1),
                ];
                let (edge, _) = gaps.iter().min_by_key(|(_, g)| *g).copied().unwrap_or(("right", 0));
                let ratio = if edge_is_horizontal(edge) {
                    (centre.0 - mx) as f64 / mw.max(1) as f64
                } else {
                    (centre.1 - my) as f64 / mh.max(1) as f64
                };
                {
                    let st = app.state::<AppState>();
                    let mut c = st.cfg.lock().unwrap();
                    c.edge = edge.to_string();
                    c.notch_y = ratio.clamp(0.0, 1.0);
                    config::save(&c);
                }
                applog(&format!("notch drag: snapped to {edge} ratio={ratio:.3}"));
                emit_config(&app);
            }
            place_notch(&app); // settle into the snapped/clamped spot
        }
        DRAGGING.store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = app.emit("drag_end", moved);
    });
}
pub fn place_bar(app: &AppHandle) {
    place_notch(app);
}
pub fn toggle_drag(app: &AppHandle) {
    // The notch stays welded to the edge; kept as a no-op for the tray menu code path
    let _ = app;
}

pub fn apply_lang(app: &AppHandle, lang: &str) {
    {
        let st = app.state::<AppState>();
        let mut c = st.cfg.lock().unwrap();
        c.lang = lang.to_string();
        config::save(&c);
    }
    if let Some(tray) = app.tray_by_id("main") {
        if let Ok(menu) = tray::build_menu(app, lang) {
            let _ = tray.set_menu(Some(menu));
        }
    }
    broadcast(app);
}

/// The notch must never take focus: WS_EX_NOACTIVATE + WS_EX_TOOLWINDOW
#[cfg(windows)]
fn noactivate(app: &AppHandle) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    };
    if let Some(w) = app.get_webview_window("notch") {
        if let Ok(h) = w.hwnd() {
            unsafe {
                let hwnd =
                    windows::Win32::Foundation::HWND(h.0 as isize as *mut core::ffi::c_void);
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(
                    hwnd,
                    GWL_EXSTYLE,
                    ex | WS_EX_NOACTIVATE.0 as isize | WS_EX_TOOLWINDOW.0 as isize,
                );
            }
        }
    }
}
#[cfg(not(windows))]
fn noactivate(_app: &AppHandle) {}

// ---------------- commands ----------------

#[tauri::command]
fn get_state(state: tauri::State<AppState>) -> state::Snapshot {
    let store = state.store.lock().unwrap();
    let cfg = state.cfg.lock().unwrap();
    store.snapshot(&cfg.lang, &resolved_lang(&cfg.lang), false)
}

#[tauri::command]
fn get_usage(state: tauri::State<AppState>) -> usage::UsageSnapshot {
    state.usage.lock().unwrap().clone()
}

#[tauri::command]
fn refresh_usage(app: AppHandle) {
    {
        let st = app.state::<AppState>();
        let mut u = st.usage.lock().unwrap();
        u.backoff_until = 0;
    }
    usage::request_refresh();
    codex::request_refresh();
    cursor::request_refresh();
    antigravity::request_refresh();
    commandcode::request_refresh();
    router9::request_refresh();
}

#[tauri::command]
fn get_antigravity(state: tauri::State<AppState>) -> usage::UsageSnapshot {
    state.antigravity.lock().unwrap().clone()
}

#[tauri::command]
fn get_commandcode(state: tauri::State<AppState>) -> usage::UsageSnapshot {
    state.commandcode.lock().unwrap().clone()
}

#[tauri::command]
fn get_router9(state: tauri::State<AppState>) -> usage::UsageSnapshot {
    state.router9.lock().unwrap().clone()
}

#[tauri::command]
fn get_config(state: tauri::State<AppState>) -> config::Config {
    state.cfg.lock().unwrap().clone()
}

/// Pushes the current config to the page (opacity/scale live-applied via CSS) and repositions/resizes the window
pub fn emit_config(app: &AppHandle) {
    let cfg = {
        let st = app.state::<AppState>();
        let c = st.cfg.lock().unwrap();
        c.clone()
    };
    let _ = app.emit("config", &cfg);
}

#[tauri::command]
fn get_activity(state: tauri::State<AppState>) -> Vec<activity::Activity> {
    state.activity.lock().unwrap().clone()
}

#[tauri::command]
fn get_glyphs(state: tauri::State<AppState>) -> std::collections::HashMap<String, glyphs::Glyph> {
    state.glyphs.lock().unwrap().clone()
}

/// Collects the glyphs again and pushes them to the page (tray refresh, or the user just dropped in an override)
pub fn reload_glyphs(app: &AppHandle) {
    let m = glyphs::collect();
    let st = app.state::<AppState>();
    *st.glyphs.lock().unwrap() = m.clone();
    let _ = app.emit("glyphs", &m);
}

#[tauri::command]
fn open_data_dir() {
    let dir = config::config_path().parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let _ = std::fs::create_dir_all(glyphs::user_dir());
    let mut cmd = std::process::Command::new("explorer");
    cmd.arg(dir.as_os_str());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd.spawn();
}

#[tauri::command]
fn get_cursor(state: tauri::State<AppState>) -> usage::UsageSnapshot {
    state.cursor.lock().unwrap().clone()
}

#[tauri::command]
fn get_codex(state: tauri::State<AppState>) -> usage::UsageSnapshot {
    state.codex.lock().unwrap().clone()
}

/// A click on a cell opens that provider's usage page
#[tauri::command]
fn open_provider_page(provider: String) {
    let url = match provider.as_str() {
        "codex" => "https://chatgpt.com/#settings/Account",
        "cursor" => "https://cursor.com/dashboard",
        "gemini" => "https://antigravity.google",
        "commandcode" => "https://commandcode.ai",
        "router9" => "http://127.0.0.1:20128/dashboard",
        _ => "https://claude.ai/settings/usage",
    };
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/C", "start", "", url]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd.spawn();
}

/// Card expansion state: Some(hot rectangles, in **physical pixels** relative to the window's
/// top-left as x,y,w,h) = expanded; None = collapsed. The page converts the rectangles with its
/// own devicePixelRatio before reporting them, so no scale conversion happens on this side —
/// WebView2's DPR and the window's scale_factor can disagree (see report_dpr).
static HOT: Mutex<Option<Vec<[f64; 4]>>> = Mutex::new(None);

#[tauri::command]
fn set_expanded(on: bool, rects: Option<Vec<[f64; 4]>>) {
    *HOT.lock().unwrap() = if on { Some(rects.unwrap_or_default()) } else { None };
}

/// The WebView zoom currently applied (1.0 = uncorrected)
static ZOOM: Mutex<f64> = Mutex::new(1.0);

pub fn applog(line: &str) {
    use std::io::Write;
    let log = config::config_path().with_file_name("run.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log) {
        let _ = writeln!(f, "{line}");
    }
}

/// Root cause: with two monitors (150 % / 200 %) WebView2 picked a devicePixelRatio of 2.0 while
/// the window was sized for the primary monitor's 1.5, so the page was 255 CSS px wide instead of
/// the designed 340 and every coordinate conversion was off (the watchdog misfired and the card
/// flashed away). Fix: the page reports its DPR, and when it differs from the primary monitor's
/// scale, set_zoom pulls the effective DPR back to that scale, restoring the 340 px width.
#[tauri::command]
fn report_dpr(app: AppHandle, dpr: f64, w: f64, h: f64) {
    let Some(win) = app.get_webview_window("notch") else { return };
    let want = win
        .primary_monitor()
        .ok()
        .flatten()
        .map(|m| m.scale_factor())
        .unwrap_or_else(|| win.scale_factor().unwrap_or(1.0));
    let mut z = ZOOM.lock().unwrap();
    let base = if *z > 0.0 { dpr / *z } else { dpr };
    let target = if base > 0.0 { want / base } else { 1.0 };
    applog(&format!(
        "dpr report: dpr={dpr:.3} viewport={w:.0}x{h:.0} monitor_scale={want:.3} zoom_applied={:.3} -> target_zoom={target:.3}",
        *z
    ));
    // Oscillation guard: at most three corrections per process (if the DPR does not follow the zoom, stop chasing it)
    static APPLIED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if (dpr - want).abs() > 0.02
        && (target - *z).abs() > 0.01
        && (0.25..=4.0).contains(&target)
        && APPLIED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3
    {
        match win.set_zoom(target) {
            Ok(()) => {
                *z = target;
                applog(&format!("dpr correction: set_zoom({target:.3}) ok"));
            }
            Err(e) => applog(&format!("dpr correction failed: {e}")),
        }
    }
}

/// WebView2's mouseleave is unreliable inside a NOACTIVATE transparent window — a cursor that
/// leaves quickly often produces no WM_MOUSELEAVE, and the card stays up. Rather than trust DOM
/// events, the Rust side watches the system cursor while the card is expanded and emits
/// pointer_left once the cursor is outside; the page collapses after its 250 ms grace period.
/// "Outside the window" is not the test, though: the window has a 340×460 transparent area, so
/// the cursor is compared against the hot rectangles the page reports (pill, card, and the gap
/// between them), and two consecutive misses (300 ms) count as leaving.
fn start_pointer_watchdog(app: AppHandle) {
    std::thread::spawn(move || {
        let mut miss = 0u8;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(150));
            let rects = match HOT.lock().unwrap().clone() {
                Some(r) => r,
                None => {
                    miss = 0;
                    continue;
                }
            };
            let Some(w) = app.get_webview_window("notch") else { continue };
            let (Ok(pos), Ok(cur)) = (w.outer_position(), app.cursor_position()) else { continue };
            // Cursor position relative to the window's top-left, in physical pixels; the hot rectangles are physical too, so no scale conversion
            let lx = cur.x - pos.x as f64;
            let ly = cur.y - pos.y as f64;
            const PAD: f64 = 10.0;
            let in_window = w
                .outer_size()
                .map(|s| lx >= 0.0 && ly >= 0.0 && lx < s.width as f64 && ly < s.height as f64)
                .unwrap_or(true);
            let mut inside = in_window && rects.iter().any(|r| {
                lx >= r[0] - PAD && ly >= r[1] - PAD && lx < r[0] + r[2] + PAD && ly < r[1] + r[3] + PAD
            });
            // The gap between hot rectangles (pill and card) counts as inside: use the bounding box of all of them
            if !inside && in_window && rects.len() > 1 {
                let x0 = rects.iter().map(|r| r[0]).fold(f64::MAX, f64::min);
                let y0 = rects.iter().map(|r| r[1]).fold(f64::MAX, f64::min);
                let x1 = rects.iter().map(|r| r[0] + r[2]).fold(f64::MIN, f64::max);
                let y1 = rects.iter().map(|r| r[1] + r[3]).fold(f64::MIN, f64::max);
                inside = lx >= x0 && ly >= y0 && lx < x1 && ly < y1;
            }
            static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 12 {
                applog(&format!(
                    "watchdog: cursor_rel=({lx:.0},{ly:.0}) inside={inside} rects={rects:?} winpos=({},{})",
                    pos.x, pos.y
                ));
            }
            if inside {
                miss = 0;
            } else {
                miss += 1;
                if miss >= 2 {
                    miss = 0;
                    *HOT.lock().unwrap() = None;
                    let _ = app.emit("pointer_left", ());
                }
            }
        }
    });
}

/// Log channel for the page: JS writes key diagnostics into run.log (if invoke itself fails, the page reports on screen instead)
#[tauri::command]
fn log_js(msg: String) {
    applog(&format!("js: {}", msg.chars().take(600).collect::<String>()));
}

#[tauri::command]
fn open_usage_page() {
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/C", "start", "", "https://claude.ai/settings/usage"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let _ = cmd.spawn();
}

#[tauri::command]
fn focus_session(app: AppHandle, id: String) -> bool {
    let ppid = {
        let st = app.state::<AppState>();
        let store = st.store.lock().unwrap();
        store.ppid_of(&id)
    };
    match ppid {
        Some(p) => focus::focus_terminal(p),
        None => focus::focus_claude_desktop(),
    }
}

#[tauri::command]
fn dismiss_session(app: AppHandle, id: String) {
    {
        let st = app.state::<AppState>();
        let mut store = st.store.lock().unwrap();
        store.dismiss(&id);
    }
    broadcast(&app);
}

#[tauri::command]
fn set_lang(app: AppHandle, lang: String) {
    apply_lang(&app, &lang);
}

/// Seen-clears-it: looking at a session acknowledges it (engine behaviour, unchanged)
#[cfg(windows)]
fn ack_scan(app: &AppHandle) -> bool {
    let need = {
        let st = app.state::<AppState>();
        let store = st.store.lock().unwrap();
        store.has_done()
    };
    if !need {
        return false;
    }
    let fg = focus::fg_pid();
    if fg == 0 {
        return false;
    }
    let maps = focus::proc_maps();
    let fg_name = maps.name.get(&fg).cloned().unwrap_or_default();
    let fg_is_claude_desktop = fg_name.contains("claude") && !fg_name.contains("codenotch");
    let st = app.state::<AppState>();
    let mut store = st.store.lock().unwrap();
    store.ack_done(|s| {
        if s.ppid == 0 {
            fg_is_claude_desktop
        } else {
            focus::pid_hits_chain(fg, &focus::chain_of(s.ppid, &maps.ppid), &maps)
        }
    })
}
#[cfg(not(windows))]
fn ack_scan(_app: &AppHandle) -> bool {
    false
}

// ---------------- main ----------------

#[cfg(windows)]
fn attach_console() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    unsafe {
        let _ = AttachConsole(ATTACH_PARENT_PROCESS);
    }
}
#[cfg(not(windows))]
fn attach_console() {}

fn report(r: Result<String, String>) {
    let msg = match r {
        Ok(m) => format!("OK: {m}"),
        Err(e) => format!("FAILED: {e}"),
    };
    println!("{msg}");
    let log = config::config_path().with_file_name("install.log");
    let _ = std::fs::write(log, &msg);
}

fn main() {
    attach_console();
    let args: Vec<String> = std::env::args().collect();
    if let Some(cmd) = args.get(1) {
        match cmd.as_str() {
            "install-hooks" => {
                report(hooks_install::install());
                return;
            }
            "uninstall-hooks" => {
                report(hooks_install::uninstall());
                return;
            }
            "autostart" => {
                let r = match args.get(2).map(|s| s.as_str()) {
                    Some("on") => autostart::enable(),
                    Some("off") => autostart::disable(),
                    _ => Err("usage: codenotch.exe autostart on|off".into()),
                };
                report(r);
                return;
            }
            "doctor" => {
                let out = if args.get(2).map(|s| s.as_str()) == Some("deep") { diag::run() } else { doctor::run() };
                println!("{out}");
                let log = config::config_path().with_file_name("doctor.log");
                let _ = std::fs::write(log, &out);
                return;
            }
            _ => {}
        }
    }

    let cfg = config::load();
    let port = cfg.port;

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // Launching a freshly built exe while the old one is still running lands here: the new
            // instance is turned away and what stays on screen is the old process. Say so loudly.
            applog(&format!("single instance: another launch was refused; the running instance is build={BUILD} — quit it from the tray first if you just rebuilt"));
            let _ = app.emit("notice", format!("Codenotch is already running ({BUILD}) — quit it from the tray before starting a new build"));
        }))
        .manage(AppState {
            store: Mutex::new(Default::default()),
            cfg: Mutex::new(cfg),
            usage: Mutex::new(usage::load_persisted()),
            codex: Mutex::new(codex::load_persisted()),
            cursor: Mutex::new(cursor::load_persisted()),
            antigravity: Mutex::new(antigravity::load_persisted()),
            commandcode: Mutex::new(commandcode::load_persisted()),
            router9: Mutex::new(router9::load_persisted()),
            glyphs: Mutex::new(Default::default()),
            activity: Mutex::new(Vec::new()),
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_usage,
            get_codex,
            get_cursor,
            get_antigravity,
            get_commandcode,
            get_router9,
            get_config,
            notch_expand,
            get_glyphs,
            get_activity,
            open_data_dir,
            drag_begin,
            open_provider_page,
            refresh_usage,
            open_usage_page,
            set_expanded,
            report_dpr,
            log_js,
            focus_session,
            dismiss_session,
            set_lang
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            place_notch(&handle);
            noactivate(&handle);
            if let Some(w) = handle.get_webview_window("notch") {
                let _ = w.show();
            }
            emit_config(&handle);
            tray::setup(&handle)?;
            server::start(handle.clone(), port);
            watcher::start(handle.clone());
            usage::start(handle.clone());
            codex::start(handle.clone());
            cursor::start(handle.clone());
            antigravity::start(handle.clone());
            commandcode::start(handle.clone());
            router9::start(handle.clone());
            activity::start(handle.clone());
            // Collecting glyphs may read icon resources out of a few executables; do it off the main thread and push when done
            let gh = handle.clone();
            std::thread::spawn(move || reload_glyphs(&gh));
            start_pointer_watchdog(handle.clone());
            // Seen-clears-it scan
            let acker = handle.clone();
            std::thread::spawn(move || {
                activity::lower_thread_priority();
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    if ack_scan(&acker) {
                        broadcast(&acker);
                    }
                }
            });
            // Stale session cleanup
            let sweeper = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(30));
                let changed = {
                    let st = sweeper.state::<AppState>();
                    let mut s = st.store.lock().unwrap();
                    s.sweep()
                };
                if changed {
                    broadcast(&sweeper);
                }
            });
            // Persist the config (codenotch-hook reads the port from it)
            {
                let st = handle.state::<AppState>();
                let c = st.cfg.lock().unwrap();
                config::save(&c);
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Codenotch failed to start");
}
