use serde_json::Value;
use tauri::{Emitter, Listener, Manager};

mod actions;
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
fn open_help(app: tauri::AppHandle) {
    // If help window already exists, just show and focus it
    if let Some(win) = app.get_webview_window("help") {
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }
    let _ = tauri::WebviewWindowBuilder::new(
        &app,
        "help",
        tauri::WebviewUrl::App("help.html".into()),
    )
    .title("Trigr — User Guide")
    .inner_size(1000.0, 700.0)
    .resizable(true)
    .build();
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
                        actions::execute_action(&macro_val, false, target_hwnd, false);
                    }
                }
            }
            "expansion" | "autocorrect" => {
                if let Some(raw_text) = result.get("text").and_then(|v| v.as_str()) {
                    // Resolve dynamic tokens ({date:...}, {time:...}, {clipboard}, {cursor}, etc.)
                    let global_vars = expansions::get_global_variables();
                    let (resolved, cursor_back) = expansions::resolve_tokens(raw_text, &global_vars);

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
            config::init(app_data);

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
            browse_for_folder,
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
