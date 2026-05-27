use serde_json::Value;
use tauri::{Emitter, Listener, Manager};

mod actions;
mod analytics;
mod clipboard;
mod config;
mod expansions;
mod foreground;
mod hotkeys;
mod licence;
mod ocr;
mod tray;
mod voice;

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
        // Voice phrases live inside the assignments blob, which is part of every save.
        // Pre-warm asynchronously so the next recognition is cache-hit fast.
        voice::prewarm_from_state();
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
    // If the user manually unsets shared config, any grace-period timestamp
    // is moot — clear it so the banner disappears immediately.
    let _ = config::set_pro_expired_at(None);
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

// Enumerate installed apps via PowerShell's Get-StartApps. Returns an array
// of { name, appId } where appId is the AUMID (for Store/UWP apps) or the
// folder-GUID-prefixed path (for Win32 apps with Start Menu shortcuts). Both
// forms can be launched portably across devices via `shell:AppsFolder\<appId>`.
#[tauri::command]
fn list_installed_apps() -> Value {
    use std::process::Command;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-StartApps | ConvertTo-Json -Compress",
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .output();

    let output = match output {
        Ok(o) => o,
        Err(e) => {
            log::warn!("[Trigr] list_installed_apps: failed to run PowerShell: {}", e);
            return Value::Array(vec![]);
        }
    };

    if !output.status.success() {
        log::warn!(
            "[Trigr] list_installed_apps: PowerShell exited non-zero (stderr: {})",
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
            log::warn!("[Trigr] list_installed_apps: unexpected JSON shape");
            return Value::Array(vec![]);
        }
        Err(e) => {
            log::warn!("[Trigr] list_installed_apps: JSON parse error: {}", e);
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
            Some(serde_json::json!({ "name": name, "appId": app_id }))
        })
        .collect();

    // Sort case-insensitive by name for a stable picker order.
    apps.sort_by(|a, b| {
        let an = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let bn = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
        an.to_lowercase().cmp(&bn.to_lowercase())
    });

    log::info!("[Trigr] list_installed_apps: returned {} apps", apps.len());
    Value::Array(apps)
}

#[tauri::command]
fn get_app_icon(path: String) -> Value {
    use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};
    use windows_sys::Win32::Graphics::Gdi::{
        GetDIBits, DeleteObject, CreateCompatibleDC, DeleteDC, GetObjectW,
        BITMAPINFO, BITMAPINFOHEADER, BITMAP, BI_RGB, DIB_RGB_COLORS,
    };

    // SHGetFileInfoW is in Shell — define it manually since the feature may not expose it
    #[link(name = "shell32")]
    extern "system" {
        fn SHGetFileInfoW(
            pszPath: *const u16,
            dwFileAttributes: u32,
            psfi: *mut SHFILEINFOW,
            cbFileInfo: u32,
            uFlags: u32,
        ) -> usize;
    }

    #[repr(C)]
    #[allow(non_snake_case)]
    struct SHFILEINFOW {
        hIcon: *mut std::ffi::c_void,
        iIcon: i32,
        dwAttributes: u32,
        szDisplayName: [u16; 260],
        szTypeName: [u16; 80],
    }

    const SHGFI_ICON: u32 = 0x000000100;
    const SHGFI_LARGEICON: u32 = 0x000000000;

    let wide_path: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let mut shfi: SHFILEINFOW = std::mem::zeroed();
        let result = SHGetFileInfoW(
            wide_path.as_ptr(),
            0,
            &mut shfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if result == 0 || shfi.hIcon.is_null() {
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

#[tauri::command]
fn get_cursor_position() -> Value {
    let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
    unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
    }
    serde_json::json!({ "x": point.x, "y": point.y })
}

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
fn update_assignments(assignments: Value, profile: String) {
    // Convert Value map to HashMap
    let map: std::collections::HashMap<String, Value> = assignments
        .as_object()
        .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();
    hotkeys::update_assignments(map.clone(), profile);
    expansions::update_assignments(map);
    // Voice phrase grammar may have changed — pre-warm asynchronously
    voice::prewarm_from_state();
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

#[tauri::command]
fn start_voice_recognition(phrases: Vec<String>, app: tauri::AppHandle) {
    voice::start_recognition(phrases, app);
}

#[tauri::command]
fn start_voice_continuous(phrases: Vec<String>, app: tauri::AppHandle) {
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
    let state = hotkeys::engine_state().lock().unwrap();

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

// ── Help / External (Phase 3) ───────────────────────────────────────────────

#[tauri::command]
fn open_help() {
    let _ = opener::open("https://usetrigr.com/trigr-help.html");
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
        log::warn!("[Trigr] open_clipboard_folder: clipboard module not initialised yet");
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

/// Whether voice mode is locked in continuous mode. Set by the pill click via
/// the set_voice_continuous Tauri command. Cleared by hide_overlay() on close.
static VOICE_CONTINUOUS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Timestamp when the voice overlay was opened — used for double-tap detection.
static VOICE_OVERLAY_OPEN_TIME: std::sync::OnceLock<StdMutex<Option<StdInstant>>> = std::sync::OnceLock::new();

fn voice_overlay_open_time() -> &'static StdMutex<Option<StdInstant>> {
    VOICE_OVERLAY_OPEN_TIME.get_or_init(|| StdMutex::new(None))
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
    let search_data = {
        let state = hotkeys::engine_state().lock().unwrap();
        serde_json::json!({
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
            "voiceEnabled": cfg.get("voiceCommandsEnabled").and_then(|v| v.as_bool()).unwrap_or(true)
        })
    };
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

fn show_voice_overlay(app: &tauri::AppHandle) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetForegroundWindow};
    use windows_sys::Win32::Foundation::POINT;

    log::info!("[Trigr] show_voice_overlay: START");

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

    let scale = overlay.scale_factor().unwrap_or(1.0);
    let log_left = wa_left as f64 / scale;
    let log_top = wa_top as f64 / scale;
    let log_w = (wa_right - wa_left) as f64 / scale;
    let log_h = (wa_bottom - wa_top) as f64 / scale;
    // Compact square — bottom-centre, above taskbar
    let win_w = 72.0_f64;
    let win_h = 72.0_f64;
    let x = log_left + (log_w - win_w) / 2.0;
    let y = log_top + log_h - win_h - 12.0; // 12px above taskbar
    let _ = overlay.set_position(tauri::LogicalPosition::new(x, y));
    let _ = overlay.set_size(tauri::LogicalSize::new(win_w, win_h));

    // Send voice data to overlay
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let voice_data = {
        let state = hotkeys::engine_state().lock().unwrap();
        serde_json::json!({
            "assignments": state.assignments,
            "activeProfile": state.active_profile,
            "theme": cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark"),
            "voiceMicId": cfg.get("voiceMicId").and_then(|v| v.as_str()).unwrap_or(""),
        })
    };
    log::info!("[Trigr] show_voice_overlay: emitting overlay-voice-data");
    let _ = overlay.emit("overlay-voice-data", voice_data);

    // Brief pause so the frontend can commit React state resets (voiceContinuous=false etc.)
    // before the window becomes visible and clickable. Imperceptible — window is hidden.
    std::thread::sleep(std::time::Duration::from_millis(30));

    log::info!("[Trigr] show_voice_overlay: showing window");
    let voice_open_now = StdInstant::now();
    *overlay_show_time().lock().unwrap() = Some(voice_open_now);
    *voice_overlay_open_time().lock().unwrap() = Some(voice_open_now);
    let _ = overlay.show();
    let _ = overlay.set_focus();
    log::info!("[Trigr] show_voice_overlay: DONE");
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

    // 730px panel + 12px shadow breathing room each side (24px total)
    let win_w = 754.0;
    let x = log_left + (log_w - win_w) / 2.0;
    let y = log_top + log_h / 3.0;
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
    let _ = win.set_size(tauri::LogicalSize::new(win_w, 500.0));

    // Send recent clipboard history + theme to the overlay
    let history = clipboard::get_history(1, 500, None, None, None, None);
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let theme = cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark");
    let mut payload = history;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("theme".to_string(), serde_json::Value::String(theme.to_string()));
    }
    use tauri::Emitter;
    let _ = win.emit("clipboard-overlay-data", payload);

    let _ = win.show();
    // No set_focus() — WS_EX_NOACTIVATE prevents focus steal; keyboard routed via LL hook.
    crate::hotkeys::CLIPBOARD_OVERLAY_VISIBLE.store(true, std::sync::atomic::Ordering::SeqCst);
    // Track HWND so the mouse hook can dismiss on click-outside (blur won't fire
    // because WS_EX_NOACTIVATE means the window never receives focus on show).
    if let Ok(hwnd) = win.hwnd() {
        crate::hotkeys::CLIPBOARD_OVERLAY_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
    }
}

fn hide_clipboard_overlay(app: &tauri::AppHandle) {
    crate::hotkeys::CLIPBOARD_OVERLAY_VISIBLE.store(false, std::sync::atomic::Ordering::SeqCst);
    crate::hotkeys::CLIPBOARD_OVERLAY_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
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

// ── Radial menu overlay show/hide ──────────────────────────────────────────

static RADIAL_MENU_TARGET_HWND: AtomicIsize = AtomicIsize::new(0);

static RADIAL_MENU_SHOW_TIME: std::sync::OnceLock<StdMutex<Option<StdInstant>>> =
    std::sync::OnceLock::new();

fn radial_menu_show_time() -> &'static StdMutex<Option<StdInstant>> {
    RADIAL_MENU_SHOW_TIME.get_or_init(|| StdMutex::new(None))
}

fn show_radial_menu(app: &tauri::AppHandle) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetCursorPos};
    use windows_sys::Win32::Foundation::POINT;

    // Force an immediate foreground/profile check so the radial menu
    // uses the correct profile even if the 1500ms poll hasn't fired yet.
    foreground::force_check(app);

    let target = unsafe { GetForegroundWindow() as isize };
    RADIAL_MENU_TARGET_HWND.store(target, std::sync::atomic::Ordering::SeqCst);

    let win = match app.get_webview_window("radialmenu") {
        Some(w) => w,
        None => return,
    };

    // Position: centre 620x620 window on cursor
    let (cx, cy) = unsafe {
        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        (pt.x, pt.y)
    };

    let scale = win.scale_factor().unwrap_or(1.0);

    let win_size = 525.0;
    // Always centre on cursor — no clamping to work area.
    // Items near screen edges may be clipped, but the cursor stays
    // at the wheel centre which preserves muscle memory.
    let x = cx as f64 / scale - win_size / 2.0;
    let y = cy as f64 / scale - win_size / 2.0;
    let _ = win.set_position(tauri::LogicalPosition::new(x, y));
    let _ = win.set_size(tauri::LogicalSize::new(win_size, win_size));

    // Build payload: resolve radial menu items for the CURRENT active profile.
    // Use radialMenuItemsByProfile[activeProfile] rather than the flat radialMenuItems
    // array, which may be stale if a profile switch hasn't flushed to disk yet.
    let cfg = config::load_config().unwrap_or_else(|| serde_json::json!({}));
    let theme = cfg.get("theme").and_then(|v| v.as_str()).unwrap_or("dark");
    let state = hotkeys::engine_state().lock().unwrap();
    let active_profile = state.active_profile.clone();
    let radial_items = cfg
        .get("radialMenuItemsByProfile")
        .and_then(|m| m.get(&active_profile))
        .cloned()
        .or_else(|| cfg.get("radialMenuItems").cloned())
        .unwrap_or_else(|| serde_json::json!([]));
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

    let payload = serde_json::json!({
        "items": resolved_items,
        "theme": theme,
    });
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
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd as _);
        }
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
    let h = height.max(60.0).min(400.0);
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.set_size(tauri::LogicalSize::new(620.0, h));
    }
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

    match result_type {
        "assignment" | "quickaction" => {
            if let Some(storage_key) = result.get("storageKey").and_then(|v| v.as_str()) {
                let state = hotkeys::engine_state().lock().unwrap();
                if let Some(macro_val) = state.assignments.get(storage_key).cloned() {
                    drop(state);
                    actions::execute_action(&macro_val, false, target_hwnd, false, Some(storage_key), app);
                    let at = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("hotkey");
                    let label = macro_val.get("label").and_then(|v| v.as_str()).unwrap_or("");
                    let macro_steps = if at == "macro" {
                        macro_val.get("data")
                            .and_then(|d| d.get("steps"))
                            .and_then(|s| s.as_array())
                            .map(|arr| arr.iter().filter_map(|s| s.get("type").and_then(|v| v.as_str()).map(String::from)).collect())
                    } else { None };
                    analytics::log_action_ext(at, 0, storage_key, label, macro_steps);
                }
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
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                            target_hwnd as _,
                        );
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                // fire_expansion_by_trigger handles image / variant / fill-in /
                // plain text and logs analytics in each sub-path itself.
                expansions::fire_expansion_by_trigger(trigger);
            }
        }
        "autocorrect" => {
            // Autocorrect entries are simple replacements (no fill-in, no
            // variants) keyed under a different namespace, so keep the direct
            // token-resolve + clipboard-paste path here.
            if let Some(raw_text) = result.get("text").and_then(|v| v.as_str()) {
                // Resolve dynamic tokens ({date:...}, {time:...}, {clipboard}, {cursor}, etc.)
                let global_vars = expansions::get_global_variables();
                let (resolved, cursor_back) = expansions::resolve_tokens(raw_text, &global_vars);

                let trigger = result.get("trigger").and_then(|v| v.as_str()).unwrap_or("");
                analytics::log_action("expansion", resolved.chars().filter(|c| *c != '\r').count() as u32, trigger, trigger);

                actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                let _suppress = actions::SuppressionGuard::new();

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
                drop(_suppress); // SUPPRESS_SIMULATED = false (even on panic)

                std::thread::sleep(std::time::Duration::from_millis(50));
                actions::write_clipboard_pub(&prev);
                actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                    .store(false, std::sync::atomic::Ordering::Relaxed);
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
            }
        }
        _ => {}
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

#[tauri::command]
fn get_daily_chart(days: u32) -> Value {
    analytics::get_daily_chart(days)
}

#[tauri::command]
fn get_assignment_breakdown(days: Option<u32>) -> Value {
    analytics::get_assignment_breakdown(days.unwrap_or(0))
}

#[tauri::command]
fn get_type_breakdown(days: Option<u32>) -> Value {
    analytics::get_type_breakdown(days.unwrap_or(0))
}

#[tauri::command]
fn get_hourly_heatmap(days: Option<u32>) -> Value {
    analytics::get_hourly_heatmap(days.unwrap_or(7))
}

#[tauri::command]
fn get_top_apps(days: Option<u32>) -> Value {
    analytics::get_top_apps(days.unwrap_or(0))
}

#[tauri::command]
fn get_expansion_efficiency() -> Value {
    analytics::get_expansion_efficiency()
}

#[tauri::command]
fn get_expansion_counts() -> Value {
    analytics::get_expansion_counts()
}

#[tauri::command]
fn get_streaks() -> Value {
    analytics::get_streaks()
}

#[tauri::command]
fn export_analytics_csv() -> String {
    analytics::export_csv()
}

// ── Clipboard Manager ──────────────────────────────────────────────────────

#[tauri::command]
fn get_clipboard_history(
    page: u32,
    per_page: u32,
    date_filter: Option<String>,
    app_filter: Option<String>,
    tag_filter: Option<String>,
    search: Option<String>,
) -> Value {
    clipboard::get_history(page, per_page, date_filter, app_filter, tag_filter, search)
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
        // Counter is incremented after the paste path succeeds (set inside the match below).
        let mut pasted_ok = false;
        // Hide the overlay first so focus transfer is clean
        std::thread::sleep(std::time::Duration::from_millis(30));

        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _suppress = actions::SuppressionGuard::new();

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
                    // If write fails (e.g. Excel holds clipboard lock), skip paste —
                    // pasting now would send whatever was already on the clipboard.
                    if !actions::write_clipboard_pub(text) {
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
fn paste_text(text: String, source_id: Option<i64>, _app: tauri::AppHandle) {
    if text.is_empty() {
        return;
    }
    let target_hwnd = CLIPBOARD_OVERLAY_TARGET.load(std::sync::atomic::Ordering::SeqCst);

    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(30));

        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _suppress = actions::SuppressionGuard::new();

        let held = actions::release_held_modifiers();

        if target_hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                    target_hwnd as _,
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
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
        actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(true, std::sync::atomic::Ordering::SeqCst);

        match item.content_type.as_str() {
            "text" => {
                if let Some(text) = &item.text_content {
                    actions::write_clipboard_pub(text);
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
#[tauri::command]
async fn ocr_clipboard_image(id: i64) -> Result<String, String> {
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
#[tauri::command]
fn get_clipboard_image_colors(id: i64) -> Vec<[u8; 3]> {
    let blob = match clipboard::get_image_blob(id) {
        Some(b) => b,
        None => return Vec::new(),
    };
    clipboard::dominant_colors(&blob, 5)
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
/// Self-contained version for the clipboard paste path.
fn write_image_to_clipboard(bgra_pixels: &[u8], width: u32, height: u32, png_bytes: &[u8]) {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

    const CF_DIB_: u32 = 8;

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

        CloseClipboard();
    }
}

#[tauri::command]
fn close_clipboard_overlay(app: tauri::AppHandle) {
    hide_clipboard_overlay(&app);
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
fn get_clipboard_date_buckets() -> Value {
    clipboard::get_date_buckets()
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

#[tauri::command]
fn set_clipboard_capture_enabled(enabled: bool) {
    clipboard::set_capture_enabled(enabled);
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

#[tauri::command]
fn fillin_resize(height: f64, app: tauri::AppHandle) {
    let h = height.max(150.0).min(600.0);
    if let Some(win) = app.get_webview_window("fillin") {
        let _ = win.set_size(tauri::LogicalSize::new(448.0, h));
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

// ── App builder ──────────────────────────────────────────────────────────────

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
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
            // Initialize config module with app data dir
            let app_data = app.path().app_data_dir()?;
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
            }
            clipboard::init(app_data.clone(), app.handle().clone());
            actions::cleanup_stale_ahk_scripts(app_data);

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
                .title("Trigr Radial Menu")
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
                        unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd as _);
                        }
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
            set_editing_active,
            // Settings
            update_global_settings,
            update_autocorrect_enabled,
            update_global_variables,
            // Pause
            set_global_pause_key,
            clear_global_pause_key,
            set_clipboard_paste_key,
            clear_clipboard_paste_key,
            set_voice_hotkey,
            clear_voice_hotkey,
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
            get_app_icon,
            list_installed_apps,
            browse_for_folder,
            read_image_base64,
            // Profile export/import
            export_profile,
            import_profile,
            // Window enumeration
            list_open_windows,
            get_cursor_position,
            // Startup
            get_startup_enabled,
            set_startup_enabled,
            get_app_version,
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
            voice_overlay_error_expand,
            voice_overlay_examples_expand,
            set_voice_continuous,
            execute_search_result,
            update_search_settings,
            // Radial Menu
            set_radial_menu_hotkey,
            clear_radial_menu_hotkey,
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
            export_analytics_csv,
            // Clipboard
            get_clipboard_history,
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
            get_clipboard_image,
            get_distinct_source_apps,
            get_clipboard_date_buckets,
            update_clipboard_item,
            get_clipboard_settings,
            set_clipboard_settings,
            set_clipboard_capture_enabled,
            set_clipboard_excluded_apps,
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
            // Licence
            get_licence_status,
            activate_licence,
            deactivate_licence,
            check_licence_revalidation,
            start_trial,
            mark_trial_offer_shown,
            reset_trial,
            get_grace_period_state,
            migrate_shared_to_local_now,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
