use log::{error, info, warn};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
                "[Keyfire] Shared config dir not found: {} — falling back to local",
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

/// Serializes all reads/writes to trigr-local-settings.json across modules.
static LOCAL_SETTINGS_LOCK: Mutex<()> = Mutex::new(());

fn local_settings_path() -> PathBuf {
    app_data_dir().join(LOCAL_SETTINGS_FILE)
}

/// Raw local-settings read. `Ok(None)` = file missing (fresh install),
/// `Ok(Some(v))` = parsed, `Err` = file exists but could not be read/parsed.
/// The distinction matters: this file holds licence state, the shared-config
/// path and the telemetry opt-out, and every writer does load → mutate →
/// save. A transient read error that surfaced as `{}` was then persisted by
/// the next writer (24h licence revalidation, 6h telemetry tick) as a fresh
/// file: user silently back to Free, shared path dropped, opt-out reset.
fn read_local_settings_raw() -> Result<Option<Value>, String> {
    let path = local_settings_path();
    if !path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("read: {}", e))?;
    if raw.trim().is_empty() {
        return Err("file is empty".to_string());
    }
    serde_json::from_str::<Value>(&raw)
        .map(Some)
        .map_err(|e| format!("parse: {}", e))
}

/// Read the full local settings JSON for READERS. Returns an empty object if
/// the file is missing or unreadable (readers degrade to defaults). Writers
/// must use `load_local_settings_json_strict` so they never persist `{}` over
/// a file that merely failed to read.
pub fn load_local_settings_json() -> Value {
    let _guard = LOCAL_SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match read_local_settings_raw() {
        Ok(Some(v)) => v,
        Ok(None) => serde_json::json!({}),
        Err(e) => {
            warn!("[Keyfire] Failed to load local settings ({}); using defaults for this read", e);
            serde_json::json!({})
        }
    }
}

/// Writer-side load: `Some({})` for a missing file, `Some(v)` when parsed,
/// `None` when the file exists but is unreadable — the caller must abort the
/// write. A `.corrupt` copy is kept once for forensics.
pub fn load_local_settings_json_strict() -> Option<Value> {
    let _guard = LOCAL_SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match read_local_settings_raw() {
        Ok(Some(v)) => Some(v),
        Ok(None) => Some(serde_json::json!({})),
        Err(e) => {
            let path = local_settings_path();
            let corrupt = path.with_extension("json.corrupt");
            if !corrupt.exists() {
                let _ = fs::copy(&path, &corrupt);
            }
            error!(
                "[Keyfire] Local settings unreadable ({}); refusing to overwrite. Copy kept at {}",
                e,
                corrupt.display()
            );
            None
        }
    }
}

/// Write the full local settings JSON to disk atomically (tmp + rename), same
/// as the main config, so a crash mid-write can't leave a truncated file.
pub fn save_local_settings_json(val: &Value) -> bool {
    let _guard = LOCAL_SETTINGS_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = local_settings_path();
    let tmp = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match serde_json::to_string_pretty(val) {
        Ok(json) => match fs::write(&tmp, json).and_then(|_| fs::rename(&tmp, &path)) {
            Ok(()) => {
                info!("[Keyfire] Local settings saved");
                true
            }
            Err(e) => {
                error!("[Keyfire] Failed to write local settings: {}", e);
                let _ = fs::remove_file(&tmp);
                false
            }
        },
        Err(e) => {
            error!("[Keyfire] Failed to serialize local settings: {}", e);
            false
        }
    }
}

fn load_local_settings() {
    let val = load_local_settings_json();
    if let Some(shared) = val.get("shared_config_path").and_then(|v| v.as_str()) {
        if !shared.is_empty() {
            let shared_path = PathBuf::from(shared);
            info!("[Keyfire] Shared config path from local settings: {}", shared_path.display());
            set_shared_config_dir(Some(shared_path));
        }
    }
}

/// Save shared config path to local settings (merge-based — preserves other keys like licence).
pub fn save_local_settings(shared_path: Option<&Path>) -> bool {
    let Some(mut val) = load_local_settings_json_strict() else { return false; };
    let obj = val.as_object_mut().unwrap();
    match shared_path {
        Some(p) => {
            obj.insert("shared_config_path".to_string(), Value::String(p.to_string_lossy().to_string()));
        }
        None => {
            obj.remove("shared_config_path");
        }
    }
    save_local_settings_json(&val)
}

pub fn set_shared_config_dir(path: Option<PathBuf>) {
    if let Ok(mut guard) = SHARED_CONFIG_DIR.write() {
        match &path {
            Some(p) => info!("[Keyfire] Shared config dir set to: {}", p.display()),
            None => info!("[Keyfire] Shared config dir cleared — using local AppData"),
        }
        *guard = path;
    }
}

pub fn get_shared_config_dir() -> Option<PathBuf> {
    SHARED_CONFIG_DIR.read().ok().and_then(|g| g.clone())
}

// ── Pro grace period for shared config ──────────────────────────────────────
//
// When the user loses Pro status while shared config is active, we don't snap
// the rug out from under them. Instead we record `pro_expired_at` and run a
// 7-day grace period during which everything works normally. At expiry we
// copy the current shared file to local AppData, clear the override, and
// stop the watcher. Pro re-upgrade at any point clears the timestamp and
// cancels the migration.

const GRACE_PERIOD_DAYS: i64 = 7;

/// Read the grace-period start timestamp from local settings. None means no
/// grace period is currently active.
pub fn get_pro_expired_at() -> Option<chrono::DateTime<chrono::Utc>> {
    let val = load_local_settings_json();
    val.get("pro_expired_at")
        .and_then(|v| v.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.to_utc())
}

/// Write or clear the grace-period start timestamp. Merge-safe.
pub fn set_pro_expired_at(ts: Option<chrono::DateTime<chrono::Utc>>) -> bool {
    let Some(mut val) = load_local_settings_json_strict() else { return false; };
    let obj = val.as_object_mut().unwrap();
    match ts {
        Some(t) => {
            obj.insert("pro_expired_at".to_string(), Value::String(t.to_rfc3339()));
        }
        None => {
            obj.remove("pro_expired_at");
        }
    }
    save_local_settings_json(&val)
}

/// True if the user has hit the deferred-migration state (shared file was
/// unreachable when we last tried to migrate). Surfaces a different banner.
pub fn get_migration_deferred() -> bool {
    load_local_settings_json()
        .get("migration_deferred")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn set_migration_deferred(deferred: bool) {
    let Some(mut val) = load_local_settings_json_strict() else { return; };
    let obj = val.as_object_mut().unwrap();
    if deferred {
        obj.insert("migration_deferred".to_string(), Value::Bool(true));
    } else {
        obj.remove("migration_deferred");
    }
    let _ = save_local_settings_json(&val);
}

// ── Telemetry opt-out (machine-local, NOT shared config) ───────────────────
// Default is opt-IN (telemetry ON). The flag is only present in the JSON when
// the user has explicitly opted out, so a fresh install reads `false` and
// telemetry runs. Stored machine-local because the user may want different
// settings on different machines (e.g. work laptop off, home desktop on).

/// True when the user has explicitly disabled anonymous-usage telemetry.
/// Default false on first launch.
pub fn get_telemetry_opt_out() -> bool {
    load_local_settings_json()
        .get("telemetry_opt_out")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// Persist the opt-out flag. Writing `false` removes the key entirely so the
/// file stays lean for users who never touched the toggle.
pub fn set_telemetry_opt_out(opted_out: bool) -> bool {
    let Some(mut val) = load_local_settings_json_strict() else { return false; };
    let obj = val.as_object_mut().unwrap();
    if opted_out {
        obj.insert("telemetry_opt_out".to_string(), Value::Bool(true));
    } else {
        obj.remove("telemetry_opt_out");
    }
    save_local_settings_json(&val)
}

/// The telemetry epoch: the local date (YYYY-MM-DD) the telemetry-enabled
/// build first ran on this machine. Nothing dated before it is ever
/// aggregated or sent — usage recorded by older builds predates the telemetry
/// disclosure in onboarding and must stay local. Empty string until stamped.
pub fn get_telemetry_epoch() -> String {
    load_local_settings_json()
        .get("telemetry_epoch")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Stamp the epoch. Called once, on the first telemetry tick of this machine.
pub fn set_telemetry_epoch(date: &str) -> bool {
    let Some(mut val) = load_local_settings_json_strict() else { return false; };
    let obj = val.as_object_mut().unwrap();
    obj.insert("telemetry_epoch".to_string(), Value::String(date.to_string()));
    save_local_settings_json(&val)
}

/// Days remaining in the grace period, or None if no grace period is active.
/// Returns 0 if grace period has expired and migration is pending.
pub fn grace_period_days_remaining() -> Option<i64> {
    let expired_at = get_pro_expired_at()?;
    let elapsed = chrono::Utc::now().signed_duration_since(expired_at);
    let remaining = GRACE_PERIOD_DAYS - elapsed.num_days();
    Some(remaining.max(0))
}

/// Copy the current shared config file to local AppData, then clear the
/// shared override and stop the file watcher. Atomic on the local side via
/// temp-file + rename. Returns Err if the shared file is unreachable or
/// invalid — caller leaves `pro_expired_at` set so we retry next time.
pub fn migrate_shared_to_local() -> Result<(), String> {
    let shared_dir = get_shared_config_dir()
        .ok_or_else(|| "No shared config override set".to_string())?;
    let shared_file = shared_dir.join("keyforge-config.json");
    let local_file = app_data_dir().join("keyforge-config.json");

    if !shared_file.exists() {
        return Err(format!(
            "Shared config file not found: {}",
            shared_file.display()
        ));
    }

    let content = fs::read_to_string(&shared_file)
        .map_err(|e| format!("Cannot read shared config: {}", e))?;

    // Sanity-check it parses as JSON before we overwrite local. We don't want
    // to copy a half-synced or truncated file from OneDrive over the user's
    // local backup.
    serde_json::from_str::<Value>(&content)
        .map_err(|e| format!("Shared config is not valid JSON: {}", e))?;

    // Atomic write: temp file + rename. Avoids partial-write corruption if
    // Keyfire is killed mid-migration.
    let tmp_path = local_file.with_extension("tmp");
    fs::write(&tmp_path, &content)
        .map_err(|e| format!("Cannot write local config temp: {}", e))?;
    fs::rename(&tmp_path, &local_file)
        .map_err(|e| format!("Cannot finalize local config: {}", e))?;

    // Clear the shared override AFTER the local copy succeeded, so a failure
    // mid-migration doesn't leave the user with no config source.
    set_shared_config_dir(None);
    save_local_settings(None);
    stop_config_watcher();

    info!(
        "[Keyfire] Migration complete: {} -> {}",
        shared_file.display(),
        local_file.display()
    );
    Ok(())
}

/// Driven by every licence revalidation. Three transitions:
///   - Pro user with grace timestamp → clear it (cancelling pending migration).
///   - Non-Pro user with shared config, no timestamp → start the 7-day clock.
///   - Non-Pro user with shared config, timestamp older than 7 days → migrate.
pub fn check_and_migrate_if_due() {
    let is_pro = crate::licence::is_pro();
    let has_shared = get_shared_config_dir().is_some();
    let expired_at = get_pro_expired_at();

    if is_pro {
        if expired_at.is_some() {
            info!("[Keyfire] Pro restored during grace period — cancelling shared migration");
            let _ = set_pro_expired_at(None);
            set_migration_deferred(false);
        }
        return;
    }

    if !has_shared {
        // Nothing to migrate. Clear any stale grace state.
        if expired_at.is_some() {
            let _ = set_pro_expired_at(None);
            set_migration_deferred(false);
        }
        return;
    }

    match expired_at {
        None => {
            let now = chrono::Utc::now();
            let _ = set_pro_expired_at(Some(now));
            let due = now + chrono::Duration::days(GRACE_PERIOD_DAYS);
            info!(
                "[Keyfire] Pro grace period started for shared config (migrates at {})",
                due.to_rfc3339()
            );
        }
        Some(t) => {
            let elapsed = chrono::Utc::now().signed_duration_since(t);
            if elapsed.num_days() >= GRACE_PERIOD_DAYS {
                info!("[Keyfire] Pro grace expired, migrating shared config to local");
                match migrate_shared_to_local() {
                    Ok(()) => {
                        let _ = set_pro_expired_at(None);
                        set_migration_deferred(false);
                    }
                    Err(e) => {
                        warn!(
                            "[Keyfire] Migration deferred — shared file unreachable: {}. Will retry.",
                            e
                        );
                        set_migration_deferred(true);
                    }
                }
            }
        }
    }
}

// ── File watcher ────────────────────────────────────────────────────────────

/// Set to true before Keyfire writes config, cleared after. Prevents self-reload.
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
            error!("[Keyfire] Failed to create file watcher: {}", e);
            return;
        }
    };

    if let Err(e) = watcher.watch(&watched_dir, RecursiveMode::NonRecursive) {
        error!("[Keyfire] Failed to watch directory {}: {}", watched_dir.display(), e);
        return;
    }

    info!("[Keyfire] Config watcher started on: {}", watched_dir.display());

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
                    info!("[Keyfire] Config watcher thread stopping");
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

                        // Clipboard encryption artefacts (v0.5) must NEVER feed
                        // config-sync activity. The is_target filename check below
                        // already excludes them, but this guard makes the invariant
                        // explicit so a future broadening of the filter (e.g. glob
                        // matching, multi-file sync) can't silently pick them up.
                        let is_crypto_artefact = event.paths.iter().any(|p| {
                            if let Some(name) = p.file_name().and_then(|f| f.to_str()) {
                                name.ends_with(".dpapi")
                                    || name.ends_with(".plaintext-backup")
                                    || name.ends_with(".plaintext-backup-expires")
                            } else {
                                false
                            }
                        });
                        if is_crypto_artefact {
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
                        info!("[Keyfire] Config watcher channel disconnected");
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
                                info!("[Keyfire] Config watcher: ignoring self-write in progress");
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
                                    warn!("[Keyfire] Config watcher: could not read file after retries");
                                    continue;
                                }
                            };

                            // Check content hash against our last write
                            let content_hash = simple_hash(content.as_bytes());
                            if let Ok(guard) = LAST_WRITTEN_HASH.lock() {
                                if let Some(last_hash) = *guard {
                                    if content_hash == last_hash {
                                        info!("[Keyfire] Config watcher: content matches last write — skipping");
                                        continue;
                                    }
                                }
                            }

                            // Validate the config
                            match serde_json::from_str::<Value>(&content) {
                                Ok(cfg) if is_valid_config(&cfg) => {
                                    // Protect against a cross-device clobber that
                                    // empties the radial layout before we apply it.
                                    guard_destructive_sync(&cfg);
                                    // Phase 2: the reloaded config is now the
                                    // frontend's base for any subsequent save.
                                    snapshot_loaded(&cfg);
                                    info!("[Keyfire] Config watcher: valid config change detected — emitting reload event");
                                    if let Err(e) = app_handle.emit("config-reloaded-from-sync", &cfg) {
                                        error!("[Keyfire] Failed to emit config reload event: {}", e);
                                    }
                                }
                                Ok(_) => {
                                    warn!("[Keyfire] Config watcher: changed file has invalid structure — ignoring");
                                }
                                Err(e) => {
                                    warn!("[Keyfire] Config watcher: changed file is not valid JSON: {}", e);
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
            info!("[Keyfire] Config watcher stopped");
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
                        "[Keyfire] Config read attempt {} failed ({}), retrying in {}ms",
                        attempt + 1,
                        e,
                        delay.as_millis()
                    );
                    std::thread::sleep(delay);
                } else {
                    error!("[Keyfire] Config read failed after {} attempts: {}", retries, e);
                }
            }
        }
    }
    None
}

// ── Validation ──────────────────────────────────────────────────────────────

pub fn is_valid_config(cfg: &Value) -> bool {
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

// ── Starter pack (first-install seed) ──────────────────────────────────────
// Content is authored in `resources/starter-pack.json` (compile-time embedded
// via `include_str!`, so no bundler config changes and no runtime file lookup).
// Called from `lib.rs::load_config`'s all-fallbacks-miss branch — a brand-new
// install lands here with nothing on disk. If the JSON fails to parse (should
// only ever happen if someone corrupts the source file), log + return an empty
// factory config so the app still starts.
//
// The JSON already contains `assignments` + `radialMenuItemsByProfile` and
// carries `starterPackVersion` for future migration passes. We layer in the
// baseline `profiles` + `activeProfile` fields the frontend expects and strip
// the doc-comment `_` key so it never round-trips to disk.
pub fn build_starter_config() -> Value {
    const STARTER_PACK_JSON: &str = include_str!("../resources/starter-pack.json");
    let mut cfg: Value = match serde_json::from_str(STARTER_PACK_JSON) {
        Ok(v) => v,
        Err(e) => {
            error!("[Keyfire] starter-pack.json parse error ({}); using bare factory defaults", e);
            serde_json::json!({})
        }
    };
    if let Some(obj) = cfg.as_object_mut() {
        obj.entry("profiles".to_string())
            .or_insert_with(|| serde_json::json!(["Default"]));
        obj.entry("activeProfile".to_string())
            .or_insert_with(|| serde_json::json!("Default"));
        obj.entry("assignments".to_string())
            .or_insert_with(|| serde_json::json!({}));
        obj.remove("_");
    }
    cfg
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
                error!("[Keyfire] Failed to parse config: {}", e);
                None
            }
        },
        Err(e) => {
            error!("[Keyfire] Failed to read config: {}", e);
            None
        }
    }
}

/// Save-path read. When the file EXISTS but can't be read or parsed right
/// now (AV / sync client holding it, mid-rename), retry briefly and then
/// return `Err` so the caller aborts the save instead of merging the payload
/// onto `{}` — which rewrote the config as only the payload and, if that
/// passed validation, poisoned last-known-good with it.
pub fn load_config_for_save() -> Result<Value, String> {
    let path = config_path();
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    for attempt in 0..4 {
        if let Some(v) = load_config() {
            return Ok(v);
        }
        if attempt < 3 {
            std::thread::sleep(std::time::Duration::from_millis(60));
        }
    }
    Err(format!(
        "config file exists but could not be read after retries: {}",
        path.display()
    ))
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
                Ok(_) => warn!("[Keyfire] Main config has invalid structure — trying backup"),
                Err(e) => error!("[Keyfire] Main config parse error: {}", e),
            },
            Err(e) => error!("[Keyfire] Main config unreadable: {}", e),
        }
    }

    // 2. Try last-known-good
    let lkg_path = backup_dir().join("keyforge-config-last-known-good.json");
    if lkg_path.exists() {
        match fs::read_to_string(&lkg_path) {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(cfg) if is_valid_config(&cfg) => {
                    info!("[Keyfire] Restored from last-known-good backup");
                    return (
                        Some(cfg),
                        Some("keyforge-config-last-known-good.json".to_string()),
                    );
                }
                _ => {}
            },
            Err(e) => error!("[Keyfire] last-known-good unreadable: {}", e),
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
                        info!("[Keyfire] Restored from backup: {}", f);
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
    info!("[Keyfire] Saving config to: {}", path.display());

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
                        info!("[Keyfire] Config saved ({} bytes)", json.len());
                        true
                    }
                    Err(e) => {
                        error!("[Keyfire] Failed to rename config tmp file: {}", e);
                        let _ = fs::remove_file(&tmp_path);
                        false
                    }
                },
                Err(e) => {
                    error!("[Keyfire] Failed to write config tmp file: {}", e);
                    let _ = fs::remove_file(&tmp_path);
                    false
                }
            };
            clear_self_write();
            result
        }
        Err(e) => {
            error!("[Keyfire] Failed to serialize config: {}", e);
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

/// Edit-time backups: `keyforge-config-YYYY-MM-DD-HH-MM-SS.json` (older
/// files without the seconds field still match and are pruned in order).
fn backup_filename_regex() -> regex_lite::Regex {
    regex_lite::Regex::new(r"^keyforge-config-\d{4}-\d{2}-\d{2}-\d{2}-\d{2}(-\d{2})?\.json$").unwrap()
}

/// Boot-time snapshots live in their own small ring so ten restarts can't
/// evict every edit-time restore point (the old scheme wrote a plain
/// timestamped backup on every healthy load into the same 10-slot ring).
fn boot_backup_filename_regex() -> regex_lite::Regex {
    regex_lite::Regex::new(r"^keyforge-config-boot-\d{4}-\d{2}-\d{2}-\d{2}-\d{2}-\d{2}\.json$").unwrap()
}
const MAX_BOOT_BACKUPS: usize = 2;

fn write_backup_file(config: &Value, name: &str) -> bool {
    ensure_backup_dir();
    let dest = backup_dir().join(name);
    match serde_json::to_string_pretty(config) {
        Ok(json) => match fs::write(&dest, json) {
            Ok(()) => {
                info!("[Keyfire] Backup created: {}", name);
                true
            }
            Err(e) => {
                error!("[Keyfire] Failed to create backup {}: {}", name, e);
                false
            }
        },
        Err(e) => {
            error!("[Keyfire] Failed to serialize backup: {}", e);
            false
        }
    }
}

pub fn create_timestamped_backup(config: &Value) {
    if !is_valid_config(config) {
        return;
    }
    // Seconds in the stamp: two significant changes inside one minute used to
    // share a filename and the second silently overwrote the first.
    let stamp = chrono::Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
    if write_backup_file(config, &format!("keyforge-config-{}.json", stamp)) {
        prune_backups();
    }
}

/// Healthy-load snapshot. Separate ring from edit-time backups (see above).
pub fn create_boot_backup(config: &Value) {
    if !is_valid_config(config) {
        return;
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d-%H-%M-%S").to_string();
    if write_backup_file(config, &format!("keyforge-config-boot-{}.json", stamp)) {
        prune_named(&boot_backup_filename_regex(), MAX_BOOT_BACKUPS);
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
                error!("[Keyfire] Failed to update last-known-good: {}", e);
            }
        }
        Err(e) => error!("[Keyfire] Failed to serialize LKG: {}", e),
    }
}

fn prune_backups() {
    prune_named(&backup_filename_regex(), MAX_BACKUPS);
}

fn prune_named(re: &regex_lite::Regex, keep: usize) {
    let bdir = backup_dir();
    let Ok(entries) = fs::read_dir(&bdir) else {
        return;
    };
    let mut files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|f| re.is_match(f))
        .collect();
    files.sort();
    if files.len() > keep {
        let excess = files.len() - keep;
        for f in &files[..excess] {
            let path = bdir.join(f);
            if let Err(e) = fs::remove_file(&path) {
                error!("[Keyfire] Failed to prune backup {}: {}", f, e);
            } else {
                info!("[Keyfire] Pruned old backup: {}", f);
            }
        }
    }
}

// ── Significant change detection ────────────────────────────────────────────

/// Count non-null radial layout segments across all profiles in a config.
/// Empty/null slots (placeholder segments) are not counted.
fn radial_item_count(cfg: &Value) -> usize {
    let mut n = 0;
    if let Some(by_prof) = cfg
        .get("radialMenuItemsByProfile")
        .and_then(|v| v.as_object())
    {
        for arr in by_prof.values() {
            if let Some(a) = arr.as_array() {
                n += a.iter().filter(|x| !x.is_null()).count();
            }
        }
    }
    // Legacy flat array fallback (pre per-profile migration).
    if n == 0 {
        if let Some(a) = cfg.get("radialMenuItems").and_then(|v| v.as_array()) {
            n += a.iter().filter(|x| !x.is_null()).count();
        }
    }
    n
}

/// True if `merged` zeroes-out a radial layout or assignment set that was
/// populated in `existing`. This is the signature of the cross-device shared-
/// config clobber that silently wiped radial menus: the layout (or all
/// assignments) goes from populated to empty in a single write. Callers use
/// this to force a protective backup and to refuse poisoning last-known-good.
/// It deliberately does NOT block the write — a genuine "remove everything"
/// is still allowed, just made recoverable.
pub fn is_destructive_regression(merged: &Value, existing: &Value) -> bool {
    if radial_item_count(existing) > 0 && radial_item_count(merged) == 0 {
        return true;
    }
    let ex_assign = existing
        .get("assignments")
        .and_then(|v| v.as_object())
        .map(|o| o.len())
        .unwrap_or(0);
    let mg_assign = merged
        .get("assignments")
        .and_then(|v| v.as_object())
        .map(|o| o.len())
        .unwrap_or(0);
    ex_assign > 0 && mg_assign == 0
}

/// Watcher guard: a synced change that drops a previously-populated radial
/// layout to zero is almost always a cross-device clobber rather than an
/// intentional clear. Snapshot the current last-known-good to a timestamped
/// backup so the good radial stays recoverable on every machine that sees the
/// regression. Additive and best-effort — never blocks the reload.
fn guard_destructive_sync(incoming: &Value) {
    let lkg_path = backup_dir().join("keyforge-config-last-known-good.json");
    let lkg = fs::read_to_string(&lkg_path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok());
    if let Some(lkg) = lkg {
        let good = radial_item_count(&lkg);
        if good > 0 && radial_item_count(incoming) == 0 {
            warn!(
                "[Keyfire] Config watcher: synced change drops radial layout from {} segments to 0 — snapshotting last-known-good before applying (possible cross-device clobber)",
                good
            );
            create_timestamped_backup(&lkg);
        }
    }
}

// ── Phase 2: cross-device revision tracking ─────────────────────────────────
//
// Defends against the shared-config last-writer-wins clobber: when two machines
// edit the cloud-synced config and one writes a stale base over the other's
// edits. We stamp every save with a monotonic `configRevision`; at save time
// we re-read disk and, if disk's revision is ahead of what the frontend last
// loaded from, run a 3-way merge (base / incoming / existing) so each side's
// genuinely-edited top-level keys both survive. Phase 1's destructive-regression
// guard stays in place as the backstop for keys both sides edited.

static LAST_LOADED_REVISION: AtomicU64 = AtomicU64::new(0);
static LAST_LOADED_BASE: Mutex<Option<Value>> = Mutex::new(None);

/// Top-level fields that are not persistent user state and must be excluded
/// when diffing the frontend's view against on-disk state during a merge.
/// `configRevision` and `lastModifiedUtc` are written by Fire itself on every
/// save; `_restoredFrom` is injected at load time and never belongs in the
/// merge calculus.
pub const INTERNAL_CONFIG_KEYS: &[&str] = &["configRevision", "lastModifiedUtc", "_restoredFrom"];

/// Read the monotonic revision from a config, defaulting to 0 for configs
/// produced by pre-Phase 2 builds (the field will be absent).
pub fn config_revision(cfg: &Value) -> u64 {
    cfg.get("configRevision").and_then(|v| v.as_u64()).unwrap_or(0)
}

/// Update the in-process snapshot of what the frontend's view started from.
/// Called whenever a fresh authoritative state lands: initial load, our own
/// successful write, or a watcher-driven sync reload (before the reload event
/// fires so the next save merges against the post-reload base).
pub fn snapshot_loaded(cfg: &Value) {
    LAST_LOADED_REVISION.store(config_revision(cfg), Ordering::SeqCst);
    if let Ok(mut guard) = LAST_LOADED_BASE.lock() {
        *guard = Some(cfg.clone());
    }
}

pub fn last_loaded_revision() -> u64 {
    LAST_LOADED_REVISION.load(Ordering::SeqCst)
}

pub fn last_loaded_base() -> Option<Value> {
    LAST_LOADED_BASE.lock().ok().and_then(|g| g.clone())
}

/// Outcome of a save-time 3-way merge. `remote_preserved` lists the top-level
/// keys whose values came from disk (i.e. another machine edited them while
/// this view was active) — empty when no conflict occurred.
pub struct MergeOutcome {
    pub merged: Value,
    pub remote_preserved: Vec<String>,
}

/// 3-way merge for cross-device shared-config safety. Compares three states:
///   - `base`: what the frontend's view started from (last load/save/reload)
///   - `incoming`: the frontend's just-submitted save
///   - `existing`: what's on disk right now (may include another machine's
///     edits delivered by cloud sync since `base` was captured)
/// Per top-level key, we take `incoming` when the user edited locally and
/// `existing` when only the remote did. Same-key edits on both sides last-
/// writer-wins on `incoming` — Phase 1's destructive-regression guard backs
/// up the prior state if the local edit would zero out a populated set.
/// Internal keys (configRevision/lastModifiedUtc/_restoredFrom) are excluded
/// from the diff because the save path stamps them itself.
pub fn merge_with_remote(base: &Value, incoming: &Value, existing: &Value) -> MergeOutcome {
    let (base_obj, in_obj, ex_obj) = match (
        base.as_object(),
        incoming.as_object(),
        existing.as_object(),
    ) {
        (Some(b), Some(i), Some(e)) => (b, i, e),
        _ => {
            // Degenerate input — fall back to the legacy shallow-merge so the
            // save still completes.
            return MergeOutcome {
                merged: shallow_merge(incoming, existing),
                remote_preserved: Vec::new(),
            };
        }
    };

    let mut merged = ex_obj.clone();
    let mut remote_preserved: Vec<String> = Vec::new();

    let mut keys: std::collections::BTreeSet<String> = base_obj.keys().cloned().collect();
    keys.extend(in_obj.keys().cloned());
    keys.extend(ex_obj.keys().cloned());

    for k in keys {
        if INTERNAL_CONFIG_KEYS.contains(&k.as_str()) {
            continue;
        }
        let b = base_obj.get(&k);
        let i = in_obj.get(&k);
        let e = ex_obj.get(&k);

        // A key ABSENT from the incoming payload is "no local opinion", not a
        // local delete. Almost every frontend save is a partial patch (one to
        // sixteen keys), so treating absence as an edit turned a single
        // Settings toggle on the conflict path into a wipe of assignments,
        // profiles, radial, autocorrect and hotkeys (`None != Some(_)` was
        // true for every omitted key and the None arm removed it). Deletion
        // of a top-level key is never expressed by omission anywhere in the
        // frontend, so the remove arm is gone.
        let local_edited = match i {
            Some(v) => Some(v) != b,
            None => false,
        };
        let remote_edited = e != b;

        if local_edited {
            if let Some(v) = i {
                merged.insert(k.clone(), v.clone());
            }
        } else if remote_edited {
            // Disk value already lives in `merged` from ex_obj.clone(); record
            // the key so the caller can surface it in a "kept remote edits" toast.
            remote_preserved.push(k.clone());
        }
    }

    MergeOutcome {
        merged: Value::Object(merged),
        remote_preserved,
    }
}

/// Legacy shallow-merge: every key in `incoming` overwrites `existing`, keys
/// only present in `existing` are preserved. Used on the non-conflict path
/// and as a fallback when 3-way merge inputs aren't objects.
pub fn shallow_merge(incoming: &Value, existing: &Value) -> Value {
    match (existing.as_object(), incoming.as_object()) {
        (Some(ex), Some(inc)) => {
            let mut m = ex.clone();
            for (k, v) in inc {
                m.insert(k.clone(), v.clone());
            }
            Value::Object(m)
        }
        _ => incoming.clone(),
    }
}

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
        // Any REMOVED assignment is significant on its own: deleting one to
        // five keys used to take no backup at all, and LKG was immediately
        // overwritten with the post-delete state, so a mis-click was
        // unrecoverable.
        for k in &ex_keys {
            if !in_keys.contains(k) {
                return true;
            }
        }
        if diff > 5 {
            return true;
        }
    }

    // Radial layout: any change to radialMenuItemsByProfile is significant, so a
    // backup is always taken before the radial menu is altered. The layout was
    // otherwise invisible to this heuristic and could be lost without a snapshot.
    // Compared with `appIcon` blobs stripped: the editor re-saves the layout
    // on every drag and an icon re-encode is not a layout change, yet each one
    // used to write another 1-2 MB backup into the 10-slot ring.
    if let Some(inc_r) = incoming.get("radialMenuItemsByProfile") {
        let ex_r = existing.get("radialMenuItemsByProfile");
        if Some(strip_radial_icons(inc_r)) != ex_r.map(strip_radial_icons) {
            return true;
        }
    }

    false
}

/// Deep-copy of a radial layout map with every `appIcon` removed, for
/// change detection that should ignore icon re-encodes.
fn strip_radial_icons(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, val) in m {
                if k == "appIcon" {
                    continue;
                }
                out.insert(k.clone(), strip_radial_icons(val));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(strip_radial_icons).collect()),
        other => other.clone(),
    }
}

// ── Config summary ──────────────────────────────────────────────────────────

pub fn config_summary(cfg: &Value) -> (usize, usize, usize, usize) {
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
    // ::UNASSIGNED:: library entries carry no trigger — excluded so the
    // backup list's assignment count reflects keys that can actually fire.
    let assignment_count = keys
        .iter()
        .filter(|k| {
            !k.contains("::EXPANSION::")
                && !k.contains("::AUTOCORRECT::")
                && !k.contains("::UNASSIGNED::")
        })
        .count();

    // Radial layout segment count — surfaced in the backup list so a wiped
    // radial menu is visible at a glance when choosing a restore point.
    let radial_count = radial_item_count(cfg);

    (profile_count, assignment_count, expansion_count, radial_count)
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
                    let (pc, ac, ec, rc) = config_summary(&cfg);
                    timestamped.push(serde_json::json!({
                        "filename": filename,
                        "date": date,
                        "profileCount": pc,
                        "assignmentCount": ac,
                        "expansionCount": ec,
                        "radialCount": rc,
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
                let (pc, ac, ec, rc) = config_summary(&cfg);
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
                    "radialCount": rc,
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
                    info!("[Keyfire] Restored from backup: {}", filename);
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
