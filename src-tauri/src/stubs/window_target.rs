//! Non-Windows stub for monitor targeting. The real window_target.rs uses
//! GDI monitor enumeration + SetWinEventHook window moves.
#![allow(dead_code, unused_variables)]

use std::sync::mpsc::Receiver;

use serde::Serialize;

#[derive(Serialize, Clone)]
pub struct MonitorInfo {
    #[serde(rename = "deviceName")]
    pub device_name: String,
    #[serde(rename = "friendlyName")]
    pub friendly_name: String,
    #[serde(rename = "isPrimary")]
    pub is_primary: bool,
    pub number: u32,
}

pub fn enum_monitors() -> Vec<MonitorInfo> {
    Vec::new()
}

// Kept as `pub` and matching the real signature so callers compile on non-
// Windows targets. Returns None — no watcher, no move, no completion signal.
pub enum MonitorTarget {
    None,
    Primary,
    Cursor,
    Foreground(isize),
    Named(String),
}

pub enum LaunchKind<'a> {
    App { kind: &'a str, path: &'a str, app_id: &'a str, args: &'a str },
    Folder { path: &'a str },
}

pub fn parse_monitor_target(_data: Option<&serde_json::Value>, _foreground_hwnd: isize) -> MonitorTarget {
    MonitorTarget::None
}

pub fn launch_with_monitor_target(_kind: LaunchKind, _target: MonitorTarget) -> Option<Receiver<()>> {
    None
}
