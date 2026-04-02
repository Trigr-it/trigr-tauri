use log::info;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

static MACROS_ENABLED: AtomicBool = AtomicBool::new(true);
static HAS_SHOWN_BALLOON: AtomicBool = AtomicBool::new(false);

// ── Autolaunch detection ────────────────────────────────────────────────────

pub fn is_autolaunch() -> bool {
    std::env::args().any(|a| a == "--autolaunch")
}

// ── Tray setup ──────────────────────────────────────────────────────────────

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let tray_icon = load_tray_icon(app)?;
    build_tray(app.handle(), tray_icon)?;
    info!("[Trigr] System tray created");
    Ok(())
}

/// Decode a PNG file to RGBA bytes + dimensions.
fn decode_png(path: &std::path::Path) -> Result<Image<'static>, Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());

    // Convert RGB to RGBA if needed
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
            for chunk in buf.chunks(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        _ => buf, // Assume RGBA for other types
    };

    Ok(Image::new_owned(rgba, info.width, info.height))
}

fn load_tray_icon(app: &tauri::App) -> Result<Image<'static>, Box<dyn std::error::Error>> {
    // In production: bundled resource
    let resource_path = app
        .path()
        .resource_dir()
        .map(|d| d.join("icons").join("tray-icon.png"))
        .unwrap_or_default();

    if resource_path.exists() {
        return decode_png(&resource_path);
    }

    // Dev mode: assets/icons relative to project root
    let dev_path = std::env::current_dir()?
        .join("assets")
        .join("icons")
        .join("tray-icon.png");
    if dev_path.exists() {
        return decode_png(&dev_path);
    }

    // Final fallback: app icon
    let fallback = std::env::current_dir()?.join("icons").join("icon.png");
    decode_png(&fallback)
}

fn build_tray(
    app: &AppHandle,
    icon: Image<'static>,
) -> Result<(), Box<dyn std::error::Error>> {
    let enabled = MACROS_ENABLED.load(Ordering::Relaxed);

    // Menu items
    let open_item = MenuItem::with_id(app, "open", "Open Trigr", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;

    let pause_label = if enabled {
        "Pause Trigr"
    } else {
        "Resume Trigr"
    };
    let pause_item = MenuItem::with_id(app, "pause", pause_label, true, None::<&str>)?;

    let sep2 = PredefinedMenuItem::separator(app)?;

    let startup_on = get_startup_enabled_sync();
    let startup_item =
        CheckMenuItem::with_id(app, "startup", "Start with Windows", true, startup_on, None::<&str>)?;

    let sep3 = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Trigr", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &open_item,
            &sep1,
            &pause_item,
            &sep2,
            &startup_item,
            &sep3,
            &quit_item,
        ],
    )?;

    let tooltip = if enabled {
        "Trigr — Active"
    } else {
        "Trigr — Paused"
    };

    // Remove existing tray icon if any (for rebuilds)
    if let Some(existing) = app.tray_by_id("trigr-tray") {
        let _ = existing.set_menu(Some(menu));
        let _ = existing.set_tooltip(Some(tooltip));
        return Ok(());
    }

    TrayIconBuilder::with_id("trigr-tray")
        .icon(icon)
        .tooltip(tooltip)
        .menu(&menu)
        .on_menu_event(move |app, event| {
            match event.id().as_ref() {
                "open" => show_window(app),
                "pause" => toggle_pause(app),
                "startup" => {
                    let currently_on = get_startup_enabled_sync();
                    set_startup_enabled_impl(!currently_on);
                }
                "quit" => {
                    info!("[Trigr] Quit requested from tray");
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(move |tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                toggle_window_visibility(app);
            }
        })
        .build(app)?;

    Ok(())
}

/// Rebuild the tray menu (e.g. after pause/resume state change).
pub fn rebuild_tray_menu(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id("trigr-tray") {
        let enabled = MACROS_ENABLED.load(Ordering::Relaxed);

        let tooltip = if enabled {
            "Trigr — Active"
        } else {
            "Trigr — Paused"
        };
        let _ = tray.set_tooltip(Some(tooltip));

        // Rebuild menu items
        let pause_label = if enabled {
            "Pause Trigr"
        } else {
            "Resume Trigr"
        };

        if let Ok(open_item) = MenuItem::with_id(app, "open", "Open Trigr", true, None::<&str>) {
            if let Ok(sep1) = PredefinedMenuItem::separator(app) {
                if let Ok(pause_item) = MenuItem::with_id(app, "pause", pause_label, true, None::<&str>) {
                    if let Ok(sep2) = PredefinedMenuItem::separator(app) {
                        let startup_on = get_startup_enabled_sync();
                        if let Ok(startup_item) = CheckMenuItem::with_id(app, "startup", "Start with Windows", true, startup_on, None::<&str>) {
                            if let Ok(sep3) = PredefinedMenuItem::separator(app) {
                                if let Ok(quit_item) = MenuItem::with_id(app, "quit", "Quit Trigr", true, None::<&str>) {
                                    if let Ok(menu) = Menu::with_items(app, &[&open_item, &sep1, &pause_item, &sep2, &startup_item, &sep3, &quit_item]) {
                                        let _ = tray.set_menu(Some(menu));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Window management ───────────────────────────────────────────────────────

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn hide_window_to_tray(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();

        // One-time log on first hide
        if !HAS_SHOWN_BALLOON.swap(true, Ordering::Relaxed) {
            info!("[Trigr] Window hidden to tray");
        }
    }
}

fn toggle_window_visibility(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            hide_window_to_tray(app);
        } else {
            show_window(app);
        }
    }
}

// ── Pause toggle ────────────────────────────────────────────────────────────

fn toggle_pause(app: &AppHandle) {
    let was_enabled = MACROS_ENABLED.load(Ordering::Relaxed);
    MACROS_ENABLED.store(!was_enabled, Ordering::Relaxed);
    let now_enabled = !was_enabled;

    info!(
        "[Trigr] Global {} — macros {}",
        if now_enabled { "resume" } else { "pause" },
        if now_enabled { "active" } else { "paused" }
    );

    rebuild_tray_menu(app);

    // Notify the renderer of the state change
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.emit(
            "engine-status",
            serde_json::json!({
                "uiohookAvailable": false,
                "nutjsAvailable": false,
                "macrosEnabled": now_enabled,
                "activeProfile": "Default",
                "globalPauseToggleKey": null,
                "isDemoMode": false
            }),
        );
    }
}

pub fn are_macros_enabled() -> bool {
    MACROS_ENABLED.load(Ordering::Relaxed)
}

// ── Start with Windows (registry) ───────────────────────────────────────────

const REG_RUN: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const REG_NAME: &str = "Trigr";

fn get_startup_enabled_sync() -> bool {
    let output = Command::new("reg")
        .args(["query", REG_RUN, "/v", REG_NAME])
        .output();

    match output {
        Ok(o) => {
            let stdout = String::from_utf8_lossy(&o.stdout);
            stdout.contains(REG_NAME)
        }
        Err(_) => false,
    }
}

pub fn get_startup_enabled() -> bool {
    get_startup_enabled_sync()
}

fn set_startup_enabled_impl(enable: bool) {
    if enable {
        // Get the current exe path
        if let Ok(exe) = std::env::current_exe() {
            let exe_str = exe.to_string_lossy();
            let value = format!("\"{}\" --autolaunch", exe_str);
            let _ = Command::new("reg")
                .args(["add", REG_RUN, "/v", REG_NAME, "/d", &value, "/f"])
                .output();
            info!("[Trigr] Startup enabled: {}", value);
        }
    } else {
        let _ = Command::new("reg")
            .args(["delete", REG_RUN, "/v", REG_NAME, "/f"])
            .output();
        info!("[Trigr] Startup disabled");
    }
}

pub fn set_startup_enabled(enable: bool) {
    set_startup_enabled_impl(enable);
}

// ── Close-to-tray event handler ─────────────────────────────────────────────

/// Call this in the Tauri builder's `on_window_event` to intercept close.
pub fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        // Prevent the window from being destroyed — hide to tray instead
        api.prevent_close();
        hide_window_to_tray(window.app_handle());
    }
}
