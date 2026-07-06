//! Non-Windows stub for monitor targeting. The real window_target.rs uses
//! GDI monitor enumeration + SetWinEventHook window moves.
#![allow(dead_code, unused_variables)]

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
