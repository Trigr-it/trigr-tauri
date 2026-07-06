//! Non-Windows twin of window_target.rs. The real module uses GDI monitor
//! enumeration + SetWinEventHook window moves.
//!
//! Mac port: `enum_monitors` is real (CGDisplay — callable off the main
//! thread, unlike NSScreen) so the UI's monitor picker lists actual
//! displays. The launch-with-monitor-target machinery arrives with the
//! "app"/"folder" action types.
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
    #[cfg(target_os = "macos")]
    {
        use core_graphics::display::CGDisplay;
        let Ok(ids) = CGDisplay::active_displays() else {
            return Vec::new();
        };
        ids.into_iter()
            .enumerate()
            .map(|(i, id)| {
                let display = CGDisplay::new(id);
                let is_primary = display.is_main();
                let bounds = display.bounds();
                let n = (i + 1) as u32;
                MonitorInfo {
                    // CGDisplay IDs are stable per boot — good enough as the
                    // identifier the frontend round-trips.
                    device_name: format!("CGDisplay-{}", id),
                    friendly_name: format!(
                        "Display {} ({}×{}){}",
                        n,
                        bounds.size.width as u32,
                        bounds.size.height as u32,
                        if is_primary { " — Primary" } else { "" }
                    ),
                    is_primary,
                    number: n,
                }
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn enumerates_at_least_one_display_with_a_primary() {
        let monitors = super::enum_monitors();
        assert!(!monitors.is_empty(), "a Mac always has at least one display");
        assert_eq!(monitors.iter().filter(|m| m.is_primary).count(), 1);
    }
}
