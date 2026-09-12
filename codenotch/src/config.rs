use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    /// "auto" | "zh" | "en" | "ja" | "ko"
    #[serde(default = "default_lang")]
    pub lang: String,
    /// Free-floating position (physical px, window **centre**), set after a drag while drag_enabled
    /// is true. Centre rather than top-left so the island keeps its place when it grows or shrinks.
    #[serde(default)]
    pub bar_x: Option<i32>,
    #[serde(default)]
    pub bar_y: Option<i32>,
    /// Which screen edge the notch is docked to: "right" | "left" | "top" | "bottom".
    /// A drag releases onto the nearest edge (AssistiveTouch-style), unless drag_enabled is on.
    #[serde(default = "default_edge")]
    pub edge: String,
    /// Logical width of the bar (wheel-adjustable, 220-520); None = default 360. Unused for now — reserved for a future free-floating layout.
    #[serde(default)]
    pub bar_w: Option<u32>,
    /// "Move freely" (tray toggle, off by default to prevent accidental drags): true = unpinned
    /// from the right edge, dragged anywhere on screen and placed at bar_x/bar_y; false = the
    /// original edge notch, vertical-only drag along notch_y.
    #[serde(default)]
    pub drag_enabled: bool,
    /// Vertical position of the notch: the window centre as a fraction of the primary monitor's height (0 = top, 1 = bottom), default 0.5; saved after a drag
    #[serde(default = "default_notch_y")]
    pub notch_y: f64,
    /// Window opacity, 0.15-1.0 (tray submenu)
    #[serde(default = "default_opacity")]
    pub opacity: f64,
    /// Notch size multiplier, 0.7-1.6 (tray submenu)
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Base URL of the 9Router to read, e.g. "http://192.168.1.20:20128" for one running on another
    /// machine. Defaults to http://127.0.0.1:20128. Set from the API-keys window; the keys and
    /// tokens entered there are secrets and live in Credential Manager (secrets.rs), not here.
    #[serde(default)]
    pub router9_url: Option<String>,
}

fn default_notch_y() -> f64 {
    0.5
}
fn default_edge() -> String {
    "right".into()
}
fn default_opacity() -> f64 {
    1.0
}
fn default_scale() -> f64 {
    1.0
}

fn default_port() -> u16 {
    48666
}
fn default_lang() -> String {
    "auto".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: default_port(),
            lang: default_lang(),
            bar_x: None,
            bar_y: None,
            bar_w: None,
            drag_enabled: false,
            notch_y: default_notch_y(),
            edge: default_edge(),
            opacity: default_opacity(),
            scale: default_scale(),
            router9_url: None,
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("codenotch")
        .join("config.json")
}

pub fn load() -> Config {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

pub fn save(cfg: &Config) {
    let path = config_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(txt) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, txt);
    }
}
