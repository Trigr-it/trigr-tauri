use log::{error, info, warn};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const MAX_BACKUPS: usize = 10;

// ── Path resolution ─────────────────────────────────────────────────────────

static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Call once at startup with the resolved app data dir.
pub fn init(app_data_dir: PathBuf) {
    let _ = APP_DATA_DIR.set(app_data_dir);
}

fn app_data_dir() -> &'static Path {
    APP_DATA_DIR
        .get()
        .expect("config::init() must be called before using config functions")
}

pub fn config_path() -> PathBuf {
    app_data_dir().join("keyforge-config.json")
}

fn backup_dir() -> PathBuf {
    app_data_dir().join("backups")
}

// ── Validation ──────────────────────────────────────────────────────────────

fn is_valid_config(cfg: &Value) -> bool {
    let obj = match cfg.as_object() {
        Some(o) => o,
        None => return false,
    };
    // Must have non-empty profiles array with no nulls
    match obj.get("profiles").and_then(|v| v.as_array()) {
        Some(arr) if !arr.is_empty() && arr.iter().all(|p| !p.is_null()) => {}
        _ => return false,
    }
    // Must have assignments object (not array)
    match obj.get("assignments") {
        Some(v) if v.is_object() => {}
        _ => return false,
    }
    true
}

// ── Core load/save ──────────────────────────────────────────────────────────

/// Simple runtime loader — no fallback chain.
pub fn load_config() -> Option<Value> {
    let path = config_path();
    if !path.exists() {
        return None;
    }
    match fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(v) => Some(v),
            Err(e) => {
                error!("[Trigr] Failed to parse config: {}", e);
                None
            }
        },
        Err(e) => {
            error!("[Trigr] Failed to read config: {}", e);
            None
        }
    }
}

/// Resilient loader: main config -> last-known-good -> timestamped backups (newest first).
/// Returns (config, restored_from) where restored_from is None if main config was healthy.
pub fn load_config_safe() -> (Option<Value>, Option<String>) {
    // 1. Try main config
    let path = config_path();
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(cfg) if is_valid_config(&cfg) => return (Some(cfg), None),
                Ok(_) => warn!("[Trigr] Main config has invalid structure — trying backup"),
                Err(e) => error!("[Trigr] Main config parse error: {}", e),
            },
            Err(e) => error!("[Trigr] Main config unreadable: {}", e),
        }
    }

    // 2. Try last-known-good
    let lkg_path = backup_dir().join("keyforge-config-last-known-good.json");
    if lkg_path.exists() {
        match fs::read_to_string(&lkg_path) {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(cfg) if is_valid_config(&cfg) => {
                    info!("[Trigr] Restored from last-known-good backup");
                    return (
                        Some(cfg),
                        Some("keyforge-config-last-known-good.json".to_string()),
                    );
                }
                _ => {}
            },
            Err(e) => error!("[Trigr] last-known-good unreadable: {}", e),
        }
    }

    // 3. Try timestamped backups, newest first
    let bdir = backup_dir();
    ensure_backup_dir();
    if let Ok(entries) = fs::read_dir(&bdir) {
        let re_pattern = regex_lite::Regex::new(
            r"^keyforge-config-\d{4}-\d{2}-\d{2}-\d{2}-\d{2}\.json$",
        )
        .unwrap();
        let mut files: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|f| re_pattern.is_match(f))
            .collect();
        files.sort();
        files.reverse();

        for f in &files {
            let fpath = bdir.join(f);
            if let Ok(raw) = fs::read_to_string(&fpath) {
                if let Ok(cfg) = serde_json::from_str::<Value>(&raw) {
                    if is_valid_config(&cfg) {
                        info!("[Trigr] Restored from backup: {}", f);
                        return (Some(cfg), Some(f.clone()));
                    }
                }
            }
        }
    }

    (None, None)
}

/// Atomic write: write to .tmp, then rename.
pub fn save_config(config: &Value) -> bool {
    let path = config_path();
    let tmp_path = path.with_extension("json.tmp");
    info!("[Trigr] Saving config to: {}", path.display());

    // Ensure parent dir exists
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    match serde_json::to_string_pretty(config) {
        Ok(json) => {
            match fs::write(&tmp_path, &json) {
                Ok(()) => match fs::rename(&tmp_path, &path) {
                    Ok(()) => {
                        info!("[Trigr] Config saved ({} bytes)", json.len());
                        true
                    }
                    Err(e) => {
                        error!("[Trigr] Failed to rename config tmp file: {}", e);
                        let _ = fs::remove_file(&tmp_path);
                        false
                    }
                },
                Err(e) => {
                    error!("[Trigr] Failed to write config tmp file: {}", e);
                    let _ = fs::remove_file(&tmp_path);
                    false
                }
            }
        }
        Err(e) => {
            error!("[Trigr] Failed to serialize config: {}", e);
            false
        }
    }
}

// ── Backup management ───────────────────────────────────────────────────────

fn ensure_backup_dir() {
    let dir = backup_dir();
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
}

fn backup_filename_regex() -> regex_lite::Regex {
    regex_lite::Regex::new(r"^keyforge-config-\d{4}-\d{2}-\d{2}-\d{2}-\d{2}\.json$").unwrap()
}

pub fn create_timestamped_backup(config: &Value) {
    if !is_valid_config(config) {
        return;
    }
    ensure_backup_dir();
    let now = chrono::Local::now();
    let stamp = now.format("%Y-%m-%d-%H-%M").to_string();
    let dest = backup_dir().join(format!("keyforge-config-{}.json", stamp));
    match serde_json::to_string_pretty(config) {
        Ok(json) => match fs::write(&dest, json) {
            Ok(()) => {
                info!("[Trigr] Backup created: keyforge-config-{}.json", stamp);
                prune_backups();
            }
            Err(e) => error!("[Trigr] Failed to create timestamped backup: {}", e),
        },
        Err(e) => error!("[Trigr] Failed to serialize backup: {}", e),
    }
}

pub fn update_last_known_good(config: &Value) {
    if !is_valid_config(config) {
        return;
    }
    ensure_backup_dir();
    let dest = backup_dir().join("keyforge-config-last-known-good.json");
    match serde_json::to_string_pretty(config) {
        Ok(json) => {
            if let Err(e) = fs::write(&dest, json) {
                error!("[Trigr] Failed to update last-known-good: {}", e);
            }
        }
        Err(e) => error!("[Trigr] Failed to serialize LKG: {}", e),
    }
}

fn prune_backups() {
    let bdir = backup_dir();
    let re = backup_filename_regex();
    let Ok(entries) = fs::read_dir(&bdir) else {
        return;
    };
    let mut files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|f| re.is_match(f))
        .collect();
    files.sort();
    if files.len() > MAX_BACKUPS {
        let excess = files.len() - MAX_BACKUPS;
        for f in &files[..excess] {
            let path = bdir.join(f);
            if let Err(e) = fs::remove_file(&path) {
                error!("[Trigr] Failed to prune backup {}: {}", f, e);
            } else {
                info!("[Trigr] Pruned old backup: {}", f);
            }
        }
    }
}

// ── Significant change detection ────────────────────────────────────────────

pub fn is_significant_change(incoming: &Value, existing: &Value) -> bool {
    // Check if profile list changed
    if let (Some(in_p), Some(ex_p)) = (
        incoming.get("profiles").and_then(|v| v.as_array()),
        existing.get("profiles").and_then(|v| v.as_array()),
    ) {
        if in_p.len() != ex_p.len() || in_p.iter().zip(ex_p.iter()).any(|(a, b)| a != b) {
            return true;
        }
    } else if incoming.get("profiles").is_some() {
        // Profiles field exists in incoming but not existing (or vice versa)
        return true;
    }

    // Check if more than 5 assignment keys differ
    if let Some(in_a) = incoming.get("assignments").and_then(|v| v.as_object()) {
        let ex_a = existing
            .get("assignments")
            .and_then(|v| v.as_object());
        let ex_keys: std::collections::HashSet<&String> = ex_a
            .map(|a| a.keys().collect())
            .unwrap_or_default();
        let in_keys: std::collections::HashSet<&String> = in_a.keys().collect();
        let mut diff = 0usize;
        for k in &in_keys {
            if !ex_keys.contains(k) {
                diff += 1;
            }
        }
        for k in &ex_keys {
            if !in_keys.contains(k) {
                diff += 1;
            }
        }
        if diff > 5 {
            return true;
        }
    }

    false
}

// ── Config summary ──────────────────────────────────────────────────────────

pub fn config_summary(cfg: &Value) -> (usize, usize, usize) {
    let profile_count = cfg
        .get("profiles")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    let keys: Vec<&String> = cfg
        .get("assignments")
        .and_then(|v| v.as_object())
        .map(|o| o.keys().collect())
        .unwrap_or_default();

    let expansion_count = keys.iter().filter(|k| k.contains("::EXPANSION::")).count();
    let assignment_count = keys
        .iter()
        .filter(|k| !k.contains("::EXPANSION::") && !k.contains("::AUTOCORRECT::"))
        .count();

    (profile_count, assignment_count, expansion_count)
}

// ── List backups ────────────────────────────────────────────────────────────

pub fn list_backups() -> Value {
    ensure_backup_dir();
    let bdir = backup_dir();
    let re = backup_filename_regex();
    let date_re =
        regex_lite::Regex::new(r"(\d{4})-(\d{2})-(\d{2})-(\d{2})-(\d{2})").unwrap();

    // Timestamped backups, newest first
    let mut timestamped = Vec::new();
    if let Ok(entries) = fs::read_dir(&bdir) {
        let mut files: Vec<String> = entries
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|f| re.is_match(f))
            .collect();
        files.sort();
        files.reverse();

        for filename in &files {
            let fpath = bdir.join(filename);
            let date = date_re
                .captures(filename)
                .map(|m| {
                    format!(
                        "{}-{}-{} {}:{}",
                        &m[1], &m[2], &m[3], &m[4], &m[5]
                    )
                })
                .unwrap_or_else(|| filename.clone());

            match fs::read_to_string(&fpath).and_then(|raw| {
                serde_json::from_str::<Value>(&raw)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
            }) {
                Ok(cfg) => {
                    let (pc, ac, ec) = config_summary(&cfg);
                    timestamped.push(serde_json::json!({
                        "filename": filename,
                        "date": date,
                        "profileCount": pc,
                        "assignmentCount": ac,
                        "expansionCount": ec,
                    }));
                }
                Err(_) => {
                    timestamped.push(serde_json::json!({
                        "filename": filename,
                        "date": date,
                        "profileCount": 0,
                        "assignmentCount": 0,
                        "expansionCount": 0,
                        "invalid": true,
                    }));
                }
            }
        }
    }

    // Last-known-good
    let lkg_path = bdir.join("keyforge-config-last-known-good.json");
    let last_known_good = if lkg_path.exists() {
        match fs::read_to_string(&lkg_path).and_then(|raw| {
            serde_json::from_str::<Value>(&raw)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
        }) {
            Ok(cfg) => {
                let (pc, ac, ec) = config_summary(&cfg);
                let date = fs::metadata(&lkg_path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| {
                        let dt: chrono::DateTime<chrono::Local> = t.into();
                        Some(dt.format("%Y-%m-%d %H:%M").to_string())
                    })
                    .unwrap_or_default();
                serde_json::json!({
                    "filename": "keyforge-config-last-known-good.json",
                    "date": date,
                    "profileCount": pc,
                    "assignmentCount": ac,
                    "expansionCount": ec,
                    "isLkg": true,
                })
            }
            Err(_) => Value::Null,
        }
    } else {
        Value::Null
    };

    serde_json::json!({
        "backups": timestamped,
        "lastKnownGood": last_known_good,
    })
}

// ── Restore backup ──────────────────────────────────────────────────────────

pub fn restore_backup(filename: &str) -> Value {
    let src = backup_dir().join(filename);
    if !src.exists() {
        return serde_json::json!({ "ok": false, "error": "Backup file not found" });
    }
    match fs::read_to_string(&src) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(cfg) => {
                if !is_valid_config(&cfg) {
                    return serde_json::json!({ "ok": false, "error": "Backup file is not a valid config" });
                }
                if save_config(&cfg) {
                    info!("[Trigr] Restored from backup: {}", filename);
                    serde_json::json!({ "ok": true, "config": cfg })
                } else {
                    serde_json::json!({ "ok": false, "error": "Failed to write restored config" })
                }
            }
            Err(e) => serde_json::json!({ "ok": false, "error": format!("Failed to parse backup: {}", e) }),
        },
        Err(e) => serde_json::json!({ "ok": false, "error": format!("Failed to read backup: {}", e) }),
    }
}
