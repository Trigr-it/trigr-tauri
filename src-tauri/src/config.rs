use log::{error, info, warn};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};

const MAX_BACKUPS: usize = 10;
const LOCAL_SETTINGS_FILE: &str = "trigr-local-settings.json";

// ── Path resolution ─────────────────────────────────────────────────────────

static APP_DATA_DIR: OnceLock<PathBuf> = OnceLock::new();
static SHARED_CONFIG_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Call once at startup with the resolved app data dir.
pub fn init(app_data_dir: PathBuf) {
    let _ = APP_DATA_DIR.set(app_data_dir);
    load_local_settings();
}

fn app_data_dir() -> &'static Path {
    APP_DATA_DIR
        .get()
        .expect("config::init() must be called before using config functions")
}

pub fn config_path() -> PathBuf {
    // Check for shared config dir override first
    if let Ok(guard) = SHARED_CONFIG_DIR.read() {
        if let Some(ref shared_dir) = *guard {
            let shared_path = shared_dir.join("keyforge-config.json");
            // Only use the shared path if the directory actually exists
            if shared_dir.exists() {
                return shared_path;
            }
            warn!(
                "[Trigr] Shared config dir not found: {} — falling back to local",
                shared_dir.display()
            );
        }
    }
    app_data_dir().join("keyforge-config.json")
}

fn backup_dir() -> PathBuf {
    // Backups ALWAYS stay in local AppData — never follow shared path
    app_data_dir().join("backups")
}

// ── Local settings (machine-specific, never synced) ─────────────────────────

fn local_settings_path() -> PathBuf {
    app_data_dir().join(LOCAL_SETTINGS_FILE)
}

fn load_local_settings() {
    let path = local_settings_path();
    if !path.exists() {
        return;
    }
    match fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(val) => {
                if let Some(shared) = val.get("shared_config_path").and_then(|v| v.as_str()) {
                    if !shared.is_empty() {
                        let shared_path = PathBuf::from(shared);
                        info!("[Trigr] Shared config path from local settings: {}", shared_path.display());
                        set_shared_config_dir(Some(shared_path));
                    }
                }
            }
            Err(e) => warn!("[Trigr] Failed to parse local settings: {}", e),
        },
        Err(e) => warn!("[Trigr] Failed to read local settings: {}", e),
    }
}

pub fn save_local_settings(shared_path: Option<&Path>) -> bool {
    let path = local_settings_path();
    let val = match shared_path {
        Some(p) => serde_json::json!({ "shared_config_path": p.to_string_lossy() }),
        None => serde_json::json!({}),
    };
    match serde_json::to_string_pretty(&val) {
        Ok(json) => match fs::write(&path, json) {
            Ok(()) => {
                info!("[Trigr] Local settings saved");
                true
            }
            Err(e) => {
                error!("[Trigr] Failed to write local settings: {}", e);
                false
            }
        },
        Err(e) => {
            error!("[Trigr] Failed to serialize local settings: {}", e);
            false
        }
    }
}

pub fn set_shared_config_dir(path: Option<PathBuf>) {
    if let Ok(mut guard) = SHARED_CONFIG_DIR.write() {
        match &path {
            Some(p) => info!("[Trigr] Shared config dir set to: {}", p.display()),
            None => info!("[Trigr] Shared config dir cleared — using local AppData"),
        }
        *guard = path;
    }
}

pub fn get_shared_config_dir() -> Option<PathBuf> {
    SHARED_CONFIG_DIR.read().ok().and_then(|g| g.clone())
}

// ── File watcher ────────────────────────────────────────────────────────────

/// Set to true before Trigr writes config, cleared after. Prevents self-reload.
pub static SELF_WRITE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Hash of the last content we wrote, to detect our own writes after the flag clears.
static LAST_WRITTEN_HASH: Mutex<Option<u64>> = Mutex::new(None);

/// Handle to the active watcher — dropping it stops the watcher.
static WATCHER_HANDLE: Mutex<Option<notify::RecommendedWatcher>> = Mutex::new(None);

/// Signal to stop the watcher's debounce thread.
static WATCHER_STOP: AtomicBool = AtomicBool::new(false);

fn simple_hash(data: &[u8]) -> u64 {
    // FNV-1a 64-bit hash — fast, no crate needed
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

pub fn mark_self_write(content: &str) {
    SELF_WRITE_IN_PROGRESS.store(true, Ordering::SeqCst);
    if let Ok(mut guard) = LAST_WRITTEN_HASH.lock() {
        *guard = Some(simple_hash(content.as_bytes()));
    }
}

pub fn clear_self_write() {
    SELF_WRITE_IN_PROGRESS.store(false, Ordering::SeqCst);
}

pub fn start_config_watcher(dir: PathBuf, app: tauri::AppHandle) {
    use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    // Stop any existing watcher first
    stop_config_watcher();

    WATCHER_STOP.store(false, Ordering::SeqCst);

    let watched_dir = dir.clone();
    let target_filename = "keyforge-config.json";

    // Channel for notify events
    let (tx, rx) = std::sync::mpsc::channel::<Event>();

    let watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        },
        Config::default(),
    );

    let mut watcher = match watcher {
        Ok(w) => w,
        Err(e) => {
            error!("[Trigr] Failed to create file watcher: {}", e);
            return;
        }
    };

    if let Err(e) = watcher.watch(&watched_dir, RecursiveMode::NonRecursive) {
        error!("[Trigr] Failed to watch directory {}: {}", watched_dir.display(), e);
        return;
    }

    info!("[Trigr] Config watcher started on: {}", watched_dir.display());

    // Store watcher handle so it stays alive
    if let Ok(mut guard) = WATCHER_HANDLE.lock() {
        *guard = Some(watcher);
    }

    // Debounce thread — processes events with 2-second quiet window
    let app_handle = app.clone();
    std::thread::Builder::new()
        .name("config-watcher".into())
        .spawn(move || {
            use std::time::{Duration, Instant};
            use tauri::Emitter;

            let debounce_duration = Duration::from_secs(2);
            let mut last_event_time: Option<Instant> = None;
            let mut pending = false;

            loop {
                if WATCHER_STOP.load(Ordering::SeqCst) {
                    info!("[Trigr] Config watcher thread stopping");
                    break;
                }

                // Non-blocking receive with 500ms timeout
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(event) => {
                        // Filter: only care about modifications/creates to our config file
                        let dominated = matches!(
                            event.kind,
                            EventKind::Modify(_) | EventKind::Create(_)
                        );
                        if !dominated {
                            continue;
                        }

                        // Check if any path in the event matches our target file
                        let is_target = event.paths.iter().any(|p| {
                            p.file_name()
                                .map(|f| f == target_filename)
                                .unwrap_or(false)
                        });

                        // Skip temp files from sync clients
                        let is_temp = event.paths.iter().any(|p| {
                            if let Some(name) = p.file_name().and_then(|f| f.to_str()) {
                                name.starts_with("~$")
                                    || name.starts_with(".~")
                                    || name.ends_with(".tmp")
                                    || name.ends_with(".gstmp")
                            } else {
                                false
                            }
                        });

                        if is_target && !is_temp {
                            last_event_time = Some(Instant::now());
                            pending = true;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        info!("[Trigr] Config watcher channel disconnected");
                        break;
                    }
                }

                // Check if debounce window has passed
                if pending {
                    if let Some(last) = last_event_time {
                        if last.elapsed() >= debounce_duration {
                            pending = false;
                            last_event_time = None;

                            // Self-write suppression
                            if SELF_WRITE_IN_PROGRESS.load(Ordering::SeqCst) {
                                info!("[Trigr] Config watcher: ignoring self-write in progress");
                                continue;
                            }

                            // Try to read the file (with retry for sync client locks)
                            let file_content = read_with_retry(
                                &config_path(),
                                3,
                                Duration::from_millis(500),
                            );

                            let content = match file_content {
                                Some(c) => c,
                                None => {
                                    warn!("[Trigr] Config watcher: could not read file after retries");
                                    continue;
                                }
                            };

                            // Check content hash against our last write
                            let content_hash = simple_hash(content.as_bytes());
                            if let Ok(guard) = LAST_WRITTEN_HASH.lock() {
                                if let Some(last_hash) = *guard {
                                    if content_hash == last_hash {
                                        info!("[Trigr] Config watcher: content matches last write — skipping");
                                        continue;
                                    }
                                }
                            }

                            // Validate the config
                            match serde_json::from_str::<Value>(&content) {
                                Ok(cfg) if is_valid_config(&cfg) => {
                                    info!("[Trigr] Config watcher: valid config change detected — emitting reload event");
                                    if let Err(e) = app_handle.emit("config-reloaded-from-sync", &cfg) {
                                        error!("[Trigr] Failed to emit config reload event: {}", e);
                                    }
                                }
                                Ok(_) => {
                                    warn!("[Trigr] Config watcher: changed file has invalid structure — ignoring");
                                }
                                Err(e) => {
                                    warn!("[Trigr] Config watcher: changed file is not valid JSON: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        })
        .ok();
}

pub fn stop_config_watcher() {
    WATCHER_STOP.store(true, Ordering::SeqCst);
    if let Ok(mut guard) = WATCHER_HANDLE.lock() {
        if guard.is_some() {
            *guard = None;
            info!("[Trigr] Config watcher stopped");
        }
    }
}

fn read_with_retry(path: &Path, retries: u32, delay: std::time::Duration) -> Option<String> {
    for attempt in 0..retries {
        match fs::read_to_string(path) {
            Ok(content) => return Some(content),
            Err(e) => {
                if attempt < retries - 1 {
                    warn!(
                        "[Trigr] Config read attempt {} failed ({}), retrying in {}ms",
                        attempt + 1,
                        e,
                        delay.as_millis()
                    );
                    std::thread::sleep(delay);
                } else {
                    error!("[Trigr] Config read failed after {} attempts: {}", retries, e);
                }
            }
        }
    }
    None
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
/// Sets SELF_WRITE_IN_PROGRESS to suppress file watcher during our own writes.
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
            // Mark self-write before touching the file
            mark_self_write(&json);
            let result = match fs::write(&tmp_path, &json) {
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
            };
            clear_self_write();
            result
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
