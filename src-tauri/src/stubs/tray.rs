//! Non-Windows twin of tray.rs — menu bar item + login item.
//!
//! Mac port module 7: the Tauri tray API works natively as a macOS menu bar
//! item, so this is a near-verbatim port of the Windows original. The three
//! platform swaps:
//!   * "Start with Windows" (HKCU Run registry key) → "Start at Login" via a
//!     LaunchAgent plist in ~/Library/LaunchAgents (no extra deps; the
//!     SMAppService API is a possible follow-up);
//!   * show_window drops the AttachThreadInput focus-stealing workaround —
//!     set_focus activates the app on macOS without ceremony;
//!   * left-clicking the menu bar item opens the menu (macOS convention,
//!     Tauri's default) instead of toggling the window; "Open Keyfire" in
//!     the menu covers the open gesture.
//! Close-to-tray now applies here too: closing the main window hides it and
//! the app stays resident in the menu bar.
#![allow(dead_code, unused_variables)]

use log::info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

static HAS_SHOWN_BALLOON: AtomicBool = AtomicBool::new(false);

/// Pre-generated tray icons stored at startup — normal, paused, and held variants.
static TRAY_ICON_NORMAL: OnceLock<Image<'static>> = OnceLock::new();
static TRAY_ICON_PAUSED: OnceLock<Image<'static>> = OnceLock::new();
static TRAY_ICON_HELD: OnceLock<Image<'static>> = OnceLock::new();

// ── Autolaunch detection ────────────────────────────────────────────────────

pub fn is_autolaunch() -> bool {
    std::env::args().any(|a| a == "--autolaunch")
}

// ── Tray setup ──────────────────────────────────────────────────────────────

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let (rgba, width, height) = load_tray_icon_raw(app)?;

    // Store normal icon
    let normal = Image::new_owned(rgba.clone(), width, height);
    let _ = TRAY_ICON_NORMAL.set(normal);

    // Generate paused icon by dimming alpha to 1/3
    let mut paused_rgba = rgba.clone();
    for i in (3..paused_rgba.len()).step_by(4) {
        paused_rgba[i] = paused_rgba[i] / 3;
    }
    let paused = Image::new_owned(paused_rgba, width, height);
    let _ = TRAY_ICON_PAUSED.set(paused);

    // Generate held icon with red tint (boost R, halve G and B)
    let mut held_rgba = rgba.clone();
    for i in (0..held_rgba.len()).step_by(4) {
        held_rgba[i] = held_rgba[i].saturating_add(80).min(255); // R
        held_rgba[i + 1] = held_rgba[i + 1] / 2;                 // G
        held_rgba[i + 2] = held_rgba[i + 2] / 2;                 // B
        // Alpha unchanged
    }
    let held = Image::new_owned(held_rgba, width, height);
    let _ = TRAY_ICON_HELD.set(held);

    // Build tray with normal icon
    let tray_icon = Image::new_owned(rgba, width, height);
    build_tray(app.handle(), tray_icon)?;
    info!("[Keyfire] Menu bar item created");
    Ok(())
}

/// Decode a PNG to raw RGBA bytes + dimensions (for icon generation).
fn decode_png_raw(path: &std::path::Path) -> Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
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
        _ => buf,
    };
    Ok((rgba, info.width, info.height))
}

fn load_tray_icon_raw(app: &tauri::App) -> Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
    let resource_path = app
        .path()
        .resource_dir()
        .map(|d| d.join("icons").join("tray-icon.png"))
        .unwrap_or_default();
    if resource_path.exists() {
        return decode_png_raw(&resource_path);
    }
    let dev_path = std::env::current_dir()?
        .join("assets")
        .join("icons")
        .join("tray-icon.png");
    if dev_path.exists() {
        return decode_png_raw(&dev_path);
    }
    let fallback = std::env::current_dir()?.join("icons").join("icon.png");
    decode_png_raw(&fallback)
}

/// Swap the tray icon between active (normal) and paused (alpha-dimmed) states.
/// Reads from pre-generated static images — no disk I/O on toggle.
pub fn update_tray_icon(app: &AppHandle, macros_enabled: bool) {
    let icon = if macros_enabled {
        TRAY_ICON_NORMAL.get()
    } else {
        TRAY_ICON_PAUSED.get()
    };
    if let Some(img) = icon {
        if let Some(tray) = app.tray_by_id("trigr-tray") {
            let _ = tray.set_icon(Some(img.clone()));
        }
    }
}

/// Switch tray to held state — red-tinted icon + custom tooltip.
pub fn update_tray_icon_held(app: &AppHandle, held_label: &str) {
    if let Some(img) = TRAY_ICON_HELD.get() {
        if let Some(tray) = app.tray_by_id("trigr-tray") {
            let _ = tray.set_icon(Some(img.clone()));
            let tip = format!("Keyfire — Holding: {} — press again to release", held_label);
            let _ = tray.set_tooltip(Some(&tip));
        }
    }
}

/// Update tray icon to indicate a key is being repeated.
pub fn update_tray_icon_repeating(app: &AppHandle, label: &str, interval_ms: u64) {
    if let Some(img) = TRAY_ICON_HELD.get() {
        if let Some(tray) = app.tray_by_id("trigr-tray") {
            let _ = tray.set_icon(Some(img.clone()));
            let tip = format!("Keyfire — Repeating: {} ({}ms) — press again to stop", label, interval_ms);
            let _ = tray.set_tooltip(Some(&tip));
        }
    }
}

/// Restore tray to the correct non-held state (active or paused).
pub fn update_tray_icon_normal(app: &AppHandle) {
    let enabled = crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst);
    update_tray_icon(app, enabled);
    let tooltip = if enabled { "Keyfire — Active" } else { "Keyfire — Paused" };
    if let Some(tray) = app.tray_by_id("trigr-tray") {
        let _ = tray.set_tooltip(Some(tooltip));
    }
}

fn build_tray(
    app: &AppHandle,
    icon: Image<'static>,
) -> Result<(), Box<dyn std::error::Error>> {
    let enabled = crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst);

    // Menu items
    let open_item = MenuItem::with_id(app, "open", "Open Keyfire", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;

    let pause_label = if enabled {
        "Pause Keyfire"
    } else {
        "Resume Keyfire"
    };
    let pause_item = MenuItem::with_id(app, "pause", pause_label, true, None::<&str>)?;

    let sep2 = PredefinedMenuItem::separator(app)?;

    let startup_on = get_startup_enabled_sync();
    let startup_item =
        CheckMenuItem::with_id(app, "startup", "Start at Login", true, startup_on, None::<&str>)?;

    let sep3 = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Keyfire", true, None::<&str>)?;

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
        "Keyfire — Active"
    } else {
        "Keyfire — Paused"
    };

    // Remove existing tray icon if any (for rebuilds)
    if let Some(existing) = app.tray_by_id("trigr-tray") {
        let _ = existing.set_menu(Some(menu));
        let _ = existing.set_tooltip(Some(tooltip));
        return Ok(());
    }

    // macOS convention: left-click opens the menu (Tauri default), so no
    // click-to-toggle handler — "Open Keyfire" covers the open gesture.
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
                    info!("[Keyfire] Quit requested from tray");
                    app.exit(0);
                }
                _ => {}
            }
        })
        .build(app)?;

    Ok(())
}

/// Rebuild the tray menu (e.g. after pause/resume state change).
pub fn rebuild_tray_menu(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id("trigr-tray") {
        let enabled = crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst);

        let tooltip = if enabled {
            "Keyfire — Active"
        } else {
            "Keyfire — Paused"
        };
        let _ = tray.set_tooltip(Some(tooltip));

        let pause_label = if enabled {
            "Pause Keyfire"
        } else {
            "Resume Keyfire"
        };

        if let Ok(open_item) = MenuItem::with_id(app, "open", "Open Keyfire", true, None::<&str>) {
            if let Ok(sep1) = PredefinedMenuItem::separator(app) {
                if let Ok(pause_item) = MenuItem::with_id(app, "pause", pause_label, true, None::<&str>) {
                    if let Ok(sep2) = PredefinedMenuItem::separator(app) {
                        let startup_on = get_startup_enabled_sync();
                        if let Ok(startup_item) = CheckMenuItem::with_id(app, "startup", "Start at Login", true, startup_on, None::<&str>) {
                            if let Ok(sep3) = PredefinedMenuItem::separator(app) {
                                if let Ok(quit_item) = MenuItem::with_id(app, "quit", "Quit Keyfire", true, None::<&str>) {
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
    // Restore the webview memory target trimmed while hidden in the tray.
    crate::webview_mem::resume_for_show(app, "main");
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        // set_focus activates the app on macOS — no AttachThreadInput
        // ceremony needed (that block is a Windows foreground-rights dance).
        let _ = window.set_focus();
    }
}

pub fn hide_window_to_tray(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();

        // Hiding to the menu bar means "I'm done" — drop the editing lock so
        // the foreground watcher resumes auto-switching, and tell the
        // renderer to clear its selection (same contract as Windows).
        crate::foreground::set_editing_active(false);
        let _ = window.emit("reset-editing-on-hide", ());

        if !HAS_SHOWN_BALLOON.swap(true, Ordering::SeqCst) {
            info!("[Keyfire] Window hidden to menu bar");
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
    let was_enabled = crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst);
    if was_enabled {
        crate::actions::release_held_key();
        crate::actions::stop_repeating_key();
    }
    crate::hotkeys::MACROS_ENABLED.store(!was_enabled, Ordering::SeqCst);
    let now_enabled = !was_enabled;

    info!(
        "[Keyfire] Global {} — macros {}",
        if now_enabled { "resume" } else { "pause" },
        if now_enabled { "active" } else { "paused" }
    );

    rebuild_tray_menu(app);
    update_tray_icon(app, now_enabled);

    if let Some(window) = app.get_webview_window("main") {
        let state = crate::hotkeys::engine_state().lock().unwrap();
        let _ = window.emit(
            "engine-status",
            serde_json::json!({
                "uiohookAvailable": false,
                "nutjsAvailable": false,
                "macrosEnabled": now_enabled,
                "activeProfile": state.active_profile,
                "globalPauseToggleKey": state.pause_hotkey_str,
                "isDemoMode": false
            }),
        );
    }
}

pub fn are_macros_enabled() -> bool {
    crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst)
}

// ── Start at Login (LaunchAgent) ────────────────────────────────────────────
// A user LaunchAgent plist in ~/Library/LaunchAgents — no extra deps, no
// prompts (macOS surfaces a one-time "background items added" notice).
// Enabled == plist exists. SMAppService is a possible follow-up.

const LAUNCH_AGENT_LABEL: &str = "com.nodescaffold.trigr";

#[cfg(target_os = "macos")]
fn launch_agent_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| {
        std::path::PathBuf::from(h)
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{}.plist", LAUNCH_AGENT_LABEL))
    })
}

fn get_startup_enabled_sync() -> bool {
    #[cfg(target_os = "macos")]
    {
        launch_agent_path().is_some_and(|p| p.exists())
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn get_startup_enabled() -> bool {
    get_startup_enabled_sync()
}

fn set_startup_enabled_impl(enable: bool) {
    #[cfg(target_os = "macos")]
    {
        let Some(path) = launch_agent_path() else { return };
        if enable {
            // Never register a debug build for login launch — current_exe()
            // would pin the target/debug binary and every boot would race a
            // stale dev build for the single-instance lock (same trap as the
            // Windows HKCU Run guard, found live 2026-06-04).
            if cfg!(debug_assertions) {
                info!("[Keyfire] Login item skipped (debug build — would pin the dev exe path)");
                return;
            }
            let Ok(exe) = std::env::current_exe() else { return };
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{exe}</string>
        <string>--autolaunch</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
</dict>
</plist>
"#,
                label = LAUNCH_AGENT_LABEL,
                exe = exe.to_string_lossy(),
            );
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::write(&path, plist) {
                Ok(()) => info!("[Keyfire] Login item enabled: {}", path.display()),
                Err(e) => log::warn!("[Keyfire] Failed to write LaunchAgent: {}", e),
            }
        } else {
            match std::fs::remove_file(&path) {
                Ok(()) => info!("[Keyfire] Login item disabled"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => log::warn!("[Keyfire] Failed to remove LaunchAgent: {}", e),
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = enable;
    }
}

pub fn set_startup_enabled(enable: bool) {
    set_startup_enabled_impl(enable);
}

// ── Close-to-tray event handler ─────────────────────────────────────────────

/// Call this in the Tauri builder's `on_window_event` to intercept close.
/// With a live menu bar item the app stays resident when the main window
/// closes, matching Windows close-to-tray.
pub fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        hide_window_to_tray(window.app_handle());
    }
}
