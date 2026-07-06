//! Non-Windows stub for the foreground-app watcher. The real foreground.rs
//! polls GetForegroundWindow for app-profile auto-switching. NSWorkspace
//! replaces this in Phase 2 of the Mac port.
#![allow(dead_code, unused_variables)]

use serde_json::Value;
use std::collections::HashMap;
use tauri::AppHandle;

pub fn force_check(app: &AppHandle) {}

pub fn get_current_fg_proc() -> String {
    String::new()
}

pub fn set_active_global_profile(profile: String) {}

pub fn set_editing_active(active: bool) {}

pub fn start_watcher(app: AppHandle) {
    log::warn!("[stub] foreground watcher is not available on this platform yet");
}

pub fn update_profile_settings(settings: HashMap<String, Value>) {}
