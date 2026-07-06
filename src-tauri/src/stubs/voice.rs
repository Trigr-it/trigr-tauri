//! Non-Windows stub for voice recognition. The real voice.rs uses the WinRT
//! Speech API, which has no equivalent here. Voice is Windows-only for now.
#![allow(dead_code, unused_variables)]

use tauri::AppHandle;

pub fn prewarm_from_state() {}

pub fn start_recognition(phrases: Vec<String>, app: AppHandle) {
    log::warn!("[stub] voice recognition is not available on this platform");
}

pub fn start_continuous_recognition(phrases: Vec<String>, app: AppHandle) {
    log::warn!("[stub] voice recognition is not available on this platform");
}

pub fn stop_recognition() {}

pub fn stop_continuous_recognition() {}
