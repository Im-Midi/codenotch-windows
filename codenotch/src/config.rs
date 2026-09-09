use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    /// "auto" | "zh" | "en" | "ja" | "ko"
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default)]
    pub bar_x: Option<i32>,
    #[serde(default)]
    pub bar_y: Option<i32>,
    /// Logical width of the bar (wheel-adjustable, 220-520); None = default 360
    #[serde(default)]
    pub bar_w: Option<u32>,
    /// Allow dragging + wheel resizing (tray toggle, off by default to prevent accidental drags)
    #[serde(default)]
    pub drag_enabled: bool,
    /// Vertical position of the notch: the window centre as a fraction of the primary monitor's height (0 = top, 1 = bottom), default 0.5; saved after a drag
    #[serde(default = "default_notch_y")]
    pub notch_y: f64,
    #[serde(default)]
    pub monitor_id: String,
    #[serde(default)]
    pub edge: crate::placement::Edge,
    #[serde(default = "default_offsets")]
    pub offsets: [f64; 4],
    #[serde(default = "yes")]
    pub avoid_taskbar: bool,
    #[serde(default = "yes")]
    pub always_on_top: bool,
    #[serde(default = "yes")]
    pub hide_fullscreen: bool,
    #[serde(default)]
    pub auto_collapse: bool,
    #[serde(default)]
    pub reduced_motion: bool,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(default = "default_providers")]
    pub providers: Vec<String>,
    #[serde(default = "weekly")]
    pub codex_headline: String,
    #[serde(default)]
    pub codex_home: String,
}

fn yes() -> bool { true }
fn default_scale() -> f64 { 1.0 }
fn default_offsets() -> [f64; 4] { [0.5; 4] }
fn default_providers() -> Vec<String> { ["codex", "claude", "cursor", "gemini"].map(String::from).to_vec() }
fn weekly() -> String { "weekly".into() }

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if !self.scale.is_finite() || !(0.75..=1.5).contains(&self.scale) { return Err("Size must be between 75% and 150%.".into()); }
        if self.offsets.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) { return Err("Position must be between 0% and 100%.".into()); }
        let known = default_providers();
        if self.providers.len() > 4 || self.providers.iter().any(|p| !known.contains(p)) || self.providers.iter().collect::<std::collections::HashSet<_>>().len() != self.providers.len() { return Err("Provider order contains invalid or duplicate entries.".into()); }
        if !["weekly", "primary", "secondary", "highest"].contains(&self.codex_headline.as_str()) { return Err("Unknown Codex usage window.".into()); }
        if !self.codex_home.is_empty() && !std::path::Path::new(&self.codex_home).is_absolute() { return Err("Codex profile folder must be an absolute path.".into()); }
        if self.monitor_id.len() > 1024 || self.codex_home.len() > 4096 { return Err("Setting is too long.".into()); }
        Ok(())
    }
}

fn default_notch_y() -> f64 {
    0.5
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
            monitor_id: String::new(), edge: Default::default(), offsets: default_offsets(),
            avoid_taskbar: true, always_on_top: true, hide_fullscreen: true,
            auto_collapse: false, reduced_motion: false, scale: 1.0,
            providers: default_providers(), codex_headline: weekly(), codex_home: String::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("CODENOTCH_DATA_DIR") { return PathBuf::from(dir).join("config.json"); }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("codenotch")
        .join("config.json")
}

pub fn load() -> Config {
    let path = config_path();
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| {
            let mut c: Config = serde_json::from_str(&t).ok()?;
            if !t.contains("\"offsets\"") { c.offsets[0] = c.notch_y.clamp(0.0, 1.0); }
            c.validate().ok()?;
            Some(c)
        })
        .unwrap_or_default()
}

pub fn save(cfg: &Config) {
    if let Err(e) = try_save(cfg) { crate::applog(&format!("Settings not saved: {e}")); }
}

pub fn try_save(cfg: &Config) -> Result<(), String> {
    cfg.validate()?;
    let path = config_path();
    atomic_write(&path, &serde_json::to_vec_pretty(cfg).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let tmp = path.with_extension(format!("{}.{}.tmp", std::process::id(), SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(bytes)?; f.sync_all()?; drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_validation_and_atomic_replace() {
        let mut c: Config = serde_json::from_str("{}").unwrap();
        assert!(c.validate().is_ok());
        c.scale = f64::NAN; assert!(c.validate().is_err()); c.scale = 1.0;
        c.providers.push("codex".into()); assert!(c.validate().is_err());
        let dir = std::env::temp_dir().join(format!("codenotch-config-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap(); let p = dir.join("config.json");
        atomic_write(&p, b"old").unwrap(); atomic_write(&p, b"new").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
        // Replacing a directory must fail and preserve the previous contents.
        let block = dir.join("blocked"); std::fs::create_dir_all(&block).unwrap();
        std::fs::write(block.join("keep"), b"keep").unwrap();
        assert!(atomic_write(&block, b"bad").is_err());
        assert_eq!(std::fs::read(block.join("keep")).unwrap(), b"keep");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
