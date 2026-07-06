//! Non-Windows stub for tray + startup integration. The real tray.rs manages
//! the Win32 system tray and the Run registry key. On this platform there is
//! no tray yet (menu bar item lands in Phase 2 of the Mac port), so closing
//! the main window exits the app instead of hiding to tray.
#![allow(dead_code, unused_variables)]

use tauri::Manager;

pub fn is_autolaunch() -> bool {
    false
}

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    log::warn!("[stub] system tray is not available on this platform yet");
    Ok(())
}

pub fn update_tray_icon(app: &tauri::AppHandle, macros_enabled: bool) {}

pub fn rebuild_tray_menu(app: &tauri::AppHandle) {}

pub fn show_window(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

pub fn hide_window_to_tray(app: &tauri::AppHandle) {
    // No tray to hide into -- minimize instead so the app stays reachable.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.minimize();
    }
}

pub fn get_startup_enabled() -> bool {
    false
}

pub fn set_startup_enabled(enable: bool) {}

pub fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    // Without a tray, letting the main window "close to tray" would strand a
    // headless process. Exit cleanly instead.
    if let tauri::WindowEvent::CloseRequested { .. } = event {
        if window.label() == "main" {
            window.app_handle().exit(0);
        }
    }
}
