use serde_json::Value;
use tauri::{Emitter, Listener, Manager};

/// Chromium switches for EVERY WebView2 window. WebView2 refuses to create a
/// webview whose browser arguments differ from the browser process already
/// running for this user-data folder (ERROR_INVALID_STATE), so every
/// WebviewWindowBuilder in this crate AND the main window in tauri.conf.json
/// (`additionalBrowserArgs`) must use this exact string.
///
/// `--process-per-site`: Chromium normally gives each WebView its own renderer
/// process (30-120 MB private each; with 8 pre-created windows that was
/// ~500 MB at startup). All Keyfire windows load the same origin
/// (tauri.localhost), so this folds them into ONE renderer process.
/// `--disable-background-timer-throttling` / `--disable-renderer-backgrounding`:
/// webview_mem parks hidden windows with SetIsVisible(false), which makes
/// Chromium treat them as background tabs. Without these, timers in the main
/// window (hidden in the tray) would be aligned to 1 s and then to once a
/// minute after 5 min, and the shared renderer would drop to background CPU
/// priority whenever every window is hidden, i.e. most of the time.
///
/// The `--disable-features` list is wry's default and must be kept.
pub const WEBVIEW_BROWSER_ARGS: &str =
    "--process-per-site --disable-background-timer-throttling --disable-renderer-backgrounding --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

// Platform seam: the 10 engine modules below are Win32-bound. On Windows the
// real modules compile; everywhere else the compiler swaps in no-op twins from
// stubs/ so the app builds and boots UI-only. Shared modules (analytics,
// config, expression, licence, recorder, telemetry) compile on all platforms.
#[cfg(windows)]
mod actions;
#[cfg(not(windows))]
#[path = "stubs/actions.rs"]
mod actions;
mod analytics;
mod analytics_export;
#[cfg(windows)]
mod clipboard;
#[cfg(not(windows))]
#[path = "stubs/clipboard.rs"]
mod clipboard;
mod config;
#[cfg(windows)]
mod expansions;
#[cfg(not(windows))]
#[path = "stubs/expansions.rs"]
mod expansions;
mod expression;
#[cfg(windows)]
mod foreground;
#[cfg(not(windows))]
#[path = "stubs/foreground.rs"]
mod foreground;
#[cfg(windows)]
mod hotkeys;
#[cfg(not(windows))]
#[path = "stubs/hotkeys.rs"]
mod hotkeys;
mod licence;
#[cfg(windows)]
mod ocr;
#[cfg(not(windows))]
#[path = "stubs/ocr.rs"]
mod ocr;
mod recorder;
#[cfg(windows)]
mod distill;
#[cfg(not(windows))]
#[path = "stubs/distill.rs"]
mod distill;
mod telemetry;
#[cfg(windows)]
mod tray;
#[cfg(not(windows))]
#[path = "stubs/tray.rs"]
mod tray;
#[cfg(windows)]
mod voice;
#[cfg(not(windows))]
#[path = "stubs/voice.rs"]
mod voice;
#[cfg(windows)]
mod webview_mem;
#[cfg(not(windows))]
#[path = "stubs/webview_mem.rs"]
mod webview_mem;
#[cfg(windows)]
mod window_target;
#[cfg(not(windows))]
#[path = "stubs/window_target.rs"]
mod window_target;
#[cfg(windows)]
mod monitor_identify;
#[cfg(not(windows))]
#[path = "stubs/monitor_identify.rs"]
mod monitor_identify;
#[cfg(windows)]
mod volume;
#[cfg(not(windows))]
#[path = "stubs/volume.rs"]
mod volume;
#[cfg(windows)]
mod audio_devices;
#[cfg(not(windows))]
#[path = "stubs/audio_devices.rs"]
mod audio_devices;
#[cfg(windows)]
mod shell_files;
#[cfg(not(windows))]
#[path = "stubs/shell_files.rs"]
mod shell_files;

// ── Config (Phase 2) ────────────────────────────────────────────────────────

#[tauri::command]
fn load_config() -> Value {
    let (cfg, restored_from) = config::load_config_safe();
    match cfg {
        Some(mut c) => {
            if let Some(obj) = c.as_object_mut() {
                obj.insert(
                    "_restoredFrom".to_string(),
                    restored_from
                        .clone()
                        .map(Value::String)
                        .unwrap_or(Value::Null),
                );
            }
            if restored_from.is_some() {
                // Fell back to a backup — rewrite as main config + update LKG
                config::save_config(&c);
                config::update_last_known_good(&c);
            } else {
                // Healthy load — boot snapshot in its own 2-slot ring so restarts
                // can't evict edit-time backups from the main 10-slot ring.
                config::create_boot_backup(&c);
            }
            // Phase 2: record what the frontend's view started from so any
            // subsequent save can detect a cross-device sync that landed
            // between this load and the next save.
            config::snapshot_loaded(&c);
            c
        }
        None => {
            // No config on disk and no backups — a brand-new install (or an
            // install whose config + every backup source vanished). Seed with
            // the starter pack so the sidebar, expansions and radial come
            // pre-populated. See config::build_starter_config.
            log::info!("[Keyfire] No config sources found — seeding starter pack");
            let defaults = config::build_starter_config();
            config::save_config(&defaults);
            config::update_last_known_good(&defaults);
            config::snapshot_loaded(&defaults);
            defaults
        }
    }
}

/// Serialises the read → merge → write chain in `save_config` and the other
/// config writers. Without it two saves fired in the same tick (e.g. the
/// Quick-Action category handlers used to send `assignments` and
/// `quickActionCategories` as separate saves) both read the same on-disk
/// state, merged independently, and the last writer dropped the other's key.
static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// User-visible notice that works whether or not the main window is showing.
/// Every in-app toast renders inside the main window's DOM; Keyfire lives in
/// the tray most of the time, so a Rust-side failure (save failed, macro
/// stopped, device missing) used to be invisible. When the main window is
/// hidden this falls back to a native Windows notification.
pub fn emit_user_toast(app: &tauri::AppHandle, level: &str, message: &str) {
    let main_visible = app
        .get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if main_visible {
        let _ = app.emit(
            "system-action-toast",
            serde_json::json!({ "level": level, "message": message }),
        );
        return;
    }
    use tauri_plugin_notification::NotificationExt;
    let title = match level {
        "error" => "Keyfire — problem",
        "success" => "Keyfire",
        _ => "Keyfire",
    };
    if let Err(e) = app.notification().builder().title(title).body(message).show() {
        log::warn!("[Keyfire] Native notification failed ({}); message was: {}", e, message);
    }
}

/// Re-read whatever `config_path()` now points at, make it the frontend's
/// merge base, and push it to React through the same event the shared-config
/// watcher uses (App.jsx → applyLoadedConfig). Used when the config SOURCE
/// changes under a running app: shared folder adopted, shared folder
/// disconnected, shared drive reconnected.
fn reload_config_and_emit(app: &tauri::AppHandle) {
    match config::load_config() {
        Some(cfg) if config::is_valid_config(&cfg) => {
            config::snapshot_loaded(&cfg);
            if let Err(e) = app.emit("config-reloaded-from-sync", &cfg) {
                log::error!("[Keyfire] reload_config_and_emit: emit failed: {}", e);
            }
        }
        _ => log::warn!("[Keyfire] reload_config_and_emit: no valid config at the new source; UI keeps current state"),
    }
}

#[tauri::command]
async fn save_config(app: tauri::AppHandle, config: Value) -> bool {
    // Save can touch a shared/OneDrive path where fs::read_to_string blocks
    // on the sync agent; keep the entire disk-IO chain off the main event
    // loop so unrelated UI + tray + emit traffic stays responsive.
    let (ok, remote_preserved) = tauri::async_runtime::spawn_blocking(move || {
        let _save_guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Re-read from disk RIGHT NOW so we catch any cross-device sync that landed
        // between the frontend's last load and this save. Without this, we'd merge
        // against the file's state at app launch and silently overwrite another
        // machine's edits. If the file exists but can't be read right now, ABORT
        // rather than merge onto `{}` (which rewrote the config as just the payload).
        let existing = match config::load_config_for_save() {
            Ok(v) => v,
            Err(e) => {
                log::error!("[Keyfire] save_config aborted: {}", e);
                return (false, Vec::new());
            }
        };

        // Phase 2 conflict detection: disk revision ahead of what the frontend's
        // view started from means another machine wrote since we loaded.
        let existing_rev = config::config_revision(&existing);
        let last_loaded_rev = config::last_loaded_revision();
        let base = config::last_loaded_base();
        let has_conflict = base.is_some() && existing_rev > last_loaded_rev;

        let (mut merged, remote_preserved) = if has_conflict {
            let outcome = config::merge_with_remote(base.as_ref().unwrap(), &config, &existing);
            if !outcome.remote_preserved.is_empty() {
                log::warn!(
                    "[Keyfire] save_config: sync conflict — disk rev {} > loaded rev {}; preserved remote edits to {:?}",
                    existing_rev,
                    last_loaded_rev,
                    outcome.remote_preserved
                );
            }
            (outcome.merged, outcome.remote_preserved)
        } else {
            (config::shallow_merge(&config, &existing), Vec::new())
        };

        // Stamp the new revision + UTC timestamp. We bump above max(existing, loaded)
        // so two machines writing concurrently can't issue the same revision number.
        if let Some(obj) = merged.as_object_mut() {
            let new_rev = existing_rev.max(last_loaded_rev).saturating_add(1);
            obj.insert(
                "configRevision".to_string(),
                Value::Number(serde_json::Number::from(new_rev)),
            );
            obj.insert(
                "lastModifiedUtc".to_string(),
                Value::String(chrono::Utc::now().to_rfc3339()),
            );
            // `_restoredFrom` is a transient load-time marker — never persist it.
            obj.remove("_restoredFrom");
        }

        // Back up the existing config first if this is a significant change OR a
        // destructive regression (radial/assignments going from populated to empty).
        // The latter is the cross-device clobber signature — we always want a
        // recoverable snapshot of the good state before it lands.
        let destructive = config::is_destructive_regression(&merged, &existing);
        if config::is_significant_change(&config, &existing) || destructive {
            config::create_timestamped_backup(&existing);
        }
        if destructive {
            log::warn!(
                "[Keyfire] save_config: incoming change zeroes-out a previously-populated radial layout or assignment set. Backed up prior state; leaving last-known-good untouched so it stays recoverable."
            );
        }

        let ok = config::save_config(&merged);
        if ok {
            // Don't poison last-known-good with a destructive regression — otherwise
            // the one "known good" snapshot becomes the wiped state (which is exactly
            // how the radial-wipe bug defeated recovery).
            if !destructive {
                config::update_last_known_good(&merged);
            }
            // Snapshot the just-written state as the new base for the next save —
            // otherwise a follow-up save would still see disk ahead of base and
            // run the 3-way merge needlessly.
            config::snapshot_loaded(&merged);
        }
        (ok, remote_preserved)
    })
    .await
    .unwrap_or_else(|e| {
        log::error!("[Keyfire] save_config: spawn_blocking join failed: {}", e);
        (false, Vec::new())
    });

    if ok {
        // Voice pre-warm reads shared engine state, but the phrase set changes
        // rarely enough that a stray warm-up is cheap; keep it on the main
        // thread where the voice module was designed to live.
        voice::prewarm_from_state();
        if !remote_preserved.is_empty() {
            if let Err(e) = app.emit(
                "sync-conflict-resolved",
                serde_json::json!({ "sections": remote_preserved }),
            ) {
                log::error!("[Keyfire] Failed to emit sync-conflict-resolved: {}", e);
            }
        }
    } else {
        // Every frontend caller is fire-and-forget and shows its own success
        // toast, so a failed write used to look like a successful save until
        // the next restart lost the change.
        emit_user_toast(
            &app,
            "error",
            &format!(
                "Couldn't save your changes to {}. Check the file isn't locked or read-only. Changes stay active until Keyfire restarts.",
                config::config_path().display()
            ),
        );
    }
    ok
}

#[tauri::command]
fn get_config_path() -> String {
    config::config_path().to_string_lossy().to_string()
}

#[tauri::command]
fn get_shared_config_path() -> Option<String> {
    config::get_shared_config_dir().map(|p| p.to_string_lossy().to_string())
}

#[tauri::command]
async fn set_shared_config_path(app: tauri::AppHandle, path: String, mode: Option<String>) -> Value {
    let shared_dir = std::path::PathBuf::from(&path);

    // Validate: directory must exist
    if !shared_dir.exists() {
        return serde_json::json!({ "ok": false, "error": "Folder does not exist." });
    }
    if !shared_dir.is_dir() {
        return serde_json::json!({ "ok": false, "error": "Path is not a folder." });
    }

    // Check if target file already exists
    let target_file = shared_dir.join("keyforge-config.json");
    let existed = target_file.exists();
    let mode = mode.unwrap_or_default();

    if existed && mode.is_empty() {
        // File exists and no mode specified — ask the frontend to prompt
        return serde_json::json!({ "ok": false, "needs_choice": true, "existed": true });
    }

    if existed && mode == "replace" {
        // User chose to replace — copy current config over the existing file
        let current = config::config_path();
        if current.exists() {
            match std::fs::read_to_string(&current) {
                Ok(content) => {
                    if let Err(e) = std::fs::write(&target_file, &content) {
                        return serde_json::json!({
                            "ok": false,
                            "error": format!("Cannot write to folder: {}", e)
                        });
                    }
                    log::info!("[Keyfire] Replaced shared config with current: {}", target_file.display());
                }
                Err(e) => {
                    return serde_json::json!({
                        "ok": false,
                        "error": format!("Cannot read current config: {}", e)
                    });
                }
            }
        }
    }
    // mode == "use_existing" — just switch to using the file as-is

    if !existed {
        // Copy current config to shared location
        let current = config::config_path();
        if current.exists() {
            match std::fs::read_to_string(&current) {
                Ok(content) => {
                    if let Err(e) = std::fs::write(&target_file, &content) {
                        return serde_json::json!({
                            "ok": false,
                            "error": format!("Cannot write to folder: {}", e)
                        });
                    }
                    log::info!("[Keyfire] Copied config to shared location: {}", target_file.display());
                }
                Err(e) => {
                    return serde_json::json!({
                        "ok": false,
                        "error": format!("Cannot read current config: {}", e)
                    });
                }
            }
        }
    }

    // Set the override and save local settings
    config::set_shared_config_dir(Some(shared_dir.clone()));
    config::save_local_settings(Some(&shared_dir));

    // Start file watcher for sync detection
    config::start_config_watcher(shared_dir, app.clone());

    // "Use Existing": the file on disk is now the truth, but React still holds
    // this machine's config. Without a reload the next save shallow-merged the
    // local state over the file the user chose to keep (and the watcher then
    // pushed that to every other machine).
    if existed && mode == "use_existing" {
        reload_config_and_emit(&app);
    }

    serde_json::json!({ "ok": true, "existed": existed })
}

#[tauri::command]
fn clear_shared_config_path(app: tauri::AppHandle) -> bool {
    // Copy the shared file over the local one FIRST (atomic, validated), the
    // same way the Pro grace-period migration does. The local file is a
    // snapshot from the day sharing was enabled; switching back to it while
    // React held the shared content produced a piecemeal hybrid on the next
    // save. `migrate_shared_to_local` also clears the override, saves local
    // settings and stops the watcher on success.
    let copied = match config::migrate_shared_to_local() {
        Ok(()) => true,
        Err(e) => {
            log::warn!("[Keyfire] clear_shared_config_path: could not copy shared → local ({}); disconnecting anyway", e);
            config::stop_config_watcher();
            config::set_shared_config_dir(None);
            false
        }
    };
    // If the user manually unsets shared config, any grace-period timestamp
    // is moot — clear it so the banner disappears immediately.
    let _ = config::set_pro_expired_at(None);
    let ok = if copied { true } else { config::save_local_settings(None) };
    reload_config_and_emit(&app);
    if !copied {
        emit_user_toast(&app, "info", "Shared folder disconnected. The shared file couldn't be copied, so Keyfire is using its last local copy.");
    }
    ok
}

#[tauri::command]
async fn export_config(app: tauri::AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let default_name = format!("keyforge-backup-{}.json", today);

    // Get desktop path for default save location
    let desktop = app
        .path()
        .desktop_dir()
        .unwrap_or_default()
        .join(&default_name);

    let file_path = app
        .dialog()
        .file()
        .set_title("Export Keyfire Config")
        .set_file_name(&default_name)
        .add_filter("JSON", &["json"])
        .set_directory(desktop.parent().unwrap_or(std::path::Path::new("")))
        .blocking_save_file();

    let file_path = match file_path {
        Some(p) => p.into_path().unwrap(),
        None => return serde_json::json!({ "ok": false }),
    };

    let (cfg, restored_from) = config::load_config_safe();
    match cfg {
        Some(c) => {
            if let Some(rf) = &restored_from {
                log::warn!(
                    "[Keyfire] Export — main config unreadable, using backup: {}",
                    rf
                );
            }
            match serde_json::to_string_pretty(&c) {
                Ok(json) => match std::fs::write(&file_path, json) {
                    Ok(()) => {
                        log::info!("[Keyfire] Config exported to: {}", file_path.display());
                        serde_json::json!({ "ok": true })
                    }
                    Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
                },
                Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
            }
        }
        None => {
            serde_json::json!({ "ok": false, "error": "No valid config found to export." })
        }
    }
}

#[tauri::command]
async fn import_config(app: tauri::AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let file_path = app
        .dialog()
        .file()
        .set_title("Import Keyfire Config")
        .add_filter("JSON", &["json"])
        .blocking_pick_file();

    let file_path = match file_path {
        Some(p) => p.into_path().unwrap(),
        None => return serde_json::json!({ "ok": false }),
    };

    match std::fs::read_to_string(&file_path) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(mut cfg) => {
                // A Profile export also carries an `assignments` object; importing
                // one here used to pass validation and wipe every other section.
                if cfg.get("trigr_profile").is_some() {
                    return serde_json::json!({
                        "ok": false,
                        "error": "That file is a Profile export. Use Import Profile in the sidebar instead."
                    });
                }
                if !config::is_valid_config(&cfg) {
                    return serde_json::json!({
                        "ok": false,
                        "error": "That file isn't a Keyfire config export (it needs a profiles list and an assignments object)."
                    });
                }
                if let Some(obj) = cfg.as_object_mut() {
                    obj.insert("hasSeenWelcome".to_string(), Value::Bool(true));
                }
                // Nothing is written here. The frontend shows its confirm dialog
                // and then calls `commit_import_config`; previously the file (and
                // LKG) were already replaced by the time the user saw "Are you
                // sure?", so Cancel didn't cancel.
                log::info!("[Keyfire] Config import candidate read from: {}", file_path.display());
                serde_json::json!({ "ok": true, "config": cfg, "path": file_path.to_string_lossy() })
            }
            Err(e) => {
                serde_json::json!({ "ok": false, "error": format!("Could not parse file: {}", e) })
            }
        },
        Err(e) => {
            serde_json::json!({ "ok": false, "error": format!("Could not read file: {}", e) })
        }
    }
}

/// Second half of Import Config: runs only after the user confirmed. Backs up
/// the current file, writes the import, promotes it to last-known-good and
/// makes it the merge base so the next partial save can't 3-way-merge
/// against the pre-import file.
#[tauri::command]
async fn commit_import_config(app: tauri::AppHandle, config: Value) -> Value {
    let ok = tauri::async_runtime::spawn_blocking(move || {
        let _save_guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !config::is_valid_config(&config) {
            return false;
        }
        if let Some(current) = config::load_config() {
            config::create_timestamped_backup(&current);
        }
        if config::save_config(&config) {
            config::update_last_known_good(&config);
            config::snapshot_loaded(&config);
            log::info!("[Keyfire] Config import committed");
            true
        } else {
            false
        }
    })
    .await
    .unwrap_or(false);
    if !ok {
        emit_user_toast(&app, "error", "Couldn't write the imported config to disk. Your current config is unchanged.");
        return serde_json::json!({ "ok": false, "error": "Could not write imported config to disk." });
    }
    serde_json::json!({ "ok": true })
}

#[tauri::command]
fn list_backups() -> Value {
    config::list_backups()
}

#[tauri::command]
fn restore_backup(filename: String) -> Value {
    config::restore_backup(&filename)
}

// ── Profile export/import ──────────────────────────────────────────────────

#[tauri::command]
async fn export_profile(app: tauri::AppHandle, filename_hint: String, content: String) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let desktop = app
        .path()
        .desktop_dir()
        .unwrap_or_default()
        .join(&filename_hint);

    let file_path = app
        .dialog()
        .file()
        .set_title("Export Keyfire Profile")
        .set_file_name(&filename_hint)
        .add_filter("JSON", &["json"])
        .set_directory(desktop.parent().unwrap_or(std::path::Path::new("")))
        .blocking_save_file();

    let file_path = match file_path {
        Some(p) => p.into_path().unwrap(),
        None => return serde_json::json!({ "ok": false }),
    };

    match std::fs::write(&file_path, &content) {
        Ok(()) => {
            log::info!("[Keyfire] Profile exported to: {}", file_path.display());
            serde_json::json!({ "ok": true })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

#[tauri::command]
async fn import_profile(app: tauri::AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let file_path = app
        .dialog()
        .file()
        .set_title("Import Keyfire Profile")
        .add_filter("JSON", &["json"])
        .blocking_pick_file();

    let file_path = match file_path {
        Some(p) => p.into_path().unwrap(),
        None => return serde_json::json!({ "ok": false }),
    };

    match std::fs::read_to_string(&file_path) {
        Ok(raw) => {
            log::info!("[Keyfire] Profile file read from: {}", file_path.display());
            serde_json::json!({ "ok": true, "content": raw })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": format!("Could not read file: {}", e) }),
    }
}

/// Generic save-text-file dialog — export_profile with a caller-chosen filter
/// (CSV corrections packs and friends) instead of the hardwired JSON one.
#[tauri::command]
async fn export_text_file(
    app: tauri::AppHandle,
    filename_hint: String,
    content: String,
    title: String,
    filter_name: String,
    extensions: Vec<String>,
) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let desktop = app
        .path()
        .desktop_dir()
        .unwrap_or_default()
        .join(&filename_hint);

    let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
    let file_path = app
        .dialog()
        .file()
        .set_title(&title)
        .set_file_name(&filename_hint)
        .add_filter(&filter_name, &exts)
        .set_directory(desktop.parent().unwrap_or(std::path::Path::new("")))
        .blocking_save_file();

    let file_path = match file_path {
        Some(p) => p.into_path().unwrap(),
        None => return serde_json::json!({ "ok": false }),
    };

    match std::fs::write(&file_path, &content) {
        Ok(()) => {
            log::info!("[Keyfire] Text file exported to: {}", file_path.display());
            serde_json::json!({ "ok": true })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e.to_string() }),
    }
}

/// Generic open-text-file dialog — import_profile with a caller-chosen filter.
#[tauri::command]
async fn import_text_file(
    app: tauri::AppHandle,
    title: String,
    filter_name: String,
    extensions: Vec<String>,
) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let exts: Vec<&str> = extensions.iter().map(|s| s.as_str()).collect();
    let file_path = app
        .dialog()
        .file()
        .set_title(&title)
        .add_filter(&filter_name, &exts)
        .blocking_pick_file();

    let file_path = match file_path {
        Some(p) => p.into_path().unwrap(),
        None => return serde_json::json!({ "ok": false }),
    };

    match std::fs::read_to_string(&file_path) {
        Ok(raw) => {
            log::info!("[Keyfire] Text file read from: {}", file_path.display());
            serde_json::json!({ "ok": true, "content": raw })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": format!("Could not read file: {}", e) }),
    }
}

// ── File dialogs (Phase 2) ──────────────────────────────────────────────────

#[tauri::command]
async fn browse_for_file(app: tauri::AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let file = app
        .dialog()
        .file()
        .set_title("Select File")
        .add_filter("Executables", &["exe", "bat", "cmd", "lnk"])
        .add_filter("All Files", &["*"])
        .blocking_pick_file();

    match file {
        Some(p) => {
            let path_str = p.into_path().unwrap().to_string_lossy().to_string();
            Value::String(path_str)
        }
        None => Value::Null,
    }
}

#[tauri::command]
async fn browse_for_image(app: tauri::AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let file = app
        .dialog()
        .file()
        .set_title("Select Image")
        .add_filter("Images", &["png", "jpg", "jpeg"])
        .blocking_pick_file();

    match file {
        Some(p) => {
            let path_str = p.into_path().unwrap().to_string_lossy().to_string();
            Value::String(path_str)
        }
        None => Value::Null,
    }
}

#[tauri::command]
async fn browse_for_audio(app: tauri::AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;
    let file = app
        .dialog()
        .file()
        .set_title("Select Audio File")
        .add_filter("Audio", &["mp3", "wav", "ogg", "flac", "m4a", "aac", "wma", "opus"])
        .add_filter("All Files", &["*"])
        .blocking_pick_file();
    match file {
        Some(p) => Value::String(p.into_path().unwrap().to_string_lossy().to_string()),
        None => Value::Null,
    }
}

#[tauri::command]
async fn browse_for_video(app: tauri::AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;
    let file = app
        .dialog()
        .file()
        .set_title("Select Video File")
        .add_filter("Video", &["mp4", "mov", "avi", "mkv", "webm", "wmv", "flv", "m4v"])
        .add_filter("All Files", &["*"])
        .blocking_pick_file();
    match file {
        Some(p) => Value::String(p.into_path().unwrap().to_string_lossy().to_string()),
        None => Value::Null,
    }
}

// Enumerate installed apps via PowerShell's Get-StartApps. Returns an array
// of { name, appId } where appId is the AUMID (for Store/UWP apps) or the
// folder-GUID-prefixed path (for Win32 apps with Start Menu shortcuts). Both
// forms can be launched portably across devices via `shell:AppsFolder\<appId>`.
#[cfg(not(windows))]
#[tauri::command]
fn list_installed_apps() -> Value {
    Value::Array(Vec::new())
}

#[cfg(windows)]
#[tauri::command]
fn list_installed_apps() -> Value {
    use std::process::Command;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    // Enrich Get-StartApps output with the source Start Menu shortcut path.
    // That shortcut is what carries the real icon for classic apps (e.g. Steam
    // .url shortcuts whose AppID is a bare "steam://" URL that no icon API can
    // resolve). Correlation is by filename ↔ Name; misses stay null and the
    // caller falls back to appId (works for UWP AUMIDs via shell namespace).
    const PS_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$starts = Get-StartApps

$map = @{}
$roots = @(
  [Environment]::GetFolderPath('StartMenu'),
  (Join-Path $env:ProgramData 'Microsoft\Windows\Start Menu')
)
foreach ($root in $roots) {
  if (-not (Test-Path $root)) { continue }
  Get-ChildItem $root -Recurse -Include *.lnk,*.url -ErrorAction SilentlyContinue | ForEach-Object {
    $key = [System.IO.Path]::GetFileNameWithoutExtension($_.Name).ToLowerInvariant()
    if (-not $map.ContainsKey($key)) { $map[$key] = $_.FullName }
  }
}

$starts | ForEach-Object {
  $key = $_.Name.ToLowerInvariant()
  $src = if ($map.ContainsKey($key)) { $map[$key] } else { $null }
  [PSCustomObject]@{ Name = $_.Name; AppID = $_.AppID; IconSource = $src }
} | ConvertTo-Json -Compress
"#;

    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            PS_SCRIPT,
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            log::warn!("[Keyfire] list_installed_apps: failed to run PowerShell: {}", e);
            return Value::Array(vec![]);
        }
    };

    if !output.status.success() {
        log::warn!(
            "[Keyfire] list_installed_apps: PowerShell exited non-zero (stderr: {})",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Value::Array(vec![]);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Result<Value> = serde_json::from_str(&stdout);

    let raw_items = match parsed {
        Ok(Value::Array(arr)) => arr,
        // ConvertTo-Json emits a bare object when there's only one result.
        Ok(obj @ Value::Object(_)) => vec![obj],
        Ok(_) => {
            log::warn!("[Keyfire] list_installed_apps: unexpected JSON shape");
            return Value::Array(vec![]);
        }
        Err(e) => {
            log::warn!("[Keyfire] list_installed_apps: JSON parse error: {}", e);
            return Value::Array(vec![]);
        }
    };

    let mut apps: Vec<Value> = raw_items
        .into_iter()
        .filter_map(|item| {
            let name = item.get("Name").and_then(|v| v.as_str())?.trim().to_string();
            let app_id = item.get("AppID").and_then(|v| v.as_str())?.trim().to_string();
            if name.is_empty() || app_id.is_empty() {
                return None;
            }
            let icon_source = item
                .get("IconSource")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            Some(serde_json::json!({
                "name": name,
                "appId": app_id,
                "iconSource": icon_source,
            }))
        })
        .collect();

    // Sort case-insensitive by name for a stable picker order.
    apps.sort_by(|a, b| {
        let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        an.to_lowercase().cmp(&bn.to_lowercase())
    });

    log::info!("[Keyfire] list_installed_apps: returned {} apps", apps.len());
    Value::Array(apps)
}

#[cfg(not(windows))]
#[tauri::command]
fn get_app_icon(path: String) -> Value {
    let _ = path;
    Value::Null
}

#[cfg(not(windows))]
#[tauri::command]
fn get_app_icon_by_name(name: String) -> Value {
    let _ = name;
    Value::Null
}

#[cfg(windows)]
#[tauri::command]
fn get_app_icon(path: String) -> Value {
    icon_data_url_from_path(path)
}

/// v0.8.4 legacy source_app icon resolver. The list payload only carries a
/// full exe path for rows written after this patch — pre-existing rows have
/// only a basename ("chrome.exe"). This command lets the frontend resolve
/// an icon from just the name by walking a chain of Windows lookups:
///   1. HKLM / HKCU / WOW6432Node `App Paths\<name>` registry (covers most
///      user-installed apps — Chrome, Slack, VS Code, Discord, Steam …).
///   2. Currently-running process with a matching exe filename (catches
///      portable / less-common apps that are open right now).
///   3. `%SystemRoot%\System32\<name>` and `%SystemRoot%\<name>` (catches
///      system apps — notepad.exe, explorer.exe, cmd.exe …).
/// Any hit is fed to the existing icon-data-URL pipeline. None on total miss
/// so the frontend renders the text-badge fallback.
#[cfg(windows)]
#[tauri::command]
async fn get_app_icon_by_name(name: String) -> Value {
    // Server-side session cache — identical names hit repeatedly on panel
    // open (one call per unique legacy source_app per session pre-cache).
    // Both a hit (Some(url)) and a miss (None) are memoised so we never
    // re-walk App Paths / running processes / System32 for a name that
    // has no icon on this machine.
    {
        let cache = app_icon_by_name_cache().lock().unwrap();
        if let Some(entry) = cache.get(&name) {
            return match entry {
                Some(url) => Value::String(url.clone()),
                None => Value::Null,
            };
        }
    }

    // spawn_blocking: SHGetFileInfoW + registry walks + ToolHelp snapshot are
    // all synchronous Win32 calls. Running sync on the main IPC thread means
    // a first-load panel open with 15 legacy apps serialises 15 registry
    // walks + icon extractions on the event loop.
    let name_for_task = name.clone();
    let icon = tauri::async_runtime::spawn_blocking(move || {
        resolve_exe_path_for_name(&name_for_task).map(icon_data_url_from_path)
    })
    .await
    .ok()
    .flatten();

    let (result, cached_val): (Value, Option<String>) = match icon {
        Some(Value::String(url)) => (Value::String(url.clone()), Some(url)),
        _ => (Value::Null, None),
    };
    let mut cache = app_icon_by_name_cache().lock().unwrap();
    cache.insert(name, cached_val);
    result
}

#[cfg(windows)]
static APP_ICON_BY_NAME_CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Option<String>>>> = std::sync::OnceLock::new();
#[cfg(windows)]
fn app_icon_by_name_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Option<String>>> {
    APP_ICON_BY_NAME_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(windows)]
fn icon_data_url_from_path(path: String) -> Value {
    use std::ffi::c_void;
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
    use windows_sys::Win32::Graphics::Gdi::{
        GetDIBits, DeleteObject, CreateCompatibleDC, DeleteDC, GetObjectW,
        BITMAPINFO, BITMAPINFOHEADER, BITMAP, BI_RGB, DIB_RGB_COLORS,
    };

    // Declared manually so we don't depend on windows-sys shell/com features.
    #[link(name = "shell32")]
    extern "system" {
        fn SHGetFileInfoW(
            pszPath: *const u16,
            dwFileAttributes: u32,
            psfi: *mut SHFILEINFOW,
            cbFileInfo: u32,
            uFlags: u32,
        ) -> usize;

        fn SHParseDisplayName(
            pszName: *const u16,
            pbc: *mut c_void,
            ppidl: *mut *mut c_void,
            sfgaoIn: u32,
            psfgaoOut: *mut u32,
        ) -> i32;
    }
    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(pvReserved: *mut c_void, dwCoInit: u32) -> i32;
        fn CoTaskMemFree(pv: *mut c_void);
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct SHFILEINFOW {
        hIcon: *mut c_void,
        iIcon: i32,
        dwAttributes: u32,
        szDisplayName: [u16; 260],
        szTypeName: [u16; 80],
    }

    const SHGFI_ICON: u32 = 0x000000100;
    const SHGFI_LARGEICON: u32 = 0x000000000;
    const SHGFI_PIDL: u32 = 0x000000008;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const S_OK: i32 = 0;

    // Route selection:
    //   • '!' or shell:AppsFolder → AUMID (Store apps, UWP)
    //   • .url / .lnk file path   → shortcut file; shell namespace resolution
    //     runs the per-file icon handler (needed for .url's IconFile= line —
    //     SHGetFileInfoW alone returns the generic internet-shortcut icon)
    //   • anything else           → plain filesystem path
    let is_aumid = path.starts_with("shell:AppsFolder") || (path.contains('!') && !path.contains('\\'));
    let ext_lower = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase());
    let is_shortcut_file = matches!(ext_lower.as_deref(), Some("url") | Some("lnk"));
    let use_shell_ns = is_aumid || is_shortcut_file;
    let shell_path = if is_aumid && !path.starts_with("shell:AppsFolder") {
        format!("shell:AppsFolder\\{}", path)
    } else {
        path.clone()
    };

    unsafe {
        // Shell items need COM. Idempotent per thread; the runtime pool worker
        // stays initialized after the call — S_FALSE / RPC_E_CHANGED_MODE are
        // non-fatal ("already initialized in this / another mode").
        let _ = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);

        let mut shfi: SHFILEINFOW = std::mem::zeroed();
        let acquired = if use_shell_ns {
            let wide: Vec<u16> = shell_path.encode_utf16().chain(std::iter::once(0)).collect();
            let mut pidl: *mut c_void = std::ptr::null_mut();
            let hr = SHParseDisplayName(
                wide.as_ptr(),
                std::ptr::null_mut(),
                &mut pidl,
                0,
                std::ptr::null_mut(),
            );
            if hr != S_OK || pidl.is_null() {
                log::warn!("[Keyfire] get_app_icon: SHParseDisplayName failed hr=0x{:08x} for '{}'", hr, shell_path);
                false
            } else {
                let r = SHGetFileInfoW(
                    pidl as *const u16,
                    0,
                    &mut shfi,
                    std::mem::size_of::<SHFILEINFOW>() as u32,
                    SHGFI_ICON | SHGFI_LARGEICON | SHGFI_PIDL,
                );
                CoTaskMemFree(pidl);
                let ok = r != 0 && !shfi.hIcon.is_null();
                if !ok {
                    log::warn!("[Keyfire] get_app_icon: SHGetFileInfoW(PIDL) returned null icon for '{}'", shell_path);
                }
                ok
            }
        } else {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            let r = SHGetFileInfoW(
                wide.as_ptr(),
                0,
                &mut shfi,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            );
            let ok = r != 0 && !shfi.hIcon.is_null();
            if !ok {
                log::warn!("[Keyfire] get_app_icon: SHGetFileInfoW returned null icon for '{}'", path);
            }
            ok
        };

        if !acquired {
            return Value::Null;
        }

        let mut icon_info: ICONINFO = std::mem::zeroed();
        if GetIconInfo(shfi.hIcon as _, &mut icon_info) == 0 {
            DestroyIcon(shfi.hIcon as _);
            return Value::Null;
        }

        let mut bmp: BITMAP = std::mem::zeroed();
        GetObjectW(
            icon_info.hbmColor as _,
            std::mem::size_of::<BITMAP>() as i32,
            &mut bmp as *mut _ as *mut _,
        );

        let width = bmp.bmWidth;
        let height = bmp.bmHeight;
        if width <= 0 || height <= 0 {
            if !icon_info.hbmColor.is_null() { DeleteObject(icon_info.hbmColor); }
            if !icon_info.hbmMask.is_null() { DeleteObject(icon_info.hbmMask); }
            DestroyIcon(shfi.hIcon as _);
            return Value::Null;
        }

        let hdc = CreateCompatibleDC(std::ptr::null_mut());
        let mut bmi: BITMAPINFO = std::mem::zeroed();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = width;
        bmi.bmiHeader.biHeight = -height; // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB as u32;

        let pixel_count = (width * height) as usize;
        let mut pixels: Vec<u8> = vec![0u8; pixel_count * 4];
        GetDIBits(
            hdc,
            icon_info.hbmColor,
            0,
            height as u32,
            pixels.as_mut_ptr() as *mut _,
            &mut bmi,
            DIB_RGB_COLORS,
        );
        DeleteDC(hdc);

        if !icon_info.hbmColor.is_null() { DeleteObject(icon_info.hbmColor); }
        if !icon_info.hbmMask.is_null() { DeleteObject(icon_info.hbmMask); }
        DestroyIcon(shfi.hIcon as _);

        // BGRA → RGBA
        for i in 0..pixel_count {
            let off = i * 4;
            pixels.swap(off, off + 2);
        }

        let img = match image::RgbaImage::from_raw(width as u32, height as u32, pixels) {
            Some(img) => img,
            None => return Value::Null,
        };
        let mut png_buf: Vec<u8> = Vec::new();
        if img.write_to(&mut std::io::Cursor::new(&mut png_buf), image::ImageFormat::Png).is_err() {
            return Value::Null;
        }

        Value::String(format!("data:image/png;base64,{}", base64_encode(&png_buf)))
    }
}

/// Locate an executable given only its basename (e.g. "chrome.exe"). Returns
/// the first hit across a chain of Windows lookups. Case-insensitive on the
/// basename compare — Windows filesystems are case-insensitive so "Chrome.exe"
/// and "chrome.exe" resolve to the same registry key and the same process.
#[cfg(windows)]
fn resolve_exe_path_for_name(name: &str) -> Option<String> {
    if name.is_empty() { return None; }
    // 1. Registry App Paths — HKLM 64-bit, HKCU, and HKLM 32-bit WOW6432.
    if let Some(p) = read_app_paths_registry(name) { return Some(p); }
    // 2. Currently-running process match.
    if let Some(p) = find_running_process_path_by_name(name) { return Some(p); }
    // 3. System paths — %SystemRoot%\System32\<name> and %SystemRoot%\<name>.
    if let Ok(sys) = std::env::var("SystemRoot") {
        for suffix in ["System32", "SysWOW64", ""] {
            let candidate = if suffix.is_empty() {
                format!("{}\\{}", sys, name)
            } else {
                format!("{}\\{}\\{}", sys, suffix, name)
            };
            if std::path::Path::new(&candidate).exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Try the three App Paths registry locations Windows searches. Returns the
/// first non-empty default value (the exe path). Path is stripped of trailing
/// nulls / whitespace and quotes.
#[cfg(windows)]
fn read_app_paths_registry(name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
        KEY_READ, KEY_WOW64_32KEY, KEY_WOW64_64KEY, REG_EXPAND_SZ, REG_SZ,
    };
    // Subkey is always "SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\<name>".
    let subkey = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{}", name);
    let wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();

    // (root, wow-view). WOW64_64KEY forces the 64-bit view; WOW64_32KEY forces
    // the 32-bit view (WOW6432Node) — some 32-bit apps only register there.
    let attempts: [(isize, u32); 3] = [
        (HKEY_LOCAL_MACHINE as isize, KEY_READ | KEY_WOW64_64KEY),
        (HKEY_CURRENT_USER as isize, KEY_READ),
        (HKEY_LOCAL_MACHINE as isize, KEY_READ | KEY_WOW64_32KEY),
    ];

    for (root, flags) in attempts {
        unsafe {
            let mut hkey: windows_sys::Win32::System::Registry::HKEY = std::ptr::null_mut();
            let status = RegOpenKeyExW(root as _, wide.as_ptr(), 0, flags, &mut hkey);
            if status != 0 { continue; }
            // Query the default (unnamed) value, which App Paths keys use to
            // store the exe full path.
            let mut data_type: u32 = 0;
            let mut size: u32 = 0;
            // First call gets required buffer size.
            let s1 = RegQueryValueExW(
                hkey,
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut data_type,
                std::ptr::null_mut(),
                &mut size,
            );
            if s1 != 0 || size == 0 || (data_type != REG_SZ && data_type != REG_EXPAND_SZ) {
                RegCloseKey(hkey);
                continue;
            }
            let mut buf = vec![0u8; size as usize];
            let s2 = RegQueryValueExW(
                hkey,
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut data_type,
                buf.as_mut_ptr(),
                &mut size,
            );
            RegCloseKey(hkey);
            if s2 != 0 { continue; }
            // Reinterpret as u16 slice, strip trailing null, expand env vars if needed.
            let u16_len = (size / 2) as usize;
            let slice = std::slice::from_raw_parts(buf.as_ptr() as *const u16, u16_len);
            let trimmed = if slice.last() == Some(&0) { &slice[..u16_len - 1] } else { slice };
            let raw = String::from_utf16_lossy(trimmed);
            let cleaned = raw.trim().trim_matches('"').to_string();
            if cleaned.is_empty() { continue; }
            let resolved = if data_type == REG_EXPAND_SZ {
                expand_environment_strings(&cleaned).unwrap_or(cleaned)
            } else {
                cleaned
            };
            if std::path::Path::new(&resolved).exists() {
                return Some(resolved);
            }
        }
    }
    None
}

/// Expand `%Var%` placeholders using the calling process's environment.
#[cfg(windows)]
fn expand_environment_strings(input: &str) -> Option<String> {
    #[link(name = "kernel32")]
    extern "system" {
        fn ExpandEnvironmentStringsW(lpSrc: *const u16, lpDst: *mut u16, nSize: u32) -> u32;
    }
    let src: Vec<u16> = input.encode_utf16().chain(std::iter::once(0)).collect();
    // First call with null buffer returns needed length INCLUDING the null.
    unsafe {
        let needed = ExpandEnvironmentStringsW(src.as_ptr(), std::ptr::null_mut(), 0);
        if needed == 0 { return None; }
        let mut buf = vec![0u16; needed as usize];
        let written = ExpandEnvironmentStringsW(src.as_ptr(), buf.as_mut_ptr(), needed);
        if written == 0 { return None; }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]))
    }
}

/// Enumerate live processes via ToolHelp; return the full image path of the
/// first process whose exe basename matches `name` case-insensitively. Bounded
/// by the ToolHelp snapshot size — a running-machine snapshot is typically
/// 200-500 processes, walk cost is sub-millisecond.
#[cfg(windows)]
fn find_running_process_path_by_name(name: &str) -> Option<String> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let target = name.to_ascii_lowercase();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() || snap == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut entry) == 0 {
            CloseHandle(snap);
            return None;
        }
        loop {
            let exe_len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
            let exe = String::from_utf16_lossy(&entry.szExeFile[..exe_len]);
            if exe.to_ascii_lowercase() == target {
                // Match — open the process and query its full path.
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, entry.th32ProcessID);
                if !handle.is_null() {
                    let mut buf = [0u16; 32768];
                    let mut size: u32 = buf.len() as u32;
                    let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
                    CloseHandle(handle);
                    if ok != 0 && size > 0 {
                        CloseHandle(snap);
                        return Some(String::from_utf16_lossy(&buf[..size as usize]));
                    }
                }
            }
            if Process32NextW(snap, &mut entry) == 0 { break; }
        }
        CloseHandle(snap);
    }
    None
}

#[tauri::command]
async fn browse_for_folder(app: tauri::AppHandle) -> Value {
    use tauri_plugin_dialog::DialogExt;

    let folder = app
        .dialog()
        .file()
        .set_title("Select Folder")
        .blocking_pick_folder();

    match folder {
        Some(p) => {
            let path_str = p.into_path().unwrap().to_string_lossy().to_string();
            Value::String(path_str)
        }
        None => Value::Null,
    }
}

// ── Image preview ─────────────────────────────────────────────────────────

#[tauri::command]
fn read_image_base64(path: String) -> Option<String> {
    let data = std::fs::read(&path).ok()?;
    let ext = std::path::Path::new(&path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();
    let mime = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        _ => "image/png",
    };
    Some(format!("data:{};base64,{}", mime, base64_encode(&data)))
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 { out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char); } else { out.push('='); }
        if chunk.len() > 2 { out.push(CHARS[(triple & 0x3F) as usize] as char); } else { out.push('='); }
    }
    out
}

// ── Window enumeration ─────────────────────────────────────────────────────

#[cfg(not(windows))]
#[tauri::command]
fn get_cursor_position() -> Value {
    serde_json::json!({ "x": 0, "y": 0 })
}

#[cfg(windows)]
#[tauri::command]
fn get_cursor_position() -> Value {
    let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
    }
    serde_json::json!({ "x": point.x, "y": point.y })
}

// Eyedropper for the Wait for Pixel macro step: cursor position + the screen
// pixel colour under it, captured atomically so the coordinates and colour
// can never disagree. Both use the physical-pixel coordinate space (the
// process is per-monitor DPI aware), matching what Click at Position feeds
// to SendInput.
#[cfg(not(windows))]
#[tauri::command]
fn get_cursor_pixel() -> Value {
    serde_json::json!({ "x": 0, "y": 0, "color": "#ffffff" })
}

#[cfg(windows)]
#[tauri::command]
fn get_cursor_pixel() -> Value {
    let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
    }
    let color = actions::read_screen_pixel(point.x, point.y)
        .map(|(r, g, b)| format!("#{:02x}{:02x}{:02x}", r, g, b))
        .unwrap_or_else(|| "#ffffff".to_string());
    serde_json::json!({ "x": point.x, "y": point.y, "color": color })
}

// Re-sample a fixed point AFTER the eyedropper's cursor has moved away.
// Buttons and links restyle on hover, so the colour under the cursor at pick
// time is the hover state — not what the pixel shows when the macro later
// polls it (cursor elsewhere). The editor captures position first, waits for
// the mouse to leave, then calls this for the rest-state colour. null = no
// reading (off-screen / CLR_INVALID); the caller keeps its fallback.
#[cfg(not(windows))]
#[tauri::command]
fn get_pixel_color(x: i32, y: i32) -> Value {
    let _ = (x, y);
    serde_json::json!({ "color": null })
}

#[cfg(windows)]
#[tauri::command]
fn get_pixel_color(x: i32, y: i32) -> Value {
    let color = actions::read_screen_pixel(x, y)
        .map(|(r, g, b)| format!("#{:02x}{:02x}{:02x}", r, g, b));
    serde_json::json!({ "color": color })
}

// Arm / disarm the global eyedropper: the next left click anywhere on screen
// picks that pixel (click suppressed, result emitted as pixel-pick-result);
// right click or ESC cancels (pixel-pick-cancelled). The editor hides the
// main window first via recorder_hide_main so the pick happens over the
// user's real screen, and restores it after sampling.
#[cfg(not(windows))]
#[tauri::command]
fn set_pixel_pick_active(active: bool) {
    let _ = active;
}

#[cfg(windows)]
#[tauri::command]
fn set_pixel_pick_active(active: bool) {
    hotkeys::set_pixel_pick_active(active);
}

#[tauri::command]
fn enum_monitors() -> Vec<window_target::MonitorInfo> {
    window_target::enum_monitors()
}

#[tauri::command]
fn show_monitor_identify(app: tauri::AppHandle, dark: bool) {
    // Must run on the main thread — the overlays are raw Win32 windows whose
    // WM_PAINT messages will only be dispatched by tao's main event loop.
    // Creating them on a Tauri IPC worker thread leaves them blank.
    let _ = app.run_on_main_thread(move || {
        monitor_identify::show_identify_overlays(dark);
    });
}

#[tauri::command]
fn hide_monitor_identify(app: tauri::AppHandle) {
    let _ = app.run_on_main_thread(|| {
        monitor_identify::hide_identify_overlays();
    });
}

#[cfg(not(windows))]
#[tauri::command]
fn list_open_windows() -> Vec<Value> {
    Vec::new()
}

#[cfg(windows)]
#[tauri::command]
fn list_open_windows() -> Vec<Value> {
    use std::collections::HashSet;
    use windows_sys::Win32::Foundation::CloseHandle as CloseHandleWin;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    static EXCLUDED: &[&str] = &[
        "explorer.exe",
        "shellexperiencehost.exe",
        "searchhost.exe",
        "startmenuexperiencehost.exe",
        "textinputhost.exe",
        "applicationframehost.exe",
    ];

    struct ListState {
        windows: Vec<(String, String)>,
        excluded: HashSet<String>,
    }

    unsafe extern "system" fn enum_cb(
        hwnd: windows_sys::Win32::Foundation::HWND,
        lparam: isize,
    ) -> i32 {
        let state = &mut *(lparam as *mut ListState);

        // Must be visible (skip hidden system windows, but allow minimized apps)
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }

        // Get window title
        let mut title_buf = [0u16; 512];
        let title_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 512);
        if title_len <= 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&title_buf[..title_len as usize]);

        // Get process name
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return 1;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return 1;
        }
        let mut buf = [0u16; 260];
        let mut size: u32 = 260;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandleWin(handle);
        if ok == 0 || size == 0 {
            return 1;
        }
        let full_path = String::from_utf16_lossy(&buf[..size as usize]);
        let process = std::path::Path::new(&full_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        // Skip excluded system processes
        if state.excluded.contains(&process.to_lowercase()) {
            return 1;
        }

        state.windows.push((process, title));
        1 // continue enumeration
    }

    let mut state = ListState {
        windows: Vec::new(),
        excluded: EXCLUDED.iter().map(|s| s.to_string()).collect(),
    };

    unsafe {
        EnumWindows(Some(enum_cb), &mut state as *mut ListState as isize);
    }

    state.windows.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    state
        .windows
        .into_iter()
        .map(|(process, title)| {
            serde_json::json!({ "process": process, "title": title })
        })
        .collect()
}

// ── Engine (Phase 4) ────────────────────────────────────────────────────────

#[tauri::command]
fn get_engine_status() -> Value {
    hotkeys::get_engine_status()
}

#[tauri::command]
async fn update_assignments(assignments: Value, profile: String) {
    // Parsing + cloning the whole assignment map and rebuilding the suppress
    // set ran synchronously on the main thread on EVERY edit (see
    // feedback_tauri_sync_commands_main_thread); with a large config that was
    // a visible stutter per save.
    tauri::async_runtime::spawn_blocking(move || {
        let map: std::collections::HashMap<String, Value> = assignments
            .as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        hotkeys::update_assignments(map.clone(), profile);
        expansions::update_assignments(map);
    })
    .await
    .ok();
    // Voice phrase grammar may have changed — pre-warm (main thread by design).
    voice::prewarm_from_state();
}

/// Called by the frontend immediately before the updater's
/// downloadAndInstall(): the plugin calls process::exit right after launching
/// the installer, skipping RunEvent::Exit, so any synthetic input Keyfire is
/// holding (Hold-mode key, repeat, bare-key remap) would stay down in Windows
/// through the install and relaunch.
#[tauri::command]
fn release_input_for_exit() {
    actions::release_held_key();
    actions::stop_repeating_key();
    actions::release_all_bare_remaps();
    actions::kill_all_ahk_processes();
}

#[tauri::command]
fn toggle_macros(enabled: bool, app: tauri::AppHandle) {
    // Release any held/repeating key before changing state
    if !enabled {
        actions::release_held_key();
        actions::stop_repeating_key();
    }
    hotkeys::set_macros_enabled(enabled);
    tray::rebuild_tray_menu(&app);
    tray::update_tray_icon(&app, enabled);
}

#[tauri::command]
fn input_focus_changed(focused: bool) {
    hotkeys::set_input_focused(focused);
}

// ── Settings window ──────────────────────────────────────────────────────
// Pre-created hidden at startup (see setup). The main window owns all
// settings state; showing broadcasts "settings-shown" so App.jsx re-emits a
// fresh "settings-state" payload before the window paints. `section` deep-
// links the sidebar (e.g. "licence" from the upgrade modal).

fn show_settings_window_impl(app: &tauri::AppHandle, section: Option<String>) {
    webview_mem::resume_for_show(app, "settings");
    if let Some(win) = app.get_webview_window("settings") {
        let _ = app.emit("settings-shown", serde_json::json!({ "section": section }));
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[tauri::command]
fn show_settings_window(app: tauri::AppHandle, section: Option<String>) {
    show_settings_window_impl(&app, section);
}

#[tauri::command]
fn hide_settings_window(app: tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("settings") {
        let _ = win.hide();
    }
}

// ── Snip overlay (drag-select region picker) ────────────────────────────
// Reusable region picker for any macro editor that needs a screen rect.
// Show resizes the pre-created hidden overlay to the full virtual desktop,
// positions it there and gives it focus. The overlay's own JS captures the
// drag and calls back via emit_snip_result / emit_snip_cancelled which
// forward as `region-snip-result` / `region-snip-cancelled` events to the
// caller (main window). Wait for Text is the first consumer; Wait for
// Image / template capture will reuse the same overlay unchanged.

/// Overlay pulls this on mount to learn the virtual-desktop bounds without
/// depending on the async `snip-overlay-shown` emit (Tauri listener race —
/// the overlay's React listener isn't registered when the emit fires the
/// first time). Returns the SAME values `show_snip_overlay` also emits.
#[cfg(not(windows))]
#[tauri::command]
fn get_snip_overlay_config() -> serde_json::Value {
    serde_json::json!({ "originX": 0, "originY": 0, "width": 0, "height": 0 })
}

#[cfg(windows)]
#[tauri::command]
fn get_snip_overlay_config() -> serde_json::Value {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    let (vsx, vsy, vsw, vsh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    serde_json::json!({
        "originX": vsx,
        "originY": vsy,
        "width": vsw,
        "height": vsh,
    })
}

#[cfg(not(windows))]
#[tauri::command]
fn show_snip_overlay(_app: tauri::AppHandle) {}

#[cfg(windows)]
#[tauri::command]
fn show_snip_overlay(app: tauri::AppHandle) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };
    webview_mem::resume_for_show(&app, "snipoverlay");
    let (vsx, vsy, vsw, vsh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    if let Some(win) = app.get_webview_window("snipoverlay") {
        // Payload FIRST so the mounted component knows the virtual-desktop
        // origin — its own drag rect is in overlay-local pixels and must
        // add (vsx, vsy) to produce screen coords the BitBlt path uses.
        let _ = app.emit(
            "snip-overlay-shown",
            serde_json::json!({
                "originX": vsx,
                "originY": vsy,
                "width": vsw,
                "height": vsh,
            }),
        );
        let _ = win.set_position(tauri::PhysicalPosition::new(vsx, vsy));
        let _ = win.set_size(tauri::PhysicalSize::new(vsw as u32, vsh as u32));
        let _ = win.show();
        let _ = win.set_focus();
    }
}

#[tauri::command]
fn hide_snip_overlay(app: tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("snipoverlay") {
        let _ = win.hide();
    }
}

/// Overlay JS calls this on mouse-up with a completed rect. Rust re-emits
/// as `region-snip-result` so main-window listeners look identical whether
/// the picker was the overlay or (during transition) any fallback.
#[tauri::command]
fn emit_snip_result(app: tauri::AppHandle, x: i32, y: i32, w: i32, h: i32) {
    if let Some(win) = app.get_webview_window("snipoverlay") {
        let _ = win.hide();
    }
    let _ = app.emit(
        "region-snip-result",
        serde_json::json!({ "x": x, "y": y, "w": w, "h": h }),
    );
}

/// Overlay JS calls this on ESC / right-click / any cancel path.
#[tauri::command]
fn emit_snip_cancelled(app: tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("snipoverlay") {
        let _ = win.hide();
    }
    let _ = app.emit("region-snip-cancelled", serde_json::json!({}));
}

#[tauri::command]
fn toggle_settings_window(app: tauri::AppHandle, section: Option<String>) {
    let visible = app
        .get_webview_window("settings")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if visible {
        hide_settings_window(app);
    } else {
        show_settings_window_impl(&app, section);
    }
}

#[tauri::command]
fn start_hotkey_recording() {
    log::info!("[CAPTURE] start_hotkey_recording called");
    hotkeys::set_recording(true);
}

#[tauri::command]
fn stop_hotkey_recording() {
    log::info!("[CAPTURE] stop_hotkey_recording called");
    hotkeys::set_recording(false);
}

#[tauri::command]
fn start_key_capture() {
    log::info!("[CAPTURE] start_key_capture called");
    hotkeys::set_capturing(true);
}

#[tauri::command]
fn stop_key_capture() {
    log::info!("[CAPTURE] stop_key_capture called");
    hotkeys::set_capturing(false);
}

/// JS keydown forwarder — alternative capture path when Keyfire's WebView2 has focus.
/// The LL hook can't see keypresses directed at the WebView2, so the JS keydown
/// listener in tauriAPI.js calls this command during recording/capture mode.
#[tauri::command]
fn js_key_event(code: String, ctrl: bool, shift: bool, alt: bool, meta: bool, app: tauri::AppHandle) {
    hotkeys::handle_js_key_event(&code, ctrl, shift, alt, meta, &app);
}

// ── Macro recorder (Phase 1 — literal replay) ───────────────────────────────

/// Show the countdown overlay positioned bottom-centre on the cursor's
/// monitor, 100px above the work-area bottom. The countdown JS animates
/// 3-2-1 then invokes `recorder_countdown_complete` directly (we used to
/// rely on a JS-emit → Rust-listen handshake, but the event bus wasn't
/// reliably crossing webviews — the listener never fired ~50% of the time).
/// If the user hits Esc or Cancel, the component invokes
/// `recorder_countdown_abort` which hides the window and emits a Tauri
/// event so the main window can unwind UI state.
#[tauri::command]
fn show_recorder_countdown(app: tauri::AppHandle) {
    // This command is the EDITOR flow's entry point (MacroPanel Record
    // button). Quick Record enters via show_recorder_bar directly after
    // setting TEMP_RECORDING_ACTIVE=true. Clear any stale Quick Record flag
    // here so the editor recording's stop routes to the frontend listener,
    // not the temp-macro slot — the routing branch in the stop handlers
    // reads this flag.
    recorder::TEMP_RECORDING_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    show_recorder_bar(app);
}

/// Build (if needed), position and show the bottom-centre recording bar, then
/// start the recorder. Shared by the editor-flow command above and the global
/// Quick Record processor handler in hotkeys.rs. Separate from the Tauri
/// command because #[tauri::command] generates a sibling __cmd__ macro that
/// conflicts with cross-module pub(crate) visibility.
#[cfg(not(windows))]
pub(crate) fn show_recorder_bar(app: tauri::AppHandle) {
    let _ = app;
    log::warn!("[stub] macro recorder UI is not available on this platform yet");
}

#[cfg(windows)]
pub(crate) fn show_recorder_bar(app: tauri::AppHandle) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    // Build the countdown window on demand if it doesn't exist. This is the
    // zero-idle-RAM path — the window only exists for the duration of a
    // recording flow, then hide_recorder_countdown destroys it.
    let win = match app.get_webview_window("countdown") {
        Some(w) => w,
        None => {
            let url = tauri::WebviewUrl::App("index.html?countdown=1".into());
            let builder = tauri::WebviewWindowBuilder::new(&app, "countdown", url)
                .additional_browser_args(WEBVIEW_BROWSER_ARGS)
                .title("Keyfire Recorder")
                .inner_size(380.0, 320.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .shadow(false);
            let built = match builder.build() {
                Ok(w) => w,
                Err(e) => {
                    log::error!("[RECORDER] Failed to build countdown window: {}", e);
                    return;
                }
            };
            #[cfg(target_os = "windows")]
            {
                let _ = built.with_webview(|webview| unsafe {
                    use webview2_com::Microsoft::Web::WebView2::Win32::{
                        ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
                    };
                    use windows_core::Interface;
                    let controller = webview.controller();
                    if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
                        let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                            R: 0, G: 0, B: 0, A: 0,
                        });
                    }
                });
            }
            built
        }
    };
    let _ = webview_mem::resume_for_show(&app, "countdown");

    // Pick the monitor the MAIN window is on, not the cursor's. The user just
    // clicked Record inside main, so that's the screen they're looking at —
    // cursor may have moved away in the milliseconds before the command ran,
    // landing the modal on a different monitor (observed: y=1700 off-screen).
    // Fall back to the cursor's monitor only if main has no HWND yet.
    let (wa_left, wa_top, wa_right, wa_bottom, scale) = unsafe {
        let hmon = app
            .get_webview_window("main")
            .and_then(|w| w.hwnd().ok())
            .map(|h| windows_sys::Win32::Graphics::Gdi::MonitorFromWindow(
                h.0 as _,
                windows_sys::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST,
            ))
            .unwrap_or_else(|| {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
            });
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let s = monitor_scale_factor(hmon);
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom, s)
        } else {
            (0, 0, 1920, 1080, s)
        }
    };

    // Big centered countdown box — 400x400 logical. The frontend renders a
    // 3-2-1 numeral inside, invokes `recorder_countdown_complete` when done,
    // which then morphs this window to the compact pill for the actual
    // recording phase.
    let w_logical = 400.0_f64;
    let h_logical = 400.0_f64;
    let phys_w = (w_logical * scale).round() as i32;
    let phys_h = (h_logical * scale).round() as i32;
    let phys_x = wa_left + ((wa_right - wa_left) - phys_w) / 2;
    let phys_y = wa_top + ((wa_bottom - wa_top) - phys_h) / 2;

    let _ = win.set_size(tauri::PhysicalSize::new(phys_w as u32, phys_h as u32));
    let _ = win.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));

    let _ = win.show();
    // No set_focus — the user's target app must keep keyboard focus during
    // the 3-2-1 (and during recording itself).
    log::info!("[RECORDER] Countdown overlay shown at {}x{} ({}x{})", phys_x, phys_y, phys_w, phys_h);

    // The countdown timing is owned by RUST, not the webview. History: two
    // webview-driven attempts both broke — (a) frontend invoking
    // recorder_countdown_complete off a visibilityState check ghost-started
    // recordings at app launch (Chromium briefly reports 'visible' on hidden
    // windows), and (b) a Rust-set pending flag raced visibilitychange and
    // the countdown never started at all. A Rust thread has neither problem:
    // recording starts exactly 3s after show even if the webview renders
    // nothing. The webview's 3-2-1 numeral is cosmetic — it polls
    // get_recording_status to switch to the pill phase.
    use std::sync::atomic::Ordering as O;
    recorder::COUNTDOWN_CANCEL.store(false, O::SeqCst);
    if !recorder::COUNTDOWN_THREAD_RUNNING.swap(true, O::SeqCst) {
        // Publish the deadline BEFORE spawning — the webview numeral derives
        // from countdown_remaining_ms() via its get_recording_status poll.
        recorder::countdown_begin(3000);
        let app2 = app.clone();
        std::thread::spawn(move || {
            // 3 seconds in 100ms polls per [[feedback_polled_sleep_for_cancel]]
            // so Esc / Cancel aborts within 100ms.
            for _ in 0..30 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if recorder::COUNTDOWN_CANCEL.load(O::SeqCst) {
                    recorder::countdown_clear();
                    recorder::COUNTDOWN_THREAD_RUNNING.store(false, O::SeqCst);
                    log::info!("[RECORDER] Countdown cancelled before start");
                    return;
                }
            }
            recorder::countdown_clear();
            morph_countdown_to_pill(&app2);
            recorder::start();
            recorder::COUNTDOWN_THREAD_RUNNING.store(false, O::SeqCst);
            log::info!("[RECORDER] Countdown done -> recording started");
        });
    } else {
        log::warn!("[RECORDER] Countdown thread already running — ignoring duplicate show");
    }
}

/// Resize + reposition the countdown window into the small top-right pill
/// shown while a recording is in progress. Called by
/// `recorder_countdown_complete` at the moment 3-2-1 finishes.
#[cfg(not(windows))]
fn morph_countdown_to_pill(app: &tauri::AppHandle) {
    let _ = app;
}

#[cfg(windows)]
fn morph_countdown_to_pill(app: &tauri::AppHandle) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let Some(win) = app.get_webview_window("countdown") else { return; };

    // Same monitor selection as show_recorder_bar: the main window's monitor
    // (the user launched the flow from there), cursor's monitor as fallback.
    // Keeps the countdown box and the pill on the same screen.
    let (wa_left, _wa_top, wa_right, wa_bottom, scale) = unsafe {
        let hmon = app
            .get_webview_window("main")
            .and_then(|w| w.hwnd().ok())
            .map(|h| windows_sys::Win32::Graphics::Gdi::MonitorFromWindow(
                h.0 as _,
                MONITOR_DEFAULTTONEAREST,
            ))
            .unwrap_or_else(|| {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
            });
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let s = monitor_scale_factor(hmon);
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom, s)
        } else {
            (0, 0, 1920, 1080, s)
        }
    };

    // 320x50 logical, bottom-centre with 30px margin — the recording bar's
    // long-standing home (users look for the timer + Stop there). The old
    // top-right placement was a leftover from an abandoned design.
    let w_logical = 320.0_f64;
    let h_logical = 50.0_f64;
    let phys_w = (w_logical * scale).round() as i32;
    let phys_h = (h_logical * scale).round() as i32;
    let margin = (30.0 * scale).round() as i32;
    let phys_x = wa_left + ((wa_right - wa_left) - phys_w) / 2;
    let phys_y = wa_bottom - phys_h - margin;

    let _ = win.set_size(tauri::PhysicalSize::new(phys_w as u32, phys_h as u32));
    let _ = win.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));
}

#[tauri::command]
fn hide_recorder_countdown(app: tauri::AppHandle) {
    hide_recorder_bar(app);
}

/// Hide the recording bar window. Shared by the Tauri command above and the
/// global Quick Record processor handler. See `show_recorder_bar` for the
/// pub(crate) / #[tauri::command] split rationale.
pub(crate) fn hide_recorder_bar(app: tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("countdown") {
        let _ = win.hide();
        log::info!("[RECORDER] Countdown overlay hidden");
    }
}

/// Called by the countdown component the moment the 3-2-1 finishes. Direct
/// invoke (not event emit) so we don't depend on JS→Rust event-bus delivery
/// crossing webviews. Morphs the window into the recording pill and flips
/// IS_RECORDING_MACRO to true via recorder::start().
#[tauri::command]
fn recorder_countdown_complete(_app: tauri::AppHandle) {
    // DELIBERATE NO-OP. The countdown timing is owned by the Rust thread in
    // show_recorder_bar — recording starts there, morph happens there. This
    // command is kept registered purely as a tombstone: a stale webview
    // bundle (dev HMR, cached first-build) may still invoke it, and if it
    // did anything it could double-start a recording or ghost-start one at
    // app launch (both shipped as real bugs during 2026-08-10 dev). Do not
    // put logic back here.
    log::debug!("[RECORDER] recorder_countdown_complete invoked (no-op — Rust thread owns the countdown)");
}

/// Called by the countdown component when the user hits Esc or Cancel during
/// the 3-2-1. Hides the overlay and emits a Tauri event to the main window
/// so it can restore itself + clear the recording UI state.
#[tauri::command]
fn recorder_countdown_abort(app: tauri::AppHandle) {
    // Tell the Rust countdown thread to bail before it morphs + starts.
    // Polled every 100ms, so the thread exits within a tick. Clear the
    // published deadline immediately so the webview numeral vanishes on the
    // next poll rather than waiting for the thread to notice.
    recorder::COUNTDOWN_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    recorder::countdown_clear();
    // Clear the Quick Record routing flag — an aborted flow must never leave
    // it set, or the NEXT editor-flow stop would misroute events into the
    // temp-macro slot.
    recorder::TEMP_RECORDING_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    // If the recorder already started (Esc arrived after the 3s), discard the
    // partial buffer — Esc means cancel, not save.
    if recorder::is_recording() {
        recorder::discard();
    }
    if let Some(win) = app.get_webview_window("countdown") {
        let _ = win.hide();
    }
    use tauri::Emitter as _;
    let _ = app.emit("recorder-countdown-cancelled", ());
    log::info!("[RECORDER] Countdown aborted");
}

/// Stop the recording from the pill's Stop button. Mirrors what the LL hook
/// does when it detects the configured record hotkey: flips IS_RECORDING_MACRO
/// false and branches on TEMP_RECORDING_ACTIVE — editor flow emits to frontend
/// so the React listener retrieves the buffer; global flow finalises here,
/// saves to the temp slot, and hides the pill.
#[tauri::command]
fn recorder_stop_from_pill(app: tauri::AppHandle) {
    use std::sync::atomic::Ordering as O;
    recorder::IS_RECORDING_MACRO.store(false, O::SeqCst);
    let (count, dur) = recorder::status_snapshot();
    use tauri::Emitter as _;
    if recorder::TEMP_RECORDING_ACTIVE.load(O::SeqCst) {
        recorder::TEMP_RECORDING_ACTIVE.store(false, O::SeqCst);
        let events = recorder::stop();
        let captured_at = chrono::Local::now().to_rfc3339();
        if let Ok(mut state) = hotkeys::engine_state().lock() {
            state.temp_macro_events = Some(events.clone());
            state.temp_macro_captured_at = Some(captured_at.clone());
        }
        persist_temp_macro(&events, &captured_at);
        hide_recorder_bar(app.clone());
        let _ = app.emit(
            "temp-macro-saved",
            serde_json::json!({
                "count": events.len(),
                "durationMs": dur,
                "capturedAt": captured_at,
            }),
        );
        log::info!("[RECORDER] Temp macro saved via pill Stop ({} events, {}ms)", events.len(), dur);
    } else {
        let _ = app.emit(
            "recorder-stop-requested",
            serde_json::json!({ "count": count, "durationMs": dur }),
        );
        log::info!("[RECORDER] Stop button clicked → stop relayed to frontend");
    }
}

/// Hide the main window for the recorder flow. We use `hide()` rather than
/// `minimize()` because Windows brings minimised windows in the same process
/// back to foreground when a sibling window (the countdown overlay) is
/// shown — observed as "main bounces straight back to full size". hide() is
/// the right primitive: the user sees their target app, Keyfire disappears
/// from the taskbar, EDITING_ACTIVE stays set (unlike hide_window_to_tray
/// which deliberately clears it + emits reset-editing-on-hide). The macro
/// editor selection therefore survives the recording round-trip.
#[tauri::command]
fn recorder_hide_main(app: tauri::AppHandle) {
    // Open the flow gate FIRST so the foreground watcher can't fire a
    // profile-switch in the brief window between hide() and the countdown
    // becoming visible. Without this, switching to the target app post-hide
    // unmounts the ReplayRecordingValue component and closes everything.
    recorder::RECORDER_FLOW_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
        log::info!("[RECORDER] Main hidden for recording (flow gate opened)");
    }
}

/// Restore the main window after a recording flow. Reuses the tray-restore
/// path which handles unminimize + show + AttachThreadInput focus dance.
#[tauri::command]
fn recorder_restore_main(app: tauri::AppHandle) {
    tray::show_window(&app);
    recorder::RECORDER_FLOW_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
    log::info!("[RECORDER] Main restored after recording (flow gate closed)");
}

#[tauri::command]
fn start_macro_recording() {
    log::info!("[RECORDER] start_macro_recording called");
    recorder::start();
}

/// Stop the recording and return the captured events as a JSON value. The
/// frontend stuffs this into a "Replay Recording" macro step's value field
/// (serialised), then saves the assignment via the normal config flow.
#[tauri::command]
fn stop_macro_recording() -> Value {
    log::info!("[RECORDER] stop_macro_recording called");
    let events = recorder::stop();
    serde_json::to_value(&events).unwrap_or(serde_json::Value::Array(Vec::new()))
}

#[tauri::command]
fn discard_macro_recording() {
    log::info!("[RECORDER] discard_macro_recording called");
    recorder::discard();
}

/// Distil a raw event stream into semantic macro steps. Called from the
/// MacroPanel when the user clicks the Distil button on a Record Macro step.
/// Pure function — no I/O beyond the ToUnicodeEx layout lookup per KeyDown.
///
/// Returns `{ steps, targetApp }` — the frontend saves both alongside the raw
/// events. targetApp is auto-extracted from the first ForegroundChanged event
/// so distilled macros are automatically bound to the app they were recorded
/// against; replay aborts with a modal if that app isn't running.
///
/// Pro-gated: free tier gets an empty steps array + null targetApp. Distillation
/// + window-relative clicks + target-app binding together give a recorded macro
/// "runs in this specific app" behaviour that would cannibalise the Pro
/// app-linking pitch, so it lives behind the same gate as voice / advanced
/// analytics / expression engine per [[feedback_licence_entitlement_invariants]].
#[tauri::command]
async fn distill_events(events: Value) -> Value {
    if !licence::is_pro() {
        log::info!("[DISTILL] free tier — returning empty step list");
        return serde_json::json!({ "steps": [], "targetApp": null });
    }
    // Long recordings can produce 20K+ events; the full-walk distillation is
    // CPU-bound and would block the main thread if run sync. spawn_blocking
    // moves parse + distill onto a worker thread so the UI stays responsive.
    let result = tauri::async_runtime::spawn_blocking(move || {
        let events_vec: Vec<recorder::RecordedEvent> = match serde_json::from_value(events) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("[DISTILL] Failed to parse events: {}", e);
                return serde_json::json!({ "steps": [], "targetApp": null });
            }
        };
        let steps = distill::distill(&events_vec);
        let target_app = distill::extract_target_app(&events_vec);
        log::info!(
            "[DISTILL] {} events → {} steps, target_app={:?}",
            events_vec.len(),
            steps.len(),
            target_app.as_ref().map(|t| t.exe.clone())
        );
        serde_json::json!({
            "steps": steps,
            "targetApp": target_app,
        })
    })
    .await;
    result.unwrap_or_else(|e| {
        log::warn!("[DISTILL] spawn_blocking join failed: {}", e);
        serde_json::json!({ "steps": [], "targetApp": null })
    })
}

/// Returns `{ recording: bool, count: usize, durationMs: u64 }`. Polled by the
/// recording-status indicator. Cheap — atomic reads + a mutex lock on the
/// events vec for the count.
#[tauri::command]
fn get_recording_status() -> Value {
    let (count, dur) = recorder::status_snapshot();
    serde_json::json!({
        "recording": recorder::is_recording(),
        "count": count,
        "durationMs": dur,
        // Non-zero while the pre-recording 3-2-1 is in flight. The countdown
        // webview derives its numeral from this — Rust's clock, not a local
        // JS timer — so the display can never be stale.
        "countdownRemainingMs": recorder::countdown_remaining_ms(),
    })
}

// ── Profiles (Phase 6) ──────────────────────────────────────────────────────

#[tauri::command]
fn set_active_global_profile(profile: String) {
    foreground::set_active_global_profile(profile.clone());
    hotkeys::set_active_profile(profile);
    // Active-profile assignments change → grammar changes
    voice::prewarm_from_state();
}

#[tauri::command]
fn update_profile_settings(settings: Value) {
    let map: std::collections::HashMap<String, Value> = settings
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    hotkeys::update_profile_settings(map.clone());
    foreground::update_profile_settings(map);
}

#[tauri::command]
fn get_foreground_process() -> String {
    foreground::get_current_fg_proc()
}

#[tauri::command]
fn set_editing_active(active: bool) {
    foreground::set_editing_active(active);
}

// ── Settings (Phase 5) ──────────────────────────────────────────────────────

#[tauri::command]
fn update_global_settings(settings: Value) {
    hotkeys::update_global_settings(&settings);
}

#[tauri::command]
fn update_autocorrect_enabled(enabled: bool) {
    expansions::set_autocorrect_enabled(enabled);
}

#[tauri::command]
fn get_builtin_autocorrect_entries() -> Vec<(String, String, String)> {
    expansions::builtin_autocorrect_entries()
}

#[tauri::command]
fn update_autocorrect_settings(settings: expansions::AutocorrectSettings) {
    expansions::set_autocorrect_settings(settings);
}

#[tauri::command]
fn update_expansion_excluded_apps(apps: Vec<String>) {
    expansions::set_expansion_excluded_apps(apps);
}

/// Global variables accept two shapes at the IPC boundary:
///   - string       → static value: `"Rory Brady"`
///   - array of str → random-pick set: `["hi","hello","hey"]`
/// Array-shaped values are re-encoded to a JSON `[...]` string here so the
/// backend HashMap keeps its `String` value type; `resolve_tokens` detects the
/// array literal at fire time and picks one entry per fire (see the pick-cache
/// in expansions::resolve_tokens for consistency across multiple occurrences
/// in the same expansion body).
#[tauri::command]
fn update_global_variables(vars: std::collections::HashMap<String, Value>) {
    let mut normalized: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (name, v) in vars {
        let stored = match v {
            Value::String(s) => s,
            Value::Array(_) => v.to_string(), // JSON-encode: ["hi","hello"]
            other => other.to_string(),        // numbers/bools stringify; nulls become "null"
        };
        normalized.insert(name, stored);
    }
    expansions::update_global_variables(normalized);
}

// ── Audio output device switching ──────────────────────────────────────────
// Feeds the picker in the "Change Audio Output" macro step's config UI and
// executes the switch at fire time. Both commands are sync — MMDevice
// enumeration and IPolicyConfig::SetDefaultEndpoint each return within a few
// ms, so a blocking call is fine (Tauri sync commands run on the main thread —
// see [[feedback_tauri_sync_commands_main_thread]]).

#[tauri::command]
fn list_audio_output_devices() -> Vec<audio_devices::AudioOutputDevice> {
    audio_devices::list_output_devices()
}

/// Called from the frontend's step preview / test button. The macro-fire path
/// calls audio_devices::set_default_output_device directly from actions.rs
/// (no round-trip through Tauri).
#[tauri::command]
fn set_audio_output_device(device_id: String) -> Result<String, audio_devices::SetOutputError> {
    audio_devices::set_default_output_device(&device_id)
}

// ── Pause (Phase 3) ─────────────────────────────────────────────────────────

#[tauri::command]
fn set_global_pause_key(combo: String) -> Value {
    hotkeys::set_pause_hotkey(&combo);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
fn clear_global_pause_key() {
    hotkeys::clear_pause_hotkey();
}

#[tauri::command]
fn set_clipboard_paste_key(combo: String) -> Value {
    hotkeys::set_clipboard_paste_hotkey(&combo);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
fn clear_clipboard_paste_key() {
    hotkeys::clear_clipboard_paste_hotkey();
}

#[tauri::command]
fn set_voice_hotkey(combo: String) -> Value {
    hotkeys::set_voice_hotkey(&combo);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
fn clear_voice_hotkey() {
    hotkeys::clear_voice_hotkey();
}

// ── Quick Record (temp macro) ───────────────────────────────────────────────

#[tauri::command]
fn set_temp_macro_record_hotkey(combo: String) -> Value {
    hotkeys::set_temp_macro_record_hotkey(&combo);
    persist_temp_macro_hotkeys();
    serde_json::json!({ "ok": true })
}

#[tauri::command]
fn clear_temp_macro_record_hotkey() {
    hotkeys::set_temp_macro_record_hotkey("");
    persist_temp_macro_hotkeys();
}

#[tauri::command]
fn set_temp_macro_play_hotkey(combo: String) -> Value {
    hotkeys::set_temp_macro_play_hotkey(&combo);
    persist_temp_macro_hotkeys();
    serde_json::json!({ "ok": true })
}

#[tauri::command]
fn clear_temp_macro_play_hotkey() {
    hotkeys::set_temp_macro_play_hotkey("");
    persist_temp_macro_hotkeys();
}

#[tauri::command]
fn set_temp_macro_loop_hotkey(combo: String) -> Value {
    hotkeys::set_temp_macro_loop_hotkey(&combo);
    persist_temp_macro_hotkeys();
    serde_json::json!({ "ok": true })
}

#[tauri::command]
fn clear_temp_macro_loop_hotkey() {
    hotkeys::set_temp_macro_loop_hotkey("");
    persist_temp_macro_hotkeys();
}

#[tauri::command]
fn get_temp_macro_status() -> Value {
    if let Ok(state) = hotkeys::engine_state().lock() {
        let event_count = state.temp_macro_events.as_ref().map(|v| v.len()).unwrap_or(0);
        let loop_active = recorder::TEMP_MACRO_LOOP_ACTIVE.load(std::sync::atomic::Ordering::SeqCst);
        return serde_json::json!({
            "hasEvents": event_count > 0,
            "eventCount": event_count,
            "capturedAt": state.temp_macro_captured_at.clone(),
            "recordHotkey": state.temp_macro_record_hotkey_str.clone(),
            "playHotkey": state.temp_macro_play_hotkey_str.clone(),
            "loopHotkey": state.temp_macro_loop_hotkey_str.clone(),
            "loopActive": loop_active,
        });
    }
    serde_json::json!({
        "hasEvents": false,
        "eventCount": 0,
        "capturedAt": serde_json::Value::Null,
        "recordHotkey": serde_json::Value::Null,
        "playHotkey": serde_json::Value::Null,
        "loopHotkey": serde_json::Value::Null,
        "loopActive": false,
    })
}

#[tauri::command]
fn clear_temp_macro() -> bool {
    if let Ok(mut state) = hotkeys::engine_state().lock() {
        state.temp_macro_events = None;
        state.temp_macro_captured_at = None;
    }
    let existing = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let mut merged = existing;
    // Only remove the events + timestamp keys — the user's record/play hotkey
    // choices live in the same tempMacro object and must survive a Clear so
    // they don't silently revert to defaults on the next restart.
    if let Some(obj) = merged.as_object_mut() {
        if let Some(temp) = obj.get_mut("tempMacro").and_then(|v| v.as_object_mut()) {
            temp.remove("events");
            temp.remove("capturedAt");
        }
    }
    config::save_config(&merged)
}

/// One-shot startup cleanup for users who auto-updated from a pre-rebrand
/// install (≤ v0.5.6). The old NSIS installer placed `Trigr.lnk` in the user's
/// Start Menu pointing at `trigr.exe`. v0.6.0 rebranded the product to Keyfire
/// AND renamed the binary to `keyfire.exe` — the new installer creates
/// `Keyfire.lnk` but Tauri's NSIS template only manages shortcuts named
/// `${PRODUCTNAME}.lnk`, so the stale `Trigr.lnk` survives auto-update as a
/// broken shortcut pointing at a now-missing binary. We delete it from every
/// known location on startup. Idempotent: silent no-op when nothing's there.
/// Covers per-user Start Menu (currentUser install mode) and Desktop.
#[cfg(not(windows))]
fn cleanup_stale_trigr_shortcuts() {}

#[cfg(windows)]
fn cleanup_stale_trigr_shortcuts() {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(appdata) = std::env::var("APPDATA") {
        let start_menu = std::path::PathBuf::from(&appdata)
            .join("Microsoft").join("Windows").join("Start Menu").join("Programs");
        // Both layouts the old installer might have produced — flat .lnk or
        // a "Trigr" subfolder containing the .lnk.
        candidates.push(start_menu.join("Trigr.lnk"));
        candidates.push(start_menu.join("Trigr").join("Trigr.lnk"));
    }
    if let Ok(userprofile) = std::env::var("USERPROFILE") {
        // Optional desktop icon — only present if the user ticked it during install.
        candidates.push(std::path::PathBuf::from(userprofile).join("Desktop").join("Trigr.lnk"));
    }

    let mut removed = 0;
    for path in &candidates {
        if path.exists() {
            match std::fs::remove_file(path) {
                Ok(_) => {
                    log::info!("[Keyfire] Removed stale Trigr shortcut: {}", path.display());
                    removed += 1;
                }
                Err(e) => log::warn!("[Keyfire] Failed to remove stale Trigr shortcut {}: {}", path.display(), e),
            }
        }
    }

    // Tidy: if the old installer used a "Trigr" subfolder under Programs and
    // it's now empty, remove it too. remove_dir only succeeds on empty dirs,
    // so this is safe — a non-empty Trigr folder (user customised) survives.
    if let Ok(appdata) = std::env::var("APPDATA") {
        let trigr_subfolder = std::path::PathBuf::from(appdata)
            .join("Microsoft").join("Windows").join("Start Menu").join("Programs").join("Trigr");
        if trigr_subfolder.exists() && trigr_subfolder.is_dir() {
            let _ = std::fs::remove_dir(&trigr_subfolder);
        }
    }

    if removed > 0 {
        log::info!("[Keyfire] Stale Trigr shortcut cleanup: {} removed", removed);
    }
}

/// Ensure a working Keyfire Start Menu shortcut exists for the current user.
///
/// Tauri's auto-updater does in-place binary replacement and does NOT re-run
/// the NSIS installer, so users who auto-updated from a pre-rebrand build
/// (≤ v0.5.6) had their Trigr.lnk cleaned up by `cleanup_stale_trigr_shortcuts`
/// but never got a Keyfire.lnk in its place — leaving them with no way to
/// launch Keyfire from the Start Menu after the rebrand (only the
/// Start-with-Windows registry autorun would start it). This fixes that gap
/// by creating the shortcut ourselves on every startup if it's missing.
/// Idempotent: silent no-op when the shortcut already exists.
///
/// Implementation: PowerShell shell-out via WScript.Shell COM. ~100ms one-time
/// cost on the first startup that finds the file missing, then silent on
/// subsequent runs. Pure-Rust IShellLink would be ~50 LOC of unsafe COM glue
/// plus CoInitialize lifecycle management — not worth the complexity for a
/// once-per-machine fix that touches no hot path.
///
/// Target comes from `std::env::current_exe()` so the same code works for
/// both install-dir cases: fresh v0.6.0+ install at `AppData\Local\Keyfire\`
/// and auto-updated users still at `AppData\Local\Trigr\` (Tauri preserved
/// the install dir name across the rebrand because the updater never moves
/// directories — only swaps the .exe binary in place).
#[cfg(not(windows))]
fn ensure_keyfire_shortcut() {}

#[cfg(windows)]
fn ensure_keyfire_shortcut() {
    let Ok(appdata) = std::env::var("APPDATA") else {
        log::warn!("[Keyfire] ensure_keyfire_shortcut: APPDATA env var missing");
        return;
    };
    let shortcut = std::path::PathBuf::from(&appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Keyfire.lnk");
    if shortcut.exists() {
        return; // Already there — silent idempotent no-op on every later startup.
    }

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            log::warn!("[Keyfire] ensure_keyfire_shortcut: current_exe failed: {}", e);
            return;
        }
    };
    let exe_str = exe.to_string_lossy().to_string();
    let work_dir = exe
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let shortcut_str = shortcut.to_string_lossy().to_string();

    // PowerShell single-quoted strings: literal single quote inside is doubled.
    // Real install paths shouldn't contain quotes, but escape defensively.
    let esc = |s: &str| s.replace('\'', "''");

    let ps = format!(
        "$s = (New-Object -ComObject WScript.Shell).CreateShortcut('{}'); \
         $s.TargetPath = '{}'; \
         $s.WorkingDirectory = '{}'; \
         $s.IconLocation = '{},0'; \
         $s.Description = 'Keyfire: Windows hotkey and macro manager'; \
         $s.Save()",
        esc(&shortcut_str),
        esc(&exe_str),
        esc(&work_dir),
        esc(&exe_str),
    );

    // -NoProfile + -NonInteractive + Hidden window keep startup fast and silent.
    // -ExecutionPolicy Bypass guards against tight per-user policies; -Command
    // inline does not need scripts-from-disk policy anyway, this is belt-and-braces.
    match std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &ps,
        ])
        .spawn()
    {
        Ok(_) => log::info!(
            "[Keyfire] Creating missing Start Menu shortcut at {} -> {}",
            shortcut.display(),
            exe.display()
        ),
        Err(e) => log::warn!(
            "[Keyfire] ensure_keyfire_shortcut: PowerShell spawn failed: {}",
            e
        ),
    }
}

/// Persist the cached temp macro hotkey strings to config. Called after each
/// setter so the user's choice survives restart. Hotkeys + macro-event slot
/// live under a single `tempMacro` object in config to keep the schema tidy.
fn persist_temp_macro_hotkeys() {
    let (record, play, loop_combo) = match hotkeys::engine_state().lock() {
        Ok(s) => (
            s.temp_macro_record_hotkey_str.clone(),
            s.temp_macro_play_hotkey_str.clone(),
            s.temp_macro_loop_hotkey_str.clone(),
        ),
        Err(_) => return,
    };
    let existing = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let mut merged = existing;
    if let Some(obj) = merged.as_object_mut() {
        let temp = obj.entry("tempMacro".to_string()).or_insert_with(|| serde_json::json!({}));
        if let Some(t) = temp.as_object_mut() {
            t.insert("recordHotkey".to_string(), record.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null));
            t.insert("playHotkey".to_string(), play.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null));
            t.insert("loopHotkey".to_string(), loop_combo.map(serde_json::Value::String).unwrap_or(serde_json::Value::Null));
        }
    }
    config::save_config(&merged);
}

/// Persist a freshly captured temp macro to disk. Called from the processor
/// after the global-flow stop saves events into engine state. Stored as a
/// JSON-serialised array under `tempMacro.events` alongside `capturedAt`.
pub fn persist_temp_macro(events: &[crate::recorder::RecordedEvent], captured_at: &str) {
    let existing = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let mut merged = existing;
    if let Some(obj) = merged.as_object_mut() {
        let temp = obj.entry("tempMacro".to_string()).or_insert_with(|| serde_json::json!({}));
        if let Some(t) = temp.as_object_mut() {
            t.insert("events".to_string(), serde_json::to_value(events).unwrap_or(serde_json::Value::Null));
            t.insert("capturedAt".to_string(), serde_json::Value::String(captured_at.to_string()));
        }
    }
    config::save_config(&merged);
}

#[tauri::command]
fn start_voice_recognition(phrases: Vec<String>, app: tauri::AppHandle) {
    // Voice commands are Pro. Gate at the start path so neither a forced
    // `voiceCommandsEnabled` config nor a direct IPC call can run recognition
    // without entitlement. Enforced via is_pro() so it tracks Paddle later.
    if !licence::is_pro() {
        return;
    }
    voice::start_recognition(phrases, app);
}

#[tauri::command]
fn start_voice_continuous(phrases: Vec<String>, app: tauri::AppHandle) {
    if !licence::is_pro() {
        return;
    }
    voice::start_continuous_recognition(phrases, app);
}

#[tauri::command]
fn stop_voice_continuous() {
    voice::stop_continuous_recognition();
}

#[tauri::command]
fn stop_voice_recognition() {
    voice::stop_recognition();
}

#[tauri::command]
fn check_hotkey_conflict(combo: String, from_slot: Option<String>) -> Value {
    let parsed = match hotkeys::parse_hotkey_combo(&combo) {
        Some(p) => p,
        None => return serde_json::json!({ "conflict": false, "conflictWith": null }),
    };
    let from = from_slot.unwrap_or_default();
    let state = hotkeys::engine_state_lock();

    if from != "overlay" && state.overlay_hotkey == Some(parsed) {
        return serde_json::json!({ "conflict": true, "conflictWith": "Quick Search overlay" });
    }
    if from != "pause" && state.pause_hotkey == Some(parsed) {
        return serde_json::json!({ "conflict": true, "conflictWith": "Pause hotkey" });
    }
    if from != "voice" && state.voice_hotkey == Some(parsed) {
        return serde_json::json!({ "conflict": true, "conflictWith": "Voice hotkey" });
    }
    if from != "radial" && state.radial_menu_hotkey == Some(parsed) {
        return serde_json::json!({ "conflict": true, "conflictWith": "Radial menu" });
    }
    if from != "clipboard_paste" && state.clipboard_paste_hotkey == Some(parsed) {
        return serde_json::json!({ "conflict": true, "conflictWith": "Clipboard quick paste" });
    }
    if from != "temp_macro_record" && state.temp_macro_record_hotkey == Some(parsed) {
        return serde_json::json!({ "conflict": true, "conflictWith": "Quick Record (record)" });
    }
    if from != "temp_macro_play" && state.temp_macro_play_hotkey == Some(parsed) {
        return serde_json::json!({ "conflict": true, "conflictWith": "Quick Record (play)" });
    }
    if from != "temp_macro_loop" && state.temp_macro_loop_hotkey == Some(parsed) {
        return serde_json::json!({ "conflict": true, "conflictWith": "Quick Record (loop)" });
    }

    // Regular per-profile assignments — only check active profile single-press
    // (storage format: ProfileName::Modifier::KeyCode; double-press has an extra
    // "::double" suffix and can legitimately coexist with single-press).
    if from != "assignment" {
        let prefix = format!("{}::", state.active_profile);
        for (key, value) in state.assignments.iter() {
            if !key.starts_with(&prefix) { continue; }
            let parts: Vec<&str> = key.split("::").collect();
            if parts.len() != 3 { continue; } // skip double-press / malformed
            let assignment_combo = format!("{}+{}", parts[1], parts[2]);
            if let Some(p) = hotkeys::parse_hotkey_combo(&assignment_combo) {
                if p == parsed {
                    let label = value.get("label").and_then(|v| v.as_str()).unwrap_or("Unnamed");
                    return serde_json::json!({
                        "conflict": true,
                        "conflictWith": format!("Assignment: {}", label),
                    });
                }
            }
        }
    }

    serde_json::json!({ "conflict": false, "conflictWith": null })
}

// ── Window (Phase 3) ────────────────────────────────────────────────────────

#[tauri::command]
fn window_minimize(window: tauri::Window) {
    let _ = window.minimize();
}

#[tauri::command]
fn window_maximize(window: tauri::Window) {
    if window.is_maximized().unwrap_or(false) {
        let _ = window.unmaximize();
    } else {
        let _ = window.maximize();
    }
}

#[tauri::command]
fn window_close(app: tauri::AppHandle) {
    tray::hide_window_to_tray(&app);
}

#[tauri::command]
fn show_window(app: tauri::AppHandle) {
    tray::show_window(&app);
}

#[tauri::command]
fn hide_window(app: tauri::AppHandle) {
    tray::hide_window_to_tray(&app);
}

#[tauri::command]
fn set_window_resizable(resizable: bool, app: tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_resizable(resizable);
    }
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle) {
    actions::release_held_key();
    actions::stop_repeating_key();
    actions::kill_all_ahk_processes();
    app.exit(0);
}

// ── Startup (Phase 3) ───────────────────────────────────────────────────────

#[tauri::command]
fn get_startup_enabled() -> bool {
    tray::get_startup_enabled()
}

#[tauri::command]
fn set_startup_enabled(enabled: bool) {
    tray::set_startup_enabled(enabled);
}

#[tauri::command]
fn get_app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Best-effort guess at the physical keyboard form factor from the active
/// input language: "iso" (tall Enter, extra key beside left Shift) for the UK
/// and mainland-European layouts, "ansi" otherwise. Windows does not expose
/// the real shape; the frontend upgrades the guess to ISO the first time the
/// hook sees the ISO key (scancode 0x56) and the user can override it.
#[tauri::command]
fn get_keyboard_layout_hint() -> String {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyboardLayout;
    // 0 = the calling thread's layout; sync commands run on the main thread,
    // whose layout follows the user's active input language.
    let hkl = unsafe { GetKeyboardLayout(0) } as usize;
    let langid = (hkl & 0xFFFF) as u32;
    let primary = langid & 0x3FF;
    let sub = langid >> 10;
    let iso = match primary {
        0x09 => matches!(sub, 0x02 | 0x06), // en-GB, en-IE (other English = ANSI)
        0x02 | 0x03 | 0x05 | 0x06 | 0x07 | 0x08 | 0x0A | 0x0B | 0x0C | 0x0E | 0x0F | 0x10
        | 0x13 | 0x14 | 0x15 | 0x16 | 0x18 | 0x1A | 0x1B | 0x1D | 0x1F | 0x22 | 0x24
        | 0x25 | 0x26 | 0x27 | 0x2D | 0x56 => true, // bg ca cs da de el es fi fr hu is it nl nb pl pt ro hr sk sv tr uk sl et lv lt eu gl
        _ => false,
    };
    if iso { "iso".to_string() } else { "ansi".to_string() }
}

/// Per-position legends (and hook key ids) from the active Windows input
/// layout, for the on-screen keyboard. Sync so it runs on the main thread,
/// whose layout follows the user's active input language.
#[tauri::command]
fn get_keyboard_legends() -> Vec<hotkeys::KeyLegend> {
    hotkeys::keyboard_legends()
}

// ── Help / External (Phase 3) ───────────────────────────────────────────────

#[tauri::command]
fn open_help() {
    let _ = opener::open("https://keyfire.app/trigr-help.html");
}

#[tauri::command]
fn open_config_folder(_app: tauri::AppHandle) {
    let path = config::config_path();
    if let Some(parent) = path.parent() {
        let _ = opener::open(parent.to_string_lossy().as_ref());
    }
}

#[tauri::command]
fn open_logs_folder(app: tauri::AppHandle) {
    if let Ok(log_dir) = app.path().app_log_dir() {
        let _ = std::fs::create_dir_all(&log_dir);
        let _ = opener::open(log_dir.to_string_lossy().as_ref());
    }
}

#[tauri::command]
fn open_clipboard_folder(_app: tauri::AppHandle) {
    // Opens the actual folder containing trigr-clipboard.db (+ .db-wal, .db-shm).
    // Uses clipboard::data_dir() so we get the real path the writer thread is
    // using — not whatever Tauri's app_local_data_dir() / app_data_dir() guesses.
    if let Some(dir) = clipboard::data_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = opener::open(dir.to_string_lossy().as_ref());
    } else {
        log::warn!("[Keyfire] open_clipboard_folder: clipboard module not initialised yet");
    }
}

#[tauri::command]
fn open_external(url: String) {
    let _ = opener::open(&url);
}

/// Generic JS→Rust debug logging — prints to terminal from any webview window.
#[tauri::command]
fn log_debug(message: String) {
    log::debug!("{}", message);
}

// ── Overlay / Quick Search (Phase 9) ────────────────────────────────────────

use std::sync::atomic::{AtomicIsize, Ordering as AtomicOrdering};
use std::time::Instant as StdInstant;
use std::sync::Mutex as StdMutex;

/// HWND of the foreground window captured when the overlay was shown.
static OVERLAY_TARGET_HWND: AtomicIsize = AtomicIsize::new(0);

/// True when the search bar sits low enough that its results dropdown would
/// run off the bottom of the work area — the results render above the input
/// and overlay_resize grows the window upward (bottom edge anchored) instead
/// of downward. Decided per show in show_overlay, consumed by overlay_resize
/// and mirrored to the frontend as `flipUp` in the search-data payload.
static OVERLAY_FLIP_UP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Max height of the search overlay window in logical px. Must exceed the
/// tallest content the frontend can measure (12 + input 54 + results
/// max-height 340 + 16 = 422) — a smaller cap clips the window edge, which
/// in flip mode cuts into the input row.
const OVERLAY_MAX_H: f64 = 430.0;

/// Timestamp when overlay was last shown — used for blur dismiss guard.
static OVERLAY_SHOW_TIME: std::sync::OnceLock<StdMutex<Option<StdInstant>>> = std::sync::OnceLock::new();

fn overlay_show_time() -> &'static StdMutex<Option<StdInstant>> {
    OVERLAY_SHOW_TIME.get_or_init(|| StdMutex::new(None))
}

/// Whether voice mode is locked in continuous mode. Set by the pill click via
/// the set_voice_continuous Tauri command. Cleared by hide_overlay() on close.
static VOICE_CONTINUOUS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Timestamp when the voice overlay was opened — used for double-tap detection.
static VOICE_OVERLAY_OPEN_TIME: std::sync::OnceLock<StdMutex<Option<StdInstant>>> = std::sync::OnceLock::new();

fn voice_overlay_open_time() -> &'static StdMutex<Option<StdInstant>> {
    VOICE_OVERLAY_OPEN_TIME.get_or_init(|| StdMutex::new(None))
}

/// Resolve a monitor's effective DPI scale via Win32 GetDpiForMonitor.
/// Returns the linear scale factor (e.g. 2.25 for 225% scaling). Falls
/// back to 1.0 if the API call fails.
///
/// Use this instead of `Window::scale_factor()` when positioning an overlay
/// onto a target monitor. `scale_factor()` returns the scale of the
/// monitor the window is CURRENTLY on, which may not match the target.
/// On a hidden pre-created overlay window, scale_factor() can also return
/// stale 1.0 before Windows establishes the window's DPI, producing wrong
/// physical coordinates after Tauri's logical-to-physical conversion at
/// set_position/set_size. Going through PhysicalPosition + PhysicalSize
/// computed from this helper bypasses all of that.
#[cfg(windows)]
fn monitor_scale_factor(hmon: windows_sys::Win32::Graphics::Gdi::HMONITOR) -> f64 {
    use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    let mut dx: u32 = 96;
    let mut dy: u32 = 96;
    unsafe {
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    }
    dx as f64 / 96.0
}

/// Map the JS-side overlay name to the Tauri window label.
fn overlay_position_label(name: &str) -> Option<&'static str> {
    match name {
        "search" => Some("overlay"),
        "clipboard" => Some("clipboardoverlay"),
        _ => None,
    }
}

/// Saved user-dragged position for an overlay window, as fractions of the
/// active monitor's work area minus the window size (0..1 keeps the window
/// fully on-screen by construction). Stored in trigr-local-settings.json —
/// machine-specific by design, monitor layouts don't sync across devices.
/// Fraction storage means the position maps onto whichever monitor is active
/// at show time and survives DPI/resolution changes.
fn saved_overlay_frac(name: &str) -> Option<(f64, f64)> {
    let val = config::load_local_settings_json();
    let pos = val.get("overlayPositions")?.get(name)?;
    let x = pos.get("xFrac")?.as_f64()?;
    let y = pos.get("yFrac")?.as_f64()?;
    Some((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)))
}

#[cfg(not(windows))]
fn show_overlay(app: &tauri::AppHandle) {
    let _ = app;
    log::warn!("[stub] quick search overlay is not available on this platform yet");
}

#[cfg(windows)]
fn show_overlay(app: &tauri::AppHandle) {
    // Wake a suspended webview BEFORE any emit/show — see webview_mem.rs invariant.
    webview_mem::resume_for_show(app, "overlay");
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};

    // Capture target HWND before we steal focus
    let target = unsafe { GetForegroundWindow() as isize };
    OVERLAY_TARGET_HWND.store(target, AtomicOrdering::Relaxed);

    let overlay = match app.get_webview_window("overlay") {
        Some(w) => w,
        None => return,
    };

    // Get cursor position to identify the active monitor
    let (cx, cy) = unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        (pt.x, pt.y)
    };

    // Get the work area + DPI of the monitor containing the cursor.
    // Physical coords throughout: avoids the hidden-window scale_factor
    // race and Tauri's logical-to-physical re-conversion at set_position
    // using a possibly-different scale (the bug behind the 4K@225% clip).
    let (wa_left, wa_top, wa_right, wa_bottom, scale) = unsafe {
        let pt = POINT { x: cx, y: cy };
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let s = monitor_scale_factor(hmon);
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom, s)
        } else {
            (0, 0, 1920, 1080, s)
        }
    };

    // Centre on active monitor, one-third from top. Physical units.
    let win_w_logical = 620.0_f64;
    let win_h_logical = 103.0_f64;
    let phys_w = (win_w_logical * scale).round() as i32;
    let phys_h = (win_h_logical * scale).round() as i32;
    let mut phys_x = wa_left + ((wa_right - wa_left) - phys_w) / 2;
    let mut phys_y = wa_top + (wa_bottom - wa_top) / 3;
    // User-dragged position override (grip drag on the search bar). Voice
    // mode is untouched — show_voice_overlay keeps its bottom-centre pill.
    if let Some((fx, fy)) = saved_overlay_frac("search") {
        let span_x = ((wa_right - wa_left) - phys_w).max(0) as f64;
        let span_y = ((wa_bottom - wa_top) - phys_h).max(0) as f64;
        phys_x = wa_left + (fx * span_x).round() as i32;
        phys_y = wa_top + (fy * span_y).round() as i32;
    }
    let _ = overlay.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));
    let _ = overlay.set_size(tauri::PhysicalSize::new(phys_w as u32, phys_h as u32));

    // Flip the dropdown when the bar sits low enough that a full results
    // list would run past the work area.
    let flip_up = phys_y + (OVERLAY_MAX_H * scale).round() as i32 > wa_bottom - 16;
    OVERLAY_FLIP_UP.store(flip_up, AtomicOrdering::SeqCst);
    if flip_up {
        // Flip sessions use a FIXED full-height window with the panel pinned
        // to its bottom edge by CSS — the results list grows and shrinks
        // purely inside the DOM and overlay_resize is a no-op. Per-keystroke
        // top-edge window resizing cannot be made smooth on Windows (the
        // stale framebuffer stays anchored top-left during a resize, so
        // every content change flashed the bar a frame out of place). With
        // zero window ops after show, nothing can jitter.
        //
        // The window bottom sits at bar_y + 82 logical (panel top room 12 +
        // input row 54 + bottom margin 16 — mirrors the JS measure constants
        // and SearchOverlay.css) so the input row lands on exactly the same
        // pixels a normal-mode bar at this position would.
        let anchor = phys_y + (82.0 * scale).round() as i32;
        let max_h_phys = (OVERLAY_MAX_H * scale).round() as i32;
        let _ = overlay.set_position(tauri::PhysicalPosition::new(phys_x, anchor - max_h_phys));
        let _ = overlay.set_size(tauri::PhysicalSize::new(phys_w as u32, max_h_phys as u32));
    }

    // Send search data to the overlay — includes ALL assignments (profile + global).
    // Same builder the overlay's self-heal pull uses (get_search_overlay_data),
    // so push and pull can never drift.
    let search_data = build_search_overlay_data(Some(flip_up));
    let _ = overlay.emit("overlay-search-data", search_data);

    // Show and focus
    *overlay_show_time().lock().unwrap() = Some(StdInstant::now());
    let _ = overlay.show();
    let _ = overlay.set_focus();
    // Track HWND so the mouse hook can dismiss on click-outside even when set_focus
    // didn't actually grab OS focus (foreground-stealing restrictions).
    if let Ok(hwnd) = overlay.hwnd() {
        hotkeys::SEARCH_OVERLAY_HWND.store(hwnd.0 as isize, AtomicOrdering::SeqCst);
    }
    // Notify any listeners (onboarding tour Step 7 waits for this) that Quick
    // Search was actually fired by the user.
    let _ = app.emit("search-overlay-shown", serde_json::Value::Null);
}

/// Quick Search payload: every assignment (profile + global), Pro-capped search
/// templates, overlay settings and theme. `flip_up` is a show-time geometry
/// fact; the pull path passes `None` so the overlay keeps whatever the last
/// show told it.
fn build_search_overlay_data(flip_up: Option<bool>) -> Value {
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    // Pro gate: Free users get the first 5 templates. Anything beyond
    // is preserved in config and returns when the user upgrades.
    let search_templates = {
        let templates = cfg.get("searchTemplates").cloned().unwrap_or_else(|| serde_json::json!([]));
        if licence::is_pro() {
            templates
        } else if let Some(arr) = templates.as_array() {
            serde_json::Value::Array(arr.iter().take(5).cloned().collect())
        } else {
            templates
        }
    };
    let state = hotkeys::engine_state_lock();
    let mut data = serde_json::json!({
        "assignments": state.assignments,
        "activeProfile": state.active_profile,
        "globalInputMethod": cfg.get("globalInputMethod").and_then(|v| v.as_str()).unwrap_or("direct"),
        "theme": cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark"),
        "searchTemplates": search_templates,
        "settings": {
            "showAll": cfg.get("overlayShowAll").and_then(|v| v.as_bool()).unwrap_or(true),
            "closeAfterFiring": cfg.get("overlayCloseAfterFiring").and_then(|v| v.as_bool()).unwrap_or(true),
            "includeAutocorrect": cfg.get("overlayIncludeAutocorrect").and_then(|v| v.as_bool()).unwrap_or(false),
        },
        "voiceEnabled": cfg.get("voiceCommandsEnabled").and_then(|v| v.as_bool()).unwrap_or(true),
    });
    if let (Some(flip), Some(obj)) = (flip_up, data.as_object_mut()) {
        obj.insert("flipUp".to_string(), Value::Bool(flip));
    }
    data
}

/// Self-heal pull for the Quick Search overlay (mirrors the clipboard popup's
/// v0.8.5 fix). The pushed `overlay-search-data` emit has two loss windows —
/// cold start before the lazy chunk registers its listener, and the
/// resume-from-TrySuspend IPC race — either of which left the bar blank /
/// stale / dark until closed and reopened. Side-effect free: plain config
/// read, never `load_config` the command (boot side effects).
#[tauri::command]
async fn get_search_overlay_data() -> Value {
    tauri::async_runtime::spawn_blocking(|| build_search_overlay_data(None))
        .await
        .unwrap_or_else(|_| serde_json::json!({}))
}

/// Same for the radial menu (`radial-menu-data`). Also carries holdToSelect +
/// holdKey, so a lost push used to disable hold-release firing for that open.
#[tauri::command]
async fn get_radial_menu_data() -> Value {
    tauri::async_runtime::spawn_blocking(build_radial_menu_data)
        .await
        .unwrap_or_else(|_| serde_json::json!({}))
}

/// Radial wheel payload for the CURRENT active profile. Shared by the show
/// path (push) and `get_radial_menu_data` (self-heal pull).
fn build_radial_menu_data() -> Value {
    // Build payload: resolve radial menu items for the CURRENT active profile.
    // Use radialMenuItemsByProfile[activeProfile] rather than the flat radialMenuItems
    // array, which may be stale if a profile switch hasn't flushed to disk yet.
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let theme = cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark");
    let state = hotkeys::engine_state_lock();
    let active_profile = state.active_profile.clone();
    // Per-profile map is the source of truth. The legacy flat radialMenuItems
    // field is read ONLY when the map is entirely absent (pre-per-profile
    // configs). Falling back per-profile would show stale items for profiles
    // the user never configured — the editor shows them an empty wheel, the
    // live overlay must match. The frontend no longer writes the flat field.
    let radial_items = match cfg.get("radialMenuItemsByProfile") {
        Some(m) => m.get(&active_profile).cloned().unwrap_or_else(|| serde_json::json!([])),
        None => cfg
            .get("radialMenuItems")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
    };
    let resolve_item = |item: &Value| -> Option<Value> {
        // Check if this is a folder item (has type: "folder")
        let is_folder = item
            .get("type")
            .and_then(|v| v.as_str())
            .map(|t| t == "folder")
            .unwrap_or(false);

        if is_folder {
            let label = item
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("Folder");
            let children_raw = item
                .get("children")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();

            // Resolve each child
            let resolved_children: Vec<Value> = children_raw
                .iter()
                .filter_map(|child| {
                    let sk = child.get("storageKey")?.as_str()?;
                    let assignment = state.assignments.get(sk);
                    let child_label_override = child
                        .get("label")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty());
                    let default_label = assignment
                        .and_then(|a| a.get("label").and_then(|l| l.as_str()))
                        .unwrap_or("");
                    let assign_type = assignment
                        .and_then(|a| a.get("type").and_then(|t| t.as_str()))
                        .unwrap_or("");
                    let child_type = if sk.starts_with("GLOBAL::EXPANSION::") {
                        "expansion"
                    } else if sk.starts_with("GLOBAL::QUICKACTION::") {
                        "quickaction"
                    } else if sk.starts_with("GLOBAL::AUTOCORRECT::") {
                        "autocorrect"
                    } else {
                        "assignment"
                    };
                    Some(serde_json::json!({
                        "id": child.get("id"),
                        "storageKey": sk,
                        "label": child_label_override.unwrap_or(default_label),
                        "icon": child.get("icon"),
                        "iconColor": child.get("iconColor"),
                        "appIcon": child.get("appIcon"),
                        "assignType": assign_type,
                        "exists": assignment.is_some(),
                        "type": child_type,
                        "data": assignment.and_then(|a| a.get("data").cloned()),
                    }))
                })
                .collect();

            return Some(serde_json::json!({
                "id": item.get("id"),
                "type": "folder",
                "label": label,
                "icon": item.get("icon"),
            "iconColor": item.get("iconColor"),
            "appIcon": item.get("appIcon"),
                "exists": true,
                "children": resolved_children,
            }));
        }

        // Regular item
        let storage_key = item.get("storageKey")?.as_str()?;
        let assignment = state.assignments.get(storage_key);
        let label_override = item
            .get("label")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty());
        let default_label = assignment
            .and_then(|a| a.get("label").and_then(|l| l.as_str()))
            .unwrap_or("");
        let assign_type = assignment
            .and_then(|a| a.get("type").and_then(|t| t.as_str()))
            .unwrap_or("");
        let item_type = if storage_key.starts_with("GLOBAL::EXPANSION::") {
            "expansion"
        } else if storage_key.starts_with("GLOBAL::QUICKACTION::") {
            "quickaction"
        } else if storage_key.starts_with("GLOBAL::AUTOCORRECT::") {
            "autocorrect"
        } else {
            "assignment"
        };

        Some(serde_json::json!({
            "id": item.get("id"),
            "storageKey": storage_key,
            "label": label_override.unwrap_or(default_label),
            "icon": item.get("icon"),
            "iconColor": item.get("iconColor"),
            "appIcon": item.get("appIcon"),
            "assignType": assign_type,
            "exists": assignment.is_some(),
            "type": item_type,
            "data": assignment.and_then(|a| a.get("data").cloned()),
        }))
    };

    let resolved_items: Vec<Value> = radial_items
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|item| {
            if item.is_null() {
                Value::Null
            } else {
                resolve_item(item).unwrap_or(Value::Null)
            }
        })
        .collect();
    drop(state);

    // Hold-to-select: the overlay holds keyboard focus (set_focus below), so
    // its own web layer detects the launch-key release. Pass whether the mode
    // is on and which key ends the gesture (the action segment of the radial
    // hotkey combo, in KeyboardEvent.code form, e.g. "KeyW" from
    // "Ctrl+Alt+KeyW"). This replaces the earlier hook-based detection, which
    // could never observe the release of a key whose keydown we suppress.
    let hold_to_select = cfg
        .get("radialHoldToSelect")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let hold_key = cfg
        .get("radialMenuHotkey")
        .and_then(|v| v.as_str())
        .and_then(|s| s.rsplit('+').next())
        .unwrap_or("")
        .to_string();
    let payload = serde_json::json!({
        "items": resolved_items,
        "theme": theme,
        "holdToSelect": hold_to_select,
        "holdKey": hold_key,
    });
    payload
}

#[cfg(not(windows))]
fn show_voice_overlay(app: &tauri::AppHandle) {
    let _ = app;
}

#[cfg(windows)]
fn show_voice_overlay(app: &tauri::AppHandle) {
    // Wake a suspended webview BEFORE any emit/show — see webview_mem.rs invariant.
    webview_mem::resume_for_show(app, "overlay");
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};
    use windows_sys::Win32::Foundation::POINT;

    log::info!("[Keyfire] show_voice_overlay: START");

    // Capture target HWND before we steal focus
    let target = unsafe { GetForegroundWindow() as isize };
    OVERLAY_TARGET_HWND.store(target, AtomicOrdering::Relaxed);

    let overlay = match app.get_webview_window("overlay") {
        Some(w) => w,
        None => return,
    };

    // Position on active monitor — same logic as show_overlay
    let (cx, cy) = unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        (pt.x, pt.y)
    };
    let (wa_left, _wa_top, wa_right, wa_bottom, scale) = unsafe {
        let pt = POINT { x: cx, y: cy };
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let s = monitor_scale_factor(hmon);
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom, s)
        } else {
            (0, 0, 1920, 1080, s)
        }
    };

    // Compact square, bottom-centre, above taskbar. Physical units to dodge
    // the hidden-window scale_factor race (see monitor_scale_factor docstring).
    let win_w_logical = 72.0_f64;
    let win_h_logical = 72.0_f64;
    let phys_w = (win_w_logical * scale).round() as i32;
    let phys_h = (win_h_logical * scale).round() as i32;
    let pad_phys = (12.0 * scale).round() as i32; // 12px above taskbar, in physical
    let phys_x = wa_left + ((wa_right - wa_left) - phys_w) / 2;
    let phys_y = wa_bottom - phys_h - pad_phys;
    let _ = overlay.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));
    let _ = overlay.set_size(tauri::PhysicalSize::new(phys_w as u32, phys_h as u32));

    // Send voice data to overlay
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let voice_data = {
        let state = hotkeys::engine_state_lock();
        serde_json::json!({
            "assignments": state.assignments,
            "activeProfile": state.active_profile,
            "theme": cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark"),
            "voiceMicId": cfg.get("voiceMicId").and_then(|v| v.as_str()).unwrap_or(""),
        })
    };
    log::info!("[Keyfire] show_voice_overlay: emitting overlay-voice-data");
    let _ = overlay.emit("overlay-voice-data", voice_data);

    // Brief pause so the frontend can commit React state resets (voiceContinuous=false etc.)
    // before the window becomes visible and clickable. Imperceptible — window is hidden.
    std::thread::sleep(std::time::Duration::from_millis(30));

    log::info!("[Keyfire] show_voice_overlay: showing window");
    let voice_open_now = StdInstant::now();
    *overlay_show_time().lock().unwrap() = Some(voice_open_now);
    *voice_overlay_open_time().lock().unwrap() = Some(voice_open_now);
    let _ = overlay.show();
    let _ = overlay.set_focus();
    log::info!("[Keyfire] show_voice_overlay: DONE");
}

fn hide_overlay(app: &tauri::AppHandle) {
    hotkeys::SEARCH_OVERLAY_HWND.store(0, AtomicOrdering::SeqCst);
    hotkeys::clear_overlay_opened_flag();
    hotkeys::clear_voice_active();
    VOICE_CONTINUOUS.store(false, AtomicOrdering::SeqCst);
    *voice_overlay_open_time().lock().unwrap() = None;
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
    }
}

// ── Clipboard overlay show/hide ──────────────────────────────────────────

static CLIPBOARD_OVERLAY_TARGET: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(0);

#[cfg(not(windows))]
fn show_clipboard_overlay(app: &tauri::AppHandle) {
    let _ = app;
    log::warn!("[stub] clipboard popup is not available on this platform yet");
}

#[cfg(windows)]
fn show_clipboard_overlay(app: &tauri::AppHandle) {
    // Wake a suspended webview BEFORE any emit/show — see webview_mem.rs invariant.
    webview_mem::resume_for_show(app, "clipboardoverlay");
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let target = unsafe { GetForegroundWindow() as isize };
    CLIPBOARD_OVERLAY_TARGET.store(target, std::sync::atomic::Ordering::SeqCst);

    let win = match app.get_webview_window("clipboardoverlay") {
        Some(w) => w,
        None => return,
    };

    // Arm keystroke routing IMMEDIATELY — before the (potentially slow)
    // history fetch below. From the VISIBLE store onward the LL hook
    // suppresses typed keys from the still-focused target app and the
    // processor routes them to the popup's search, so characters typed in
    // the first moments after the hotkey can never leak into the user's
    // document. The reset emit goes first so those routed keys land in a
    // cleared search box; ordering is guaranteed because this function and
    // the key routing both run on the processor thread.
    use tauri::Emitter;
    let _ = win.emit("clipboard-overlay-reset", serde_json::Value::Null);
    if let Ok(hwnd) = win.hwnd() {
        crate::hotkeys::CLIPBOARD_OVERLAY_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
    }
    crate::hotkeys::CLIPBOARD_OVERLAY_VISIBLE.store(true, std::sync::atomic::Ordering::SeqCst);

    // Position: center of active monitor, 1/3 from top (same pattern as search overlay)
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let (wa_left, wa_top, wa_right, wa_bottom, scale) = unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let s = monitor_scale_factor(hmon);
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom, s)
        } else {
            (0, 0, 1920, 1080, s)
        }
    };

    // 730px panel + 12px shadow breathing room each side (24px total).
    // Physical units throughout to dodge the hidden-window scale_factor
    // race that clipped 4K@225% users in v0.5.0 (see monitor_scale_factor).
    let win_w_logical = 754.0_f64;
    let win_h_logical = 500.0_f64;
    let phys_w_unclamped = (win_w_logical * scale).round() as i32;
    let phys_h_unclamped = (win_h_logical * scale).round() as i32;

    // High-scaling clamp: at 200%+ on smaller panels (e.g. 1080p laptop at 250%)
    // the unclamped popup would overflow the work area. Cap to (work-area minus
    // 32px margin) on each axis, with sensible floors. Popup body is internally
    // scrollable both axes so capping is safe. Width clamp added v0.5.3 after a
    // 4K TV tester at 225% reported left/right clipping.
    let wa_w = wa_right - wa_left;
    let wa_h = wa_bottom - wa_top;
    let max_w = (wa_w - 32).max(400);
    let max_h = (wa_h - 32).max(200);
    let phys_w = phys_w_unclamped.min(max_w);
    let phys_h = phys_h_unclamped.min(max_h);

    let ideal_y = wa_top + wa_h / 3;
    let max_y = wa_bottom - phys_h - 16;
    let mut phys_y = ideal_y.min(max_y).max(wa_top + 16);

    let mut phys_x = wa_left + (wa_w - phys_w) / 2;

    // User-dragged position override (grip drag in the popup header). The
    // fraction spans the work area minus the (already clamped) window size,
    // so the result is always fully on-screen.
    if let Some((fx, fy)) = saved_overlay_frac("clipboard") {
        let span_x = (wa_w - phys_w).max(0) as f64;
        let span_y = (wa_h - phys_h).max(0) as f64;
        phys_x = wa_left + (fx * span_x).round() as i32;
        phys_y = wa_top + (fy * span_y).round() as i32;
    }

    // Fetch history + theme OFF the processor thread. get_history blocks on
    // the clipboard writer thread (up to 500 rows AES-decrypted, possibly
    // queued behind a large capture write) and load_config reads from disk;
    // doing both synchronously before the show was the "popup takes a second
    // or two to appear" delay — and it stalled ALL hotkey/expansion
    // processing for the duration. The popup now shows immediately with its
    // previous list and refreshes when the payload lands. Search state is
    // safe: the frontend resets on 'clipboard-overlay-reset' (already sent
    // above), NOT on the data event, so keys typed while the fetch runs are
    // kept.
    let win_data = win.clone();
    std::thread::spawn(move || {
        let history = clipboard::get_history(1, 500, None, None, None, None, false);
        let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
        let theme = cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark");
        let mut payload = history;
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("theme".to_string(), serde_json::Value::String(theme.to_string()));
        }
        if let Err(e) = win_data.emit("clipboard-overlay-data", payload) {
            log::warn!("[Keyfire] clipboard overlay data emit failed: {}", e);
        }
    });

    // Raw Win32 show — bypass Tauri's win.show() which calls ShowWindow(SW_SHOW)
    // and *tries* to activate. WS_EX_NOACTIVATE *should* prevent activation but
    // Tauri/WebView2 also calls SetFocus on the inner Chromium webview after
    // show, which races the activation and can briefly steal focus from the
    // target thread. Transient inline editors (emClient calendar drag-out item,
    // Outlook subject-in-place) listen for WM_KILLFOCUS and destroy themselves
    // on any focus blip — even one too short to see. SetWindowPos with
    // SWP_NOACTIVATE + SWP_SHOWWINDOW shows the window WITHOUT generating any
    // activation event, position + size set in the same atomic call.
    if let Ok(hwnd) = win.hwnd() {
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST};
            const SWP_NOACTIVATE: u32 = 0x0010;
            const SWP_SHOWWINDOW: u32 = 0x0040;
            const SWP_NOMOVE: u32 = 0x0002;
            const SWP_NOSIZE: u32 = 0x0001;
            SetWindowPos(
                hwnd.0 as _,
                HWND_TOPMOST,
                phys_x,
                phys_y,
                phys_w,
                phys_h,
                SWP_NOACTIVATE | SWP_SHOWWINDOW,
            );
            // Force topmost re-ordering AFTER the initial show. HWND_TOPMOST +
            // SWP_NOACTIVATE on a first-show call places the window in the
            // topmost tier but doesn't reliably reorder *within* that tier
            // when another topmost window (Trigr's fill-in) is already on
            // screen. Result: popup was rendered but visually behind the
            // fill-in, matching the "nothing happens" bug. Second call with
            // NOMOVE|NOSIZE|NOACTIVATE is position-preserving and does the
            // reorder cleanly. No-op when no other topmost is present.
            SetWindowPos(
                hwnd.0 as _,
                HWND_TOPMOST,
                0, 0, 0, 0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

}

#[cfg(not(windows))]
fn hide_clipboard_overlay(app: &tauri::AppHandle) {
    crate::hotkeys::CLIPBOARD_OVERLAY_VISIBLE.store(false, std::sync::atomic::Ordering::SeqCst);
    crate::hotkeys::CLIPBOARD_OVERLAY_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    crate::hotkeys::CLIPBOARD_OVERLAY_FOR_FILLIN.store(false, std::sync::atomic::Ordering::SeqCst);
    if let Some(w) = app.get_webview_window("clipboardoverlay") {
        let _ = w.hide();
    }
}

#[cfg(windows)]
fn hide_clipboard_overlay(app: &tauri::AppHandle) {
    crate::hotkeys::CLIPBOARD_OVERLAY_VISIBLE.store(false, std::sync::atomic::Ordering::SeqCst);
    crate::hotkeys::CLIPBOARD_OVERLAY_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    // Reset fill-in mode so the next Ctrl+Shift+V from a normal target app
    // takes the standard NOACTIVATE + LL-hook-routed path. Fill-in mode is
    // per-show, never sticky across invocations.
    let was_fillin_mode = crate::hotkeys::CLIPBOARD_OVERLAY_FOR_FILLIN
        .swap(false, std::sync::atomic::Ordering::SeqCst);
    // Raw Win32 hide — symmetric with the raw SetWindowPos show. Tauri's
    // win.hide() was no-opping because we bypassed Tauri's internal visible
    // state when we showed via SetWindowPos; the runtime saw the window as
    // already "hidden" and skipped ShowWindow(SW_HIDE). Going raw both ways
    // keeps the actual window state in lockstep regardless of what Tauri's
    // cached visibility flag thinks.
    if let Some(win) = app.get_webview_window("clipboardoverlay") {
        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
                ShowWindow(hwnd.0 as _, SW_HIDE);
            }
        } else {
            let _ = win.hide();
        }
    }
    // Fill-in mode restore: hand focus back to the fill-in window so the user
    // can keep typing after the popup closes. Normal mode uses NOACTIVATE, so
    // there's nothing to restore — the target never lost foreground.
    if was_fillin_mode {
        let fillin_hwnd = crate::hotkeys::FILLIN_HWND
            .load(std::sync::atomic::Ordering::SeqCst);
        if fillin_hwnd != 0 {
            actions::set_foreground_robust(fillin_hwnd);
        }
    }
}

fn restore_overlay_target() {
    let hwnd = OVERLAY_TARGET_HWND.load(AtomicOrdering::Relaxed);
    if hwnd != 0 {
        actions::set_foreground_robust(hwnd);
    }
}

#[tauri::command]
fn close_overlay(app: tauri::AppHandle) {
    hide_overlay(&app);
    restore_overlay_target();
}

// ── Radial menu overlay show/hide ──────────────────────────────────────────

static RADIAL_MENU_TARGET_HWND: AtomicIsize = AtomicIsize::new(0);

static RADIAL_MENU_SHOW_TIME: std::sync::OnceLock<StdMutex<Option<StdInstant>>> =
    std::sync::OnceLock::new();

fn radial_menu_show_time() -> &'static StdMutex<Option<StdInstant>> {
    RADIAL_MENU_SHOW_TIME.get_or_init(|| StdMutex::new(None))
}

#[cfg(not(windows))]
fn show_radial_menu(app: &tauri::AppHandle) {
    let _ = app;
    log::warn!("[stub] radial menu is not available on this platform yet");
}

#[cfg(windows)]
fn show_radial_menu(app: &tauri::AppHandle) {
    // Wake a suspended webview BEFORE any emit/show — see webview_mem.rs invariant.
    webview_mem::resume_for_show(app, "radialmenu");
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetCursorPos};
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONEAREST};

    // Force an immediate foreground/profile check so the radial menu
    // uses the correct profile even if the 1500ms poll hasn't fired yet.
    foreground::force_check(app);

    let target = unsafe { GetForegroundWindow() as isize };
    RADIAL_MENU_TARGET_HWND.store(target, std::sync::atomic::Ordering::SeqCst);

    let win = match app.get_webview_window("radialmenu") {
        Some(w) => w,
        None => return,
    };

    // Position: centre 525x525 window on cursor. Physical units throughout
    // to dodge the hidden-window scale_factor race (see monitor_scale_factor).
    let (cx, cy) = unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        (pt.x, pt.y)
    };

    let scale = unsafe {
        let pt = POINT { x: cx, y: cy };
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        monitor_scale_factor(hmon)
    };

    let win_size_logical = 525.0_f64;
    let phys_size = (win_size_logical * scale).round() as i32;
    // Always centre on cursor — no clamping to work area.
    // Items near screen edges may be clipped, but the cursor stays
    // at the wheel centre which preserves muscle memory.
    let phys_x = cx - phys_size / 2;
    let phys_y = cy - phys_size / 2;
    let _ = win.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));
    let _ = win.set_size(tauri::PhysicalSize::new(phys_size as u32, phys_size as u32));

    // Build payload — shared with get_radial_menu_data (self-heal pull).
    let payload = build_radial_menu_data();
    use tauri::Emitter;
    let _ = win.emit("radial-menu-data", payload);

    *radial_menu_show_time().lock().unwrap() = Some(StdInstant::now());
    let _ = win.show();
    let _ = win.set_focus();
}

fn hide_radial_menu(app: &tauri::AppHandle) {
    hotkeys::clear_radial_menu_open();
    if let Some(win) = app.get_webview_window("radialmenu") {
        let _ = win.hide();
    }
}

fn restore_radial_menu_target() {
    let hwnd = RADIAL_MENU_TARGET_HWND.load(std::sync::atomic::Ordering::SeqCst);
    if hwnd != 0 {
        actions::set_foreground_robust(hwnd);
    }
}

#[tauri::command]
fn set_radial_menu_hotkey(combo: String) -> Value {
    hotkeys::set_radial_menu_hotkey(&combo);
    serde_json::json!({ "ok": true })
}

#[tauri::command]
fn clear_radial_menu_hotkey() {
    hotkeys::clear_radial_menu_hotkey();
}

/// Hold-to-select is resolved from config at wheel-open time (see
/// show_radial_menu, which passes `holdToSelect` + `holdKey` to the overlay
/// and the overlay's own web layer detects the launch-key release while it
/// holds focus). No engine state to update — this command exists only so the
/// frontend's existing call site keeps working; the persisted config is the
/// source of truth.
#[tauri::command]
fn set_radial_hold_to_select(_enabled: bool) {}

#[tauri::command]
fn close_radial_menu(app: tauri::AppHandle) {
    hide_radial_menu(&app);
    restore_radial_menu_target();
}

#[tauri::command]
fn radial_menu_resize(width: f64, height: f64, app: tauri::AppHandle) {
    let w = width.max(200.0).min(525.0);
    let h = height.max(200.0).min(525.0);
    if let Some(win) = app.get_webview_window("radialmenu") {
        let _ = win.set_size(tauri::LogicalSize::new(w, h));
    }
}

/// Called by the frontend when the user clicks the voice pill to toggle continuous mode.
/// on=true: stay-alive after each command fires; on=false: close overlay immediately.
#[tauri::command]
fn set_voice_continuous(on: bool, app: tauri::AppHandle) {
    VOICE_CONTINUOUS.store(on, AtomicOrdering::SeqCst);
    if !on {
        hide_overlay(&app);
        restore_overlay_target();
    }
}

#[tauri::command]
fn overlay_resize(height: f64, app: tauri::AppHandle) {
    // Flip mode: the window is fixed at full height for the whole session and
    // the list grows inside the DOM (panel bottom-pinned) — see show_overlay.
    // Content resizes are ignored; resizing the top edge per keystroke is
    // what made the bar jitter.
    if OVERLAY_FLIP_UP.load(AtomicOrdering::SeqCst) {
        return;
    }
    let h = height.max(60.0).min(OVERLAY_MAX_H);
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_size(tauri::LogicalSize::new(620.0, h));
    }
}

/// Persist an overlay's current position after a user drag. `name` is the
/// JS-side overlay name ("search" | "clipboard"). Position is stored as a
/// fraction of the current monitor's work area minus the window size — see
/// saved_overlay_frac for why.
#[cfg(windows)]
#[tauri::command]
fn save_overlay_position(name: String, app: tauri::AppHandle) {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let Some(label) = overlay_position_label(&name) else { return };
    let Some(win) = app.get_webview_window(label) else { return };
    let (Ok(pos), Ok(size), Ok(hwnd)) = (win.outer_position(), win.outer_size(), win.hwnd())
    else {
        return;
    };
    let (wl, wt, wr, wb, scale) = unsafe {
        let hmon = MonitorFromWindow(hwnd.0 as _, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        let s = monitor_scale_factor(hmon);
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom, s)
        } else {
            return;
        }
    };
    // The search bar resizes with its results, so raw window geometry is the
    // WRONG thing to persist: a drag taken while the list is open would save
    // the expanded window's top-left — in flip mode that's ~340px above the
    // visible bar — and every restore would spawn the bar somewhere else.
    // Normalise to the bar's base geometry: the y a fresh 103-logical-tall
    // normal-mode window would need for the input row to land exactly where
    // it is now (flipped: input sits at window_bottom - 82 logical, which is
    // that window's top), and the base height for the span so the fraction
    // round-trips exactly. The clipboard popup doesn't resize — raw
    // geometry is already right there.
    let (eff_y, eff_h) = if name == "search" {
        let base_h = (103.0 * scale).round() as i32;
        let y = if OVERLAY_FLIP_UP.load(AtomicOrdering::SeqCst) {
            pos.y + size.height as i32 - (82.0 * scale).round() as i32
        } else {
            pos.y
        };
        (y, base_h)
    } else {
        (pos.y, size.height as i32)
    };
    let span_x = ((wr - wl) - size.width as i32).max(1) as f64;
    let span_y = ((wb - wt) - eff_h).max(1) as f64;
    let fx = ((pos.x - wl) as f64 / span_x).clamp(0.0, 1.0);
    let fy = ((eff_y - wt) as f64 / span_y).clamp(0.0, 1.0);
    let Some(mut val) = config::load_local_settings_json_strict() else { return; };
    if let Some(obj) = val.as_object_mut() {
        let positions = obj
            .entry("overlayPositions".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(pobj) = positions.as_object_mut() {
            pobj.insert(name.clone(), serde_json::json!({ "xFrac": fx, "yFrac": fy }));
        }
        config::save_local_settings_json(&val);
        log::info!(
            "[Keyfire] Overlay position saved: {} xFrac={:.3} yFrac={:.3}",
            name, fx, fy
        );
    }
}

#[cfg(not(windows))]
#[tauri::command]
fn save_overlay_position(name: String, app: tauri::AppHandle) {
    let _ = (name, app);
}

/// Clear a saved overlay position — the overlay returns to its default spot
/// (centred, one-third from the top of the active monitor's work area). If
/// the overlay is currently visible it snaps back immediately; the next show
/// recomputes the exact default either way.
#[tauri::command]
fn reset_overlay_position(name: String, app: tauri::AppHandle) {
    let Some(mut val) = config::load_local_settings_json_strict() else { return; };
    if let Some(positions) = val.get_mut("overlayPositions").and_then(|v| v.as_object_mut()) {
        if positions.remove(&name).is_some() {
            config::save_local_settings_json(&val);
        }
    }
    log::info!("[Keyfire] Overlay position reset: {}", name);
    #[cfg(windows)]
    {
        let Some(label) = overlay_position_label(&name) else { return };
        if let Some(win) = app.get_webview_window(label) {
            if win.is_visible().unwrap_or(false) {
                apply_default_overlay_position(&win);
                // A flipped search session must also un-flip, or the bar
                // (pinned to the bottom of the fixed full-height window)
                // lands ~350px below the default spot until the next open.
                // Order matters: clear the flag BEFORE the emit so the JS
                // measure effect's resizeOverlay call isn't ignored.
                if name == "search" && OVERLAY_FLIP_UP.swap(false, AtomicOrdering::SeqCst) {
                    use tauri::Emitter;
                    let _ = win.emit("overlay-flip", false);
                }
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = app;
    }
}

/// Move an overlay window back to the shared default spot (centred, 1/3 from
/// the top of the work area of whichever monitor it's currently on).
#[cfg(windows)]
fn apply_default_overlay_position(win: &tauri::WebviewWindow) {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let (Ok(size), Ok(hwnd)) = (win.outer_size(), win.hwnd()) else { return };
    let (wl, wt, wr, wb) = unsafe {
        let hmon = MonitorFromWindow(hwnd.0 as _, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom)
        } else {
            return;
        }
    };
    let phys_x = wl + ((wr - wl) - size.width as i32) / 2;
    let ideal_y = wt + (wb - wt) / 3;
    let max_y = wb - size.height as i32 - 16;
    let phys_y = ideal_y.min(max_y).max(wt + 16);
    let _ = win.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));
}

/// Expand the voice overlay from its compact 72×72 pill to a wider error banner.
/// Re-centres the window for the new width so it stays bottom-centre on screen.
#[tauri::command]
fn voice_overlay_error_expand(app: tauri::AppHandle) {
    let new_w = 340.0_f64;
    let new_h = 72.0_f64;
    if let Some(overlay) = app.get_webview_window("overlay") {
        let scale = overlay.scale_factor().unwrap_or(1.0);
        // Current window is 72px wide, centred. Shift x left to re-centre for new width.
        if let Ok(pos) = overlay.outer_position() {
            let log_x = pos.x as f64 / scale;
            let log_y = pos.y as f64 / scale;
            let adj_x = (log_x - (new_w - 72.0) / 2.0).max(0.0);
            let _ = overlay.set_position(tauri::LogicalPosition::new(adj_x, log_y));
        }
        let _ = overlay.set_size(tauri::LogicalSize::new(new_w, new_h));
    }
}

/// Expand the voice overlay to fit a no-match examples banner (3 phrase rows).
/// Re-centres horizontally and grows upward so the pill stays bottom-centre.
#[tauri::command]
fn voice_overlay_examples_expand(app: tauri::AppHandle) {
    let new_w = 340.0_f64;
    let new_h = 168.0_f64;
    if let Some(overlay) = app.get_webview_window("overlay") {
        let scale = overlay.scale_factor().unwrap_or(1.0);
        if let Ok(pos) = overlay.outer_position() {
            let log_x = pos.x as f64 / scale;
            let log_y = pos.y as f64 / scale;
            let adj_x = (log_x - (new_w - 72.0) / 2.0).max(0.0);
            // Grow upward — keep the pill at the same bottom edge
            let adj_y = (log_y - (new_h - 72.0)).max(0.0);
            let _ = overlay.set_position(tauri::LogicalPosition::new(adj_x, adj_y));
        }
        let _ = overlay.set_size(tauri::LogicalSize::new(new_w, new_h));
    }
}

/// Shared dispatch logic for executing an item from any overlay (search, radial menu).
/// Runs on a background thread — caller must pass cloned values.
fn execute_item_impl(result: &Value, target_hwnd: isize, app: &tauri::AppHandle) {
    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("");

    // Entry log: the click → action chain previously had five silent failure
    // gates with no log output, making "clicked a segment, nothing happened"
    // reports undiagnosable. Every no-op gate below now warns.
    log::info!(
        "[Keyfire] Execute item: type=\"{}\" storageKey=\"{}\" label=\"{}\"",
        result_type,
        result.get("storageKey").and_then(|v| v.as_str()).unwrap_or("-"),
        result.get("label").and_then(|v| v.as_str()).unwrap_or("-")
    );

    match result_type {
        "assignment" | "quickaction" => {
            if let Some(storage_key) = result.get("storageKey").and_then(|v| v.as_str()) {
                // Re-assert foreground right before fire. restore_radial_menu_target
                // already ran before the 180ms head-start sleep in
                // execute_radial_menu_item, but focus can drift in that window
                // (esp. fullscreen / hardened targets). Mirrors the expansion +
                // autocorrect branches below. Without this, SendInput from
                // execute_action lands on whatever holds focus when the sleep
                // ends, which on fullscreen games means nothing reaches them.
                if target_hwnd != 0 {
                    actions::set_foreground_robust(target_hwnd);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                let state = hotkeys::engine_state_lock();
                if let Some(macro_val) = state.assignments.get(storage_key).cloned() {
                    drop(state);
                    actions::execute_action(&macro_val, false, target_hwnd, false, Some(storage_key), app);
                    let label = macro_val.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    analytics::log_assignment_fired(storage_key, label, &macro_val);
                } else {
                    log::warn!(
                        "[Keyfire] Execute item no-op: storageKey \"{}\" not found in engine state (source renamed/deleted, or assignments not synced)",
                        storage_key
                    );
                }
            } else {
                log::warn!("[Keyfire] Execute item no-op: {} payload has no storageKey", result_type);
            }
        }
        "expansion" => {
            // Route through the shared expansion fire path so fill-in fields
            // ({fillIn:...}), variant pickers, and HTML/rich-text forwarding all
            // behave identically to live-typed and macro-fired expansions.
            // Previously this branch pasted the raw stored text directly, which
            // skipped the fill-in prompt and the variant chooser (and dropped
            // HTML). The trigger is embedded in the storage key as
            // GLOBAL::EXPANSION::<trigger>.
            if let Some(trigger) = result
                .get("storageKey")
                .and_then(|v| v.as_str())
                .and_then(|k| k.strip_prefix("GLOBAL::EXPANSION::"))
            {
                // Ensure the target app is foreground before the shared path
                // captures GetForegroundWindow() for its paste / fill-in flow
                // (the overlay had focus until execute_search_result hid it).
                if target_hwnd != 0 {
                    actions::set_foreground_robust(target_hwnd);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                // fire_expansion_by_trigger handles image / variant / fill-in /
                // plain text and logs analytics in each sub-path itself.
                expansions::fire_expansion_by_trigger(trigger);
            } else {
                log::warn!("[Keyfire] Execute item no-op: expansion payload missing GLOBAL::EXPANSION:: storageKey");
            }
        }
        "autocorrect" => {
            // Autocorrect entries are simple replacements (no fill-in, no
            // variants) keyed under a different namespace, so keep the direct
            // token-resolve + clipboard-paste path here.
            if let Some(raw_text) = result.get("text").and_then(|v| v.as_str()) {
                // Resolve dynamic tokens ({date:...}, {time:...}, {clipboard}, {cursor}, etc.)
                let global_vars = expansions::get_global_variables();
                let empty_fillin: std::collections::HashMap<String, String> = std::collections::HashMap::new();
                let (resolved, cursor_back) = expansions::resolve_tokens(raw_text, &global_vars, &empty_fillin);

                let trigger = result.get("trigger").and_then(|v| v.as_str()).unwrap_or("");
                analytics::log_action("expansion", resolved.chars().filter(|c| *c != '\r').count() as u32, trigger, trigger);

                actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let _suppress = actions::SuppressionGuard::new();

                let held = actions::release_held_modifiers();
                if target_hwnd != 0 {
                    actions::set_foreground_robust(target_hwnd);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }

                // Use clipboard paste
                let prev = actions::read_clipboard_pub().unwrap_or_default();
                actions::write_clipboard_pub(&resolved);
                std::thread::sleep(std::time::Duration::from_millis(10));

                // Ctrl+V paste
                actions::send_vk_key_pub(0xA2, false); // LCtrl down
                actions::send_vk_key_pub(0x56, false); // V down
                actions::send_vk_key_pub(0x56, true);  // V up
                actions::send_vk_key_pub(0xA2, true);  // LCtrl up

                // Move cursor back if {cursor} was present
                if cursor_back > 0 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    for _ in 0..cursor_back {
                        actions::send_vk_key_pub(0x25, false); // VK_LEFT down
                        actions::send_vk_key_pub(0x25, true);  // VK_LEFT up
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }

                actions::restore_modifiers(&held);
                drop(_suppress); // SUPPRESS_SIMULATED = false (even on panic)

                std::thread::sleep(std::time::Duration::from_millis(50));
                actions::write_clipboard_pub(&prev);
                actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                    .store(false, std::sync::atomic::Ordering::Relaxed);
            } else {
                log::warn!("[Keyfire] Execute item no-op: autocorrect payload has no text field");
            }
        }
        "search_template" => {
            let url_template = result.get("url_template").and_then(|v| v.as_str()).unwrap_or("");
            let query = result.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let encode_query = result.get("encode_query").and_then(|v| v.as_bool()).unwrap_or(true);
            let label = result.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let trigger = result.get("trigger").and_then(|v| v.as_str()).unwrap_or("");

            if !url_template.is_empty() && !query.is_empty() {
                let encoded_query = if encode_query {
                    percent_encode_query(query)
                } else {
                    query.to_string()
                };
                let final_url = url_template.replace("{query}", &encoded_query);
                let _ = opener::open(&final_url);
                analytics::log_action("search_template", 0, trigger, label);
            } else {
                log::warn!("[Keyfire] Execute item no-op: search_template missing url_template or query");
            }
        }
        other => {
            log::warn!("[Keyfire] Execute item no-op: unknown type \"{}\"", other);
        }
    }
}

#[tauri::command]
fn execute_search_result(result: Value, app: tauri::AppHandle) {
    let is_continuous = VOICE_CONTINUOUS.load(AtomicOrdering::SeqCst);

    if !is_continuous {
        hide_overlay(&app);
    }
    // Always restore focus to target app so actions execute in the right window.
    // In continuous mode the overlay stays visible (always_on_top) above the target.
    restore_overlay_target();

    let target_hwnd = OVERLAY_TARGET_HWND.load(AtomicOrdering::Relaxed);
    let app_cont = if is_continuous { Some(app.clone()) } else { None };

    std::thread::spawn(move || {
        // Wait for focus transfer to target app
        std::thread::sleep(std::time::Duration::from_millis(180));
        execute_item_impl(&result, target_hwnd, &app);

        // Continuous voice mode: signal frontend to restart listening after the action executed.
        if let Some(app2) = app_cont {
            let _ = app2.emit("voice-continuous-restart", ());
        }
    });
}

#[tauri::command]
fn execute_radial_menu_item(result: Value, app: tauri::AppHandle) {
    hide_radial_menu(&app);
    restore_radial_menu_target();

    let target_hwnd = RADIAL_MENU_TARGET_HWND.load(std::sync::atomic::Ordering::SeqCst);
    if target_hwnd == 0 {
        // Gate 5: text-output actions (Type Text, Press Key, paste) silently
        // no-op without a target window. Open URL / Open App still work.
        log::warn!("[Keyfire] Radial fire with no target window captured — text-output actions will no-op");
    }

    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(180));
        execute_item_impl(&result, target_hwnd, &app);
    });
}

/// Percent-encode a query string for URL substitution.
/// Encodes everything except unreserved characters (RFC 3986: A-Z a-z 0-9 - _ . ~).
fn percent_encode_query(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            b' ' => {
                // Use + for spaces (standard query encoding)
                encoded.push('+');
            }
            _ => {
                encoded.push('%');
                encoded.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
                encoded.push(char::from(b"0123456789ABCDEF"[(byte & 0x0F) as usize]));
            }
        }
    }
    encoded
}

/// Engine-side Quick Search settings. Persistence is the frontend's job
/// (`handleUpdateSearchSettings` saves the patch to config); this only
/// registers the hotkey. `searchOverlayEnabled: false` clears the hotkey so
/// the combo passes through to the focused app — the frontend always sends
/// `searchOverlayEnabled` and `searchOverlayHotkey` together so re-enabling
/// restores the user's combo without a restart.
#[tauri::command]
fn update_search_settings(settings: Value) {
    let enabled = settings
        .get("searchOverlayEnabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !enabled {
        hotkeys::clear_overlay_hotkey();
        return;
    }
    if let Some(hotkey) = settings.get("searchOverlayHotkey").and_then(|v| v.as_str()) {
        if hotkey.is_empty() {
            hotkeys::clear_overlay_hotkey();
        } else {
            hotkeys::set_overlay_hotkey(hotkey);
        }
    }
}

// ── Onboarding ─────────────────────────────────────────────────────────────

#[tauri::command]
fn reset_onboarding() -> bool {
    let existing = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let mut merged = existing.clone();
    if let Some(obj) = merged.as_object_mut() {
        obj.insert("onboarding_complete".to_string(), Value::Bool(false));
    }
    config::save_config(&merged)
}

// ── Analytics ───────────────────────────────────────────────────────────────

#[tauri::command]
fn get_analytics() -> Value {
    analytics::get_stats()
}

#[tauri::command]
fn reset_analytics() -> bool {
    analytics::reset_stats()
}

// ── Advanced analytics (Pro) ────────────────────────────────────────────────
// The headline stats (get_analytics), type breakdown and expansion counts stay
// free. The richer views below are Pro: each returns an empty payload for
// non-Pro so a direct IPC call yields nothing, while the UI keeps them behind
// its own isPro display gates. All gate on is_pro() so they follow Paddle.

/// Strict YYYY-MM-DD shape check — window dates are inlined into SQL by
/// analytics::Window, so nothing else may pass.
fn is_valid_iso_date(s: &str) -> bool {
    s.len() == 10
        && s.bytes().enumerate().all(|(i, b)| match i {
            4 | 7 => b == b'-',
            _ => b.is_ascii_digit(),
        })
        && chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// Build a query window from IPC params. A valid custom from/to range wins;
/// otherwise days (0/None = all time). Reversed ranges are swapped.
fn window_from_params(days: Option<u32>, from: Option<String>, to: Option<String>) -> analytics::Window {
    if let (Some(f), Some(t)) = (from.as_deref(), to.as_deref()) {
        if is_valid_iso_date(f) && is_valid_iso_date(t) {
            let (f, t) = if f <= t { (f, t) } else { (t, f) };
            return analytics::Window::Range(f.to_string(), t.to_string());
        }
    }
    match days.unwrap_or(0) {
        0 => analytics::Window::All,
        d => analytics::Window::Days(d),
    }
}

#[tauri::command]
fn get_daily_chart(days: Option<u32>, from: Option<String>, to: Option<String>) -> Value {
    if !licence::is_pro() {
        return serde_json::json!([]);
    }
    analytics::get_daily_chart(window_from_params(days, from, to))
}

#[tauri::command]
fn get_assignment_breakdown(days: Option<u32>, from: Option<String>, to: Option<String>) -> Value {
    if !licence::is_pro() {
        return serde_json::json!([]);
    }
    analytics::get_assignment_breakdown(window_from_params(days, from, to))
}

#[tauri::command]
fn get_type_breakdown(days: Option<u32>, from: Option<String>, to: Option<String>) -> Value {
    analytics::get_type_breakdown(window_from_params(days, from, to))
}

#[tauri::command]
fn get_hourly_heatmap(days: Option<u32>, from: Option<String>, to: Option<String>) -> Value {
    if !licence::is_pro() {
        return serde_json::json!([]);
    }
    analytics::get_hourly_heatmap(window_from_params(Some(days.unwrap_or(7)), from, to))
}

#[tauri::command]
fn get_top_apps(days: Option<u32>, from: Option<String>, to: Option<String>) -> Value {
    if !licence::is_pro() {
        return serde_json::json!([]);
    }
    analytics::get_top_apps(window_from_params(days, from, to))
}

#[tauri::command]
fn get_expansion_efficiency(days: Option<u32>, from: Option<String>, to: Option<String>) -> Value {
    if !licence::is_pro() {
        return serde_json::json!([]);
    }
    analytics::get_expansion_efficiency(window_from_params(days, from, to))
}

#[tauri::command]
fn get_expansion_counts() -> Value {
    analytics::get_expansion_counts()
}

#[tauri::command]
fn get_streaks() -> Value {
    if !licence::is_pro() {
        return serde_json::json!({});
    }
    analytics::get_streaks()
}

/// Sanitize a UI-supplied preset period to the values the dropdown offers.
fn sanitize_export_days(days: Option<u32>) -> u32 {
    match days.unwrap_or(0) {
        d @ (1 | 7 | 14 | 30) => d,
        _ => 0, // anything else = all time
    }
}

/// Build an export window: valid custom from/to wins, else preset days.
fn export_window(days: Option<u32>, from: Option<String>, to: Option<String>) -> analytics::Window {
    let win = window_from_params(Some(sanitize_export_days(days)), from, to);
    match win {
        analytics::Window::Days(d) => match d {
            1 | 7 | 14 | 30 => analytics::Window::Days(d),
            _ => analytics::Window::All,
        },
        other => other,
    }
}

/// Filename fragment for an export window ("" for all time).
fn export_period_slug(win: &analytics::Window) -> String {
    match win {
        analytics::Window::All => String::new(),
        analytics::Window::Days(1) => "-Today".to_string(),
        analytics::Window::Days(n) => format!("-{}-Days", n),
        analytics::Window::Range(f, t) => format!("-{}-to-{}", f, t),
    }
}

/// Export the analytics dataset as a multi-sheet XLSX workbook, optionally
/// scoped to a preset period (days) or a custom from/to date range.
/// Native Save As dialog; returns { ok, path } / { ok: false, cancelled } /
/// { ok: false, error }. Async so blocking_save_file runs off the main thread.
#[tauri::command]
async fn export_analytics_xlsx(
    app: tauri::AppHandle,
    days: Option<u32>,
    from: Option<String>,
    to: Option<String>,
) -> Value {
    if !licence::is_pro() {
        return serde_json::json!({ "ok": false, "error": "PRO_REQUIRED" });
    }
    use tauri_plugin_dialog::DialogExt;

    let win = export_window(days, from, to);
    let default_name = format!(
        "Keyfire-Analytics{}-{}.xlsx",
        export_period_slug(&win),
        chrono::Local::now().format("%Y-%m-%d")
    );
    let downloads = app.path().download_dir().unwrap_or_default();
    let file_path = app
        .dialog()
        .file()
        .set_title("Export Analytics Workbook")
        .set_file_name(&default_name)
        .add_filter("Excel Workbook", &["xlsx"])
        .set_directory(&downloads)
        .blocking_save_file();

    let file_path = match file_path {
        Some(p) => match p.into_path() {
            Ok(pb) => pb,
            Err(_) => return serde_json::json!({ "ok": false, "error": "Invalid path" }),
        },
        None => return serde_json::json!({ "ok": false, "cancelled": true }),
    };

    match analytics::export_xlsx(file_path.clone(), win) {
        Ok(()) => {
            log::info!("[Keyfire] Analytics workbook exported to: {}", file_path.display());
            serde_json::json!({ "ok": true, "path": file_path.to_string_lossy() })
        }
        Err(e) => {
            log::error!("[Keyfire] Analytics XLSX export failed: {}", e);
            serde_json::json!({ "ok": false, "error": e })
        }
    }
}

// ── Analytics PDF report export ─────────────────────────────────────────────
// Flow: export_analytics_pdf (Pro gate + Save As dialog) creates a hidden
// "report" window on ?report=1. The page fetches its data, renders, then
// invokes analytics_report_ready, which drives WebView2 PrintToPdf against
// the live DOM (A4, backgrounds on). The completion handler destroys the
// window and emits analytics-pdf-done to the main window. The window is
// created fresh per export and destroyed after — never pre-created, never
// suspended by webview_mem.

/// In-flight PDF export: (generation, output path). Some = a report window is
/// mid-flight; also consumed as the handshake between the two commands. The
/// generation counter stops a stale watchdog from killing a newer export.
static PDF_EXPORT_STATE: std::sync::Mutex<Option<(u64, std::path::PathBuf)>> =
    std::sync::Mutex::new(None);
static PDF_EXPORT_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn finish_pdf_export(app: &tauri::AppHandle, ok: bool, path: Option<&str>, error: Option<&str>) {
    if let Some(w) = app.get_webview_window("report") {
        let _ = w.destroy();
    }
    let _ = app.emit(
        "analytics-pdf-done",
        serde_json::json!({ "ok": ok, "path": path, "error": error }),
    );
}

#[tauri::command]
async fn export_analytics_pdf(
    app: tauri::AppHandle,
    days: Option<u32>,
    from: Option<String>,
    to: Option<String>,
) -> Value {
    if !licence::is_pro() {
        return serde_json::json!({ "ok": false, "error": "PRO_REQUIRED" });
    }
    #[cfg(not(windows))]
    {
        let _ = (&app, days, from, to);
        serde_json::json!({ "ok": false, "error": "PDF export is not available on this platform yet" })
    }
    #[cfg(windows)]
    {
        if PDF_EXPORT_STATE.lock().unwrap().is_some() {
            return serde_json::json!({ "ok": false, "error": "A report export is already in progress" });
        }

        use tauri_plugin_dialog::DialogExt;
        let win = export_window(days, from, to);
        let default_name = format!(
            "Keyfire-Analytics-Report{}-{}.pdf",
            export_period_slug(&win),
            chrono::Local::now().format("%Y-%m-%d")
        );
        let downloads = app.path().download_dir().unwrap_or_default();
        let file_path = app
            .dialog()
            .file()
            .set_title("Export Analytics Report")
            .set_file_name(&default_name)
            .add_filter("PDF Report", &["pdf"])
            .set_directory(&downloads)
            .blocking_save_file();

        let file_path = match file_path {
            Some(p) => match p.into_path() {
                Ok(pb) => pb,
                Err(_) => return serde_json::json!({ "ok": false, "error": "Invalid path" }),
            },
            None => return serde_json::json!({ "ok": false, "cancelled": true }),
        };

        let gen = PDF_EXPORT_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        *PDF_EXPORT_STATE.lock().unwrap() = Some((gen, file_path));

        // Fresh window per export. Destroy any stale one first (e.g. a prior
        // run whose watchdog fired while the page was still loading).
        if let Some(w) = app.get_webview_window("report") {
            let _ = w.destroy();
        }
        let url_query = match &win {
            analytics::Window::All => "days=0".to_string(),
            analytics::Window::Days(d) => format!("days={}", d),
            analytics::Window::Range(f, t) => format!("from={}&to={}", f, t),
        };
        let url = tauri::WebviewUrl::App(format!("index.html?report=1&{}", url_query).into());
        let built = tauri::WebviewWindowBuilder::new(&app, "report", url)
            .additional_browser_args(WEBVIEW_BROWSER_ARGS)
            .title("Keyfire Report")
            .inner_size(940.0, 1240.0)
            .visible(false)
            .skip_taskbar(true)
            .build();
        if let Err(e) = built {
            *PDF_EXPORT_STATE.lock().unwrap() = None;
            return serde_json::json!({ "ok": false, "error": format!("Could not create report window: {}", e) });
        }

        // Watchdog: if the page never reports ready (JS error, blank data
        // path), fail the export instead of leaving the button spinning.
        let app2 = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let should_fail = {
                let mut guard = PDF_EXPORT_STATE.lock().unwrap();
                match guard.as_ref() {
                    Some((g, _)) if *g == gen => {
                        *guard = None;
                        true
                    }
                    _ => false,
                }
            };
            if should_fail {
                log::error!("[Keyfire] PDF export: report window never became ready (30s)");
                finish_pdf_export(&app2, false, None, Some("The report timed out while rendering"));
            }
        });

        serde_json::json!({ "ok": true, "started": true })
    }
}

/// Invoked by the report page once its data is fetched, fonts are loaded and
/// the DOM has painted. Drives PrintToPdf on the report webview.
#[tauri::command]
fn analytics_report_ready(app: tauri::AppHandle) {
    #[cfg(not(windows))]
    {
        let _ = app;
    }
    #[cfg(windows)]
    {
        let taken = { PDF_EXPORT_STATE.lock().unwrap().take() };
        let Some((_gen, path)) = taken else {
            // Watchdog already failed this export, or a stray ready ping.
            return;
        };
        let Some(report_win) = app.get_webview_window("report") else {
            finish_pdf_export(&app, false, None, Some("Report window disappeared"));
            return;
        };

        let app_for_com = app.clone();
        let path_str = path.to_string_lossy().to_string();
        let res = report_win.with_webview(move |webview| {
            let result = unsafe { print_report_to_pdf(&app_for_com, webview, &path_str) };
            if let Err(e) = result {
                log::error!("[Keyfire] PDF export: PrintToPdf setup failed: {}", e);
                finish_pdf_export(&app_for_com, false, None, Some("PDF printing is not available in this WebView2 runtime"));
            }
        });
        if let Err(e) = res {
            log::error!("[Keyfire] PDF export: with_webview failed: {}", e);
            finish_pdf_export(&app, false, None, Some("Could not access the report window"));
        }
    }
}

/// COM body of the PDF print. Runs on the main thread (with_webview closure).
/// A4 paper, no OS header/footer, zero margins (the page's CSS owns them),
/// backgrounds on so the dark theme prints as rendered.
#[cfg(windows)]
unsafe fn print_report_to_pdf(
    app: &tauri::AppHandle,
    webview: tauri::webview::PlatformWebview,
    path: &str,
) -> Result<(), String> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2Environment6, ICoreWebView2PrintSettings, ICoreWebView2_2, ICoreWebView2_7,
    };
    use webview2_com::PrintToPdfCompletedHandler;
    use windows_core::Interface;

    let core = webview
        .controller()
        .CoreWebView2()
        .map_err(|e| format!("CoreWebView2: {}", e))?;
    let core2: ICoreWebView2_2 = core.cast().map_err(|e| format!("ICoreWebView2_2: {}", e))?;
    let env = core2.Environment().map_err(|e| format!("Environment: {}", e))?;
    let env6: ICoreWebView2Environment6 = env
        .cast()
        .map_err(|e| format!("ICoreWebView2Environment6: {}", e))?;
    let settings: ICoreWebView2PrintSettings = env6
        .CreatePrintSettings()
        .map_err(|e| format!("CreatePrintSettings: {}", e))?;
    settings
        .SetShouldPrintBackgrounds(true)
        .map_err(|e| format!("SetShouldPrintBackgrounds: {}", e))?;
    settings
        .SetShouldPrintHeaderAndFooter(false)
        .map_err(|e| format!("SetShouldPrintHeaderAndFooter: {}", e))?;
    let _ = settings.SetMarginTop(0.0);
    let _ = settings.SetMarginBottom(0.0);
    let _ = settings.SetMarginLeft(0.0);
    let _ = settings.SetMarginRight(0.0);
    // A4 in inches — matches the page CSS's @page size.
    let _ = settings.SetPageWidth(8.27);
    let _ = settings.SetPageHeight(11.69);

    let core7: ICoreWebView2_7 = core.cast().map_err(|e| format!("ICoreWebView2_7: {}", e))?;

    let app2 = app.clone();
    let path_owned = path.to_string();
    let handler = PrintToPdfCompletedHandler::create(Box::new(move |error_code, is_successful| {
        let ok = error_code.is_ok() && is_successful;
        if ok {
            log::info!("[Keyfire] Analytics report exported to: {}", path_owned);
            finish_pdf_export(&app2, true, Some(&path_owned), None);
        } else {
            log::error!(
                "[Keyfire] PDF export: PrintToPdf completed with error={:?} successful={}",
                error_code,
                is_successful
            );
            finish_pdf_export(&app2, false, None, Some("Windows could not write the PDF file"));
        }
        Ok(())
    }));

    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    core7
        .PrintToPdf(
            windows_core::PCWSTR(wide.as_ptr()),
            &settings,
            &handler,
        )
        .map_err(|e| format!("PrintToPdf: {}", e))?;
    Ok(())
}

// ── Clipboard Manager ──────────────────────────────────────────────────────

#[tauri::command]
async fn get_clipboard_history(
    page: u32,
    per_page: u32,
    date_filter: Option<String>,
    app_filter: Option<String>,
    tag_filter: Option<String>,
    search: Option<String>,
    // Main UI passes true so starred items sort above pinned. Popup omits or
    // passes false so only pinned promote (starred items stay in the timeline).
    promote_starred: Option<bool>,
) -> Value {
    // get_history blocks on the clipboard writer thread (up to 500 rows
    // AES-decrypted, possibly queued behind a large capture write) — keep
    // that roundtrip off the main event loop.
    tauri::async_runtime::spawn_blocking(move || {
        clipboard::get_history(
            page, per_page, date_filter, app_filter, tag_filter, search,
            promote_starred.unwrap_or(false),
        )
    })
    .await
    .unwrap_or_else(|_| serde_json::json!({ "items": [] }))
}

#[tauri::command]
async fn get_theme() -> String {
    // Plain config read only. The load_config command is NOT a substitute
    // here: it carries boot-sequence side effects (timestamped backup, LKG
    // rewrite, snapshot_loaded for cross-device save-conflict detection)
    // that must stay exclusive to the main window's load path. Used by the
    // clipboard popup's self-heal pull.
    tauri::async_runtime::spawn_blocking(|| {
        config::load_config()
            .and_then(|c| c.get("theme").and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_else(|| "dark".to_string())
    })
    .await
    .unwrap_or_else(|_| "dark".to_string())
}

#[tauri::command]
fn paste_clipboard_item(id: i64, app: tauri::AppHandle) {
    let item = match clipboard::get_item_full(id) {
        Some(i) => i,
        None => return,
    };

    // Fill-in mode: emit the picked text back to FillInWindow.jsx and hide the
    // popup instead of running the Ctrl+V injection path. Ctrl+V into another
    // Trigr WebView2 is unreliable per [[feedback_webview2_input_injection]],
    // and the fill-in already knows which of its input fields has focus — an
    // event-driven insert via document.activeElement is both simpler and more
    // reliable. Images are dropped in this mode (fill-in inputs are text-only).
    if crate::hotkeys::CLIPBOARD_OVERLAY_FOR_FILLIN
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        if item.content_type == "text" {
            if let Some(text) = item.text_content {
                use tauri::Emitter;
                let _ = app.emit(
                    "fillin-insert-text",
                    serde_json::json!({ "text": text, "id": id }),
                );
                clipboard::increment_paste_count(id);
            }
        }
        hide_clipboard_overlay(&app);
        return;
    }

    // Read stored target HWND — captured when the overlay was shown, before focus was stolen
    let target_hwnd = CLIPBOARD_OVERLAY_TARGET.load(std::sync::atomic::Ordering::SeqCst);

    std::thread::spawn(move || {
        // Re-entrancy guard: drop instantly if another paste/copy op is in
        // flight. Without this, repeated Enter on the clipboard overlay (LL
        // hook key-repeat, or clicks during UI lag) spawns concurrent threads
        // whose read-prev/write-text/restore-prev interleave and flood the
        // clipboard. See actions::PasteOpGuard.
        let _paste_guard = match actions::PasteOpGuard::try_acquire() {
            Some(g) => g,
            None => {
                log::info!("[Keyfire] paste_clipboard_item skipped — concurrent paste/copy op in flight");
                return;
            }
        };

        // Counter is incremented after the paste path succeeds (set inside the match below).
        let mut pasted_ok = false;
        // Brief settle delay before paste — lets the overlay's win.hide()
        // commit fully so any in-flight Tauri events finish before Ctrl+V.
        std::thread::sleep(std::time::Duration::from_millis(30));

        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _suppress = actions::SuppressionGuard::new();

        let held = actions::release_held_modifiers();

        // NO focus restore — overlay is WS_EX_NOACTIVATE, target app held
        // foreground throughout. The previous `set_foreground_robust` call
        // restored the top-level HWND captured at show-time, NOT the focused
        // child control, so apps with modal dialogs (emClient calendar name
        // field, Slack composer, Notion sidebars) pasted into the wrong
        // place. Removing the restore lets the paste land on whatever child
        // still has keyboard focus — which is what the user expects.
        let _ = target_hwnd; // kept for future diagnostic logging if needed

        match item.content_type.as_str() {
            "text" => {
                if let Some(text) = &item.text_content {
                    let prev = actions::read_clipboard_pub().unwrap_or_default();
                    // Route through the dual-format writer when the row has a
                    // CF_HTML fragment (rich-text copy captured from Word,
                    // Outlook, Chrome, Slack, etc.). Rich-text-aware target
                    // apps read CF_HTML and reproduce bullets, links, bold
                    // and colour; plain-text-only apps automatically fall back
                    // to CF_UNICODETEXT — no per-target branching needed.
                    // Plain-text rows keep the original single-format path.
                    let wrote = match item.html_content.as_deref() {
                        Some(html) if !html.is_empty() => {
                            crate::expansions::write_clipboard_dual(text, Some(html))
                        }
                        _ => actions::write_clipboard_pub(text),
                    };
                    // If write fails (e.g. Excel holds clipboard lock), skip paste —
                    // pasting now would send whatever was already on the clipboard.
                    if !wrote {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));

                    // Ctrl+V
                    actions::send_vk_key_pub(0xA2, false);
                    actions::send_vk_key_pub(0x56, false);
                    actions::send_vk_key_pub(0x56, true);
                    actions::send_vk_key_pub(0xA2, true);

                    actions::restore_modifiers(&held);

                    // 150ms: Excel processes clipboard paste via its message queue,
                    // slower than most apps. 50ms caused Excel to read the restored
                    // old content instead of the selected clipboard item.
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    // Only restore if clipboard still holds the item we pasted —
                    // if the user copied something in the meantime, leave it alone.
                    let current = actions::read_clipboard_pub().unwrap_or_default();
                    if !prev.is_empty() && current.trim() == text.trim() {
                        actions::write_clipboard_pub(&prev);
                    }
                    pasted_ok = true;
                }
            }
            "image" => {
                if let Some(png_bytes) = &item.image_blob {
                    if let Ok(img) = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png) {
                        use image::GenericImageView;
                        let (width, height) = img.dimensions();
                        let rgba = img.to_rgba8();
                        let row_stride = (width * 4) as usize;
                        let mut bgra = vec![0u8; row_stride * height as usize];
                        for y in 0..height as usize {
                            let src_row = &rgba.as_raw()[y * row_stride..(y + 1) * row_stride];
                            let dst_y = (height as usize - 1) - y;
                            let dst_row = &mut bgra[dst_y * row_stride..(dst_y + 1) * row_stride];
                            for x in 0..width as usize {
                                let si = x * 4;
                                dst_row[si] = src_row[si + 2];     // B
                                dst_row[si + 1] = src_row[si + 1]; // G
                                dst_row[si + 2] = src_row[si];     // R
                                dst_row[si + 3] = src_row[si + 3]; // A
                            }
                        }

                        write_image_to_clipboard(&bgra, width, height, png_bytes);

                        // Ctrl+V
                        actions::send_vk_key_pub(0xA2, false);
                        actions::send_vk_key_pub(0x56, false);
                        actions::send_vk_key_pub(0x56, true);
                        actions::send_vk_key_pub(0xA2, true);

                        actions::restore_modifiers(&held);
                        pasted_ok = true;
                    }
                }
            }
            _ => {}
        }

        drop(_suppress); // SUPPRESS_SIMULATED = false (even if image decode panicked)
        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(false, std::sync::atomic::Ordering::SeqCst);

        // Increment the paste counter for this entry (best-effort, fire-and-forget).
        if pasted_ok {
            clipboard::increment_paste_count(id);
        }
    });
}

/// Paste arbitrary text via the standard release-modifiers + write-clipboard + Ctrl+V
/// pipeline. Used for transformed/edited paste from the clipboard preview pane —
/// does NOT modify the source clip's stored text. If `source_id` is provided, the
/// source entry's paste_count is incremented.
#[tauri::command]
fn paste_text(text: String, source_id: Option<i64>, app: tauri::AppHandle) {
    if text.is_empty() {
        return;
    }
    // Fill-in mode: same event-emit path as paste_clipboard_item. Reached when
    // the user clicks "Paste plain" on a rich clipboard item — inside a fill-in
    // it's still just text going into a text input, no formatting to preserve.
    if crate::hotkeys::CLIPBOARD_OVERLAY_FOR_FILLIN
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        use tauri::Emitter;
        let _ = app.emit(
            "fillin-insert-text",
            serde_json::json!({ "text": text, "id": source_id }),
        );
        if let Some(id) = source_id {
            clipboard::increment_paste_count(id);
        }
        hide_clipboard_overlay(&app);
        return;
    }
    let target_hwnd = CLIPBOARD_OVERLAY_TARGET.load(std::sync::atomic::Ordering::SeqCst);

    std::thread::spawn(move || {
        // Re-entrancy guard: same defect as paste_clipboard_item — concurrent
        // calls would interleave read-prev/write/restore-prev and flood the
        // clipboard. See actions::PasteOpGuard.
        let _paste_guard = match actions::PasteOpGuard::try_acquire() {
            Some(g) => g,
            None => {
                log::info!("[Keyfire] paste_text skipped — concurrent paste/copy op in flight");
                return;
            }
        };

        std::thread::sleep(std::time::Duration::from_millis(30));

        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _suppress = actions::SuppressionGuard::new();

        let held = actions::release_held_modifiers();

        if target_hwnd != 0 {
            actions::set_foreground_robust(target_hwnd);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }

        let prev = actions::read_clipboard_pub().unwrap_or_default();
        let pasted_ok = if actions::write_clipboard_pub(&text) {
            std::thread::sleep(std::time::Duration::from_millis(10));

            // Ctrl+V
            actions::send_vk_key_pub(0xA2, false);
            actions::send_vk_key_pub(0x56, false);
            actions::send_vk_key_pub(0x56, true);
            actions::send_vk_key_pub(0xA2, true);

            actions::restore_modifiers(&held);

            std::thread::sleep(std::time::Duration::from_millis(150));
            let current = actions::read_clipboard_pub().unwrap_or_default();
            if !prev.is_empty() && current.trim() == text.trim() {
                actions::write_clipboard_pub(&prev);
            }
            true
        } else {
            actions::restore_modifiers(&held);
            false
        };

        drop(_suppress);
        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(false, std::sync::atomic::Ordering::SeqCst);

        if pasted_ok {
            if let Some(id) = source_id {
                clipboard::increment_paste_count(id);
            }
        }
    });
}

/// Copy a clipboard history item back onto the system clipboard without pasting.
/// Used by the main clipboard panel (the popup overlay still uses `paste_clipboard_item`
/// for fast in-place paste). The user is expected to switch to their target app and
/// paste with Ctrl+V themselves. No focus games, no key injection — sidesteps the
/// WebView2 input-injection problem when the main window is focused.
#[tauri::command]
fn copy_clipboard_item(id: i64) {
    let item = match clipboard::get_item_full(id) {
        Some(i) => i,
        None => return,
    };

    std::thread::spawn(move || {
        // Re-entrancy guard: shared with paste_clipboard_item / paste_text so a
        // burst of copy-then-paste calls (or repeated copy clicks) can't race.
        // See actions::PasteOpGuard.
        let _paste_guard = match actions::PasteOpGuard::try_acquire() {
            Some(g) => g,
            None => {
                log::info!("[Keyfire] copy_clipboard_item skipped — concurrent paste/copy op in flight");
                return;
            }
        };

        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(true, std::sync::atomic::Ordering::SeqCst);

        match item.content_type.as_str() {
            "text" => {
                if let Some(text) = &item.text_content {
                    // Preserve CF_HTML when the row has a captured fragment so
                    // a subsequent Ctrl+V into Word / Outlook / Gmail keeps the
                    // formatting. Plain-text-only apps still receive
                    // CF_UNICODETEXT as before.
                    match item.html_content.as_deref() {
                        Some(html) if !html.is_empty() => {
                            crate::expansions::write_clipboard_dual(text, Some(html));
                        }
                        _ => {
                            actions::write_clipboard_pub(text);
                        }
                    }
                }
            }
            "image" => {
                if let Some(png_bytes) = &item.image_blob {
                    if let Ok(img) = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png) {
                        use image::GenericImageView;
                        let (width, height) = img.dimensions();
                        let rgba = img.to_rgba8();
                        let row_stride = (width * 4) as usize;
                        let mut bgra = vec![0u8; row_stride * height as usize];
                        for y in 0..height as usize {
                            let src_row = &rgba.as_raw()[y * row_stride..(y + 1) * row_stride];
                            let dst_y = (height as usize - 1) - y;
                            let dst_row = &mut bgra[dst_y * row_stride..(dst_y + 1) * row_stride];
                            for x in 0..width as usize {
                                let si = x * 4;
                                dst_row[si] = src_row[si + 2];
                                dst_row[si + 1] = src_row[si + 1];
                                dst_row[si + 2] = src_row[si];
                                dst_row[si + 3] = src_row[si + 3];
                            }
                        }
                        write_image_to_clipboard(&bgra, width, height, png_bytes);
                    }
                }
            }
            _ => {}
        }

        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(false, std::sync::atomic::Ordering::SeqCst);

        // Promote-on-use: the copied row is now the live clipboard content,
        // so float it to the top of the timeline (no duplicate row — the
        // internal write above is suppressed from the listener).
        clipboard::touch_item(id);
    });
}

/// Copy arbitrary (possibly edited / transformed) text onto the system clipboard.
/// Counterpart to `paste_text` for the main clipboard panel — writes without firing
/// Ctrl+V. Does not modify any source row.
///
/// Uses the recordable write path: the clipboard listener will record this as a
/// new history entry (the text is a genuinely new variant the user just created).
#[tauri::command]
fn copy_text(text: String) {
    if text.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        actions::write_clipboard_recordable_pub(&text);
    });
}

/// Run OCR over a clipboard image. Returns Ok(text) or Err(reason). Runs the
/// blocking WinRT calls on a separate thread so the IPC caller does not stall.
/// On success, caches the recognised text back on the image row via
/// `clipboard::set_ocr_text` so re-selecting the same image returns instantly
/// without re-running OCR.
///
/// Pro-gated: OCR (both this manual path and the auto-OCR-on-capture path)
/// is part of the Pro clipboard tier. Returns the sentinel string
/// "OCR_PRO_REQUIRED" so the frontend can distinguish "needs upgrade" from
/// generic failures and route to the upgrade modal.
#[tauri::command]
async fn ocr_clipboard_image(id: i64) -> Result<String, String> {
    if !licence::is_pro() {
        return Err("OCR_PRO_REQUIRED".to_string());
    }
    let blob = match clipboard::get_image_blob(id) {
        Some(b) => b,
        None => return Err("Image not found".to_string()),
    };
    // tauri::async_runtime is tokio under the hood — spawn_blocking is the right call.
    let text = tauri::async_runtime::spawn_blocking(move || ocr::ocr_png_bytes(&blob))
        .await
        .map_err(|e| format!("OCR task join failed: {}", e))??;
    clipboard::set_ocr_text(id, text.clone());
    Ok(text)
}

/// Returns up to 5 dominant RGB colours (as [r,g,b] arrays) for a clipboard image.
/// Async + spawn_blocking for the same reason as get_clipboard_image: full
/// image decrypt + decode must never run on the main thread.
#[tauri::command]
async fn get_clipboard_image_colors(id: i64) -> Vec<[u8; 3]> {
    let result = tauri::async_runtime::spawn_blocking(move || {
        let blob = match clipboard::get_image_blob(id) {
            Some(b) => b,
            None => return Vec::new(),
        };
        clipboard::dominant_colors(&blob, 5)
    })
    .await;
    result.unwrap_or_default()
}

/// Save a clipboard image to a user-selected file path. format: "png" or "jpg".
/// PNG is written directly from the BLOB; JPG is re-encoded via the image crate.
#[tauri::command]
fn save_clipboard_image_as(id: i64, format: String, app: tauri::AppHandle) -> bool {
    use tauri_plugin_dialog::DialogExt;

    let blob = match clipboard::get_image_blob(id) {
        Some(b) => b,
        None => return false,
    };

    let (default_name, filter_name, filter_exts): (String, &str, &[&str]) = if format == "jpg" {
        (format!("clipboard-{}.jpg", id), "JPEG", &["jpg", "jpeg"])
    } else {
        (format!("clipboard-{}.png", id), "PNG", &["png"])
    };

    let file_path = app
        .dialog()
        .file()
        .set_title("Save image")
        .set_file_name(&default_name)
        .add_filter(filter_name, filter_exts)
        .blocking_save_file();

    let path = match file_path {
        Some(p) => match p.into_path() {
            Ok(pb) => pb,
            Err(_) => return false,
        },
        None => return false,
    };

    if format == "jpg" {
        // Re-encode PNG -> JPEG.
        match image::load_from_memory_with_format(&blob, image::ImageFormat::Png) {
            Ok(img) => img.save_with_format(&path, image::ImageFormat::Jpeg).is_ok(),
            Err(_) => false,
        }
    } else {
        // PNG: blob is already PNG-encoded — write straight to disk.
        std::fs::write(&path, &blob).is_ok()
    }
}

/// Write image to clipboard as CF_DIB + PNG stream + CF_UNICODETEXT.
/// Self-contained version for the clipboard paste path. Applies the same three
/// protections as text writes so the image doesn't leak into Win+V history or
/// re-enter Keyfire's own clipboard DB on the listener's WM_CLIPBOARDUPDATE:
///   1. SUPPRESS_NEXT_CLIPBOARD_WRITE (level flag for synchronous window)
///   2. mark_clipboard_excluded (Windows Clipboard History opt-out)
///   3. record_self_clipboard_write (seqnum record for async H3 race)
/// Callers also set SUPPRESS_NEXT externally — that's redundant but harmless.
#[cfg(not(windows))]
fn write_image_to_clipboard(bgra_pixels: &[u8], width: u32, height: u32, png_bytes: &[u8]) {
    let _ = (bgra_pixels, width, height, png_bytes);
    log::warn!("[stub] image clipboard write is not available on this platform yet");
}

#[cfg(windows)]
fn write_image_to_clipboard(bgra_pixels: &[u8], width: u32, height: u32, png_bytes: &[u8]) {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_DIB_: u32 = 8;

    let header_size: u32 = 40;
    let pixel_data_size = bgra_pixels.len();
    let total_size = header_size as usize + pixel_data_size;

    actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
        .store(true, std::sync::atomic::Ordering::SeqCst);

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return;
        }
        EmptyClipboard();

        // CF_DIB
        let h_dib = GlobalAlloc(GMEM_MOVEABLE, total_size);
        if !h_dib.is_null() {
            let ptr = GlobalLock(h_dib) as *mut u8;
            if !ptr.is_null() {
                let hp = ptr as *mut u32;
                *hp = header_size;
                *hp.add(1) = width;
                *hp.add(2) = height;
                *hp.add(3) = 1 | (32 << 16);
                *hp.add(4) = 0;
                *hp.add(5) = pixel_data_size as u32;
                *hp.add(6) = 0;
                *hp.add(7) = 0;
                *hp.add(8) = 0;
                *hp.add(9) = 0;
                std::ptr::copy_nonoverlapping(bgra_pixels.as_ptr(), ptr.add(header_size as usize), pixel_data_size);
                GlobalUnlock(h_dib);
                SetClipboardData(CF_DIB_, h_dib as _);
            }
        }

        // PNG stream
        if !png_bytes.is_empty() {
            let fmt_name: Vec<u16> = "PNG\0".encode_utf16().collect();
            let fmt_id = RegisterClipboardFormatW(fmt_name.as_ptr());
            if fmt_id != 0 {
                let h_png = GlobalAlloc(GMEM_MOVEABLE, png_bytes.len());
                if !h_png.is_null() {
                    let p = GlobalLock(h_png) as *mut u8;
                    if !p.is_null() {
                        std::ptr::copy_nonoverlapping(png_bytes.as_ptr(), p, png_bytes.len());
                        GlobalUnlock(h_png);
                        SetClipboardData(fmt_id, h_png as _);
                    }
                }
            }
        }

        // Keep image writes out of Win+V / cloud clipboard — must be set while
        // clipboard is OPEN and AFTER the real content formats.
        crate::expansions::mark_clipboard_excluded();

        CloseClipboard();
    }

    // Record the seqnum produced by this write so the listener skips it even
    // if WM_CLIPBOARDUPDATE arrives after SUPPRESS_NEXT was cleared.
    actions::record_self_clipboard_write();
}

#[tauri::command]
fn close_clipboard_overlay(app: tauri::AppHandle) {
    hide_clipboard_overlay(&app);
}

/// Show the clipboard popup in fill-in mode. Reached via two routes:
///  1. FillInWindow.jsx catches Ctrl+Shift+V at DOM level and invokes the
///     `show_clipboard_overlay_for_fillin` Tauri command (works when the
///     fill-in webview has real DOM keyboard focus).
///  2. The LL hook clipboard-hotkey handler detects the combo while
///     `FILLIN_HWND` is non-zero and emits `toggle-clipboard-overlay-for-fillin`
///     which routes here (works even when the LL hook has already eaten the
///     combo via `suppress_keys` before the fill-in's DOM ever sees it).
///
/// Sets `CLIPBOARD_OVERLAY_FOR_FILLIN` so downstream:
///  - The overlay is shown via an activating `SetWindowPos` (no `SWP_NOACTIVATE`)
///    so its own DOM handles keyboard input.
///  - `paste_clipboard_item` / `paste_text` route the picked text back via a
///    `fillin-insert-text` event, sidestepping Ctrl+V injection into the wrong
///    window (WebView2 → WebView2 is unreliable per
///    [[feedback_webview2_input_injection]]).
#[cfg(not(windows))]
fn show_clipboard_overlay_for_fillin_impl(app: &tauri::AppHandle) {
    let _ = app;
}

#[cfg(windows)]
fn show_clipboard_overlay_for_fillin_impl(app: &tauri::AppHandle) {
    crate::hotkeys::CLIPBOARD_OVERLAY_FOR_FILLIN.store(true, std::sync::atomic::Ordering::SeqCst);
    webview_mem::resume_for_show(app, "clipboardoverlay");

    // Send history + theme BEFORE showing so the payload is ready when the
    // window becomes visible. Same pattern as show_clipboard_overlay.
    let history = clipboard::get_history(1, 500, None, None, None, None, false);
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let theme = cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark");
    let mut payload = history;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("theme".to_string(), serde_json::Value::String(theme.to_string()));
    }

    if let Some(win) = app.get_webview_window("clipboardoverlay") {
        use tauri::Emitter;
        // Clear search/selection on every show — the data event no longer
        // resets them (see ClipboardOverlay.jsx 'clipboard-overlay-reset').
        let _ = win.emit("clipboard-overlay-reset", serde_json::Value::Null);
        let _ = win.emit("clipboard-overlay-data", payload);

        // Position like show_clipboard_overlay: center of active monitor,
        // 1/3 from top, clamped to work area. Physical units to dodge the
        // hidden-window scale-factor race per monitor_scale_factor.
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

        let (wa_left, wa_top, wa_right, wa_bottom, scale) = unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            let s = monitor_scale_factor(hmon);
            if GetMonitorInfoW(hmon, &mut mi) != 0 {
                (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom, s)
            } else {
                (0, 0, 1920, 1080, s)
            }
        };
        let win_w_logical = 754.0_f64;
        let win_h_logical = 500.0_f64;
        let phys_w_unclamped = (win_w_logical * scale).round() as i32;
        let phys_h_unclamped = (win_h_logical * scale).round() as i32;
        let wa_w = wa_right - wa_left;
        let wa_h = wa_bottom - wa_top;
        let max_w = (wa_w - 32).max(400);
        let max_h = (wa_h - 32).max(200);
        let phys_w = phys_w_unclamped.min(max_w);
        let phys_h = phys_h_unclamped.min(max_h);
        let ideal_y = wa_top + wa_h / 3;
        let max_y = wa_bottom - phys_h - 16;
        let mut phys_y = ideal_y.min(max_y).max(wa_top + 16);
        let mut phys_x = wa_left + (wa_w - phys_w) / 2;

        // Same user-dragged override as show_clipboard_overlay — the popup
        // lives where the user put it in fill-in mode too.
        if let Some((fx, fy)) = saved_overlay_frac("clipboard") {
            let span_x = (wa_w - phys_w).max(0) as f64;
            let span_y = (wa_h - phys_h).max(0) as f64;
            phys_x = wa_left + (fx * span_x).round() as i32;
            phys_y = wa_top + (fy * span_y).round() as i32;
        }

        if let Ok(hwnd) = win.hwnd() {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST};
                const SWP_SHOWWINDOW: u32 = 0x0040;
                // Activating show — the fill-in gives up focus to the popup so
                // popup's DOM handles keys (search input + arrow nav + Enter).
                // No SWP_NOACTIVATE here — deliberate departure from the LL-hook
                // path used by show_clipboard_overlay.
                SetWindowPos(
                    hwnd.0 as _,
                    HWND_TOPMOST,
                    phys_x,
                    phys_y,
                    phys_w,
                    phys_h,
                    SWP_SHOWWINDOW,
                );
            }
            crate::hotkeys::CLIPBOARD_OVERLAY_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
        }
        // Force keyboard focus onto the popup's webview so the search input
        // receives the user's search-as-they-type keystrokes.
        let _ = win.set_focus();
    }
    crate::hotkeys::CLIPBOARD_OVERLAY_VISIBLE.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Tauri command wrapper — invoked from FillInWindow.jsx's DOM keydown listener.
/// The LL-hook-driven path uses the `toggle-clipboard-overlay-for-fillin` event
/// listener instead, which also calls `show_clipboard_overlay_for_fillin_impl`.
#[tauri::command]
fn show_clipboard_overlay_for_fillin(app: tauri::AppHandle) {
    show_clipboard_overlay_for_fillin_impl(&app);
}

#[tauri::command]
fn clipboard_overlay_resize(width: f64, height: f64, app: tauri::AppHandle) {
    let w = width.max(500.0).min(1200.0);
    let h = height.max(60.0).min(600.0);
    if let Some(win) = app.get_webview_window("clipboardoverlay") {
        let _ = win.set_size(tauri::LogicalSize::new(w, h));
    }
}

#[tauri::command]
fn delete_clipboard_item(id: i64) -> bool {
    clipboard::delete_item(id)
}

#[tauri::command]
fn clear_clipboard_history() -> bool {
    clipboard::clear_all()
}

#[tauri::command]
fn get_clipboard_encryption_status() -> Value {
    clipboard::encryption_status()
}

#[tauri::command]
fn delete_clipboard_plaintext_backup() -> bool {
    clipboard::delete_plaintext_backup_now()
}

#[tauri::command]
fn reset_clipboard_storage() -> bool {
    clipboard::reset_storage()
}

#[tauri::command]
fn pin_clipboard_item(id: i64, pinned: bool) -> bool {
    clipboard::pin_item(id, pinned)
}

#[tauri::command]
fn star_clipboard_item(id: i64, starred: bool) -> bool {
    clipboard::star_item(id, starred)
}

#[tauri::command]
fn reorder_clipboard_pinned(ids: Vec<i64>) -> bool {
    clipboard::reorder_pinned(ids)
}

#[tauri::command]
fn reorder_clipboard_starred(ids: Vec<i64>) -> bool {
    clipboard::reorder_starred(ids)
}

// Saved folders — flat folders organising the Saved tier. Internal naming
// stays folder/starred; only UI strings say "Saved".
#[tauri::command]
fn create_clipboard_folder(name: String) -> Option<i64> {
    // Backend mirror of the UI Pro gate (same defence-in-depth as the
    // retention clamp). Creation only — existing folders stay fully usable
    // after a licence lapses, matching the never-hold-data-hostage rule.
    if !licence::is_pro() {
        return None;
    }
    clipboard::create_folder(name)
}

#[tauri::command]
fn rename_clipboard_folder(id: i64, name: String) -> bool {
    clipboard::rename_folder(id, name)
}

#[tauri::command]
fn delete_clipboard_folder(id: i64) -> bool {
    clipboard::delete_folder(id)
}

#[tauri::command]
fn move_clipboard_item_to_folder(id: i64, folder_id: Option<i64>) -> bool {
    clipboard::move_to_folder(id, folder_id)
}

#[tauri::command]
fn get_clipboard_folders() -> serde_json::Value {
    clipboard::get_folders()
}

#[tauri::command]
async fn get_clipboard_image(id: i64) -> Option<String> {
    // Async + spawn_blocking: sync Tauri commands run ON THE MAIN THREAD.
    // This command decrypts a full-resolution PNG and base64-encodes it —
    // seconds per call in debug builds. The clipboard popup fires one per
    // image row on open, so running them on the main thread serialised a
    // minutes-long freeze of the entire event loop (webview evals, event
    // emits, window ops). Off-thread they trickle in without blocking
    // anything.
    let result = tauri::async_runtime::spawn_blocking(move || {
        get_clipboard_image_blocking(id)
    })
    .await;
    result.ok().flatten()
}

fn get_clipboard_image_blocking(id: i64) -> Option<String> {
    clipboard::get_image_blob(id).map(|bytes| {
        // Base64 encode without external crate
        const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut result = String::with_capacity((bytes.len() + 2) / 3 * 4);
        for chunk in bytes.chunks(3) {
            let b0 = chunk[0] as u32;
            let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
            let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
            let triple = (b0 << 16) | (b1 << 8) | b2;
            result.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
            result.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);
            if chunk.len() > 1 {
                result.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
            } else {
                result.push('=');
            }
            if chunk.len() > 2 {
                result.push(CHARS[(triple & 0x3F) as usize] as char);
            } else {
                result.push('=');
            }
        }
        result
    })
}

#[tauri::command]
fn get_distinct_source_apps() -> Vec<String> {
    clipboard::get_distinct_source_apps()
}

#[tauri::command]
fn get_clipboard_date_buckets(
    app_filter: Option<String>,
    tag_filter: Option<String>,
) -> Value {
    clipboard::get_date_buckets(app_filter, tag_filter)
}

#[tauri::command]
fn update_clipboard_item(id: i64, new_text: String) -> Option<String> {
    clipboard::update_item(id, new_text)
}

#[tauri::command]
fn get_clipboard_settings() -> Value {
    serde_json::json!({
        "retention_days": clipboard::get_retention(),
        "enabled": true,
        "auto_ocr": clipboard::auto_ocr_enabled(),
        "search_inside_images": clipboard::search_inside_images_enabled(),
    })
}

#[tauri::command]
fn set_clipboard_settings(retention_days: u32) {
    let max_days = if licence::is_pro() { 30 } else { 7 };
    let clamped = retention_days.min(max_days).clamp(1, 30);
    clipboard::set_retention_days(clamped);

    // Persist to config so the setting survives restart. Without this the
    // value lived only in the RETENTION_DAYS static and every relaunch fell
    // back to DEFAULT_RETENTION_DAYS (7), silently undoing the user's choice.
    let mut cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("clipboardRetentionDays".to_string(), serde_json::json!(clamped));
        config::save_config(&cfg);
    }
}

/// Set both auto-OCR toggles in one command. Both default true for Pro; the
/// gates in clipboard.rs (auto-OCR dispatch + search_history) enforce Pro at
/// use-time, so a Free user can technically flip them but nothing happens.
#[tauri::command]
fn set_clipboard_ocr_settings(auto_ocr: bool, search_inside_images: bool) {
    clipboard::set_auto_ocr_enabled(auto_ocr);
    clipboard::set_search_inside_images_enabled(search_inside_images);

    let mut cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert("clipboardAutoOcr".to_string(), serde_json::json!(auto_ocr));
        obj.insert(
            "clipboardSearchInsideImages".to_string(),
            serde_json::json!(search_inside_images),
        );
        config::save_config(&cfg);
    }
}

/// Kick off the one-off OCR backfill for existing image rows. Pro-gated
/// inside clipboard::run_ocr_backfill so the guard survives even if the
/// frontend calls it without checking. Returns immediately — work happens
/// on a background thread and emits progress events.
#[tauri::command]
fn backfill_clipboard_ocr() {
    clipboard::run_ocr_backfill();
}

/// v0.8.4 one-shot thumbnail backfill for legacy image rows. Frontend
/// guards on localStorage `trigr_thumb_backfilled_v1` so this only runs once
/// per install. Fires progress + done events on the same pattern as OCR.
#[tauri::command]
fn backfill_clipboard_thumbnails() {
    clipboard::run_thumb_backfill();
}

/// Fetch just the decrypted OCR text for a row. Used by the auto-OCR
/// completion listener in ClipboardPanel to merge newly-recognised text
/// into local state without a full get_item_full round-trip. Returns
/// empty string if the row has no OCR text yet.
#[tauri::command]
async fn get_clipboard_ocr_text(id: i64) -> String {
    tauri::async_runtime::spawn_blocking(move || {
        clipboard::get_item_full(id)
            .and_then(|item| item.ocr_text)
            .unwrap_or_default()
    })
    .await
    .unwrap_or_default()
}

/// Lazy-fetch the decrypted text + html for a text row. The history list
/// SELECT drops text_content and html_content so the payload stays small
/// even with multi-MB pastes in the timeline; the frontend calls this on
/// selection / edit to populate the detail pane. Image_blob is intentionally
/// not returned — image rows use getClipboardImage which handles the
/// full-res round-trip separately.
#[tauri::command]
async fn get_clipboard_item_text_full(id: i64) -> Value {
    tauri::async_runtime::spawn_blocking(move || {
        match clipboard::get_item_full(id) {
            Some(item) => serde_json::json!({
                "text_content": item.text_content,
                "html_content": item.html_content,
            }),
            None => serde_json::json!({
                "text_content": Value::Null,
                "html_content": Value::Null,
            }),
        }
    })
    .await
    .unwrap_or_else(|_| serde_json::json!({
        "text_content": Value::Null,
        "html_content": Value::Null,
    }))
}

#[tauri::command]
fn set_clipboard_capture_enabled(enabled: bool) {
    clipboard::set_capture_enabled(enabled);
}

// ── Telemetry opt-out (machine-local) ──────────────────────────────────────
// The Settings UI exposes a "Send anonymous usage stats" toggle. Internally
// we store the inverse (opt-OUT) so a fresh install reads `false` and
// telemetry runs by default. The reciprocal command flips the bool for the
// JS side.

#[tauri::command]
fn get_telemetry_enabled() -> bool {
    !config::get_telemetry_opt_out()
}

#[tauri::command]
fn set_telemetry_enabled(enabled: bool) {
    config::set_telemetry_opt_out(!enabled);
}

#[tauri::command]
fn set_clipboard_excluded_apps(apps: Vec<String>) {
    clipboard::set_excluded_apps(apps);
}

#[tauri::command]
fn get_clipboard_storage_size() -> u64 {
    clipboard::get_storage_size()
}

// ── Auto-updater (Phase 10) ─────────────────────────────────────────────────

#[tauri::command]
fn check_for_updates() -> Value {
    serde_json::json!({ "success": false })
}

#[tauri::command]
fn install_update() -> Value {
    serde_json::json!({ "success": false })
}

#[tauri::command]
fn start_download(version: String) {
    let _ = version;
}

// ── Fill-in (Phase 7) ───────────────────────────────────────────────────────

#[tauri::command]
fn fill_in_ready() {
    if let Ok(mut guard) = expansions::fill_in_ready_tx().lock() {
        if let Some(tx) = guard.take() {
            let _ = tx.send(());
        }
    }
}

/// JS confirms the fill-in / picker window actually rendered (fired from the
/// FillInWindow.jsx `fill-in-show` handler after state is committed). Rust's
/// wait side has a 2s timeout — if this ACK doesn't arrive, the picker never
/// appeared (WebView2 hung, HMR replaced the listener mid-fire, etc.) and
/// Rust aborts the fire cleanly instead of blocking the response wait.
#[tauri::command]
fn fill_in_shown_ack() {
    if let Ok(mut guard) = expansions::fill_in_shown_tx().lock() {
        if let Some(tx) = guard.take() {
            let _ = tx.send(());
        }
    }
}

#[cfg(not(windows))]
#[tauri::command]
fn fillin_resize(height: f64, app: tauri::AppHandle) {
    let Some(win) = app.get_webview_window("fillin") else { return };
    let _ = win.set_size(tauri::LogicalSize::new(448.0, height.max(120.0)));
}

#[cfg(windows)]
#[tauri::command]
fn fillin_resize(height: f64, app: tauri::AppHandle) {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };

    let Some(win) = app.get_webview_window("fillin") else { return };
    let win_w = 448.0;
    let margin = 16.0;

    // Read the work area of the monitor the window currently sits on. Used both
    // to cap the window height (so it never extends past the screen) and to
    // re-center it vertically after the content-driven resize.
    let hwnd_isize = match win.hwnd() {
        Ok(h) => h.0 as isize,
        Err(_) => {
            // No HWND yet — best-effort size only; centering not possible.
            let _ = win.set_size(tauri::LogicalSize::new(win_w, height.max(150.0).min(600.0)));
            return;
        }
    };
    let scale = win.scale_factor().unwrap_or(1.0);

    let work_area = unsafe {
        let hmon = MonitorFromWindow(hwnd_isize as HWND, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            Some((mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom))
        } else {
            None
        }
    };

    let Some((wa_left, wa_top, wa_right, wa_bottom)) = work_area else {
        // No work-area info — fall back to fixed cap, no recenter.
        let _ = win.set_size(tauri::LogicalSize::new(win_w, height.max(150.0).min(600.0)));
        return;
    };

    let log_left = wa_left as f64 / scale;
    let log_top = wa_top as f64 / scale;
    let log_w = (wa_right - wa_left) as f64 / scale;
    let log_h = (wa_bottom - wa_top) as f64 / scale;

    // Cap height at the actual work area (minus 2× margin) instead of a fixed
    // 600px. Multi-field typed fill-ins (dropdown with many options, multiple
    // multi-line fields) easily exceed 600; the old cap forced the panel to
    // render beyond the window viewport. CSS `max-height` on `.fillin-win`
    // makes `.fillin-win-fields` scroll internally when content still exceeds
    // the new dynamic cap.
    let max_h = (log_h - margin * 2.0).max(150.0);
    let h = height.max(150.0).min(max_h);

    let _ = win.set_size(tauri::LogicalSize::new(win_w, h));

    // Re-center horizontally + vertically using the new dimensions. Clamp Y so
    // the window can never land above or below the work area regardless of
    // monitor size or content height.
    let x = log_left + ((log_w - win_w) / 2.0).max(0.0);
    let y_centered = log_top + (log_h - h) / 2.0;
    let y_min = log_top + margin;
    let y_max = log_top + log_h - h - margin;
    let y = y_centered.max(y_min).min(y_max.max(y_min));

    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
}

#[tauri::command]
fn fill_in_submit(values: Value) {
    // Convert Value to Option<HashMap<String, String>>: null = cancelled, object = submitted values
    let result: Option<std::collections::HashMap<String, String>> = if values.is_null() {
        None
    } else {
        values.as_object().map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string()))
                .collect()
        })
    };
    if let Ok(guard) = expansions::fill_in_tx().lock() {
        if let Some(ref tx) = *guard {
            let _ = tx.send(result);
        }
    }
}

// ── Licence ─────────────────────────────────────────────────────────────────

#[tauri::command]
fn get_licence_status() -> Value {
    serde_json::to_value(licence::get_licence_status()).unwrap_or(serde_json::json!({}))
}

#[tauri::command]
async fn activate_licence(key: String) -> Value {
    match licence::activate_licence(key).await {
        Ok(status) => {
            // Pro restored — cancel any pending shared-config migration.
            config::check_and_migrate_if_due();
            serde_json::json!({ "ok": true, "status": serde_json::to_value(status).unwrap_or(Value::Null) })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

#[tauri::command]
async fn deactivate_licence() -> Value {
    match licence::deactivate_licence().await {
        Ok(status) => {
            // Deactivating a key flips is_pro to false. Drive the grace-period
            // state machine so a user with shared config gets the banner
            // immediately, not at the next revalidation tick.
            config::check_and_migrate_if_due();
            serde_json::json!({ "ok": true, "status": serde_json::to_value(status).unwrap_or(Value::Null) })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

#[tauri::command]
async fn check_licence_revalidation() -> Value {
    let status = licence::check_and_revalidate().await;
    // Drive the shared-config grace-period state machine on every revalidation.
    // Idempotent: starts grace on first observation of Pro=false+shared, clears
    // grace on Pro=true, runs the migration when 7 days have elapsed.
    config::check_and_migrate_if_due();
    serde_json::to_value(status).unwrap_or(serde_json::json!({}))
}

#[tauri::command]
fn get_grace_period_state() -> Value {
    let expired_at = config::get_pro_expired_at();
    let shared_active = config::get_shared_config_dir().is_some();
    let days_remaining = config::grace_period_days_remaining();
    let migration_deferred = config::get_migration_deferred();
    serde_json::json!({
        "pro_expired_at": expired_at.map(|d| d.to_rfc3339()),
        "shared_active": shared_active,
        "days_remaining": days_remaining,
        "migration_deferred": migration_deferred,
    })
}

#[tauri::command]
fn migrate_shared_to_local_now(app: tauri::AppHandle) -> Value {
    match config::migrate_shared_to_local() {
        Ok(()) => {
            let _ = config::set_pro_expired_at(None);
            // Frontend listens for this to refresh state + show toast.
            let _ = app.emit("shared-config-migrated", serde_json::json!({}));
            serde_json::json!({ "ok": true })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

#[tauri::command]
async fn start_trial() -> Value {
    match licence::start_trial().await {
        Ok(status) => {
            // Trial gives Pro — cancel any pending shared-config migration.
            config::check_and_migrate_if_due();
            serde_json::json!({ "ok": true, "status": serde_json::to_value(status).unwrap_or(Value::Null) })
        }
        Err(e) => serde_json::json!({ "ok": false, "error": e }),
    }
}

#[tauri::command]
async fn mark_trial_offer_shown() -> Value {
    serde_json::to_value(licence::mark_trial_offer_shown().await).unwrap_or(serde_json::json!({}))
}

#[tauri::command]
async fn reset_trial() -> Value {
    serde_json::to_value(licence::reset_trial().await).unwrap_or(serde_json::json!({}))
}

// Dev-only Pro tier override. `pro`: Some(true) forces Pro, Some(false)
// forces Free, None clears the override and returns to real licence state.
// Release builds ignore the setter — see licence::dev_set_pro_override.
#[tauri::command]
async fn dev_set_pro_override(pro: Option<bool>) -> Value {
    serde_json::to_value(licence::dev_set_pro_override(pro).await).unwrap_or(serde_json::json!({}))
}

// Runs at frontend startup so the SettingsPanel can decide whether to show
// the dev-only Pro/Free toggle. Returns true only for cargo tauri dev
// (debug build), false for shipped release binaries.
#[tauri::command]
fn is_debug_build() -> bool {
    cfg!(debug_assertions)
}

// ── Demo mode ────────────────────────────────────────────────────────────────
//
// `--demo` launches Keyfire against a throwaway data dir (AppData\...\demo\):
// blank config (full first-run onboarding fires), fresh clipboard + analytics
// DBs, no telemetry. The real licence is seeded across so Pro features work
// on camera. The demo dir is wiped on every demo launch (crash-safe fresh
// state) AND best-effort on exit. Used for recording marketing videos and
// clean-profile bug repros. No visible DEMO badge in the main UI by design —
// a watermark would end up in the footage; the indicator lives in Settings.

static DEMO_MODE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

pub fn is_demo_mode() -> bool {
    *DEMO_MODE.get_or_init(|| std::env::args().any(|a| a == "--demo"))
}

// `--profile <name>` = the PERSISTENT sibling of --demo: same data-dir
// redirect (AppData\...\profiles\<name>\) but never wiped, so a curated
// staging setup (e.g. the "studio" profile for feature-showcase videos)
// survives between launches. Licence seeds on first creation only; after
// that the profile owns its own state. --demo wins if both flags are passed.
static PROFILE_MODE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

pub fn profile_mode() -> Option<&'static str> {
    PROFILE_MODE
        .get_or_init(|| {
            let args: Vec<String> = std::env::args().collect();
            let name = args
                .iter()
                .position(|a| a == "--profile")
                .and_then(|i| args.get(i + 1))?;
            // Path-safe subset only — the name becomes a folder under profiles\.
            let clean: String = name
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .take(32)
                .collect();
            if clean.is_empty() { None } else { Some(clean) }
        })
        .as_deref()
}

/// Copy ONLY the licence object from the real local settings into the demo
/// dir. Everything else is deliberately dropped — in particular
/// `shared_config_path`, which would point the demo session at the user's
/// REAL shared config file.
fn seed_demo_local_settings(real_dir: &std::path::Path, demo_dir: &std::path::Path) {
    let src = real_dir.join("trigr-local-settings.json");
    let Ok(raw) = std::fs::read_to_string(&src) else { return };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
    if let Some(lic) = val.get("licence") {
        let seeded = serde_json::json!({ "licence": lic });
        if std::fs::write(demo_dir.join("trigr-local-settings.json"), seeded.to_string()).is_ok() {
            log::info!("[Keyfire] Demo mode: licence seeded from real install");
        }
    }
}

#[tauri::command]
fn get_demo_mode() -> bool {
    is_demo_mode()
}

#[tauri::command]
fn get_profile_mode() -> Option<String> {
    profile_mode().map(|s| s.to_string())
}

/// Restart Keyfire with different launch args (none = normal mode). Spawns a
/// detached helper that waits ~2s (the single-instance lock would swallow the
/// new launch if the old process is still alive — LL hooks, DB writers and
/// the tray need to tear down first) then starts the current exe, and exits
/// this instance.
fn spawn_relaunch(extra_args: &str, app: &tauri::AppHandle) {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            log::error!("[Keyfire] spawn_relaunch: current_exe failed: {}", e);
            return;
        }
    };
    let suffix = if extra_args.is_empty() {
        String::new()
    } else {
        format!(" {}", extra_args)
    };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // raw_arg, NOT args: Command's default quoting wraps the whole /C
        // line in an extra quote layer (it contains spaces) and cmd.exe's
        // nested-quote stripping mangles the inner start command — the OS
        // then pops "Windows cannot find '\\\'" (caught in dev 2026-08-05).
        let launch = format!(
            "ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"{}",
            exe.display(),
            suffix
        );
        let _ = std::process::Command::new("cmd")
            .raw_arg("/C")
            .raw_arg(launch)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
    #[cfg(not(windows))]
    {
        let launch = format!("sleep 2; \"{}\"{}", exe.display(), suffix);
        let _ = std::process::Command::new("sh").args(["-c", &launch]).spawn();
    }
    log::info!(
        "[Keyfire] Relaunching with args '{}' — exiting current instance",
        extra_args
    );
    app.exit(0);
}

#[tauri::command]
fn relaunch_demo_mode(enable: bool, app: tauri::AppHandle) {
    spawn_relaunch(if enable { "--demo" } else { "" }, &app);
}

/// Restart into a persistent named profile (see PROFILE_MODE). The name is
/// sanitised here AND on parse so a bad name can't become a stray folder.
#[tauri::command]
fn relaunch_profile_mode(name: String, app: tauri::AppHandle) {
    let clean: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    if clean.is_empty() {
        log::error!("[Keyfire] relaunch_profile_mode: invalid profile name {:?}", name);
        return;
    }
    spawn_relaunch(&format!("--profile {}", clean), &app);
}

// ── App builder ──────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Panics anywhere (macro thread, expansion thread, processor) used to go
    // to stderr only, which nobody sees in a tray app. Log them so a "hotkeys
    // just stopped" report has a trace to chase; the processor loop recovers
    // from its own panics (see hotkeys::process_events).
    std::panic::set_hook(Box::new(|info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_else(|| "unknown".to_string());
        let msg = if let Some(s) = info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "non-string panic payload".to_string()
        };
        log::error!("[PANIC] thread '{}' at {}: {}", std::thread::current().name().unwrap_or("?"), location, msg);
    }));
    tauri::Builder::default()
        // Single-instance lock — second launches focus the existing main
        // window and exit immediately. Prevents the WebView2 shared-runtime
        // blank-window bug (one instance killed via Task Manager would tear
        // down the shared browser process tree, leaving the survivor without
        // a renderer) plus double LL hooks / clipboard listeners / SQLite
        // writers / config watchers / tray icons. Must be registered before
        // any other plugin per the plugin's own docs. Args are ignored —
        // future protocol-handler support (trigr://) can add argv parsing.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            // tray::show_window does resume_for_show + unminimize + the
            // AttachThreadInput foreground dance; bare set_focus here let the
            // window surface BEHIND the current app, which read as "nothing
            // happened" when the user double-clicked the exe while in the tray.
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                .clear_targets()
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::LogDir { file_name: Some("trigr.log".into()) },
                ))
                .target(tauri_plugin_log::Target::new(
                    tauri_plugin_log::TargetKind::Stdout,
                ))
                .max_file_size(5_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // Initialize config module with app data dir. In demo mode
            // (--demo) EVERYTHING (config, backups, local settings, clipboard
            // + analytics DBs, scratchpad) redirects to a throwaway demo\
            // subfolder — wiped fresh on every demo launch, licence seeded
            // across so Pro features work. Real data never opened.
            let app_data = {
                let real = app.path().app_data_dir()?;
                if is_demo_mode() {
                    let demo = real.join("demo");
                    let _ = std::fs::remove_dir_all(&demo);
                    std::fs::create_dir_all(&demo)?;
                    seed_demo_local_settings(&real, &demo);
                    log::info!("[Keyfire] DEMO MODE — data dir redirected to {}", demo.display());
                    demo
                } else if let Some(name) = profile_mode() {
                    // Persistent profile — same redirect as demo but NEVER
                    // wiped. Licence seeds on first creation only; after that
                    // the profile owns its own local settings.
                    let dir = real.join("profiles").join(name);
                    std::fs::create_dir_all(&dir)?;
                    if !dir.join("trigr-local-settings.json").exists() {
                        seed_demo_local_settings(&real, &dir);
                    }
                    log::info!("[Keyfire] PROFILE '{}' — data dir redirected to {}", name, dir.display());
                    dir
                } else {
                    real
                }
            };
            std::fs::create_dir_all(&app_data)?;
            config::init(app_data.clone());
            licence::init();
            analytics::init(app_data.clone());

            // One-time migration: recalculate time_saved for old analytics entries
            // using the current assignments to determine actual action types.
            if let Some(cfg) = config::load_config() {
                if let Some(assignments) = cfg.get("assignments").and_then(|v| v.as_object()) {
                    let map: std::collections::HashMap<String, serde_json::Value> =
                        assignments.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    analytics::migrate_time_saved(map);
                }
                // Restore Quick Record (temp macro) state — hotkey combos +
                // the most recently captured event stream. Engine defaults
                // (Ctrl+Alt+R / Ctrl+Alt+P) apply when config has no entry.
                if let Some(temp) = cfg.get("tempMacro").and_then(|v| v.as_object()) {
                    if let Some(combo) = temp.get("recordHotkey").and_then(|v| v.as_str()) {
                        hotkeys::set_temp_macro_record_hotkey(combo);
                    }
                    if let Some(combo) = temp.get("playHotkey").and_then(|v| v.as_str()) {
                        hotkeys::set_temp_macro_play_hotkey(combo);
                    }
                    if let Some(combo) = temp.get("loopHotkey").and_then(|v| v.as_str()) {
                        hotkeys::set_temp_macro_loop_hotkey(combo);
                    }
                    if let Some(events_val) = temp.get("events") {
                        if let Ok(events) = serde_json::from_value::<Vec<recorder::RecordedEvent>>(events_val.clone()) {
                            if let Ok(mut state) = hotkeys::engine_state().lock() {
                                let captured_at = temp.get("capturedAt")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                if !events.is_empty() {
                                    state.temp_macro_events = Some(events);
                                    state.temp_macro_captured_at = captured_at;
                                }
                            }
                        }
                    }
                }
            }
            clipboard::init(app_data.clone(), app.handle().clone());
            actions::cleanup_stale_ahk_scripts(app_data);
            cleanup_stale_trigr_shortcuts();
            ensure_keyfire_shortcut();
            tray::heal_startup_registration();

            // Telemetry timer thread — 30s after startup, then every 6h. Owns
            // its own read-only SQLite connection (no contention with the
            // analytics writer's exclusive connection) and routes writes back
            // through the analytics writer thread via channel messages.
            // Honours the trigr-local-settings.json opt-out flag on every tick.
            // Demo/profile sessions never report — a staging launch is not
            // real usage and would inflate the daily-actives dashboard.
            if is_demo_mode() || profile_mode().is_some() {
                log::info!("[Keyfire] Demo/profile mode: telemetry disabled for this session");
            } else {
                let app_version = app.package_info().version.to_string();
                std::thread::Builder::new()
                    .name("trigr-telemetry".into())
                    .spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(30));
                        loop {
                            telemetry::tick(&app_version);
                            std::thread::sleep(std::time::Duration::from_secs(6 * 60 * 60));
                        }
                    })
                    .ok();
            }

            // Start file watcher if shared config path is configured
            if let Some(shared_dir) = config::get_shared_config_dir() {
                if shared_dir.exists() {
                    config::start_config_watcher(shared_dir.clone(), app.handle().clone());
                } else {
                    // Dir doesn't exist yet (drive disconnected?) — poll every 30s
                    let app_handle = app.handle().clone();
                    std::thread::Builder::new()
                        .name("shared-config-reconnect".into())
                        .spawn(move || {
                            loop {
                                std::thread::sleep(std::time::Duration::from_secs(30));
                                if shared_dir.exists() {
                                    log::info!("[Keyfire] Shared config dir became available: {}", shared_dir.display());
                                    config::start_config_watcher(shared_dir, app_handle.clone());
                                    // The session has been running on the stale LOCAL copy;
                                    // re-base on the shared file before any save can
                                    // overwrite it with local state.
                                    reload_config_and_emit(&app_handle);
                                    break;
                                }
                            }
                        })
                        .ok();
                }
            }

            // Set up system tray
            if let Err(e) = tray::setup_tray(app) {
                log::error!("[Keyfire] Failed to create tray: {}", e);
            }

            // Pre-create overlay window hidden — prevents frozen first launch
            let overlay_url = tauri::WebviewUrl::App("index.html?overlay=1".into());
            let overlay_win = tauri::WebviewWindowBuilder::new(app, "overlay", overlay_url)
                .additional_browser_args(WEBVIEW_BROWSER_ARGS)
                .title("Keyfire Quick Search")
                .inner_size(620.0, 103.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .shadow(false)
                .build()?;

            // Set WebView2 default background to transparent via COM interface.
            // Tauri's transparent(true) + CSS background: transparent is not enough —
            // WebView2 renders a solid background unless SetDefaultBackgroundColor is called.
            #[cfg(target_os = "windows")]
            {
                let _ = overlay_win.with_webview(|webview| {
                    unsafe {
                        use webview2_com::Microsoft::Web::WebView2::Win32::{
                            ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
                        };
                        use windows_core::Interface;
                        let controller = webview.controller();
                        if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
                            let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                                R: 0, G: 0, B: 0, A: 0,
                            });
                        }
                    }
                });
            }

            // FILL-IN WINDOW — transparent(true) + WebView2 COM fix required
            // See FillInWindow.jsx for full sizing documentation
            // DO NOT remove transparent(true) or the with_webview COM block —
            // both are required to prevent a visible background box around the panel.
            let fillin_url = tauri::WebviewUrl::App("index.html?fillin=1".into());
            let fillin_win = tauri::WebviewWindowBuilder::new(app, "fillin", fillin_url)
                .additional_browser_args(WEBVIEW_BROWSER_ARGS)
                .title("Keyfire — Fill In")
                .inner_size(420.0, 300.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .shadow(false)
                .center()
                .build()?;

            // Set WebView2 transparent background for fill-in window (async — avoid blocking startup)
            #[cfg(target_os = "windows")]
            {
                std::thread::spawn(move || {
                    let _ = fillin_win.with_webview(|webview| {
                        unsafe {
                            use webview2_com::Microsoft::Web::WebView2::Win32::{
                                ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
                            };
                            use windows_core::Interface;
                            let controller = webview.controller();
                            if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
                                let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                                    R: 0, G: 0, B: 0, A: 0,
                                });
                            }
                        }
                    });
                });
            }

            // Pre-create clipboard overlay window hidden
            let clipoverlay_url = tauri::WebviewUrl::App("index.html?clipboardoverlay=1".into());
            let clipoverlay_win = tauri::WebviewWindowBuilder::new(app, "clipboardoverlay", clipoverlay_url)
                .additional_browser_args(WEBVIEW_BROWSER_ARGS)
                .title("Keyfire Clipboard")
                .inner_size(400.0, 300.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .shadow(false)
                .build()?;

            #[cfg(target_os = "windows")]
            {
                let _ = clipoverlay_win.with_webview(|webview| {
                    unsafe {
                        use webview2_com::Microsoft::Web::WebView2::Win32::{
                            ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
                        };
                        use windows_core::Interface;
                        let controller = webview.controller();
                        if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
                            let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                                R: 0, G: 0, B: 0, A: 0,
                            });
                        }
                    }
                });
            }
            // Apply WS_EX_NOACTIVATE so the overlay never steals focus from the
            // active app when shown. Keyboard input is routed via the LL hook instead.
            #[cfg(target_os = "windows")]
            if let Ok(hwnd) = clipoverlay_win.hwnd() {
                unsafe {
                    use windows_sys::Win32::UI::WindowsAndMessaging::{
                        GetWindowLongW, SetWindowLongW,
                    };
                    const GWL_EXSTYLE: i32 = -20;
                    const WS_EX_NOACTIVATE: u32 = 0x08000000;
                    let ex = GetWindowLongW(hwnd.0 as _, GWL_EXSTYLE) as u32;
                    SetWindowLongW(hwnd.0 as _, GWL_EXSTYLE, (ex | WS_EX_NOACTIVATE) as i32);
                }
            }

            // Suppress unused variable warning
            let _ = &clipoverlay_win;

            // Pre-create radial menu window hidden
            let radial_url = tauri::WebviewUrl::App("index.html?radialmenu=1".into());
            let radial_win = tauri::WebviewWindowBuilder::new(app, "radialmenu", radial_url)
                .additional_browser_args(WEBVIEW_BROWSER_ARGS)
                .title("Keyfire Radial Menu")
                .inner_size(525.0, 525.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .shadow(false)
                .build()?;

            #[cfg(target_os = "windows")]
            {
                let _ = radial_win.with_webview(|webview| {
                    unsafe {
                        use webview2_com::Microsoft::Web::WebView2::Win32::{
                            ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
                        };
                        use windows_core::Interface;
                        let controller = webview.controller();
                        if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
                            let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                                R: 0, G: 0, B: 0, A: 0,
                            });
                        }
                    }
                });
            }
            let _ = &radial_win;

            // Pre-create recorder countdown window hidden. Same pattern as
            // fillin / clipboard / radial overlays. NOTE: every window is its
            // own renderer process unless WEBVIEW_BROWSER_ARGS folds them (it
            // does, via --process-per-site); webview_mem also suspends it after
            // 5min idle. On-demand creation was attempted but proved
            // unreliable (destroy/rebuild race made the modal silently fail
            // to appear, leaving main hidden and the flow stuck).
            let countdown_url = tauri::WebviewUrl::App("index.html?countdown=1".into());
            let countdown_win = tauri::WebviewWindowBuilder::new(app, "countdown", countdown_url)
                .additional_browser_args(WEBVIEW_BROWSER_ARGS)
                .title("Keyfire Recorder")
                .inner_size(380.0, 320.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .shadow(false)
                .build()?;

            #[cfg(target_os = "windows")]
            {
                let _ = countdown_win.with_webview(|webview| unsafe {
                    use webview2_com::Microsoft::Web::WebView2::Win32::{
                        ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
                    };
                    use windows_core::Interface;
                    let controller = webview.controller();
                    if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
                        let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                            R: 0, G: 0, B: 0, A: 0,
                        });
                    }
                });
            }
            let _ = &countdown_win;

            // Pre-create the drag-select snip overlay hidden. Same pattern
            // as fillin / clipboard / radial / countdown — transparent,
            // decorations off, always-on-top, skip taskbar. Sized 1×1 here;
            // show_snip_overlay resizes to full virtual desktop before
            // showing so the drag surface covers every monitor. Reused by
            // any macro step that needs the user to pick a screen rect
            // (Wait for Text today, future Wait for Image / template
            // capture). Reusable across features — do not put step-specific
            // logic in the overlay itself.
            let snip_url = tauri::WebviewUrl::App("index.html?snipoverlay=1".into());
            let snip_win = tauri::WebviewWindowBuilder::new(app, "snipoverlay", snip_url)
                .additional_browser_args(WEBVIEW_BROWSER_ARGS)
                .title("Keyfire Snip Overlay")
                // Initial size is a placeholder — show_snip_overlay resizes
                // to full virtual desktop before show. Using 100×100 here
                // rather than 1×1 because some WebView2 versions refuse to
                // initialise a sub-pixel window and show "can't reach this
                // page" until first resize.
                .inner_size(100.0, 100.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
                .shadow(false)
                .build()?;

            #[cfg(target_os = "windows")]
            {
                let _ = snip_win.with_webview(|webview| unsafe {
                    use webview2_com::Microsoft::Web::WebView2::Win32::{
                        ICoreWebView2Controller2, COREWEBVIEW2_COLOR,
                    };
                    use windows_core::Interface;
                    let controller = webview.controller();
                    if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
                        let _ = controller2.SetDefaultBackgroundColor(COREWEBVIEW2_COLOR {
                            R: 0, G: 0, B: 0, A: 0,
                        });
                    }
                });
            }
            let _ = &snip_win;

            // Pre-create the Settings window hidden. Unlike the overlays it is
            // an ordinary opaque window (no transparency, no NOACTIVATE, has a
            // taskbar entry) — undecorated so SettingsWindow.jsx can draw the
            // app-style titlebar with a drag region. Never destroyed: the
            // CloseRequested handler hides it instead.
            let settings_url = tauri::WebviewUrl::App("index.html?settings=1".into());
            let settings_win = tauri::WebviewWindowBuilder::new(app, "settings", settings_url)
                .additional_browser_args(WEBVIEW_BROWSER_ARGS)
                .title("Keyfire Settings")
                .inner_size(900.0, 640.0)
                .min_inner_size(720.0, 520.0)
                .decorations(false)
                .resizable(true)
                .visible(false)
                .center()
                .build()?;
            let _ = &settings_win;

            // Store app handle for fill-in IPC from the expansion engine
            expansions::init_app_handle(app.handle().clone());

            // Start global input hooks on dedicated high-priority thread
            hotkeys::start_hooks(app.handle().clone());

            // Listen for overlay toggle from the hotkey system
            let app_handle = app.handle().clone();
            app.listen("toggle-overlay", move |_| {
                // Rust listeners run on the EMITTING thread — here the hotkey
                // processor. show_overlay reads + parses the config file and
                // serialises every assignment for the payload, which stalled
                // hotkey/expansion dispatch for the whole show. Do it off-thread.
                let handle = app_handle.clone();
                std::thread::Builder::new()
                    .name("keyfire-overlay-toggle".into())
                    .spawn(move || {
                        let overlay_visible = handle
                            .get_webview_window("overlay")
                            .and_then(|w| w.is_visible().ok())
                            .unwrap_or(false);
                        if overlay_visible {
                            hide_overlay(&handle);
                            restore_overlay_target();
                        } else {
                            show_overlay(&handle);
                        }
                    })
                    .ok();
            });

            // voice-open: first press of voice hotkey — VOICE_ACTIVE was false in the hook
            let app_handle_voice_open = app.handle().clone();
            app.listen("voice-open", move |_| {
                show_voice_overlay(&app_handle_voice_open);
            });

            // voice-keydown: hotkey pressed while voice overlay is active — always close.
            // Continuous mode is now toggled by clicking the overlay, not the hotkey.
            let app_handle_voice = app.handle().clone();
            app.listen("voice-keydown", move |_| {
                VOICE_CONTINUOUS.store(false, AtomicOrdering::SeqCst);
                hide_overlay(&app_handle_voice);
                restore_overlay_target();
            });

            // Listen for clipboard overlay toggle from hotkey system.
            // The CLIPBOARD_OVERLAY_VISIBLE atomic (not win.is_visible())
            // decides the toggle: it flips true at the TOP of the show path,
            // so a rapid second press during the show latency hides instead
            // of double-showing, and there's no main-thread round-trip.
            let app_handle_clip = app.handle().clone();
            app.listen("toggle-clipboard-overlay", move |_| {
                if hotkeys::CLIPBOARD_OVERLAY_VISIBLE.load(AtomicOrdering::SeqCst) {
                    hide_clipboard_overlay(&app_handle_clip);
                } else {
                    show_clipboard_overlay(&app_handle_clip);
                }
            });

            // Fill-in variant of the toggle — LL hook emits this when the
            // clipboard-paste combo fires while a fill-in window is up. Routes
            // to show_clipboard_overlay_for_fillin so the fill-in-mode flag is
            // set and paste goes via `fillin-insert-text` event instead of
            // Ctrl+V injection into the wrong window.
            let app_handle_clip_fill = app.handle().clone();
            app.listen("toggle-clipboard-overlay-for-fillin", move |_| {
                if hotkeys::CLIPBOARD_OVERLAY_VISIBLE.load(AtomicOrdering::SeqCst) {
                    hide_clipboard_overlay(&app_handle_clip_fill);
                } else {
                    show_clipboard_overlay_for_fillin_impl(&app_handle_clip_fill);
                }
            });

            // Outside-click dismissal: the mouse hook detects a click outside the
            // overlay's window rect and emits these events. Needed because the
            // blur-based path doesn't fire on the first outside click when the
            // window never grabbed OS focus on show.
            let app_handle_oc_search = app.handle().clone();
            app.listen("close-overlay-outside-click", move |_| {
                hide_overlay(&app_handle_oc_search);
                restore_overlay_target();
            });
            let app_handle_oc_clip = app.handle().clone();
            app.listen("close-clipboard-overlay-outside-click", move |_| {
                hide_clipboard_overlay(&app_handle_oc_clip);
            });

            // Listen for radial menu toggle from hotkey system
            let app_handle_radial = app.handle().clone();
            app.listen("toggle-radial-menu", move |_| {
                let visible = app_handle_radial
                    .get_webview_window("radialmenu")
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(false);
                if visible {
                    hide_radial_menu(&app_handle_radial);
                    restore_radial_menu_target();
                } else {
                    show_radial_menu(&app_handle_radial);
                }
            });

            // Start foreground watcher for app-specific profile switching
            foreground::start_watcher(app.handle().clone());

            // Park hidden windows (release their rendering resources) and
            // suspend long-idle overlays. See webview_mem.rs.
            webview_mem::start(app.handle().clone());

            // Esc-cancel clock: first use must not be inside the LL hook.
            actions::init_cancel_clock();

            // Autolaunch: if --autolaunch flag, keep window hidden (tray only)
            // Normal launch: show window
            if !tray::is_autolaunch() {
                if let Some(window) = app.get_webview_window("main") {
                    // Belt-and-braces: never show a window webview_mem may have parked.
                    webview_mem::resume_for_show(app.handle(), "main");
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            } else {
                log::info!("[Keyfire] Autolaunch mode — starting hidden");
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            let label = window.label();
            if label == "main" {
                tray::handle_window_event(window, event);
            } else if label == "overlay" {
                // Auto-hide overlay on blur (clicking outside)
                if let tauri::WindowEvent::Focused(false) = event {
                    // Voice mode: the WinRT recognizer briefly steals focus during init — never auto-close on blur
                    if hotkeys::is_voice_active() {
                        return;
                    }
                    // Guard: don't dismiss within 300ms of showing (prevents immediate dismiss)
                    let should_hide = overlay_show_time()
                        .lock()
                        .ok()
                        .and_then(|t| *t)
                        .map(|t| t.elapsed() > std::time::Duration::from_millis(300))
                        .unwrap_or(true);
                    if should_hide {
                        hide_overlay(window.app_handle());
                    }
                }
            } else if label == "clipboardoverlay" {
                if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                    let hwnd = CLIPBOARD_OVERLAY_TARGET.load(std::sync::atomic::Ordering::SeqCst);
                    if hwnd != 0 {
                        actions::set_foreground_robust(hwnd);
                    }
                }
            } else if label == "radialmenu" {
                if let tauri::WindowEvent::Focused(false) = event {
                    // If the radial hotkey is still physically held (hold-to-select),
                    // don't hide — the keyup handler will close the menu when released.
                    if hotkeys::is_radial_menu_held() {
                        return;
                    }
                    let should_hide = radial_menu_show_time()
                        .lock()
                        .ok()
                        .and_then(|t| *t)
                        .map(|t| t.elapsed() > std::time::Duration::from_millis(300))
                        .unwrap_or(true);
                    if should_hide {
                        hide_radial_menu(window.app_handle());
                        restore_radial_menu_target();
                    }
                }
            } else if label == "settings" {
                // Never destroy the pre-created settings window — hide it so
                // the next open is instant and React state survives.
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            } else if label == "fillin" {
                // Prevent fill-in window from being destroyed — hide and send cancel response
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                    // Send None (cancel) through the fill-in channel so the waiting thread unblocks
                    if let Ok(guard) = expansions::fill_in_tx().lock() {
                        if let Some(ref tx) = *guard {
                            let _ = tx.send(None);
                        }
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            // Config
            load_config,
            save_config,
            get_config_path,
            get_shared_config_path,
            set_shared_config_path,
            clear_shared_config_path,
            export_config,
            import_config,
            commit_import_config,
            list_backups,
            restore_backup,
            // Engine
            get_engine_status,
            update_assignments,
            toggle_macros,
            release_input_for_exit,
            input_focus_changed,
            show_settings_window,
            hide_settings_window,
            toggle_settings_window,
            start_hotkey_recording,
            stop_hotkey_recording,
            start_key_capture,
            stop_key_capture,
            js_key_event,
            // Macro recorder
            start_macro_recording,
            stop_macro_recording,
            discard_macro_recording,
            get_recording_status,
            distill_events,
            show_recorder_countdown,
            hide_recorder_countdown,
            recorder_countdown_complete,
            recorder_countdown_abort,
            recorder_stop_from_pill,
            recorder_hide_main,
            recorder_restore_main,
            // Profiles
            set_active_global_profile,
            update_profile_settings,
            get_foreground_process,
            set_editing_active,
            // Settings
            update_global_settings,
            update_autocorrect_enabled,
            update_autocorrect_settings,
            update_expansion_excluded_apps,
            export_text_file,
            import_text_file,
            get_builtin_autocorrect_entries,
            update_global_variables,
            list_audio_output_devices,
            set_audio_output_device,
            // Pause
            set_global_pause_key,
            clear_global_pause_key,
            set_clipboard_paste_key,
            clear_clipboard_paste_key,
            set_voice_hotkey,
            clear_voice_hotkey,
            set_temp_macro_record_hotkey,
            clear_temp_macro_record_hotkey,
            set_temp_macro_play_hotkey,
            clear_temp_macro_play_hotkey,
            set_temp_macro_loop_hotkey,
            clear_temp_macro_loop_hotkey,
            get_temp_macro_status,
            clear_temp_macro,
            start_voice_recognition,
            stop_voice_recognition,
            start_voice_continuous,
            stop_voice_continuous,
            check_hotkey_conflict,
            // Window
            window_minimize,
            window_maximize,
            window_close,
            show_window,
            hide_window,
            set_window_resizable,
            quit_app,
            // File dialogs
            browse_for_file,
            browse_for_image,
            browse_for_audio,
            browse_for_video,
            get_app_icon,
            get_app_icon_by_name,
            list_installed_apps,
            browse_for_folder,
            read_image_base64,
            // Profile export/import
            export_profile,
            import_profile,
            // Window enumeration
            list_open_windows,
            get_cursor_position,
            get_cursor_pixel,
            get_pixel_color,
            set_pixel_pick_active,
            get_snip_overlay_config,
            get_search_overlay_data,
            get_radial_menu_data,
            show_snip_overlay,
            hide_snip_overlay,
            emit_snip_result,
            emit_snip_cancelled,
            enum_monitors,
            show_monitor_identify,
            hide_monitor_identify,
            // Startup
            get_startup_enabled,
            set_startup_enabled,
            get_app_version,
            get_keyboard_layout_hint,
            get_keyboard_legends,
            // Help / External
            open_help,
            open_config_folder,
            open_logs_folder,
            open_clipboard_folder,
            open_external,
            log_debug,
            // Overlay
            close_overlay,
            overlay_resize,
            save_overlay_position,
            reset_overlay_position,
            voice_overlay_error_expand,
            voice_overlay_examples_expand,
            set_voice_continuous,
            execute_search_result,
            update_search_settings,
            // Radial Menu
            set_radial_menu_hotkey,
            clear_radial_menu_hotkey,
            set_radial_hold_to_select,
            close_radial_menu,
            radial_menu_resize,
            execute_radial_menu_item,
            // Onboarding
            reset_onboarding,
            // Analytics
            get_analytics,
            reset_analytics,
            get_daily_chart,
            get_assignment_breakdown,
            get_type_breakdown,
            get_hourly_heatmap,
            get_top_apps,
            get_expansion_efficiency,
            get_expansion_counts,
            get_streaks,
            export_analytics_xlsx,
            export_analytics_pdf,
            analytics_report_ready,
            // Clipboard
            get_clipboard_history,
            get_theme,
            paste_clipboard_item,
            paste_text,
            copy_clipboard_item,
            copy_text,
            ocr_clipboard_image,
            get_clipboard_image_colors,
            save_clipboard_image_as,
            delete_clipboard_item,
            clear_clipboard_history,
            pin_clipboard_item,
            star_clipboard_item,
            reorder_clipboard_pinned,
            reorder_clipboard_starred,
            create_clipboard_folder,
            rename_clipboard_folder,
            delete_clipboard_folder,
            move_clipboard_item_to_folder,
            get_clipboard_folders,
            get_clipboard_image,
            get_distinct_source_apps,
            get_clipboard_date_buckets,
            update_clipboard_item,
            get_clipboard_settings,
            set_clipboard_settings,
            set_clipboard_ocr_settings,
            backfill_clipboard_ocr,
            backfill_clipboard_thumbnails,
            get_clipboard_ocr_text,
            get_clipboard_item_text_full,
            set_clipboard_capture_enabled,
            set_clipboard_excluded_apps,
            get_clipboard_storage_size,
            get_clipboard_encryption_status,
            delete_clipboard_plaintext_backup,
            reset_clipboard_storage,
            // Telemetry opt-out
            get_telemetry_enabled,
            set_telemetry_enabled,
            close_clipboard_overlay,
            show_clipboard_overlay_for_fillin,
            clipboard_overlay_resize,
            // Updater
            check_for_updates,
            install_update,
            start_download,
            // Fill-in
            fill_in_ready,
            fill_in_shown_ack,
            fillin_resize,
            fill_in_submit,
            // Licence
            get_licence_status,
            activate_licence,
            deactivate_licence,
            check_licence_revalidation,
            start_trial,
            mark_trial_offer_shown,
            reset_trial,
            dev_set_pro_override,
            is_debug_build,
            get_grace_period_state,
            migrate_shared_to_local_now,
            // Demo mode + persistent profiles
            get_demo_mode,
            get_profile_mode,
            relaunch_demo_mode,
            relaunch_profile_mode,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Best-effort demo-data wipe on any exit path (tray Quit, exit-demo
            // relaunch, app.exit anywhere). The DB writer threads may still
            // hold the demo .db files open at this point — a partial delete is
            // fine because every demo LAUNCH wipes the folder first anyway.
            if let tauri::RunEvent::Exit = event {
                // Every exit path (tray Quit, updater relaunch, app.exit from
                // anywhere) releases synthetic input state. Injected key state
                // survives process exit: a Hold-mode key or repeat left DOWN
                // stayed down in Windows until the user tapped it physically,
                // and spawned AHK children were orphaned. Idempotent with quit_app.
                actions::release_held_key();
                actions::stop_repeating_key();
                actions::release_all_bare_remaps();
                actions::kill_all_ahk_processes();
                if is_demo_mode() {
                    if let Ok(dir) = app_handle.path().app_data_dir() {
                        match std::fs::remove_dir_all(dir.join("demo")) {
                            Ok(()) => log::info!("[Keyfire] Demo data wiped on exit"),
                            Err(e) => log::warn!(
                                "[Keyfire] Demo cleanup on exit incomplete ({}) — next demo launch wipes it",
                                e
                            ),
                        }
                    }
                }
            }
        });
}
