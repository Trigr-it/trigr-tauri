//! Non-Windows stub for action execution. The real actions.rs drives
//! SendInput/clipboard/AHK on Win32. No-op twins keep lib.rs compiling;
//! the native macOS injector (CGEventPost) replaces this in Phase 2.
#![allow(dead_code, unused_variables)]

use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub static SUPPRESS_NEXT_CLIPBOARD_WRITE: AtomicBool = AtomicBool::new(false);

pub fn cleanup_stale_ahk_scripts(app_data_dir: PathBuf) {}

pub fn execute_action(
    macro_val: &Value,
    is_bare: bool,
    target_hwnd: isize,
    is_altgr: bool,
    trigger_key: Option<&str>,
    app: &tauri::AppHandle,
) {
    log::warn!("[stub] execute_action: action engine is not available on this platform yet");
}

pub fn kill_all_ahk_processes() {}

/// RAII twin of the Windows paste-op guard. Always acquires; nothing to
/// release because no paste pipeline exists on this platform yet.
pub(crate) struct PasteOpGuard;

impl PasteOpGuard {
    pub(crate) fn try_acquire() -> Option<Self> {
        Some(PasteOpGuard)
    }
}

pub fn read_clipboard_pub() -> Option<String> {
    None
}

pub(crate) fn record_self_clipboard_write() {}

pub fn release_held_key() -> Option<String> {
    None
}

pub fn release_held_modifiers() -> Vec<u16> {
    Vec::new()
}

pub fn restore_modifiers(held: &[u16]) {}

pub fn send_vk_key_pub(vk: u16, key_up: bool) {}

pub fn set_foreground_robust(hwnd: isize) -> bool {
    false
}

pub fn stop_repeating_key() -> Option<String> {
    None
}

/// RAII twin of the Windows SUPPRESS_SIMULATED guard. No hook exists on
/// this platform, so construction and drop are both no-ops.
pub(crate) struct SuppressionGuard;

impl SuppressionGuard {
    pub(crate) fn new() -> Self {
        SuppressionGuard
    }
}

pub fn write_clipboard_pub(text: &str) -> bool {
    false
}

pub fn write_clipboard_recordable_pub(text: &str) -> bool {
    false
}
