//! Appearance helpers that need the OS: the Windows accent colour and the
//! per-machine interface scale (WebView2 zoom on the main + Settings
//! windows only; the popups are fixed-size windows and stay at 100 %).
//!
//! Theme presets and custom colours themselves live entirely in the
//! frontend (src/theme/); Rust only supplies these two machine facts.

use log::{info, warn};
use serde_json::Value;
use tauri::Manager;

use crate::config;

/// Windows labels that receive the interface scale. Overlays are excluded on
/// purpose: Quick Search, the clipboard popup, fill-in and the radial are
/// fixed- or content-sized windows and zooming them clips their layout.
const SCALED_WINDOWS: [&str; 2] = ["main", "settings"];

pub const UI_SCALE_MIN: f64 = 0.9;
pub const UI_SCALE_MAX: f64 = 1.25;

fn clamp_scale(scale: f64) -> f64 {
    if !scale.is_finite() {
        return 1.0;
    }
    scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX)
}

/// The machine's saved interface scale (1.0 when unset). Stored in
/// trigr-local-settings.json because it is a monitor preference, not a
/// config preference: a shared config must not push one PC's zoom onto
/// another's laptop screen.
pub fn get_ui_scale() -> f64 {
    config::load_local_settings_json()
        .get("ui_scale")
        .and_then(|v| v.as_f64())
        .map(clamp_scale)
        .unwrap_or(1.0)
}

/// Persist the interface scale. 1.0 removes the key so the file stays lean.
pub fn set_ui_scale(scale: f64) -> bool {
    let scale = clamp_scale(scale);
    let Some(mut val) = config::load_local_settings_json_strict() else { return false; };
    let obj = val.as_object_mut().unwrap();
    if (scale - 1.0).abs() < 0.001 {
        obj.remove("ui_scale");
    } else {
        obj.insert("ui_scale".to_string(), Value::from((scale * 100.0).round() / 100.0));
    }
    config::save_local_settings_json(&val)
}

/// Apply a zoom factor to every scaled window that exists. Safe to call
/// before a window is built (it is simply skipped) and on hidden windows.
pub fn apply_ui_scale(app: &tauri::AppHandle, scale: f64) {
    let scale = clamp_scale(scale);
    for label in SCALED_WINDOWS {
        if let Some(win) = app.get_webview_window(label) {
            if let Err(e) = win.set_zoom(scale) {
                warn!("[Appearance] set_zoom({}) on {} failed: {}", scale, label, e);
            }
        }
    }
    info!("[Appearance] Interface scale {:.0}%", scale * 100.0);
}

/// The Windows accent colour as `#rrggbb`, or None when it cannot be read.
///
/// `HKCU\Software\Microsoft\Windows\DWM\AccentColor` is a DWORD laid out as
/// 0xAABBGGRR (little-endian bytes R, G, B, A). Falls back to
/// `ColorizationColor` (0xAARRGGBB) on older builds that lack the first.
#[cfg(windows)]
pub fn windows_accent_hex() -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE,
        REG_DWORD,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn read_dword(subkey: &str, name: &str) -> Option<u32> {
        unsafe {
            let mut hkey: HKEY = std::ptr::null_mut();
            if RegOpenKeyExW(HKEY_CURRENT_USER, wide(subkey).as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) != 0 {
                return None;
            }
            let name_w = wide(name);
            let mut ty: u32 = 0;
            let mut buf = [0u8; 4];
            let mut size: u32 = buf.len() as u32;
            let status = RegQueryValueExW(
                hkey,
                name_w.as_ptr(),
                std::ptr::null_mut(),
                &mut ty,
                buf.as_mut_ptr(),
                &mut size,
            );
            RegCloseKey(hkey);
            if status != 0 || ty != REG_DWORD || size != 4 {
                return None;
            }
            Some(u32::from_le_bytes(buf))
        }
    }

    const DWM: &str = "Software\\Microsoft\\Windows\\DWM";
    if let Some(abgr) = read_dword(DWM, "AccentColor") {
        let r = abgr & 0xff;
        let g = (abgr >> 8) & 0xff;
        let b = (abgr >> 16) & 0xff;
        return Some(format!("#{:02x}{:02x}{:02x}", r, g, b));
    }
    if let Some(argb) = read_dword(DWM, "ColorizationColor") {
        let r = (argb >> 16) & 0xff;
        let g = (argb >> 8) & 0xff;
        let b = argb & 0xff;
        return Some(format!("#{:02x}{:02x}{:02x}", r, g, b));
    }
    None
}

#[cfg(not(windows))]
pub fn windows_accent_hex() -> Option<String> {
    None
}

// ── Battery (radial widgets, 2026-09-21) ─────────────────────────────────────

/// Battery state for the radial wheel's battery pill:
/// `{ present, percent (0..100 | null), charging }`. `present` is false on
/// desktops (BatteryFlag 128 = no system battery) and when Windows reports the
/// level as unknown (255), so the pill can be skipped instead of showing "?".
#[cfg(windows)]
pub fn battery_status() -> Value {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    let mut status = SYSTEM_POWER_STATUS {
        ACLineStatus: 0,
        BatteryFlag: 0,
        BatteryLifePercent: 0,
        SystemStatusFlag: 0,
        BatteryLifeTime: 0,
        BatteryFullLifeTime: 0,
    };
    let ok = unsafe { GetSystemPowerStatus(&mut status) } != 0;
    if !ok {
        return serde_json::json!({ "present": false, "percent": Value::Null, "charging": false });
    }
    let no_battery = status.BatteryFlag & 128 != 0 || status.BatteryFlag == 255;
    let percent_known = status.BatteryLifePercent <= 100;
    let present = !no_battery && percent_known;
    let charging = status.BatteryFlag & 8 != 0 || (present && status.ACLineStatus == 1);
    serde_json::json!({
        "present": present,
        "percent": if present { Value::from(status.BatteryLifePercent) } else { Value::Null },
        "charging": charging,
    })
}

#[cfg(not(windows))]
pub fn battery_status() -> Value {
    serde_json::json!({ "present": false, "percent": Value::Null, "charging": false })
}
