//! Merges codenotch-hook.exe into ~/.claude/settings.json without overwriting the user's own hooks.
//! Identification: the command contains "codenotch-hook". A backup is written first.

use serde_json::{json, Value};
use std::path::PathBuf;

/// (Claude Code event name, whether it needs a matcher, the internal event reported to Codenotch)
const WIRING: &[(&str, bool, &str)] = &[
    ("SessionStart", false, "session_start"),
    ("UserPromptSubmit", false, "running"),
    ("PreToolUse", true, "running"),
    ("PostToolUse", true, "running"),
    ("Notification", false, "attention"),
    ("Stop", false, "done"),
    ("SessionEnd", false, "session_end"),
];

fn settings_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("settings.json"))
}

fn is_ours(entry: &Value) -> bool {
    entry["hooks"]
        .as_array()
        .map(|hs| {
            hs.iter().any(|h| {
                h["command"]
                    .as_str()
                    .map(|c| c.contains("codenotch-hook") || c.contains("eatbean-hook") || c.contains("pacman-hook"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn load(path: &PathBuf) -> Value {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({}))
}

fn backup_and_write(path: &PathBuf, root: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    if path.exists() {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        std::fs::copy(path, path.with_extension(format!("json.codenotch-bak-{ts}")))
            .map_err(|e| e.to_string())?;
    }
    let txt = serde_json::to_string_pretty(root).map_err(|e| e.to_string())?;
    std::fs::write(path, txt).map_err(|e| e.to_string())
}

pub fn is_installed() -> bool {
    settings_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.contains("codenotch-hook"))
        .unwrap_or(false)
}

pub fn install() -> Result<String, String> {
    let path = settings_path().ok_or("cannot find the user directory")?;
    let hook_exe = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .parent()
        .ok_or("cannot locate the program directory")?
        .join("codenotch-hook.exe");
    if !hook_exe.exists() {
        return Err(format!("missing {}", hook_exe.display()));
    }

    let mut root = load(&path);
    if !root.is_object() {
        root = json!({});
    }
    if !root["hooks"].is_object() {
        root["hooks"] = json!({});
    }

    for (event, need_matcher, internal) in WIRING {
        let arr = root["hooks"][*event].as_array().cloned().unwrap_or_default();
        // Remove our own older entries first
        let mut arr: Vec<Value> = arr.into_iter().filter(|e| !is_ours(e)).collect();
        let cmd = format!("\"{}\" {}", hook_exe.display(), internal);
        let mut entry = json!({
            "hooks": [{ "type": "command", "command": cmd, "timeout": 5 }]
        });
        if *need_matcher {
            entry["matcher"] = json!("*");
        }
        arr.push(entry);
        root["hooks"][*event] = json!(arr);
    }

    backup_and_write(&path, &root)?;
    Ok(format!("wrote {} ({} events)", path.display(), WIRING.len()))
}

pub fn uninstall() -> Result<String, String> {
    let path = settings_path().ok_or("cannot find the user directory")?;
    if !path.exists() {
        return Ok("settings.json does not exist, nothing to uninstall".into());
    }
    let mut root = load(&path);
    let Some(hooks) = root["hooks"].as_object_mut() else {
        return Ok("no hooks configuration found".into());
    };
    let mut removed = 0;
    for (_, v) in hooks.iter_mut() {
        if let Some(arr) = v.as_array() {
            let filtered: Vec<Value> = arr.iter().filter(|e| !is_ours(e)).cloned().collect();
            removed += arr.len() - filtered.len();
            *v = json!(filtered);
        }
    }
    backup_and_write(&path, &root)?;
    Ok(format!("removed {removed} Codenotch hook(s)"))
}

/// The installer must not remove hooks owned by another checkout or mixed into the same group.
pub fn uninstall_current_installation() -> Result<String, String> {
    let path = settings_path().ok_or("cannot find the user directory")?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let helper = executable.with_file_name("codenotch-hook.exe");
    uninstall_from(&path, &helper)
}

fn uninstall_from(path: &PathBuf, helper: &std::path::Path) -> Result<String, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok("no settings to clean".into()),
        Err(e) => return Err(e.to_string()),
    };
    let mut root: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let removed = remove_installed_hooks(&mut root, helper);
    if removed > 0 {
        backup_and_write(path, &root)?;
    }
    Ok(format!("removed {removed} installed Codenotch hook(s)"))
}

fn remove_installed_hooks(root: &mut Value, helper: &std::path::Path) -> usize {
    let prefix = format!("\"{}\" ", helper.display());
    let Some(events) = root.get_mut("hooks").and_then(Value::as_object_mut) else { return 0 };
    let mut removed = 0;
    for groups in events.values_mut().filter_map(Value::as_array_mut) {
        groups.retain_mut(|group| {
            let Some(commands) = group.get_mut("hooks").and_then(Value::as_array_mut) else { return true };
            let before = commands.len();
            commands.retain(|hook| {
                !hook["command"].as_str().map(|command| {
                    command.strip_prefix(&prefix)
                        .map(|event| WIRING.iter().any(|(_, _, internal)| event == *internal))
                        .unwrap_or(false)
                }).unwrap_or(false)
            });
            removed += before - commands.len();
            before == commands.len() || !commands.is_empty()
        });
    }
    removed
}

#[cfg(test)]
mod installer_tests {
    use super::*;

    #[test]
    fn uninstall_preserves_other_commands_and_installations() {
        let helper = std::path::Path::new(r"C:\Users\Zoë Test\Codenotch\codenotch-hook.exe");
        let mut root = json!({"other": true, "hooks": {"Stop": [
            {"matcher": "*", "hooks": [
                {"command": format!("\"{}\" done", helper.display())},
                {"command": "echo keep me"},
                {"command": "\"C:\\Other\\codenotch-hook.exe\" done"}
            ]},
            {"hooks": [{"command": format!("\"{}\" done", helper.display())}]},
            {"hooks": []}
        ]}});
        assert_eq!(remove_installed_hooks(&mut root, helper), 2);
        assert_eq!(root, json!({"other": true, "hooks": {"Stop": [
            {"matcher": "*", "hooks": [
                {"command": "echo keep me"},
                {"command": "\"C:\\Other\\codenotch-hook.exe\" done"}
            ]}, {"hooks": []}
        ]}}));
        assert_eq!(remove_installed_hooks(&mut root, helper), 0);
    }
}
