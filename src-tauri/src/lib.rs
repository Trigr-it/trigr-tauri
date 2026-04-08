use serde_json::Value;
use tauri::{Emitter, Listener, Manager};

mod actions;
mod analytics;
mod clipboard;
mod config;
mod expansions;
mod foreground;
mod hotkeys;
mod tray;

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
                // Healthy load — create timestamped backup snapshot
                config::create_timestamped_backup(&c);
            }
            c
        }
        None => {
            // Total config failure — write factory defaults so the file always exists
            log::warn!("[Trigr] All config sources failed — writing factory defaults");
            let defaults = serde_json::json!({
                "profiles": ["Default"],
                "assignments": {},
                "activeProfile": "Default",
            });
            config::save_config(&defaults);
            config::update_last_known_good(&defaults);
            defaults
        }
    }
}

#[tauri::command]
fn save_config(config: Value) -> bool {
    let existing = config::load_config().unwrap_or_else(|| serde_json::json!({}));

    // Merge incoming over existing, preserving fields not in incoming
    let merged = if let (Some(ex_obj), Some(in_obj)) =
        (existing.as_object(), config.as_object())
    {
        let mut m = ex_obj.clone();
        for (k, v) in in_obj {
            m.insert(k.clone(), v.clone());
        }
        Value::Object(m)
    } else {
        config.clone()
    };

    // Significant change? Back up existing first
    if config::is_significant_change(&config, &existing) {
        config::create_timestamped_backup(&existing);
    }

    let ok = config::save_config(&merged);
    if ok {
        config::update_last_known_good(&merged);
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
                    log::info!("[Trigr] Replaced shared config with current: {}", target_file.display());
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
                    log::info!("[Trigr] Copied config to shared location: {}", target_file.display());
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
    config::start_config_watcher(shared_dir, app);

    serde_json::json!({ "ok": true, "existed": existed })
}

#[tauri::command]
fn clear_shared_config_path() -> bool {
    config::stop_config_watcher();
    config::set_shared_config_dir(None);
    config::save_local_settings(None)
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
        .set_title("Export Trigr Config")
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
                    "[Trigr] Export — main config unreadable, using backup: {}",
                    rf
                );
            }
            match serde_json::to_string_pretty(&c) {
                Ok(json) => match std::fs::write(&file_path, json) {
                    Ok(()) => {
                        log::info!("[Trigr] Config exported to: {}", file_path.display());
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
        .set_title("Import Trigr Config")
        .add_filter("JSON", &["json"])
        .blocking_pick_file();

    let file_path = match file_path {
        Some(p) => p.into_path().unwrap(),
        None => return serde_json::json!({ "ok": false }),
    };

    match std::fs::read_to_string(&file_path) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(mut cfg) => {
                // Validate: must have assignments object
                if !cfg.is_object()
                    || !cfg
                        .get("assignments")
                        .map(|v| v.is_object())
                        .unwrap_or(false)
                {
                    return serde_json::json!({
                        "ok": false,
                        "error": "Invalid Trigr config file — missing assignments object."
                    });
                }

                // Backup current config before overwriting
                if let Some(current) = config::load_config() {
                    config::create_timestamped_backup(&current);
                }

                // Set hasSeenWelcome
                if let Some(obj) = cfg.as_object_mut() {
                    obj.insert("hasSeenWelcome".to_string(), Value::Bool(true));
                }

                // Write directly to disk
                if config::save_config(&cfg) {
                    config::update_last_known_good(&cfg);
                    log::info!("[Trigr] Config imported from: {}", file_path.display());
                    serde_json::json!({ "ok": true, "config": cfg })
                } else {
                    serde_json::json!({ "ok": false, "error": "Could not write imported config to disk." })
                }
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
        .set_title("Export Trigr Profile")
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
            log::info!("[Trigr] Profile exported to: {}", file_path.display());
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
        .set_title("Import Trigr Profile")
        .add_filter("JSON", &["json"])
        .blocking_pick_file();

    let file_path = match file_path {
        Some(p) => p.into_path().unwrap(),
        None => return serde_json::json!({ "ok": false }),
    };

    match std::fs::read_to_string(&file_path) {
        Ok(raw) => {
            log::info!("[Trigr] Profile file read from: {}", file_path.display());
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

// ── Window enumeration ─────────────────────────────────────────────────────

#[tauri::command]
fn list_open_windows() -> Vec<Value> {
    use std::collections::HashSet;
    use windows_sys::Win32::Foundation::CloseHandle as CloseHandleWin;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
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

        // Must be visible and not minimized
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
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
fn update_assignments(assignments: Value, profile: String) {
    // Convert Value map to HashMap
    let map: std::collections::HashMap<String, Value> = assignments
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    hotkeys::update_assignments(map.clone(), profile);
    expansions::update_assignments(map);
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

#[tauri::command]
fn start_hotkey_recording() {
    println!("[CAPTURE] start_hotkey_recording called");
    hotkeys::set_recording(true);
}

#[tauri::command]
fn stop_hotkey_recording() {
    println!("[CAPTURE] stop_hotkey_recording called");
    hotkeys::set_recording(false);
}

#[tauri::command]
fn start_key_capture() {
    println!("[CAPTURE] start_key_capture called");
    hotkeys::set_capturing(true);
}

#[tauri::command]
fn stop_key_capture() {
    println!("[CAPTURE] stop_key_capture called");
    hotkeys::set_capturing(false);
}

/// JS keydown forwarder — alternative capture path when Trigr's WebView2 has focus.
/// The LL hook can't see keypresses directed at the WebView2, so the JS keydown
/// listener in tauriAPI.js calls this command during recording/capture mode.
#[tauri::command]
fn js_key_event(code: String, ctrl: bool, shift: bool, alt: bool, meta: bool, app: tauri::AppHandle) {
    hotkeys::handle_js_key_event(&code, ctrl, shift, alt, meta, &app);
}

// ── Profiles (Phase 6) ──────────────────────────────────────────────────────

#[tauri::command]
fn set_active_global_profile(profile: String) {
    foreground::set_active_global_profile(profile.clone());
    hotkeys::set_active_profile(profile);
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
fn update_global_variables(vars: std::collections::HashMap<String, String>) {
    expansions::update_global_variables(vars);
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
fn check_hotkey_conflict(combo: String) -> Value {
    let _ = combo;
    serde_json::json!({ "conflict": false })
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

// ── Help / External (Phase 3) ───────────────────────────────────────────────

#[tauri::command]
fn open_help() {
    let _ = opener::open("https://trigr-it.github.io/trigr-tauri/trigr-help.html");
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
fn open_external(url: String) {
    let _ = opener::open(&url);
}

/// Generic JS→Rust debug logging — prints to terminal from any webview window.
#[tauri::command]
fn log_debug(message: String) {
    println!("{}", message);
}

// ── Overlay / Quick Search (Phase 9) ────────────────────────────────────────

use std::sync::atomic::{AtomicIsize, Ordering as AtomicOrdering};
use std::time::Instant as StdInstant;
use std::sync::Mutex as StdMutex;

/// HWND of the foreground window captured when the overlay was shown.
static OVERLAY_TARGET_HWND: AtomicIsize = AtomicIsize::new(0);

/// Timestamp when overlay was last shown — used for blur dismiss guard.
static OVERLAY_SHOW_TIME: std::sync::OnceLock<StdMutex<Option<StdInstant>>> = std::sync::OnceLock::new();

fn overlay_show_time() -> &'static StdMutex<Option<StdInstant>> {
    OVERLAY_SHOW_TIME.get_or_init(|| StdMutex::new(None))
}

fn show_overlay(app: &tauri::AppHandle) {
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

    // Get the work area of the monitor containing the cursor
    let (wa_left, wa_top, wa_right, wa_bottom) = unsafe {
        let pt = POINT { x: cx, y: cy };
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom)
        } else {
            (0, 0, 1920, 1080)
        }
    };

    // Convert physical monitor coords to logical using the window scale factor
    let scale = overlay.scale_factor().unwrap_or(1.0);
    let log_left = wa_left as f64 / scale;
    let log_top = wa_top as f64 / scale;
    let log_w = (wa_right - wa_left) as f64 / scale;
    let log_h = (wa_bottom - wa_top) as f64 / scale;

    // Centre on active monitor, one-third from top
    let win_w = 620.0;
    let x = log_left + (log_w - win_w) / 2.0;
    let y = log_top + log_h / 3.0;
    let _ = overlay.set_position(tauri::LogicalPosition::new(x, y));
    let _ = overlay.set_size(tauri::LogicalSize::new(620.0, 103.0));

    // Send search data to the overlay — includes ALL assignments (profile + global)
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let search_data = {
        let state = hotkeys::engine_state().lock().unwrap();
        serde_json::json!({
            "assignments": state.assignments,
            "activeProfile": state.active_profile,
            "globalInputMethod": cfg.get("globalInputMethod").and_then(|v| v.as_str()).unwrap_or("direct"),
            "theme": cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark"),
            "settings": {
                "showAll": cfg.get("overlayShowAll").and_then(|v| v.as_bool()).unwrap_or(true),
                "closeAfterFiring": cfg.get("overlayCloseAfterFiring").and_then(|v| v.as_bool()).unwrap_or(true),
                "includeAutocorrect": cfg.get("overlayIncludeAutocorrect").and_then(|v| v.as_bool()).unwrap_or(false),
            }
        })
    };
    let _ = overlay.emit("overlay-search-data", search_data);

    // Show and focus
    *overlay_show_time().lock().unwrap() = Some(StdInstant::now());
    let _ = overlay.show();
    let _ = overlay.set_focus();
}

fn hide_overlay(app: &tauri::AppHandle) {
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.hide();
    }
}

// ── Clipboard overlay show/hide ──────────────────────────────────────────

static CLIPBOARD_OVERLAY_TARGET: std::sync::atomic::AtomicIsize =
    std::sync::atomic::AtomicIsize::new(0);

fn show_clipboard_overlay(app: &tauri::AppHandle) {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    let target = unsafe { GetForegroundWindow() as isize };
    CLIPBOARD_OVERLAY_TARGET.store(target, std::sync::atomic::Ordering::SeqCst);

    let win = match app.get_webview_window("clipboardoverlay") {
        Some(w) => w,
        None => return,
    };

    // Position: center of active monitor, 1/3 from top (same pattern as search overlay)
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    let (wa_left, wa_top, wa_right, wa_bottom) = unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) != 0 {
            (
                mi.rcWork.left,
                mi.rcWork.top,
                mi.rcWork.right,
                mi.rcWork.bottom,
            )
        } else {
            (0, 0, 1920, 1080)
        }
    };

    let scale = win.scale_factor().unwrap_or(1.0);
    let log_left = wa_left as f64 / scale;
    let log_top = wa_top as f64 / scale;
    let log_w = (wa_right - wa_left) as f64 / scale;
    let log_h = (wa_bottom - wa_top) as f64 / scale;

    let win_w = 750.0;
    let x = log_left + (log_w - win_w) / 2.0;
    let y = log_top + log_h / 3.0;
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
    let _ = win.set_size(tauri::LogicalSize::new(750.0, 600.0));

    // Send recent clipboard history + theme to the overlay
    let history = clipboard::get_history(1, 15);
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let theme = cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark");
    let mut payload = history;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("theme".to_string(), serde_json::Value::String(theme.to_string()));
    }
    use tauri::Emitter;
    let _ = win.emit("clipboard-overlay-data", payload);

    let _ = win.show();
    let _ = win.set_focus();
}

fn hide_clipboard_overlay(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("clipboardoverlay") {
        let _ = win.hide();
    }
    let hwnd = CLIPBOARD_OVERLAY_TARGET.load(std::sync::atomic::Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd as _);
        }
    }
}

fn restore_overlay_target() {
    let hwnd = OVERLAY_TARGET_HWND.load(AtomicOrdering::Relaxed);
    if hwnd != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd as _);
        }
    }
}

#[tauri::command]
fn close_overlay(app: tauri::AppHandle) {
    hide_overlay(&app);
    restore_overlay_target();
}

#[tauri::command]
fn overlay_resize(height: f64, app: tauri::AppHandle) {
    let h = height.max(60.0).min(400.0);
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_size(tauri::LogicalSize::new(620.0, h));
    }
}

#[tauri::command]
fn execute_search_result(result: Value, app: tauri::AppHandle) {
    hide_overlay(&app);
    restore_overlay_target();

    let result_type = result.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let target_hwnd = OVERLAY_TARGET_HWND.load(AtomicOrdering::Relaxed);

    std::thread::spawn(move || {
        // Wait for focus transfer to target app
        std::thread::sleep(std::time::Duration::from_millis(180));

        match result_type.as_str() {
            "assignment" => {
                if let Some(storage_key) = result.get("storageKey").and_then(|v| v.as_str()) {
                    let state = hotkeys::engine_state().lock().unwrap();
                    if let Some(macro_val) = state.assignments.get(storage_key).cloned() {
                        drop(state);
                        actions::execute_action(&macro_val, false, target_hwnd, false, Some(storage_key), &app);
                        let at = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("hotkey");
                        let analytics_type = if at == "macro" { "macro" } else { "hotkey" };
                        analytics::log_action(analytics_type, 0);
                    }
                }
            }
            "expansion" | "autocorrect" => {
                if let Some(raw_text) = result.get("text").and_then(|v| v.as_str()) {
                    // Resolve dynamic tokens ({date:...}, {time:...}, {clipboard}, {cursor}, etc.)
                    let global_vars = expansions::get_global_variables();
                    let (resolved, cursor_back) = expansions::resolve_tokens(raw_text, &global_vars);

                    analytics::log_action("expansion", resolved.chars().filter(|c| *c != '\r').count() as u32);

                    actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    hotkeys::SUPPRESS_SIMULATED
                        .store(true, std::sync::atomic::Ordering::Relaxed);

                    let held = actions::release_held_modifiers();
                    if target_hwnd != 0 {
                        unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                                target_hwnd as _,
                            );
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
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
                    hotkeys::SUPPRESS_SIMULATED
                        .store(false, std::sync::atomic::Ordering::Relaxed);

                    std::thread::sleep(std::time::Duration::from_millis(50));
                    actions::write_clipboard_pub(&prev);
                    actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
            }
            _ => {}
        }
    });
}

#[tauri::command]
fn update_search_settings(settings: Value) {
    if let Some(hotkey) = settings.get("searchOverlayHotkey").and_then(|v| v.as_str()) {
        hotkeys::set_overlay_hotkey(hotkey);
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

// ── Clipboard Manager ──────────────────────────────────────────────────────

#[tauri::command]
fn get_clipboard_history(page: u32, per_page: u32) -> Value {
    clipboard::get_history(page, per_page)
}

#[tauri::command]
fn paste_clipboard_item(id: i64, _app: tauri::AppHandle) {
    let item = match clipboard::get_item_full(id) {
        Some(i) => i,
        None => return,
    };

    // Read stored target HWND — captured when the overlay was shown, before focus was stolen
    let target_hwnd = CLIPBOARD_OVERLAY_TARGET.load(std::sync::atomic::Ordering::SeqCst);

    std::thread::spawn(move || {
        // Hide the overlay first so focus transfer is clean
        std::thread::sleep(std::time::Duration::from_millis(30));

        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(true, std::sync::atomic::Ordering::SeqCst);
        hotkeys::SUPPRESS_SIMULATED
            .store(true, std::sync::atomic::Ordering::SeqCst);

        let held = actions::release_held_modifiers();

        // Restore focus to the original target app
        if target_hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                    target_hwnd as _,
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        match item.content_type.as_str() {
            "text" => {
                if let Some(text) = &item.text_content {
                    let prev = actions::read_clipboard_pub().unwrap_or_default();
                    actions::write_clipboard_pub(text);
                    std::thread::sleep(std::time::Duration::from_millis(10));

                    // Ctrl+V
                    actions::send_vk_key_pub(0xA2, false);
                    actions::send_vk_key_pub(0x56, false);
                    actions::send_vk_key_pub(0x56, true);
                    actions::send_vk_key_pub(0xA2, true);

                    actions::restore_modifiers(&held);

                    std::thread::sleep(std::time::Duration::from_millis(50));
                    if !prev.is_empty() {
                        actions::write_clipboard_pub(&prev);
                    }
                }
            }
            "image" => {
                if let Some(png_bytes) = &item.image_blob {
                    // Write PNG to clipboard using the expansion engine's image clipboard writer
                    // We need to decode to get dimensions and BGRA pixels for CF_DIB
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

                        // Write CF_DIB + PNG stream + CF_UNICODETEXT to clipboard
                        // Reuse the clipboard write pattern from expansions
                        write_image_to_clipboard(&bgra, width, height, png_bytes);

                        // Ctrl+V
                        actions::send_vk_key_pub(0xA2, false);
                        actions::send_vk_key_pub(0x56, false);
                        actions::send_vk_key_pub(0x56, true);
                        actions::send_vk_key_pub(0xA2, true);

                        actions::restore_modifiers(&held);
                    }
                }
            }
            _ => {}
        }

        hotkeys::SUPPRESS_SIMULATED
            .store(false, std::sync::atomic::Ordering::SeqCst);
        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(false, std::sync::atomic::Ordering::SeqCst);
    });
}

/// Write image to clipboard as CF_DIB + PNG stream + CF_UNICODETEXT.
/// Self-contained version for the clipboard paste path.
fn write_image_to_clipboard(bgra_pixels: &[u8], width: u32, height: u32, png_bytes: &[u8]) {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_DIB_: u32 = 8;
    const CF_UNICODETEXT_: u32 = 13;

    let header_size: u32 = 40;
    let pixel_data_size = bgra_pixels.len();
    let total_size = header_size as usize + pixel_data_size;

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

        // CF_UNICODETEXT empty
        let h_text = GlobalAlloc(GMEM_MOVEABLE, 2);
        if !h_text.is_null() {
            let tp = GlobalLock(h_text) as *mut u16;
            if !tp.is_null() {
                *tp = 0;
                GlobalUnlock(h_text);
                SetClipboardData(CF_UNICODETEXT_, h_text as _);
            }
        }

        CloseClipboard();
    }
}

#[tauri::command]
fn close_clipboard_overlay(app: tauri::AppHandle) {
    hide_clipboard_overlay(&app);
}

#[tauri::command]
fn clipboard_overlay_resize(height: f64, app: tauri::AppHandle) {
    let h = height.max(60.0).min(600.0);
    if let Some(win) = app.get_webview_window("clipboardoverlay") {
        let _ = win.set_size(tauri::LogicalSize::new(750.0, h));
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
fn pin_clipboard_item(id: i64, pinned: bool) -> bool {
    clipboard::pin_item(id, pinned)
}

#[tauri::command]
fn get_clipboard_image(id: i64) -> Option<String> {
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
fn update_clipboard_item(id: i64, new_text: String) -> Option<String> {
    clipboard::update_item(id, new_text)
}

#[tauri::command]
fn get_clipboard_settings() -> Value {
    serde_json::json!({
        "retention_days": clipboard::get_retention(),
        "enabled": true,
    })
}

#[tauri::command]
fn set_clipboard_settings(retention_days: u32) {
    clipboard::set_retention_days(retention_days);
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

#[tauri::command]
fn fillin_resize(height: f64, app: tauri::AppHandle) {
    let h = height.max(150.0).min(600.0);
    if let Some(win) = app.get_webview_window("fillin") {
        let _ = win.set_size(tauri::LogicalSize::new(440.0, h));
    }
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

// ── App builder ──────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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
        .setup(|app| {
            // Initialize config module with app data dir
            let app_data = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data)?;
            config::init(app_data.clone());
            analytics::init(app_data.clone());
            clipboard::init(app_data, app.handle().clone());

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
                                    log::info!("[Trigr] Shared config dir became available: {}", shared_dir.display());
                                    config::start_config_watcher(shared_dir, app_handle);
                                    break;
                                }
                            }
                        })
                        .ok();
                }
            }

            // Set up system tray
            if let Err(e) = tray::setup_tray(app) {
                log::error!("[Trigr] Failed to create tray: {}", e);
            }

            // Pre-create overlay window hidden — prevents frozen first launch
            let overlay_url = tauri::WebviewUrl::App("index.html?overlay=1".into());
            let overlay_win = tauri::WebviewWindowBuilder::new(app, "overlay", overlay_url)
                .title("Trigr Quick Search")
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
                .title("Trigr — Fill In")
                .inner_size(420.0, 300.0)
                .decorations(false)
                .transparent(true)
                .always_on_top(true)
                .skip_taskbar(true)
                .resizable(false)
                .visible(false)
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
                .title("Trigr Clipboard")
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
            // Suppress unused variable warning
            let _ = &clipoverlay_win;

            // Store app handle for fill-in IPC from the expansion engine
            expansions::init_app_handle(app.handle().clone());

            // Start global input hooks on dedicated high-priority thread
            hotkeys::start_hooks(app.handle().clone());

            // Listen for overlay toggle from the hotkey system
            let app_handle = app.handle().clone();
            app.listen("toggle-overlay", move |_| {
                let overlay_visible = app_handle
                    .get_webview_window("overlay")
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(false);
                if overlay_visible {
                    hide_overlay(&app_handle);
                    restore_overlay_target();
                } else {
                    show_overlay(&app_handle);
                }
            });

            // Listen for clipboard overlay toggle from hotkey system
            let app_handle_clip = app.handle().clone();
            app.listen("toggle-clipboard-overlay", move |_| {
                let visible = app_handle_clip
                    .get_webview_window("clipboardoverlay")
                    .and_then(|w| w.is_visible().ok())
                    .unwrap_or(false);
                if visible {
                    hide_clipboard_overlay(&app_handle_clip);
                } else {
                    show_clipboard_overlay(&app_handle_clip);
                }
            });

            // Start foreground watcher for app-specific profile switching
            foreground::start_watcher(app.handle().clone());

            // Autolaunch: if --autolaunch flag, keep window hidden (tray only)
            // Normal launch: show window
            if !tray::is_autolaunch() {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.set_focus();
                }
            } else {
                log::info!("[Trigr] Autolaunch mode — starting hidden");
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
                    // Guard: don't dismiss within 100ms of showing (prevents immediate dismiss)
                    let should_hide = overlay_show_time()
                        .lock()
                        .ok()
                        .and_then(|t| *t)
                        .map(|t| t.elapsed() > std::time::Duration::from_millis(300))
                        .unwrap_or(true);
                    if should_hide {
                        let _ = window.hide();
                    }
                }
            } else if label == "clipboardoverlay" {
                if let tauri::WindowEvent::Focused(false) = event {
                    let _ = window.hide();
                    let hwnd = CLIPBOARD_OVERLAY_TARGET.load(std::sync::atomic::Ordering::SeqCst);
                    if hwnd != 0 {
                        unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd as _);
                        }
                    }
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
            list_backups,
            restore_backup,
            // Engine
            get_engine_status,
            update_assignments,
            toggle_macros,
            input_focus_changed,
            start_hotkey_recording,
            stop_hotkey_recording,
            start_key_capture,
            stop_key_capture,
            js_key_event,
            // Profiles
            set_active_global_profile,
            update_profile_settings,
            get_foreground_process,
            // Settings
            update_global_settings,
            update_autocorrect_enabled,
            update_global_variables,
            // Pause
            set_global_pause_key,
            clear_global_pause_key,
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
            browse_for_folder,
            // Profile export/import
            export_profile,
            import_profile,
            // Window enumeration
            list_open_windows,
            // Startup
            get_startup_enabled,
            set_startup_enabled,
            get_app_version,
            // Help / External
            open_help,
            open_config_folder,
            open_logs_folder,
            open_external,
            log_debug,
            // Overlay
            close_overlay,
            overlay_resize,
            execute_search_result,
            update_search_settings,
            // Onboarding
            reset_onboarding,
            // Analytics
            get_analytics,
            reset_analytics,
            // Clipboard
            get_clipboard_history,
            paste_clipboard_item,
            delete_clipboard_item,
            clear_clipboard_history,
            pin_clipboard_item,
            get_clipboard_image,
            get_distinct_source_apps,
            update_clipboard_item,
            get_clipboard_settings,
            set_clipboard_settings,
            get_clipboard_storage_size,
            close_clipboard_overlay,
            clipboard_overlay_resize,
            // Updater
            check_for_updates,
            install_update,
            start_download,
            // Fill-in
            fill_in_ready,
            fillin_resize,
            fill_in_submit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
