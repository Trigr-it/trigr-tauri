//! Non-Windows stub for text expansion. The real expansions.rs is coupled to
//! the Win32 keystroke buffer and clipboard injection. resolve_tokens passes
//! text through unchanged; everything else is a no-op.
#![allow(dead_code, unused_variables)]

use serde_json::Value;
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

static FILL_IN_TX: OnceLock<Mutex<Option<mpsc::Sender<Option<HashMap<String, String>>>>>> =
    OnceLock::new();
static FILL_IN_READY_TX: OnceLock<Mutex<Option<mpsc::Sender<()>>>> = OnceLock::new();

pub fn fill_in_tx() -> &'static Mutex<Option<mpsc::Sender<Option<HashMap<String, String>>>>> {
    FILL_IN_TX.get_or_init(|| Mutex::new(None))
}

pub fn fill_in_ready_tx() -> &'static Mutex<Option<mpsc::Sender<()>>> {
    FILL_IN_READY_TX.get_or_init(|| Mutex::new(None))
}

pub(crate) fn fire_expansion_by_trigger(trigger: &str) {
    log::warn!("[stub] fire_expansion_by_trigger: expansion engine is not available on this platform yet");
}

pub fn get_global_variables() -> HashMap<String, String> {
    HashMap::new()
}

pub fn init_app_handle(handle: AppHandle) {}

/// The Windows original registers a private clipboard format to mark
/// Keyfire's own writes. Nothing to mark here. `unsafe` kept to match the
/// original signature -- callers invoke it inside unsafe blocks.
pub(crate) unsafe fn mark_clipboard_excluded() {}

pub fn resolve_tokens(
    text: &str,
    global_vars: &HashMap<String, String>,
    fillin_values: &HashMap<String, String>,
) -> (String, usize) {
    (text.to_string(), 0)
}

pub fn set_autocorrect_enabled(enabled: bool) {}

pub fn set_autocorrect_settings(
    enabled: bool,
    builtin_typos: bool,
    double_caps: bool,
    double_caps_exceptions: Vec<String>,
) {}

pub fn builtin_autocorrect_entries() -> Vec<(String, String)> {
    Vec::new()
}

pub fn update_assignments(assignments: HashMap<String, Value>) {}

pub fn update_global_variables(vars: HashMap<String, String>) {}

pub(crate) fn write_clipboard_dual(text: &str, html: Option<&str>) -> bool {
    false
}
