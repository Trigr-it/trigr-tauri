//! Non-Windows stub for the hotkey engine. The real hotkeys.rs is built on
//! Win32 low-level hooks and only compiles on Windows. This twin exposes the
//! exact surface lib.rs and shared modules reference, so the app builds and
//! boots on other platforms with the engine reporting unavailable. The native
//! macOS engine (CGEventTap) replaces this in Phase 2 of the Mac port.
#![allow(dead_code, unused_variables, unused_imports)]

use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize};
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

pub static CLIPBOARD_OVERLAY_FOR_FILLIN: AtomicBool = AtomicBool::new(false);
pub static CLIPBOARD_OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);
pub static CLIPBOARD_OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);
pub static FILLIN_HWND: AtomicIsize = AtomicIsize::new(0);
pub static SEARCH_OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);

/// Mirror of the pub(crate) surface of the real EngineState. Private fields
/// of the Windows original are omitted -- lib.rs cannot touch them anyway.
pub(crate) struct EngineState {
    pub(crate) active_profile: String,
    pub(crate) assignments: HashMap<String, Value>,
    pub(crate) profile_settings: HashMap<String, Value>,
    pub(crate) overlay_hotkey: Option<(u8, u32)>,
    pub(crate) pause_hotkey: Option<(u8, u32)>,
    pub(crate) pause_hotkey_str: Option<String>,
    pub(crate) global_input_method: String,
    pub(crate) macro_speed: String,
    pub(crate) fire_on_press: bool,
    pub(crate) custom_keystroke_delay: u64,
    pub(crate) custom_pre_execution_delay: u64,
    pub(crate) clipboard_paste_hotkey: Option<(u8, u32)>,
    pub(crate) voice_hotkey: Option<(u8, u32)>,
    pub(crate) radial_menu_hotkey: Option<(u8, u32)>,
    pub(crate) temp_macro_record_hotkey: Option<(u8, u32)>,
    pub(crate) temp_macro_record_hotkey_str: Option<String>,
    pub(crate) temp_macro_play_hotkey: Option<(u8, u32)>,
    pub(crate) temp_macro_play_hotkey_str: Option<String>,
    pub(crate) temp_macro_loop_hotkey: Option<(u8, u32)>,
    pub(crate) temp_macro_loop_hotkey_str: Option<String>,
    pub(crate) temp_macro_events: Option<Vec<crate::recorder::RecordedEvent>>,
    pub(crate) temp_macro_captured_at: Option<String>,
    pub(crate) default_date_format: String,
}

impl Default for EngineState {
    fn default() -> Self {
        // Defaults mirror the Windows EngineState::default() values.
        Self {
            active_profile: "Default".to_string(),
            assignments: HashMap::new(),
            profile_settings: HashMap::new(),
            overlay_hotkey: Some((1, 0x20)),
            pause_hotkey: None,
            pause_hotkey_str: None,
            global_input_method: "direct".to_string(),
            macro_speed: "safe".to_string(),
            fire_on_press: false,
            custom_keystroke_delay: 30,
            custom_pre_execution_delay: 150,
            clipboard_paste_hotkey: Some((3, 0x56)),
            voice_hotkey: None,
            radial_menu_hotkey: None,
            temp_macro_record_hotkey: None,
            temp_macro_record_hotkey_str: None,
            temp_macro_play_hotkey: None,
            temp_macro_play_hotkey_str: None,
            temp_macro_loop_hotkey: None,
            temp_macro_loop_hotkey_str: None,
            temp_macro_events: None,
            temp_macro_captured_at: None,
            default_date_format: "DD/MM/YYYY".to_string(),
        }
    }
}

static ENGINE_STATE: OnceLock<Mutex<EngineState>> = OnceLock::new();

pub(crate) fn engine_state() -> &'static Mutex<EngineState> {
    ENGINE_STATE.get_or_init(|| Mutex::new(EngineState::default()))
}

pub fn get_engine_status() -> Value {
    let state = engine_state().lock().unwrap();
    serde_json::json!({
        "uiohookAvailable": false,
        "nutjsAvailable": false,
        "macrosEnabled": false,
        "activeProfile": state.active_profile,
        "globalPauseToggleKey": state.pause_hotkey_str,
        "isDemoMode": false,
    })
}

pub fn start_hooks(app: AppHandle) {
    log::warn!("[stub] global input hooks are not available on this platform yet");
}

pub fn handle_js_key_event(code: &str, ctrl: bool, shift: bool, alt: bool, meta: bool, app: &AppHandle) {}

pub fn is_radial_menu_held() -> bool {
    false
}

pub fn is_voice_active() -> bool {
    false
}

pub fn parse_hotkey_combo(combo: &str) -> Option<(u8, u32)> {
    None
}

pub fn set_active_profile(profile: String) {
    if let Ok(mut s) = engine_state().lock() {
        s.active_profile = profile;
    }
}

pub fn update_assignments(assignments: HashMap<String, Value>, profile: String) {
    if let Ok(mut s) = engine_state().lock() {
        s.assignments = assignments;
        s.active_profile = profile;
    }
}

pub fn update_profile_settings(settings: HashMap<String, Value>) {
    if let Ok(mut s) = engine_state().lock() {
        s.profile_settings = settings;
    }
}

pub fn update_global_settings(settings: &Value) {}

pub fn set_capturing(capturing: bool) {}
pub fn set_input_focused(focused: bool) {}
pub fn set_macros_enabled(enabled: bool) {}
pub fn set_recording(recording: bool) {}

pub fn set_clipboard_paste_hotkey(combo: &str) {}
pub fn set_overlay_hotkey(combo: &str) {}
pub fn set_pause_hotkey(combo: &str) {}
pub fn set_radial_menu_hotkey(combo: &str) {}
pub fn set_temp_macro_loop_hotkey(combo: &str) {}
pub fn set_temp_macro_play_hotkey(combo: &str) {}
pub fn set_temp_macro_record_hotkey(combo: &str) {}
pub fn set_voice_hotkey(combo: &str) {}

pub fn clear_clipboard_paste_hotkey() {}
pub fn clear_overlay_opened_flag() {}
pub fn clear_pause_hotkey() {}
pub fn clear_radial_menu_hotkey() {}
pub fn clear_radial_menu_open() {}
pub fn clear_voice_active() {}
pub fn clear_voice_hotkey() {}
