//! Non-Windows stub for WebView2 idle suspension. WKWebView has no
//! equivalent suspension API, so this is a permanent no-op off Windows.
#![allow(dead_code, unused_variables)]

pub fn resume_for_show(app: &tauri::AppHandle, label: &str) {}

pub fn start(app: tauri::AppHandle) {}
