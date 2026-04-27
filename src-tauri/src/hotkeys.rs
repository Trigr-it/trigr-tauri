use log::{error, info};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};
use std::sync::{mpsc, Mutex, OnceLock, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, PeekMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, MSG, PM_REMOVE,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

// ── Global state ────────────────────────────────────────────────────────────

static KB_HOOK: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
static HOOK_THREAD_ID: AtomicIsize = AtomicIsize::new(0);
static HOOKS_RUNNING: AtomicBool = AtomicBool::new(false);
pub(crate) static MACROS_ENABLED: AtomicBool = AtomicBool::new(true);
static IS_RECORDING_HOTKEY: AtomicBool = AtomicBool::new(false);
static IS_CAPTURING_KEY: AtomicBool = AtomicBool::new(false);
static APP_INPUT_FOCUSED: AtomicBool = AtomicBool::new(false);

/// When true, hook callbacks pass events through without processing.
pub static SUPPRESS_SIMULATED: AtomicBool = AtomicBool::new(false);

/// When true, real user keystrokes are swallowed by the hook and buffered for replay.
pub static INJECTION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Timestamp (ms since UNIX epoch) when INJECTION_IN_PROGRESS was last set to true.
/// Used by the watchdog to detect stuck injections and force-clear the flag.
static INJECTION_STARTED_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Record that injection started (called by InjectionGuard in expansions.rs).
pub fn mark_injection_start() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    INJECTION_STARTED_MS.store(now, Ordering::SeqCst);
}

/// Clear injection start timestamp.
pub fn clear_injection_start() {
    INJECTION_STARTED_MS.store(0, Ordering::SeqCst);
}

/// HWND of the fill-in window while it is visible. Set by expansions.rs, read by hook callback.
/// When the fill-in window is foreground, keystrokes pass through without buffering.
pub static FILLIN_HWND: AtomicIsize = AtomicIsize::new(0);

/// When true, a fill-in prompt is active — prevents concurrent fill-in invocations.
pub static FILL_IN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Keystroke captured during injection for later replay.
pub struct BufferedKey {
    pub vk_code: u32,
    pub scan_code: u32,
    pub is_keydown: bool,
}

static INJECTION_BUFFER: OnceLock<Mutex<Vec<BufferedKey>>> = OnceLock::new();

pub fn injection_buffer() -> &'static Mutex<Vec<BufferedKey>> {
    INJECTION_BUFFER.get_or_init(|| Mutex::new(Vec::new()))
}

/// Heartbeat incremented by hook callback. Health monitor detects stale hooks.
static HOOK_HEARTBEAT: AtomicIsize = AtomicIsize::new(0);

/// Total hook events processed — used for periodic alive heartbeat logging.
static HOOK_EVENT_COUNT: AtomicIsize = AtomicIsize::new(0);
/// Set to 1 by hook callback when nCode < 0 is received; processor thread logs and clears.
static HOOK_NCODE_NEGATIVE: AtomicBool = AtomicBool::new(false);

// Modifier state — updated on every key event
static MOD_CTRL: AtomicBool = AtomicBool::new(false);
static MOD_ALT: AtomicBool = AtomicBool::new(false);
static MOD_SHIFT: AtomicBool = AtomicBool::new(false);
static MOD_META: AtomicBool = AtomicBool::new(false);

/// Set of (modifier_bits, vk_code) combos that should be suppressed (swallowed) by the hook.
/// Rebuilt whenever assignments change. Read-locked in hook callback, write-locked on update.
/// Modifier bits: Ctrl=1, Shift=2, Alt=4, Win=8
static SUPPRESS_KEYS: OnceLock<RwLock<HashSet<(u8, u32)>>> = OnceLock::new();

fn suppress_keys() -> &'static RwLock<HashSet<(u8, u32)>> {
    SUPPRESS_KEYS.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Set of bare mouse button IDs that should be suppressed (swallowed) by the mouse hook.
/// Only populated when the active profile is app-linked and has bare mouse assignments.
/// 1=Left, 2=Right, 3=Middle, 4=Side1, 5=Side2, 6=ScrollUp, 7=ScrollDown
static SUPPRESS_BARE_MOUSE: OnceLock<RwLock<HashSet<u8>>> = OnceLock::new();

fn suppress_bare_mouse() -> &'static RwLock<HashSet<u8>> {
    SUPPRESS_BARE_MOUSE.get_or_init(|| RwLock::new(HashSet::new()))
}

const SUPPRESS_MOUSE_LEFT: u8 = 1;
const SUPPRESS_MOUSE_RIGHT: u8 = 2;
const SUPPRESS_MOUSE_MIDDLE: u8 = 3;
const SUPPRESS_MOUSE_SIDE1: u8 = 4;
const SUPPRESS_MOUSE_SIDE2: u8 = 5;
const SUPPRESS_MOUSE_SCROLL_UP: u8 = 6;
const SUPPRESS_MOUSE_SCROLL_DOWN: u8 = 7;

/// Tracks which mouse buttons had their DOWN event suppressed by the hook.
/// Only the corresponding UP is suppressed — prevents mismatched down/up when
/// the suppress set changes mid-click (e.g., profile switch during a hold).
/// Bits: 0=left, 1=right, 2=middle, 3=side1, 4=side2.
static MOUSE_DOWN_SUPPRESSED: AtomicU8 = AtomicU8::new(0);

/// Map button suppress ID (1..5) to a bitmask. Returns None for scroll events.
fn suppress_btn_bit(id: u8) -> Option<u8> {
    if id >= 1 && id <= 5 { Some(1u8 << (id - 1)) } else { None }
}

fn mouse_key_id_to_suppress(key_id: &str) -> Option<u8> {
    match key_id {
        "MOUSE_LEFT" => Some(SUPPRESS_MOUSE_LEFT),
        "MOUSE_RIGHT" => Some(SUPPRESS_MOUSE_RIGHT),
        "MOUSE_MIDDLE" => Some(SUPPRESS_MOUSE_MIDDLE),
        "MOUSE_SIDE1" => Some(SUPPRESS_MOUSE_SIDE1),
        "MOUSE_SIDE2" => Some(SUPPRESS_MOUSE_SIDE2),
        "MOUSE_SCROLL_UP" => Some(SUPPRESS_MOUSE_SCROLL_UP),
        "MOUSE_SCROLL_DOWN" => Some(SUPPRESS_MOUSE_SCROLL_DOWN),
        _ => None,
    }
}

fn modifier_bits() -> u8 {
    let mut bits = 0u8;
    if MOD_CTRL.load(Ordering::SeqCst) { bits |= 1; }
    if MOD_SHIFT.load(Ordering::SeqCst) { bits |= 2; }
    if MOD_ALT.load(Ordering::SeqCst) { bits |= 4; }
    if MOD_META.load(Ordering::SeqCst) { bits |= 8; }
    bits
}

fn modifier_string_to_bits(combo: &str) -> u8 {
    let mut bits = 0u8;
    for part in combo.split('+') {
        match part {
            "Ctrl" => bits |= 1,
            "Shift" => bits |= 2,
            "Alt" => bits |= 4,
            "Win" => bits |= 8,
            _ => {}
        }
    }
    bits
}

/// Get the VK code to suppress for a given key ID.
/// For OEM keys (symbols), uses MapVirtualKeyW to find the layout-correct VK code
/// so suppression works regardless of US/UK/other keyboard layouts.
fn suppress_vk_for_key_id(key_id: &str) -> Option<u32> {
    // For OEM keys, use scan code → MapVirtualKeyW for layout-correct VK
    if let Some(scan) = key_id_to_scan(key_id) {
        let vk = vk_for_scan(scan);
        if vk != 0 {
            return Some(vk);
        }
    }
    // Non-OEM keys have stable VK codes across layouts
    key_id_to_vk(key_id)
}

/// Rebuild the suppress key set from current assignments.
/// Must be called while holding the engine_state lock — overlay_hotkey is read from the state.
/// Keys allowed for bare mapping in static (non-app-linked) profiles.
/// Matches STATIC_BARE_ALLOWED in keyboardLayout.jsx.
fn is_static_bare_allowed(key_id: &str) -> bool {
    matches!(key_id,
        "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
        | "Insert" | "Home" | "End" | "Delete" | "PageUp" | "PageDown"
        | "PrintScreen" | "ScrollLock" | "Pause"
        | "NumLock" | "NumpadDivide" | "NumpadMultiply" | "NumpadSubtract" | "NumpadAdd"
        | "Numpad0" | "Numpad1" | "Numpad2" | "Numpad3" | "Numpad4"
        | "Numpad5" | "Numpad6" | "Numpad7" | "Numpad8" | "Numpad9"
        | "NumpadEnter" | "NumpadDecimal"
        | "Escape" | "ContextMenu"
    )
}

fn rebuild_suppress_keys(assignments: &HashMap<String, Value>, profile: &str, profile_settings: &HashMap<String, Value>) {
    let mut set = HashSet::new();
    let mut mouse_set = HashSet::new();
    let prefix = format!("{}::", profile);
    let is_linked = profile_settings.get(profile)
        .and_then(|s| s.get("linkedApp"))
        .and_then(|v| v.as_str())
        .is_some();
    for key in assignments.keys() {
        if !key.starts_with(&prefix) { continue; }
        let parts: Vec<&str> = key.split("::").collect();
        if parts.len() < 3 { continue; }
        let combo_str = parts[1];
        if combo_str == "GLOBAL" { continue; }
        // Skip ::double entries from the suppress set — double-only keys should
        // let the single press pass through to the app. When both single+double
        // exist, the single entry already adds the key to the suppress set.
        if parts.last() == Some(&"double") { continue; }
        if combo_str == "BARE" {
            let key_id = parts[2];
            // App-linked profiles: all bare keys allowed
            // Static profiles: only non-character keys (F-keys, numpad, nav)
            if is_linked || is_static_bare_allowed(key_id) {
                if let Some(mouse_id) = mouse_key_id_to_suppress(key_id) {
                    mouse_set.insert(mouse_id);
                } else if let Some(vk) = suppress_vk_for_key_id(key_id) {
                    set.insert((0u8, vk));
                }
            }
            continue;
        }
        let key_id = parts[2];
        if let Some(vk) = suppress_vk_for_key_id(key_id) {
            let bits = modifier_string_to_bits(combo_str);
            if bits != 0 {
                set.insert((bits, vk));
            }
        }
    }
    println!("[HOOK] Rebuilt suppress set: {} key combos, {} bare mouse (before overlay)", set.len(), mouse_set.len());
    if let Ok(mut w) = suppress_keys().write() {
        *w = set;
    }
    if let Ok(mut w) = suppress_bare_mouse().write() {
        *w = mouse_set;
    }
}

/// Insert the overlay hotkey into the suppress set. Called separately because
/// rebuild_suppress_keys runs while holding engine_state, so it can't re-lock to read overlay_hotkey.
fn add_overlay_to_suppress(overlay: Option<(u8, u32)>) {
    if let Some(combo) = overlay {
        if let Ok(mut w) = suppress_keys().write() {
            w.insert(combo);
            println!("[HOOK] Overlay hotkey added to suppress set: bits={} vk=0x{:02X} (total {} combos)", combo.0, combo.1, w.len());
        }
    }
}

/// Insert the pause hotkey into the suppress set.
fn add_pause_to_suppress(pause: Option<(u8, u32)>) {
    if let Some(combo) = pause {
        if let Ok(mut w) = suppress_keys().write() {
            w.insert(combo);
        }
    }
}

/// Insert the clipboard paste hotkey into the suppress set.
fn add_clipboard_paste_to_suppress(combo: Option<(u8, u32)>) {
    if let Some(combo) = combo {
        if let Ok(mut w) = suppress_keys().write() {
            w.insert(combo);
        }
    }
}

/// Map Trigr key ID back to VK code (reverse of vk_to_key_id).
fn key_id_to_vk(key_id: &str) -> Option<u32> {
    match key_id {
        "KeyA" => Some(0x41), "KeyB" => Some(0x42), "KeyC" => Some(0x43),
        "KeyD" => Some(0x44), "KeyE" => Some(0x45), "KeyF" => Some(0x46),
        "KeyG" => Some(0x47), "KeyH" => Some(0x48), "KeyI" => Some(0x49),
        "KeyJ" => Some(0x4A), "KeyK" => Some(0x4B), "KeyL" => Some(0x4C),
        "KeyM" => Some(0x4D), "KeyN" => Some(0x4E), "KeyO" => Some(0x4F),
        "KeyP" => Some(0x50), "KeyQ" => Some(0x51), "KeyR" => Some(0x52),
        "KeyS" => Some(0x53), "KeyT" => Some(0x54), "KeyU" => Some(0x55),
        "KeyV" => Some(0x56), "KeyW" => Some(0x57), "KeyX" => Some(0x58),
        "KeyY" => Some(0x59), "KeyZ" => Some(0x5A),
        "Digit0" => Some(0x30), "Digit1" => Some(0x31), "Digit2" => Some(0x32),
        "Digit3" => Some(0x33), "Digit4" => Some(0x34), "Digit5" => Some(0x35),
        "Digit6" => Some(0x36), "Digit7" => Some(0x37), "Digit8" => Some(0x38),
        "Digit9" => Some(0x39),
        "F1" => Some(0x70), "F2" => Some(0x71), "F3" => Some(0x72),
        "F4" => Some(0x73), "F5" => Some(0x74), "F6" => Some(0x75),
        "F7" => Some(0x76), "F8" => Some(0x77), "F9" => Some(0x78),
        "F10" => Some(0x79), "F11" => Some(0x7A), "F12" => Some(0x7B),
        "ArrowLeft" => Some(0x25), "ArrowUp" => Some(0x26),
        "ArrowRight" => Some(0x27), "ArrowDown" => Some(0x28),
        "Home" => Some(0x24), "End" => Some(0x23),
        "PageUp" => Some(0x21), "PageDown" => Some(0x22),
        "Insert" => Some(0x2D), "Delete" => Some(0x2E),
        "Escape" => Some(0x1B), "Enter" => Some(0x0D), "Tab" => Some(0x09),
        "Space" => Some(0x20), "Backspace" => Some(0x08),
        "Minus" => Some(0xBD), "Equal" => Some(0xBB),
        "BracketLeft" => Some(0xDB), "BracketRight" => Some(0xDD),
        "Semicolon" => Some(0xBA), "Quote" => Some(0xDE),
        "Backquote" => Some(0xC0),
        "Backslash" => Some(0xDC),
        "Comma" => Some(0xBC), "Period" => Some(0xBE), "Slash" => Some(0xBF),
        "Numpad0" => Some(0x60), "Numpad1" => Some(0x61), "Numpad2" => Some(0x62),
        "Numpad3" => Some(0x63), "Numpad4" => Some(0x64), "Numpad5" => Some(0x65),
        "Numpad6" => Some(0x66), "Numpad7" => Some(0x67), "Numpad8" => Some(0x68),
        "Numpad9" => Some(0x69),
        _ => None,
    }
}

// Active assignments + profile — protected by mutex
static ENGINE_STATE: OnceLock<Mutex<EngineState>> = OnceLock::new();

pub(crate) fn engine_state() -> &'static Mutex<EngineState> {
    ENGINE_STATE.get_or_init(|| Mutex::new(EngineState::default()))
}

pub(crate) struct EngineState {
    pub(crate) active_profile: String,
    pub(crate) assignments: HashMap<String, Value>,
    pub(crate) profile_settings: HashMap<String, Value>,
    double_tap_window_ms: u64,
    // Double-tap tracking
    last_hotkey_time: HashMap<String, Instant>,
    pending_single_cancel: HashMap<String, Arc<AtomicBool>>,
    // Pending macro deferred until keyup (modifier release)
    pending_macro: Option<Value>,
    pending_storage_key: Option<String>,
    pending_trigger_key: Option<String>,
    pending_is_bare: bool,
    // Capture state
    capture_sole_modifier: Option<String>,
    // Overlay hotkey — parsed as (modifier_bits, vk_code)
    overlay_hotkey: Option<(u8, u32)>,
    // Global pause hotkey — parsed as (modifier_bits, vk_code)
    pause_hotkey: Option<(u8, u32)>,
    pub(crate) pause_hotkey_str: Option<String>,
    // Global input method — resolved when per-assignment method is "global" or absent
    pub(crate) global_input_method: String,
    // Macro speed preset — "safe" | "fast" | "instant" | "custom"
    pub(crate) macro_speed: String,
    // Custom speed slider values (only used when macro_speed == "custom")
    pub(crate) custom_keystroke_delay: u64,
    pub(crate) custom_pre_execution_delay: u64,
    // Clipboard quick-paste hotkey — parsed as (modifier_bits, vk_code)
    clipboard_paste_hotkey: Option<(u8, u32)>,
}

use std::sync::Arc;

impl Default for EngineState {
    fn default() -> Self {
        Self {
            active_profile: "Default".to_string(),
            assignments: HashMap::new(),
            profile_settings: HashMap::new(),
            double_tap_window_ms: 300,
            last_hotkey_time: HashMap::new(),
            pending_single_cancel: HashMap::new(),
            pending_macro: None,
            pending_storage_key: None,
            pending_trigger_key: None,
            pending_is_bare: false,
            capture_sole_modifier: None,
            overlay_hotkey: Some((1, 0x20)), // Default: Ctrl+Space (bits=1=Ctrl, vk=0x20=Space)
            pause_hotkey: None, // Set via set_global_pause_key command
            pause_hotkey_str: None,
            global_input_method: "direct".to_string(),
            macro_speed: "safe".to_string(),
            custom_keystroke_delay: 30,
            custom_pre_execution_delay: 150,
            clipboard_paste_hotkey: Some((3, 0x56)), // Default: Ctrl+Shift+V (bits=3, vk=0x56)
        }
    }
}

// ── Hook events ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum HookEvent {
    KeyDown { vk_code: u32, scan_code: u32 },
    KeyUp { vk_code: u32, scan_code: u32 },
    MouseDown { button: MouseButton },
    MouseUp { button: MouseButton },
    MouseWheel { delta: i16 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseButton {
    Left,
    Right,
    Middle,
    Side1,
    Side2,
}

// ── Wait for Input infrastructure ───────────────────────────────────────────

/// Events forwarded to a Wait for Input waiter.
#[derive(Debug, Clone)]
pub enum WaitEvent {
    KeyDown { key_id: String },
    KeyUp { key_id: String },
    MouseDown { button_name: String },
    MouseUp { button_name: String },
}

/// One-shot channel for the Wait for Input step. Set by actions.rs, read by event processor.
static WAIT_FOR_INPUT_TX: OnceLock<Mutex<Option<mpsc::Sender<WaitEvent>>>> = OnceLock::new();

fn wait_tx() -> &'static Mutex<Option<mpsc::Sender<WaitEvent>>> {
    WAIT_FOR_INPUT_TX.get_or_init(|| Mutex::new(None))
}

/// Register a waiter channel. Returns the receiver. Called from actions.rs.
pub fn register_wait_for_input() -> mpsc::Receiver<WaitEvent> {
    let (tx, rx) = mpsc::channel();
    *wait_tx().lock().unwrap() = Some(tx);
    rx
}

/// Clear the waiter channel. Must be called on completion, timeout, or cancellation.
pub fn clear_wait_for_input() {
    *wait_tx().lock().unwrap() = None;
}

/// Forward an event to the waiter if one is registered. Returns true if forwarded.
fn forward_to_waiter(event: &WaitEvent) -> bool {
    if let Ok(guard) = wait_tx().try_lock() {
        if let Some(ref tx) = *guard {
            let _ = tx.send(event.clone());
            return true;
        }
    }
    false
}

static mut EVENT_SENDER: Option<mpsc::Sender<HookEvent>> = None;

fn send_event(event: HookEvent) {
    unsafe {
        if let Some(ref sender) = EVENT_SENDER {
            let _ = sender.send(event);
        }
    }
}

// ── VK code → Trigr key ID mapping ──────────────────────────────────────────

fn vk_to_key_id(vk: u32) -> Option<&'static str> {
    match vk {
        // Letters A-Z (VK 0x41-0x5A)
        0x41 => Some("KeyA"),
        0x42 => Some("KeyB"),
        0x43 => Some("KeyC"),
        0x44 => Some("KeyD"),
        0x45 => Some("KeyE"),
        0x46 => Some("KeyF"),
        0x47 => Some("KeyG"),
        0x48 => Some("KeyH"),
        0x49 => Some("KeyI"),
        0x4A => Some("KeyJ"),
        0x4B => Some("KeyK"),
        0x4C => Some("KeyL"),
        0x4D => Some("KeyM"),
        0x4E => Some("KeyN"),
        0x4F => Some("KeyO"),
        0x50 => Some("KeyP"),
        0x51 => Some("KeyQ"),
        0x52 => Some("KeyR"),
        0x53 => Some("KeyS"),
        0x54 => Some("KeyT"),
        0x55 => Some("KeyU"),
        0x56 => Some("KeyV"),
        0x57 => Some("KeyW"),
        0x58 => Some("KeyX"),
        0x59 => Some("KeyY"),
        0x5A => Some("KeyZ"),
        // Digits 0-9 (VK 0x30-0x39)
        0x30 => Some("Digit0"),
        0x31 => Some("Digit1"),
        0x32 => Some("Digit2"),
        0x33 => Some("Digit3"),
        0x34 => Some("Digit4"),
        0x35 => Some("Digit5"),
        0x36 => Some("Digit6"),
        0x37 => Some("Digit7"),
        0x38 => Some("Digit8"),
        0x39 => Some("Digit9"),
        // Function keys
        0x70 => Some("F1"),
        0x71 => Some("F2"),
        0x72 => Some("F3"),
        0x73 => Some("F4"),
        0x74 => Some("F5"),
        0x75 => Some("F6"),
        0x76 => Some("F7"),
        0x77 => Some("F8"),
        0x78 => Some("F9"),
        0x79 => Some("F10"),
        0x7A => Some("F11"),
        0x7B => Some("F12"),
        // Navigation
        0x25 => Some("ArrowLeft"),
        0x26 => Some("ArrowUp"),
        0x27 => Some("ArrowRight"),
        0x28 => Some("ArrowDown"),
        0x24 => Some("Home"),
        0x23 => Some("End"),
        0x21 => Some("PageUp"),
        0x22 => Some("PageDown"),
        0x2D => Some("Insert"),
        0x2E => Some("Delete"),
        // Special
        0x1B => Some("Escape"),
        0x0D => Some("Enter"),
        0x09 => Some("Tab"),
        0x20 => Some("Space"),
        0x08 => Some("Backspace"),
        0x14 => Some("CapsLock"),
        0x90 => Some("NumLock"),
        0x91 => Some("ScrollLock"),
        0x2C => Some("PrintScreen"),
        0x13 => Some("Pause"),
        // Symbols
        0xBD => Some("Minus"),
        0xBB => Some("Equal"),
        0xDB => Some("BracketLeft"),
        0xDD => Some("BracketRight"),
        0xBA => Some("Semicolon"),
        0xDE => Some("Quote"),
        0xC0 => Some("Backquote"),
        0xDC => Some("Backslash"),
        0xBC => Some("Comma"),
        0xBE => Some("Period"),
        0xBF => Some("Slash"),
        // Numpad
        0x60 => Some("Numpad0"),
        0x61 => Some("Numpad1"),
        0x62 => Some("Numpad2"),
        0x63 => Some("Numpad3"),
        0x64 => Some("Numpad4"),
        0x65 => Some("Numpad5"),
        0x66 => Some("Numpad6"),
        0x67 => Some("Numpad7"),
        0x68 => Some("Numpad8"),
        0x69 => Some("Numpad9"),
        0x6E => Some("NumpadDecimal"),
        0x6A => Some("NumpadMultiply"),
        0x6B => Some("NumpadAdd"),
        0x6D => Some("NumpadSubtract"),
        0x6F => Some("NumpadDivide"),
        // Modifiers (tracked separately but included for recording)
        0xA0 => Some("ShiftLeft"),
        0xA1 => Some("ShiftRight"),
        0xA2 => Some("ControlLeft"),
        0xA3 => Some("ControlRight"),
        0xA4 => Some("AltLeft"),
        0xA5 => Some("AltRight"),
        0x5B => Some("MetaLeft"),
        0x5C => Some("MetaRight"),
        _ => None,
    }
}

fn mouse_button_to_key_id(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "MOUSE_LEFT",
        MouseButton::Right => "MOUSE_RIGHT",
        MouseButton::Middle => "MOUSE_MIDDLE",
        MouseButton::Side1 => "MOUSE_SIDE1",
        MouseButton::Side2 => "MOUSE_SIDE2",
    }
}

pub(crate) fn is_modifier_vk(vk: u32) -> bool {
    matches!(vk, 0xA0..=0xA5 | 0x5B | 0x5C)
}

/// Check if the foreground window is a dialog or popup where bare keys should
/// pass through as normal input (e.g. TAB to cycle fields, Enter to confirm).
/// Only called for bare-key checks — modified combos (Ctrl+X etc.) always fire.
/// SAFETY: safe to call from any thread; GetForegroundWindow + GetClassNameW are
/// fast kernel calls (<1ms) and will not stall the LL hook.
fn is_foreground_dialog() -> bool {
    unsafe {
        let fg = windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        if fg.is_null() { return false; }

        // Check window class name — #32770 is the standard Windows dialog class
        let mut class_buf = [0u16; 32];
        let len = windows_sys::Win32::UI::WindowsAndMessaging::GetClassNameW(
            fg, class_buf.as_mut_ptr(), 32,
        );
        if len > 0 {
            let class = String::from_utf16_lossy(&class_buf[..len as usize]);
            if class == "#32770" { return true; }
        }

        // Check extended style — WS_EX_DLGMODALFRAME indicates a dialog frame
        let ex_style = windows_sys::Win32::UI::WindowsAndMessaging::GetWindowLongW(fg, -20) as u32; // GWL_EXSTYLE = -20
        if ex_style & 0x0001 != 0 { return true; } // WS_EX_DLGMODALFRAME = 0x0001

        false
    }
}

/// Check if the cursor is over a window belonging to the foreground process AND
/// the foreground HWND matches the watcher's last poll (i.e. the linked app is
/// focused).  Used by bare mouse suppression/dispatch to prevent remaps from
/// firing when the cursor has moved outside the linked app's window.
///
/// SAFETY: fast kernel calls (GetCursorPos, WindowFromPoint, GetWindowThreadProcessId)
/// — safe from the LL hook thread.
fn is_cursor_over_linked_app() -> bool {
    unsafe {
        let fg = windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
        if fg.is_null() { return false; }

        // Foreground must match the watcher's last confirmed HWND (linked app)
        let fg_isize = fg as isize;
        if fg_isize != crate::foreground::last_fg_hwnd() { return false; }

        // Cursor must be over a window belonging to the foreground process
        let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
        let cursor_wnd = windows_sys::Win32::UI::WindowsAndMessaging::WindowFromPoint(pt);
        if cursor_wnd.is_null() { return false; }
        if cursor_wnd == fg { return true; }

        // Different window — check if same process (child window, toolbar, popup, etc.)
        let mut fg_pid: u32 = 0;
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(fg, &mut fg_pid);
        let mut cursor_pid: u32 = 0;
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(cursor_wnd, &mut cursor_pid);

        fg_pid != 0 && fg_pid == cursor_pid
    }
}

/// Is this VK in the OEM range where codes are layout-dependent?
fn is_oem_vk(vk: u32) -> bool {
    matches!(vk, 0xBA..=0xDF | 0xE2)
}

// ── Scan-code-based key identification (layout-independent) ─────────────────
// OEM symbol keys (`;`, `'`, `` ` ``, `[`, `]`, etc.) have VK codes that
// differ between keyboard layouts — e.g. VK 0xC0 is backtick on US but `'`
// on UK keyboards.  Scan codes identify physical key positions regardless of
// layout, so we use them as the authoritative source for OEM keys.

/// Map a scan code to a key ID.  Only covers OEM symbol keys — letters, digits,
/// function keys and navigation keys have stable VK codes and don't need this.
fn scan_to_key_id(scan: u32) -> Option<&'static str> {
    match scan {
        0x29 => Some("Backquote"),    // Key below ESC (`` ` ¬ `` UK / `` ` ~ `` US)
        0x0C => Some("Minus"),        // Key right of 0
        0x0D => Some("Equal"),        // Key right of -
        0x1A => Some("BracketLeft"),  // Key right of P
        0x1B => Some("BracketRight"), // Key right of [
        0x27 => Some("Semicolon"),    // Key right of L
        0x28 => Some("Quote"),        // Key right of ; (`'` on US, `'` on UK)
        0x2B => Some("Backslash"),    // Key right of ' (ANSI) / left of Enter (ISO)
        0x33 => Some("Comma"),        // Key right of M
        0x34 => Some("Period"),       // Key right of ,
        0x35 => Some("Slash"),        // Key right of .
        0x56 => Some("IntlBackslash"), // ISO key between left-Shift and Z
        _ => None,
    }
}

/// Reverse: key ID → scan code (for OEM keys only).
fn key_id_to_scan(key_id: &str) -> Option<u32> {
    match key_id {
        "Backquote"    => Some(0x29),
        "Minus"        => Some(0x0C),
        "Equal"        => Some(0x0D),
        "BracketLeft"  => Some(0x1A),
        "BracketRight" => Some(0x1B),
        "Semicolon"    => Some(0x27),
        "Quote"        => Some(0x28),
        "Backslash"    => Some(0x2B),
        "Comma"        => Some(0x33),
        "Period"       => Some(0x34),
        "Slash"        => Some(0x35),
        "IntlBackslash" => Some(0x56),
        _ => None,
    }
}

/// Get the VK code that the current keyboard layout produces for a given scan code.
/// Uses MapVirtualKeyW so we always suppress the correct VK on any layout.
fn vk_for_scan(scan: u32) -> u32 {
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::MapVirtualKeyW(scan, 1) // MAPVK_VSC_TO_VK = 1
    }
}

/// Resolve key ID using scan code for OEM keys, VK code for everything else.
fn resolve_key_id(vk: u32, scan: u32) -> Option<&'static str> {
    if is_oem_vk(vk) {
        // For OEM keys, prefer scan-code-based identification
        if let Some(id) = scan_to_key_id(scan) {
            return Some(id);
        }
    }
    vk_to_key_id(vk)
}

/// Resolve character for expansion buffer using scan code for OEM keys.
pub(crate) fn resolve_char(vk: u32, scan: u32) -> Option<char> {
    if is_oem_vk(vk) {
        return match scan_to_key_id(scan)? {
            "Backquote"    => Some('`'),
            "Quote"        => Some('\''),
            "Semicolon"    => Some(';'),
            "BracketLeft"  => Some('['),
            "BracketRight" => Some(']'),
            "Backslash"    => Some('\\'),
            "Comma"        => Some(','),
            "Period"       => Some('.'),
            "Slash"        => Some('/'),
            "Minus"        => Some('-'),
            "Equal"        => Some('='),
            _ => None,
        };
    }
    vk_to_char(vk)
}

// ── Character map for text expansion buffer ─────────────────────────────────
// Used as fallback for non-OEM keys.  OEM keys use resolve_char() above.

pub(crate) fn vk_to_char(vk: u32) -> Option<char> {
    match vk {
        0x41..=0x5A => Some((b'a' + (vk - 0x41) as u8) as char),
        0x30..=0x39 => Some((b'0' + (vk - 0x30) as u8) as char),
        0xBD => Some('-'),
        0xBB => Some('='),
        0xDB => Some('['),
        0xDD => Some(']'),
        0xBA => Some(';'),
        0xDE => Some('\''),
        0xC0 => Some('`'),
        0xDC => Some('\\'),
        0xBC => Some(','),
        0xBE => Some('.'),
        0xBF => Some('/'),
        _ => None,
    }
}

// ── Build storage key from current state ────────────────────────────────────

fn build_modifier_combo() -> String {
    let mut mods = Vec::new();
    if MOD_CTRL.load(Ordering::SeqCst) {
        mods.push("Ctrl");
    }
    if MOD_SHIFT.load(Ordering::SeqCst) {
        mods.push("Shift");
    }
    if MOD_ALT.load(Ordering::SeqCst) {
        mods.push("Alt");
    }
    if MOD_META.load(Ordering::SeqCst) {
        mods.push("Win");
    }
    mods.join("+")
}

fn has_any_modifier() -> bool {
    MOD_CTRL.load(Ordering::SeqCst)
        || MOD_ALT.load(Ordering::SeqCst)
        || MOD_SHIFT.load(Ordering::SeqCst)
        || MOD_META.load(Ordering::SeqCst)
}

fn no_modifiers_held() -> bool {
    !has_any_modifier()
}

// ── Hook callbacks (NO I/O — must return within 300ms or Windows removes the hook)

// CRITICAL: No I/O in hook callbacks. No println!, no file writes, no blocking
// operations of any kind. Windows will silently remove the LL hook if this
// callback takes >300ms to return. All logging must happen on the processor
// thread via send_event(). This was the root cause of hook death during
// development — println! to a paused console blocked the callback.
unsafe extern "system" fn keyboard_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    HOOK_HEARTBEAT.fetch_add(1, Ordering::SeqCst);
    HOOK_EVENT_COUNT.fetch_add(1, Ordering::SeqCst);
    if n_code < 0 {
        HOOK_NCODE_NEGATIVE.store(true, Ordering::SeqCst);
    }
    // Buffer real user keystrokes during injection — swallow them so they don't land in the target app.
    // Exception: if the fill-in window is foreground, pass keystrokes through so the user can type.
    if n_code >= 0 && INJECTION_IN_PROGRESS.load(Ordering::SeqCst) && !SUPPRESS_SIMULATED.load(Ordering::SeqCst) {
        let fillin = FILLIN_HWND.load(Ordering::SeqCst);
        let fg_is_fillin = fillin != 0 && {
            let fg = windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow();
            fg as isize == fillin
        };
        if !fg_is_fillin {
            let kb = &*(l_param as *const KBDLLHOOKSTRUCT);
            let is_keydown = matches!(w_param as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let is_keyup = matches!(w_param as u32, WM_KEYUP | WM_SYSKEYUP);
            if is_keydown || is_keyup {
                if let Ok(mut buf) = injection_buffer().try_lock() {
                    buf.push(BufferedKey { vk_code: kb.vkCode, scan_code: kb.scanCode, is_keydown });
                    return 1;
                }
            }
        }
    }
    if n_code >= 0 && !SUPPRESS_SIMULATED.load(Ordering::SeqCst) {
        let kb = &*(l_param as *const KBDLLHOOKSTRUCT);
        match w_param as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                send_event(HookEvent::KeyDown {
                    vk_code: kb.vkCode,
                    scan_code: kb.scanCode,
                });
                // Suppress matched hotkey combos — prevent keystroke reaching target app
                if !is_modifier_vk(kb.vkCode) && MACROS_ENABLED.load(Ordering::SeqCst) {
                    let bits = modifier_bits();
                    if let Ok(set) = suppress_keys().try_read() {
                        if set.contains(&(bits, kb.vkCode)) {
                            // Bare keys (bits=0): skip suppression in dialog/popup
                            // windows so TAB, Enter, etc. work for form navigation.
                            // Modified combos (Ctrl+X etc.) always fire.
                            if bits == 0 && is_foreground_dialog() {
                                // pass through
                            } else {
                                return 1;
                            }
                        }
                    }
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                send_event(HookEvent::KeyUp {
                    vk_code: kb.vkCode,
                    scan_code: kb.scanCode,
                });
            }
            _ => {}
        }
    }
    CallNextHookEx(KB_HOOK.load(Ordering::SeqCst) as _, n_code, w_param, l_param)
}

// CRITICAL: Same rules as keyboard_hook_proc — no I/O, no blocking. See above.
unsafe extern "system" fn mouse_hook_proc(
    n_code: i32,
    w_param: WPARAM,
    l_param: LPARAM,
) -> LRESULT {
    HOOK_HEARTBEAT.fetch_add(1, Ordering::SeqCst);
    if n_code >= 0 && !SUPPRESS_SIMULATED.load(Ordering::SeqCst) {
        let mut suppress_id: Option<u8> = None;
        let mut is_button_down = false;
        match w_param as u32 {
            WM_LBUTTONDOWN => {
                send_event(HookEvent::MouseDown { button: MouseButton::Left });
                suppress_id = Some(SUPPRESS_MOUSE_LEFT);
                is_button_down = true;
            }
            WM_LBUTTONUP => {
                send_event(HookEvent::MouseUp { button: MouseButton::Left });
                suppress_id = Some(SUPPRESS_MOUSE_LEFT);
            }
            WM_RBUTTONDOWN => {
                send_event(HookEvent::MouseDown { button: MouseButton::Right });
                suppress_id = Some(SUPPRESS_MOUSE_RIGHT);
                is_button_down = true;
            }
            WM_RBUTTONUP => {
                send_event(HookEvent::MouseUp { button: MouseButton::Right });
                suppress_id = Some(SUPPRESS_MOUSE_RIGHT);
            }
            WM_MBUTTONDOWN => {
                send_event(HookEvent::MouseDown { button: MouseButton::Middle });
                suppress_id = Some(SUPPRESS_MOUSE_MIDDLE);
                is_button_down = true;
            }
            WM_MBUTTONUP => {
                send_event(HookEvent::MouseUp { button: MouseButton::Middle });
                suppress_id = Some(SUPPRESS_MOUSE_MIDDLE);
            }
            WM_XBUTTONDOWN => {
                let ms = &*(l_param as *const MSLLHOOKSTRUCT);
                let xbutton = ((ms.mouseData >> 16) & 0xFFFF) as u16;
                let button = if xbutton == 1 { MouseButton::Side1 } else { MouseButton::Side2 };
                send_event(HookEvent::MouseDown { button });
                suppress_id = Some(if xbutton == 1 { SUPPRESS_MOUSE_SIDE1 } else { SUPPRESS_MOUSE_SIDE2 });
                is_button_down = true;
            }
            WM_XBUTTONUP => {
                let ms = &*(l_param as *const MSLLHOOKSTRUCT);
                let xbutton = ((ms.mouseData >> 16) & 0xFFFF) as u16;
                let button = if xbutton == 1 { MouseButton::Side1 } else { MouseButton::Side2 };
                send_event(HookEvent::MouseUp { button });
                suppress_id = Some(if xbutton == 1 { SUPPRESS_MOUSE_SIDE1 } else { SUPPRESS_MOUSE_SIDE2 });
            }
            WM_MOUSEWHEEL => {
                let ms = &*(l_param as *const MSLLHOOKSTRUCT);
                let delta = (ms.mouseData >> 16) as i16;
                send_event(HookEvent::MouseWheel { delta });
                suppress_id = Some(if delta > 0 { SUPPRESS_MOUSE_SCROLL_UP } else { SUPPRESS_MOUSE_SCROLL_DOWN });
            }
            _ => {}
        }
        // Suppress bare mouse events that have assignments in app-linked profiles.
        // DOWN/UP events are paired: we only suppress an UP if we suppressed the
        // matching DOWN. This prevents mismatched events when the suppress set
        // changes mid-click (e.g., profile switches while a button is held).
        if let Some(btn_id) = suppress_id {
            if MACROS_ENABLED.load(Ordering::SeqCst) {
                if let Some(bit) = suppress_btn_bit(btn_id) {
                    // Paired button event
                    if is_button_down {
                        if let Ok(set) = suppress_bare_mouse().try_read() {
                            if set.contains(&btn_id) {
                                if is_cursor_over_linked_app() && !is_foreground_dialog() {
                                    MOUSE_DOWN_SUPPRESSED.fetch_or(bit, Ordering::SeqCst);
                                    return 1;
                                }
                            }
                        }
                        // Not suppressed — clear flag so the UP passes through too
                        MOUSE_DOWN_SUPPRESSED.fetch_and(!bit, Ordering::SeqCst);
                    } else {
                        // Button-up: only suppress if the corresponding down was suppressed
                        if MOUSE_DOWN_SUPPRESSED.load(Ordering::SeqCst) & bit != 0 {
                            MOUSE_DOWN_SUPPRESSED.fetch_and(!bit, Ordering::SeqCst);
                            return 1;
                        }
                    }
                } else {
                    // Scroll event — no pairing needed, standalone check
                    if let Ok(set) = suppress_bare_mouse().try_read() {
                        if set.contains(&btn_id) {
                            if is_cursor_over_linked_app() && !is_foreground_dialog() {
                                return 1;
                            }
                        }
                    }
                }
            }
        }
    }
    CallNextHookEx(
        MOUSE_HOOK.load(Ordering::SeqCst) as _,
        n_code,
        w_param,
        l_param,
    )
}

// ── Event processing (runs on dedicated processor thread) ───────────────────

fn process_events(receiver: mpsc::Receiver<HookEvent>, app: AppHandle) {
    thread::Builder::new()
        .name("trigr-event-processor".to_string())
        .spawn(move || {
            println!("[PROC] Event processor started");
            info!("[Trigr] Event processor started");
            let mut last_heartbeat_count: isize = 0;
            while let Ok(event) = receiver.recv() {
                // Periodic heartbeat — log every 500 hook events
                let count = HOOK_EVENT_COUNT.load(Ordering::SeqCst);
                if count - last_heartbeat_count >= 500 {
                    info!("[Trigr] Hook heartbeat: {} events processed", count);
                    last_heartbeat_count = count;
                }
                // Log if hook callback received nCode < 0
                if HOOK_NCODE_NEGATIVE.swap(false, Ordering::SeqCst) {
                    info!("[Trigr] Hook nCode<0 received — hook may be dying");
                }
                if !MACROS_ENABLED.load(Ordering::SeqCst) && !IS_RECORDING_HOTKEY.load(Ordering::SeqCst) && !IS_CAPTURING_KEY.load(Ordering::SeqCst) {
                    // Still track modifiers even when paused
                    if let HookEvent::KeyDown { vk_code, .. } | HookEvent::KeyUp { vk_code, .. } = &event {
                        update_modifier_state(*vk_code, matches!(event, HookEvent::KeyDown { .. }));
                    }
                    // Pause hotkey must fire even when paused — it's the only way to unpause
                    if let HookEvent::KeyDown { vk_code, scan_code } = &event {
                        if !is_modifier_vk(*vk_code) && has_any_modifier() {
                            if let Ok(state) = engine_state().try_lock() {
                                if let Some((mod_bits, vk)) = state.pause_hotkey {
                                    // Use scan-code-aware resolution, then map back to VK
                                    let resolved_id = resolve_key_id(*vk_code, *scan_code).unwrap_or("");
                                    let resolved_vk = key_id_to_vk(resolved_id).unwrap_or(0);
                                    if modifier_bits() == mod_bits && resolved_vk == vk {
                                        let pause_str = state.pause_hotkey_str.clone();
                                        let profile = state.active_profile.clone();
                                        drop(state);
                                        MACROS_ENABLED.store(true, Ordering::SeqCst);
                                        println!("[PAUSE] Unpaused via hotkey");
                                        crate::tray::rebuild_tray_menu(&app);
                                        crate::tray::update_tray_icon(&app, true);
                                        let _ = app.emit("engine-status", serde_json::json!({
                                            "uiohookAvailable": HOOKS_RUNNING.load(Ordering::SeqCst),
                                            "nutjsAvailable": false,
                                            "macrosEnabled": true,
                                            "activeProfile": profile,
                                            "globalPauseToggleKey": pause_str,
                                            "isDemoMode": false,
                                        }));
                                    }
                                }
                            }
                        }
                    }
                    continue;
                }
                // Forward to Wait for Input waiter before normal handling
                // (waiter gets the event regardless of recording/capture mode)
                match &event {
                    HookEvent::KeyDown { vk_code, scan_code } => {
                        if !is_modifier_vk(*vk_code) {
                            if let Some(id) = resolve_key_id(*vk_code, *scan_code) {
                                let display = key_id_to_display(id).to_string();
                                forward_to_waiter(&WaitEvent::KeyDown { key_id: display });
                            }
                        }
                    }
                    HookEvent::KeyUp { vk_code, scan_code } => {
                        if !is_modifier_vk(*vk_code) {
                            if let Some(id) = resolve_key_id(*vk_code, *scan_code) {
                                let display = key_id_to_display(id).to_string();
                                forward_to_waiter(&WaitEvent::KeyUp { key_id: display });
                            }
                        }
                    }
                    HookEvent::MouseDown { button } => {
                        forward_to_waiter(&WaitEvent::MouseDown {
                            button_name: mouse_button_to_key_id(*button).to_string(),
                        });
                    }
                    HookEvent::MouseUp { button } => {
                        forward_to_waiter(&WaitEvent::MouseUp {
                            button_name: mouse_button_to_key_id(*button).to_string(),
                        });
                    }
                    _ => {}
                }

                // Normal event handling
                match event {
                    HookEvent::KeyDown { vk_code, scan_code } => handle_keydown(vk_code, scan_code, &app),
                    HookEvent::KeyUp { vk_code, scan_code } => handle_keyup(vk_code, scan_code, &app),
                    HookEvent::MouseDown { button } => handle_mouse_down(button, &app),
                    HookEvent::MouseUp { button } => handle_mouse_up(button, &app),
                    HookEvent::MouseWheel { delta } => handle_mouse_wheel(delta, &app),
                }
            }
            info!("[Trigr] Event processor stopped");
        })
        .expect("Failed to spawn event processor thread");
}

fn update_modifier_state(vk: u32, pressed: bool) {
    match vk {
        0xA0 | 0xA1 => MOD_SHIFT.store(pressed, Ordering::SeqCst),
        0xA2 | 0xA3 => MOD_CTRL.store(pressed, Ordering::SeqCst),
        0xA4 | 0xA5 => MOD_ALT.store(pressed, Ordering::SeqCst),
        0x5B | 0x5C => MOD_META.store(pressed, Ordering::SeqCst),
        _ => {}
    }
}

/// Sync modifier atomics with actual physical key state via GetAsyncKeyState.
/// Called after injection replay to ensure modifier tracking is accurate.
pub fn sync_modifier_state_from_os() {
    unsafe {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        MOD_SHIFT.store(GetAsyncKeyState(0xA0) < 0 || GetAsyncKeyState(0xA1) < 0, Ordering::SeqCst);
        MOD_CTRL.store(GetAsyncKeyState(0xA2) < 0 || GetAsyncKeyState(0xA3) < 0, Ordering::SeqCst);
        MOD_ALT.store(GetAsyncKeyState(0xA4) < 0 || GetAsyncKeyState(0xA5) < 0, Ordering::SeqCst);
        MOD_META.store(GetAsyncKeyState(0x5B) < 0 || GetAsyncKeyState(0x5C) < 0, Ordering::SeqCst);
    }
}

// ── Keydown handler ─────────────────────────────────────────────────────────

fn handle_keydown(vk: u32, scan: u32, app: &AppHandle) {
    let key_id = match resolve_key_id(vk, scan) {
        Some(id) => id,
        None => {
            return;
        }
    };

    // Update modifier state
    if is_modifier_vk(vk) {
        update_modifier_state(vk, true);
        // Clear expansion buffer on any modifier press (ARM64 timing safety)
        crate::expansions::buffer_clear();

        // Track sole modifier for key capture mode
        if IS_CAPTURING_KEY.load(Ordering::SeqCst) {
            let mut state = engine_state().lock().unwrap();
            let other_mods = match vk {
                0xA0 | 0xA1 => has_any_modifier() && (MOD_CTRL.load(Ordering::SeqCst) || MOD_ALT.load(Ordering::SeqCst) || MOD_META.load(Ordering::SeqCst)),
                0xA2 | 0xA3 => MOD_ALT.load(Ordering::SeqCst) || MOD_SHIFT.load(Ordering::SeqCst) || MOD_META.load(Ordering::SeqCst),
                0xA4 | 0xA5 => MOD_CTRL.load(Ordering::SeqCst) || MOD_SHIFT.load(Ordering::SeqCst) || MOD_META.load(Ordering::SeqCst),
                0x5B | 0x5C => MOD_CTRL.load(Ordering::SeqCst) || MOD_ALT.load(Ordering::SeqCst) || MOD_SHIFT.load(Ordering::SeqCst),
                _ => false,
            };
            if !other_mods {
                state.capture_sole_modifier = Some(match vk {
                    0xA0 | 0xA1 => "Shift".to_string(),
                    0xA2 | 0xA3 => "Ctrl".to_string(),
                    0xA4 | 0xA5 => "Alt".to_string(),
                    0x5B | 0x5C => "Win".to_string(),
                    _ => return,
                });
            } else {
                state.capture_sole_modifier = None;
            }
        }
        return;
    }

    // ── Verify modifier state against physical key state ────────────────
    // Prevents stuck modifiers (e.g. Alt+Tab where keyup was missed by hook)
    sync_modifier_state_from_os();

    // ── Release any held key on physical keypress ───────────────────────
    if crate::actions::is_key_held() {
        println!("[DEBUG] HELD RELEASE: firing before pause check, key_id={}", key_id);
        crate::actions::release_held_key();
        crate::tray::update_tray_icon_normal(app);
    }

    // ── Recording mode: capture combo and send to frontend ──────────────
    // Must run BEFORE APP_INPUT_FOCUSED check — recording works while Trigr UI is focused.
    if IS_RECORDING_HOTKEY.load(Ordering::SeqCst) {
        IS_RECORDING_HOTKEY.store(false, Ordering::SeqCst);

        let mut mods = Vec::new();
        if MOD_CTRL.load(Ordering::SeqCst) { mods.push("Ctrl"); }
        if MOD_SHIFT.load(Ordering::SeqCst) { mods.push("Shift"); }
        if MOD_ALT.load(Ordering::SeqCst) { mods.push("Alt"); }
        if MOD_META.load(Ordering::SeqCst) { mods.push("Win"); }

        let _ = app.emit(
            "hotkey-recorded",
            serde_json::json!({ "modifiers": mods, "keyId": key_id }),
        );
        return;
    }

    // ── Key capture mode: capture combo string for settings ─────────────
    // Must run BEFORE APP_INPUT_FOCUSED check — capture works while Trigr UI is focused.
    if IS_CAPTURING_KEY.load(Ordering::SeqCst) {
        IS_CAPTURING_KEY.store(false, Ordering::SeqCst);

        let mut parts = Vec::new();
        if MOD_CTRL.load(Ordering::SeqCst) { parts.push("Ctrl".to_string()); }
        if MOD_SHIFT.load(Ordering::SeqCst) { parts.push("Shift".to_string()); }
        if MOD_ALT.load(Ordering::SeqCst) { parts.push("Alt".to_string()); }
        if MOD_META.load(Ordering::SeqCst) { parts.push("Win".to_string()); }
        parts.push(key_id_to_display(key_id).to_string());

        let combo = parts.join("+");
        let _ = app.emit("key-captured", Value::String(combo));
        return;
    }

    // ── Overlay hotkey check (works even when Trigr is focused) ───────
    if MACROS_ENABLED.load(Ordering::SeqCst) && has_any_modifier() {
        let state = engine_state().lock().unwrap();
        if let Some((mod_bits, vk)) = state.overlay_hotkey {
            let current_bits = modifier_bits();
            let key_vk = key_id_to_vk(key_id);
            if current_bits == mod_bits && key_vk == Some(vk) {
                drop(state);
                // Clear modifier tracking AND send synthetic keyups via SendInput
                // so the OS itself clears the modifier state. The overlay stealing
                // focus causes real keyup events to be missed by the hook.
                MOD_CTRL.store(false, Ordering::SeqCst);
                MOD_SHIFT.store(false, Ordering::SeqCst);
                MOD_ALT.store(false, Ordering::SeqCst);
                MOD_META.store(false, Ordering::SeqCst);
                SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                crate::actions::release_held_modifiers();
                SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                let _ = app.emit("toggle-overlay", Value::Null);
                return;
            }
        }
        drop(state);
    }

    // ── Clipboard quick-paste hotkey check ─────────────────────────────
    if MACROS_ENABLED.load(Ordering::SeqCst) && has_any_modifier() {
        let state = engine_state().lock().unwrap();
        if let Some((mod_bits, vk)) = state.clipboard_paste_hotkey {
            let current_bits = modifier_bits();
            let key_vk = key_id_to_vk(key_id);
            if current_bits == mod_bits && key_vk == Some(vk) {
                drop(state);
                MOD_CTRL.store(false, Ordering::SeqCst);
                MOD_SHIFT.store(false, Ordering::SeqCst);
                MOD_ALT.store(false, Ordering::SeqCst);
                MOD_META.store(false, Ordering::SeqCst);
                SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                crate::actions::release_held_modifiers();
                SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                let _ = app.emit("toggle-clipboard-overlay", Value::Null);
                return;
            }
        }
        drop(state);
    }

    // ── Global pause hotkey check (works even when paused) ────────────
    if has_any_modifier() {
        let state = engine_state().lock().unwrap();
        if let Some((mod_bits, vk)) = state.pause_hotkey {
            let current_bits = modifier_bits();
            let key_vk = key_id_to_vk(key_id);
            println!("[DEBUG] PAUSE CHECK: has_any_modifier=true, current_bits={}, mod_bits={}, key_vk={:?}, vk={}, key_id={}", current_bits, mod_bits, key_vk, vk, key_id);
            if current_bits == mod_bits && key_vk == Some(vk) {
                println!("[DEBUG] PAUSE MATCH: firing pause");
                drop(state);
                let was_enabled = MACROS_ENABLED.load(Ordering::SeqCst);
                if was_enabled {
                    crate::actions::stop_repeating_key();
                }
                MACROS_ENABLED.store(!was_enabled, Ordering::SeqCst);
                let now_enabled = !was_enabled;
                println!("[PAUSE] Global pause toggled: macros={}", now_enabled);
                // Rebuild tray menu and notify frontend
                crate::tray::rebuild_tray_menu(app);
                crate::tray::update_tray_icon(app, now_enabled);
                {
                    let st = engine_state().lock().unwrap();
                    let _ = app.emit("engine-status", serde_json::json!({
                        "uiohookAvailable": HOOKS_RUNNING.load(Ordering::SeqCst),
                        "nutjsAvailable": false,
                        "macrosEnabled": now_enabled,
                        "activeProfile": st.active_profile,
                        "globalPauseToggleKey": st.pause_hotkey_str,
                        "isDemoMode": false,
                    }));
                }
                return;
            }
        }
        drop(state);
    }

    // CRITICAL: Recording and capture checks MUST remain above this guard.
    // If moved below, capture will silently fail when Trigr has focus.
    // Skip if Trigr input field is focused (normal hotkey matching suppressed)
    if APP_INPUT_FOCUSED.load(Ordering::SeqCst) {
        return;
    }

    // ── Normal hotkey matching ──────────────────────────────────────────
    let mut state = engine_state().lock().unwrap();

    if !has_any_modifier() {
        // Bare key — check profile assignments
        // App-linked profiles: all bare keys fire when linked app is focused
        // Static profiles: only non-character keys (F-keys, numpad, nav) fire globally
        let profile = state.active_profile.clone();
        let linked = state
            .profile_settings
            .get(&profile)
            .and_then(|s| s.get("linkedApp"))
            .and_then(|v| v.as_str())
            .is_some();

        let bare_allowed = if linked {
            !is_foreground_dialog()
        } else {
            is_static_bare_allowed(&key_id) && !is_foreground_dialog()
        };

        if bare_allowed {
            let bare_key = format!("{}::BARE::{}", profile, key_id);
            // Stop repeat if this key is the repeat trigger
            if crate::actions::is_repeating() {
                if let Some(trigger) = crate::actions::get_repeating_trigger() {
                    println!("[DEBUG] REPEAT STOP CHECK (bare): incoming={}, trigger={}", bare_key, trigger);
                    if trigger == bare_key {
                        crate::actions::stop_repeating_key();
                        crate::tray::update_tray_icon_normal(app);
                        return;
                    }
                }
            }
            if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                crate::expansions::buffer_clear();
                state.pending_macro = Some(macro_val);
                state.pending_storage_key = Some(bare_key.clone());
                state.pending_trigger_key = Some(bare_key);
                state.pending_is_bare = true;
                return;
            }

            // No single-press — check for double-only bare key
            let double_key = format!("{}::double", bare_key);
            if state.assignments.contains_key(&double_key) {
                crate::expansions::buffer_clear();
                let now = Instant::now();
                let dtw = state.double_tap_window_ms;
                if let Some(last) = state.last_hotkey_time.get(&bare_key) {
                    if now.duration_since(*last).as_millis() < dtw as u128 {
                        state.last_hotkey_time.remove(&bare_key);
                        info!("[Trigr] x2 Double-only bare: {}", bare_key);
                        state.pending_macro = state.assignments.get(&double_key).cloned();
                        state.pending_storage_key = None;
                        state.pending_trigger_key = Some(bare_key);
                        state.pending_is_bare = true;
                        return;
                    }
                }
                state.last_hotkey_time.insert(bare_key, now);
                return; // first tap — suppress but no action
            }
        }

        // No bare key match — handle text expansion buffer
        drop(state); // release engine lock before expansion calls

        // Skip expansion buffer while fill-in window is visible — keystrokes are for the fill-in input
        if FILLIN_HWND.load(Ordering::SeqCst) != 0 {
            return;
        }

        if key_id == "Backspace" {
            crate::expansions::buffer_pop();
        } else if key_id == "Space" {
            // Check for expansion/autocorrect trigger
            crate::expansions::check_space_trigger();
            crate::expansions::buffer_clear();
        } else if key_id == "Enter" || key_id == "Escape" || key_id == "Tab" {
            crate::expansions::buffer_clear();
        } else if let Some(ch) = resolve_char(vk, scan) {
            crate::expansions::buffer_push(ch);
            // Check for immediate-mode triggers after each character
            crate::expansions::check_immediate_triggers();
        }
        return;
    }

    // Build storage key from held modifiers
    let combo = build_modifier_combo();
    let profile = state.active_profile.clone();
    let storage_key = format!("{}::{}::{}", profile, combo, key_id);

    // Stop repeat if this key is the repeat trigger
    if crate::actions::is_repeating() {
        if let Some(trigger) = crate::actions::get_repeating_trigger() {
            println!("[DEBUG] REPEAT STOP CHECK (modified): incoming={}, trigger={}", storage_key, trigger);
            if trigger == storage_key {
                crate::actions::stop_repeating_key();
                crate::tray::update_tray_icon_normal(app);
                return;
            }
        }
    }

    if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
        crate::expansions::buffer_clear();
        // Check for double-tap variant
        let double_key = format!("{}::double", storage_key);
        let has_double = state.assignments.contains_key(&double_key);

        if has_double {
            let double_macro = state.assignments.get(&double_key).cloned();
            let now = Instant::now();
            let dtw = state.double_tap_window_ms;

            if let Some(last) = state.last_hotkey_time.get(&storage_key) {
                if now.duration_since(*last).as_millis() < dtw as u128 {
                    // Second tap within window — fire double immediately at keyup
                    // Cancel pending single-tap timer
                    if let Some(cancel) = state.pending_single_cancel.remove(&storage_key) {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    state.last_hotkey_time.remove(&storage_key);
                    info!("[Trigr] x2 Keydown double-tap: {}", storage_key);
                    state.pending_macro = double_macro;
                    state.pending_storage_key = None; // null → fire directly at keyup, no timer
                    state.pending_trigger_key = Some(storage_key);
                    return;
                }
            }
            // First tap — record time and start single-press timer at keydown
            state.last_hotkey_time.insert(storage_key.clone(), now);

            // Cancel any existing pending timer for this key
            if let Some(old_cancel) = state.pending_single_cancel.remove(&storage_key) {
                old_cancel.store(true, Ordering::SeqCst);
            }

            let cancel_flag = Arc::new(AtomicBool::new(false));
            state.pending_single_cancel.insert(storage_key.clone(), cancel_flag.clone());

            info!("[Trigr] x1 First tap: {} — waiting {}ms", storage_key, dtw);

            let sk = storage_key.clone();
            let app_clone = app.clone();
            let macro_clone = macro_val.clone();
            drop(state);

            thread::spawn(move || {
                thread::sleep(Duration::from_millis(dtw));
                if cancel_flag.load(Ordering::SeqCst) {
                    return; // Second tap came in — cancelled
                }
                // Single confirmed — fire directly from timer thread
                {
                    let mut state = engine_state().lock().unwrap();
                    state.pending_single_cancel.remove(&sk);
                    state.last_hotkey_time.remove(&sk);
                }
                info!("[Trigr] x1 Single confirmed: {}", sk);
                fire_macro(macro_clone, false, Some(sk), &app_clone);
            });
            // Don't set pending_macro — timer handles firing
            return;
        } else {
            // No double variant — fire directly at keyup
            state.pending_macro = Some(macro_val);
            state.pending_storage_key = None;
            state.pending_trigger_key = Some(storage_key);
        }
        state.pending_is_bare = false;
    } else {
        // No single-press — check for double-only
        let double_key = format!("{}::double", storage_key);
        if state.assignments.contains_key(&double_key) {
            crate::expansions::buffer_clear();
            let now = Instant::now();
            let dtw = state.double_tap_window_ms;
            if let Some(last) = state.last_hotkey_time.get(&storage_key) {
                if now.duration_since(*last).as_millis() < dtw as u128 {
                    state.last_hotkey_time.remove(&storage_key);
                    info!("[Trigr] x2 Double-only: {}", storage_key);
                    state.pending_macro = state.assignments.get(&double_key).cloned();
                    state.pending_storage_key = None;
                    state.pending_trigger_key = Some(storage_key);
                    state.pending_is_bare = false;
                    return;
                }
            }
            state.last_hotkey_time.insert(storage_key, now);
        }
    }
}

// ── Keyup handler ───────────────────────────────────────────────────────────

fn handle_keyup(vk: u32, _scan: u32, app: &AppHandle) {
    // Update modifier state
    if is_modifier_vk(vk) {
        update_modifier_state(vk, false);

        // Key capture: bare modifier release
        if IS_CAPTURING_KEY.load(Ordering::SeqCst) && no_modifiers_held() {
            let state = engine_state().lock().unwrap();
            if let Some(ref sole) = state.capture_sole_modifier {
                IS_CAPTURING_KEY.store(false, Ordering::SeqCst);
                let _ = app.emit("key-captured", Value::String(sole.clone()));
            }
        }
    }

    // Fire pending macro once all modifiers released (or immediately for bare keys)
    if no_modifiers_held() {
        let mut state = engine_state().lock().unwrap();
        if let Some(macro_val) = state.pending_macro.take() {
            let storage_key = state.pending_storage_key.take();
            let trigger_key = state.pending_trigger_key.take();
            let is_bare = state.pending_is_bare;
            state.pending_is_bare = false;

            // Drop state lock before dispatching
            drop(state);

            if let Some(sk) = storage_key {
                // Has a storage key → go through double-tap dispatch
                dispatch_with_double_tap(&sk, macro_val, trigger_key, app);
            } else {
                // No storage key (double-tap already resolved at keydown, or no double variant)
                fire_macro(macro_val, is_bare, trigger_key, app);
            }
        }
    }
}

// ── Mouse handlers ──────────────────────────────────────────────────────────

fn handle_mouse_down(button: MouseButton, app: &AppHandle) {
    if APP_INPUT_FOCUSED.load(Ordering::SeqCst) {
        return;
    }

    let mouse_id = mouse_button_to_key_id(button);

    // Clear any stale pending-release from a previous click cycle so it
    // can't be falsely consumed by a new hold action for this button.
    crate::actions::clear_pending_mouse_release(mouse_id);

    // Skip bare mouse processing in dialog/popup windows
    let in_dialog = is_foreground_dialog();

    // Verify the cursor is actually over the linked app — if the user moved the
    // cursor outside the app, bare mouse remaps must not fire even though the
    // linked profile is still active.
    let cursor_over_app = is_cursor_over_linked_app();

    if !has_any_modifier() {
        // Bare mouse — all buttons allowed in app-linked profiles
        let state = engine_state().lock().unwrap();
        let profile = state.active_profile.clone();
        let linked = state
            .profile_settings
            .get(&profile)
            .and_then(|s| s.get("linkedApp"))
            .and_then(|v| v.as_str())
            .is_some();

        if linked && !in_dialog && cursor_over_app {
            let bare_key = format!("{}::BARE::{}", profile, mouse_id);
            if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                drop(state);
                dispatch_with_double_tap(&bare_key, macro_val, Some(bare_key.clone()), app);
            } else {
                // No single — check for double-only bare mouse
                let double_key = format!("{}::double", bare_key);
                if state.assignments.contains_key(&double_key) {
                    let dm = state.assignments.get(&double_key).cloned();
                    drop(state);
                    dispatch_double_only(&bare_key, dm, app);
                }
            }
        }
        return;
    }

    // Modified mouse button — check for explicit modifier assignment first
    let combo = build_modifier_combo();
    let state = engine_state().lock().unwrap();
    let profile = state.active_profile.clone();
    let storage_key = format!("{}::{}::{}", profile, combo, mouse_id);

    if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
        drop(state);
        // Mouse buttons fire immediately (no deferred-to-keyup)
        dispatch_with_double_tap(&storage_key, macro_val, Some(storage_key.clone()), app);
        return;
    }

    // No modified assignment — check for double-only modified mouse
    let double_key = format!("{}::double", storage_key);
    if state.assignments.contains_key(&double_key) {
        let dm = state.assignments.get(&double_key).cloned();
        drop(state);
        dispatch_double_only(&storage_key, dm, app);
        return;
    }

    // Fall through to bare assignment in app-linked profiles.
    // Bare mouse remaps act as full button replacements: modifiers pass through
    // naturally since they're physically held (e.g. Shift+RightClick → Shift+MiddleClick).
    let linked = state
        .profile_settings
        .get(&profile)
        .and_then(|s| s.get("linkedApp"))
        .and_then(|v| v.as_str())
        .is_some();

    if linked && !in_dialog && cursor_over_app {
        let bare_key = format!("{}::BARE::{}", profile, mouse_id);
        if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
            drop(state);
            dispatch_with_double_tap(&bare_key, macro_val, Some(bare_key.clone()), app);
            return;
        }
        // No single bare — check double-only bare (modifier fallback)
        let double_key = format!("{}::double", bare_key);
        if state.assignments.contains_key(&double_key) {
            let dm = state.assignments.get(&double_key).cloned();
            drop(state);
            dispatch_double_only(&bare_key, dm, app);
        }
    }
}

fn handle_mouse_up(button: MouseButton, app: &AppHandle) {
    // Release held key if this mouse button was the trigger (press-hold mirroring)
    let mouse_id = mouse_button_to_key_id(button);
    if let Some(label) = crate::actions::release_held_if_mouse_trigger(mouse_id) {
        crate::tray::update_tray_icon_normal(app);
        info!("[Trigr] Mouse-up released hold: {}", label);
    }
}

fn handle_mouse_wheel(delta: i16, app: &AppHandle) {
    if APP_INPUT_FOCUSED.load(Ordering::SeqCst) {
        return;
    }

    let wheel_id = if delta > 0 {
        "MOUSE_SCROLL_UP"
    } else {
        "MOUSE_SCROLL_DOWN"
    };

    if !has_any_modifier() {
        // Bare scroll — only in app-linked profiles
        let state = engine_state().lock().unwrap();
        let profile = state.active_profile.clone();
        let linked = state
            .profile_settings
            .get(&profile)
            .and_then(|s| s.get("linkedApp"))
            .and_then(|v| v.as_str())
            .is_some();

        if linked {
            let bare_key = format!("{}::BARE::{}", profile, wheel_id);
            if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                drop(state);
                fire_macro(macro_val, false, Some(bare_key), app);
            }
        }
        return;
    }

    let combo = build_modifier_combo();
    let state = engine_state().lock().unwrap();
    let profile = state.active_profile.clone();
    let storage_key = format!("{}::{}::{}", profile, combo, wheel_id);

    if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
        drop(state);
        // Scroll fires immediately
        fire_macro(macro_val, false, Some(storage_key), app);
    }
}

// ── Double-tap dispatch ─────────────────────────────────────────────────────

/// Double-only dispatch for mouse: no single-press action exists.
/// First click records time, second click within the window fires.
fn dispatch_double_only(storage_key: &str, double_macro: Option<Value>, app: &AppHandle) {
    let mut state = engine_state().lock().unwrap();
    let now = Instant::now();
    let dtw = state.double_tap_window_ms;

    if let Some(last) = state.last_hotkey_time.get(storage_key) {
        if now.duration_since(*last).as_millis() < dtw as u128 {
            state.last_hotkey_time.remove(storage_key);
            info!("[Trigr] x2 Double-only: {}", storage_key);
            if let Some(dm) = double_macro {
                drop(state);
                fire_macro(dm, false, Some(storage_key.to_string()), app);
            }
            return;
        }
    }
    state.last_hotkey_time.insert(storage_key.to_string(), now);
}

fn dispatch_with_double_tap(storage_key: &str, macro_val: Value, trigger_key: Option<String>, app: &AppHandle) {
    let mut state = engine_state().lock().unwrap();
    let double_key = format!("{}::double", storage_key);
    let double_macro = state.assignments.get(&double_key).cloned();

    if double_macro.is_none() {
        // No double-tap variant — fire immediately
        drop(state);
        fire_macro(macro_val, false, trigger_key, app);
        return;
    }

    let dtw = state.double_tap_window_ms;
    let now = Instant::now();

    if let Some(last) = state.last_hotkey_time.get(storage_key) {
        if now.duration_since(*last).as_millis() < dtw as u128 {
            // Second tap within window → fire double
            if let Some(cancel) = state.pending_single_cancel.remove(storage_key) {
                cancel.store(true, Ordering::SeqCst);
            }
            state.last_hotkey_time.remove(storage_key);
            info!("[Trigr] x2 Double-tap: {}", storage_key);
            let dm = double_macro.unwrap();
            drop(state);
            fire_macro(dm, false, trigger_key, app);
            return;
        }
    }

    // First tap — schedule single after doubleTapWindow
    state.last_hotkey_time.insert(storage_key.to_string(), now);

    // Cancel any existing pending timer for this key
    if let Some(old_cancel) = state.pending_single_cancel.remove(storage_key) {
        old_cancel.store(true, Ordering::SeqCst);
    }

    let cancel_flag = Arc::new(AtomicBool::new(false));
    state
        .pending_single_cancel
        .insert(storage_key.to_string(), cancel_flag.clone());

    info!("[Trigr] x1 First tap: {} — waiting {}ms", storage_key, dtw);

    let sk = storage_key.to_string();
    let app_clone = app.clone();
    let macro_clone = macro_val.clone();
    drop(state);

    thread::spawn(move || {
        thread::sleep(std::time::Duration::from_millis(dtw));
        if cancel_flag.load(Ordering::SeqCst) {
            return; // Second tap came in — cancelled
        }
        // Single confirmed
        {
            let mut state = engine_state().lock().unwrap();
            state.pending_single_cancel.remove(&sk);
            state.last_hotkey_time.remove(&sk);
        }
        info!("[Trigr] x1 Single confirmed: {}", sk);
        fire_macro(macro_clone, false, Some(sk), &app_clone);
    });
}

// ── Fire macro — execute action + notify frontend ───────────────────────────

fn fire_macro(macro_val: Value, is_bare: bool, trigger_key: Option<String>, app: &AppHandle) {
    // Capture the target window HWND NOW, before any async delay.
    let target_hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };

    // Detect AltGr (Ctrl+Alt held simultaneously) — snapshot now, modifiers
    // will be cleared by the time execute_action runs.
    let is_altgr = MOD_CTRL.load(Ordering::SeqCst) && MOD_ALT.load(Ordering::SeqCst);
    if is_altgr {
        println!("[FIRE] AltGr combo detected — will erase dead character");
    }
    println!("[FIRE] Captured target HWND: 0x{:X}", target_hwnd);

    // Execute the action on a separate thread to avoid blocking the event processor
    let macro_clone = macro_val.clone();
    let app_clone = app.clone();
    thread::spawn(move || {
        crate::actions::execute_action(&macro_clone, is_bare, target_hwnd, is_altgr, trigger_key.as_deref(), &app_clone);

        // Log analytics
        let action_type = macro_clone.get("type").and_then(|v| v.as_str()).unwrap_or("hotkey");
        let analytics_type = match action_type { "macro" | "ahk" => "macro", _ => "hotkey" };
        let label = macro_clone.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let trigger = trigger_key.as_deref().unwrap_or("");
        crate::analytics::log_action(analytics_type, 0, trigger, label);

        // Notify frontend for visual feedback
        let _ = app_clone.emit(
            "macro-fired",
            serde_json::json!({
                "label": macro_clone.get("label").and_then(|v| v.as_str()).unwrap_or(""),
                "type": macro_clone.get("type").and_then(|v| v.as_str()).unwrap_or(""),
            }),
        );
    });
}

// ── Display name conversion ─────────────────────────────────────────────────

fn key_id_to_display(key_id: &str) -> &str {
    match key_id {
        "ArrowUp" => "Up",
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "Backquote" => "`",
        "Quote" => "'",
        "Semicolon" => ";",
        "BracketLeft" => "[",
        "BracketRight" => "]",
        "Backslash" => "\\",
        "Comma" => ",",
        "Period" => ".",
        "Slash" => "/",
        "Minus" => "-",
        "Equal" => "=",
        "CapsLock" => "Caps",
        "ContextMenu" => "Menu",
        k if k.starts_with("Key") && k.len() == 4 => &k[3..],
        k if k.starts_with("Digit") && k.len() == 6 => &k[5..],
        k => k,
    }
}

// ── Hook lifecycle ──────────────────────────────────────────────────────────

/// Spawn the dedicated hook thread with PeekMessageW polling loop and elevated priority.
fn spawn_hook_thread() {
    thread::Builder::new()
        .name("trigr-input-hooks".to_string())
        .spawn(move || {
            unsafe {
                // Elevate thread priority so the message pump is never starved
                let current_thread = windows_sys::Win32::System::Threading::GetCurrentThread();
                windows_sys::Win32::System::Threading::SetThreadPriority(current_thread, 15);

                let thread_id = windows_sys::Win32::System::Threading::GetCurrentThreadId();
                HOOK_THREAD_ID.store(thread_id as isize, Ordering::SeqCst);

                let kb = SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(keyboard_hook_proc),
                    std::ptr::null_mut(),
                    0,
                );
                if kb.is_null() {
                    let err = windows_sys::Win32::Foundation::GetLastError();
                    error!("[Trigr] Failed to install keyboard hook — GetLastError={}", err);
                    HOOKS_RUNNING.store(false, Ordering::SeqCst);
                    return;
                }
                info!("[Trigr] LL hook registered: HHOOK=0x{:X}", kb as isize);
                KB_HOOK.store(kb as isize, Ordering::SeqCst);

                let ms = SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(mouse_hook_proc),
                    std::ptr::null_mut(),
                    0,
                );
                if ms.is_null() {
                    let err = windows_sys::Win32::Foundation::GetLastError();
                    error!("[Trigr] Failed to install mouse hook — GetLastError={}", err);
                    UnhookWindowsHookEx(kb);
                    KB_HOOK.store(0, Ordering::SeqCst);
                    HOOKS_RUNNING.store(false, Ordering::SeqCst);
                    return;
                }
                info!("[Trigr] LL mouse hook registered: HHOOK=0x{:X}", ms as isize);
                MOUSE_HOOK.store(ms as isize, Ordering::SeqCst);
                HOOKS_RUNNING.store(true, Ordering::SeqCst);
                HOOK_HEARTBEAT.store(0, Ordering::SeqCst);

                // Reset shared atomics to safe defaults on reinstall — stale values
                // from a prior hook session can corrupt the new hook's behaviour.
                INJECTION_IN_PROGRESS.store(false, Ordering::SeqCst);
                SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                FILL_IN_ACTIVE.store(false, Ordering::SeqCst);
                FILLIN_HWND.store(0, Ordering::SeqCst);
                MOD_CTRL.store(false, Ordering::SeqCst);
                MOD_ALT.store(false, Ordering::SeqCst);
                MOD_SHIFT.store(false, Ordering::SeqCst);
                MOD_META.store(false, Ordering::SeqCst);
                MOUSE_DOWN_SUPPRESSED.store(0, Ordering::SeqCst);
                info!("[Trigr] Hook reinstall: shared atomics reset to safe defaults");

                println!("[HOOK] Input hooks installed (dedicated thread, high priority)");

                // PeekMessageW polling loop — actively pumps LL hook messages.
                // Unlike GetMessageW which blocks, this polls with a 1ms yield
                // to guarantee the thread is always responsive to hook dispatches.
                let mut msg: MSG = std::mem::zeroed();
                'pump: loop {
                    while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                        if msg.message == WM_QUIT {
                            break 'pump;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }

                // Cleanup
                UnhookWindowsHookEx(KB_HOOK.load(Ordering::SeqCst) as _);
                UnhookWindowsHookEx(MOUSE_HOOK.load(Ordering::SeqCst) as _);
                KB_HOOK.store(0, Ordering::SeqCst);
                MOUSE_HOOK.store(0, Ordering::SeqCst);
                HOOKS_RUNNING.store(false, Ordering::SeqCst);
            }
        })
        .expect("Failed to spawn hook thread");
}

pub fn start_hooks(app: AppHandle) {
    if HOOKS_RUNNING.load(Ordering::SeqCst) {
        return;
    }

    let (sender, receiver) = mpsc::channel();
    unsafe {
        EVENT_SENDER = Some(sender);
    }

    spawn_hook_thread();
    process_events(receiver, app);

    // Health monitor — reinstalls hooks if heartbeat stalls for 30s
    thread::Builder::new()
        .name("trigr-hook-monitor".to_string())
        .spawn(|| {
            let mut last_heartbeat = HOOK_HEARTBEAT.load(Ordering::SeqCst);
            thread::sleep(Duration::from_secs(5));
            loop {
                thread::sleep(Duration::from_secs(15));
                let current = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                if current == last_heartbeat && HOOKS_RUNNING.load(Ordering::SeqCst) {
                    // Stale — wait another 15s to confirm (30s total)
                    thread::sleep(Duration::from_secs(15));
                    let recheck = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                    if recheck == last_heartbeat {
                        println!("[HOOK] Heartbeat stale for 30s — reinstalling hooks");
                        let tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
                        if tid != 0 {
                            unsafe { PostThreadMessageW(tid as u32, WM_QUIT, 0, 0); }
                        }
                        thread::sleep(Duration::from_millis(500));
                        spawn_hook_thread();
                        // Rebuild suppress set so the new hook has correct state
                        {
                            let state = engine_state().lock().unwrap();
                            rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
                            add_overlay_to_suppress(state.overlay_hotkey);
                            add_pause_to_suppress(state.pause_hotkey);
                            add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
                        }
                        println!("[HOOK] Hooks reinstalled, suppress set rebuilt");
                        thread::sleep(Duration::from_secs(5));
                        last_heartbeat = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                        continue;
                    }
                }
                last_heartbeat = current;

                // ── Injection safety timeout ────────────────────────────
                // If INJECTION_IN_PROGRESS has been true for >5 seconds,
                // the injection thread is probably stuck (e.g. clipboard
                // blocked by another app).  Force-clear to unfreeze the
                // keyboard — the injection may produce garbled output but
                // that's better than a frozen keyboard.
                if INJECTION_IN_PROGRESS.load(Ordering::SeqCst) {
                    let started = INJECTION_STARTED_MS.load(Ordering::SeqCst);
                    if started > 0 {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64;
                        if now - started > 5000 {
                            error!("[Trigr] INJECTION_IN_PROGRESS stuck for >5s — force-clearing to unfreeze keyboard");
                            INJECTION_IN_PROGRESS.store(false, Ordering::SeqCst);
                            INJECTION_STARTED_MS.store(0, Ordering::SeqCst);
                            SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                        }
                    }
                }
            }
        })
        .expect("Failed to spawn hook monitor thread");
}

pub fn stop_hooks() {
    let tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
    if tid != 0 && HOOKS_RUNNING.load(Ordering::SeqCst) {
        unsafe { PostThreadMessageW(tid as u32, WM_QUIT, 0, 0); }
        HOOK_THREAD_ID.store(0, Ordering::SeqCst);
    }
}

pub fn hooks_running() -> bool {
    HOOKS_RUNNING.load(Ordering::SeqCst)
}

// ── JS keydown forwarder (WebView2 capture path) ────────────────────────────

/// Handle a key event forwarded from the JS keydown listener in the webview.
/// This provides an alternative capture path when the LL hook can't see
/// keypresses directed at WebView2. Emits the same events as handle_keydown.
pub fn handle_js_key_event(code: &str, ctrl: bool, shift: bool, alt: bool, meta: bool, app: &AppHandle) {
    let key_id = code;

    // Check overlay hotkey (JS path — Trigr has focus)
    if MACROS_ENABLED.load(Ordering::SeqCst) {
        let mut js_bits = 0u8;
        if ctrl { js_bits |= 1; }
        if shift { js_bits |= 2; }
        if alt { js_bits |= 4; }
        if meta { js_bits |= 8; }
        if js_bits != 0 {
            if let Ok(state) = engine_state().try_lock() {
                if let Some((mod_bits, vk)) = state.overlay_hotkey {
                    if js_bits == mod_bits && key_id_to_vk(key_id).or_else(|| parse_hotkey_combo(key_id).map(|(_, v)| v)) == Some(vk) {
                        drop(state);
                        MOD_CTRL.store(false, Ordering::SeqCst);
                        MOD_SHIFT.store(false, Ordering::SeqCst);
                        MOD_ALT.store(false, Ordering::SeqCst);
                        MOD_META.store(false, Ordering::SeqCst);
                        SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        crate::actions::release_held_modifiers();
                        SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                        let _ = app.emit("toggle-overlay", Value::Null);
                        return;
                    }
                }
            }
        }
    }

    if IS_RECORDING_HOTKEY.load(Ordering::SeqCst) {
        IS_RECORDING_HOTKEY.store(false, Ordering::SeqCst);

        let mut mods = Vec::new();
        if ctrl { mods.push("Ctrl"); }
        if shift { mods.push("Shift"); }
        if alt { mods.push("Alt"); }
        if meta { mods.push("Win"); }

        let _ = app.emit(
            "hotkey-recorded",
            serde_json::json!({ "modifiers": mods, "keyId": key_id }),
        );
    } else if IS_CAPTURING_KEY.load(Ordering::SeqCst) {
        IS_CAPTURING_KEY.store(false, Ordering::SeqCst);

        let mut parts = Vec::new();
        if ctrl { parts.push("Ctrl".to_string()); }
        if shift { parts.push("Shift".to_string()); }
        if alt { parts.push("Alt".to_string()); }
        if meta { parts.push("Win".to_string()); }
        parts.push(key_id_to_display(key_id).to_string());

        let combo = parts.join("+");
        let _ = app.emit("key-captured", Value::String(combo));
    }
}

// ── Public API for Tauri commands ───────────────────────────────────────────

pub fn set_macros_enabled(enabled: bool) {
    MACROS_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn macros_enabled() -> bool {
    MACROS_ENABLED.load(Ordering::SeqCst)
}

pub fn set_recording(recording: bool) {
    IS_RECORDING_HOTKEY.store(recording, Ordering::SeqCst);
}

pub fn set_capturing(capturing: bool) {
    IS_CAPTURING_KEY.store(capturing, Ordering::SeqCst);
    if capturing {
        let mut state = engine_state().lock().unwrap();
        state.capture_sole_modifier = None;
    }
}

pub fn set_input_focused(focused: bool) {
    APP_INPUT_FOCUSED.store(focused, Ordering::SeqCst);
}

pub fn update_assignments(assignments: HashMap<String, Value>, profile: String) {
    println!("[ENGINE] update_assignments called: {} entries for profile '{}'", assignments.len(), profile);
    let mut state = engine_state().lock().unwrap();
    state.assignments = assignments;
    state.active_profile = profile;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    println!("[ENGINE] Assignments stored: {} entries", state.assignments.len());
}

pub fn set_active_profile(profile: String) {
    let mut state = engine_state().lock().unwrap();
    state.active_profile = profile.clone();
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    info!("[Trigr] Active profile: {}", profile);
}

pub fn get_active_profile() -> String {
    engine_state().lock().unwrap().active_profile.clone()
}

pub fn update_profile_settings(settings: HashMap<String, Value>) {
    let mut state = engine_state().lock().unwrap();
    state.profile_settings = settings;
}

pub fn update_global_settings(settings: &Value) {
    let mut state = engine_state().lock().unwrap();
    if let Some(dtw) = settings.get("doubleTapWindow").and_then(|v| v.as_u64()) {
        state.double_tap_window_ms = dtw;
    }
    if let Some(m) = settings.get("globalInputMethod").and_then(|v| v.as_str()) {
        state.global_input_method = m.to_string();
    }
    if let Some(s) = settings.get("macroSpeed").and_then(|v| v.as_str()) {
        state.macro_speed = s.to_string();
    }
    if let Some(v) = settings.get("keystrokeDelay").and_then(|v| v.as_u64()) {
        state.custom_keystroke_delay = v;
    }
    if let Some(v) = settings.get("macroTriggerDelay").and_then(|v| v.as_u64()) {
        state.custom_pre_execution_delay = v;
    }
}

/// Parse a combo string like "Ctrl+Space" into (modifier_bits, vk_code).
pub fn parse_hotkey_combo(combo: &str) -> Option<(u8, u32)> {
    let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
    if parts.is_empty() {
        return None;
    }
    let key_name = parts.last().unwrap();
    let mut bits = 0u8;
    for &part in &parts[..parts.len() - 1] {
        match part {
            "Ctrl" => bits |= 1,
            "Shift" => bits |= 2,
            "Alt" => bits |= 4,
            "Win" => bits |= 8,
            _ => {}
        }
    }
    // Map display name to VK (Space, Enter, etc.)
    let vk = match key_name.to_lowercase().as_str() {
        "space" => Some(0x20u32),
        _ => key_id_to_vk(&format!("Key{}", key_name.to_uppercase()))
            .or_else(|| key_id_to_vk(key_name)),
    };
    vk.map(|v| (bits, v))
}

pub fn set_overlay_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        let mut state = engine_state().lock().unwrap();
        state.overlay_hotkey = Some(parsed);
        rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
        add_overlay_to_suppress(Some(parsed));
        add_pause_to_suppress(state.pause_hotkey);
        add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
        println!("[HOOK] Overlay hotkey set: {} → bits={} vk=0x{:02X}", combo, parsed.0, parsed.1);
    }
}

pub fn set_pause_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        let mut state = engine_state().lock().unwrap();
        state.pause_hotkey = Some(parsed);
        state.pause_hotkey_str = Some(combo.to_string());
        rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
        add_overlay_to_suppress(state.overlay_hotkey);
        add_pause_to_suppress(Some(parsed));
        add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
        println!("[HOOK] Pause hotkey set: {} → bits={} vk=0x{:02X}", combo, parsed.0, parsed.1);
    }
}

pub fn clear_pause_hotkey() {
    let mut state = engine_state().lock().unwrap();
    state.pause_hotkey = None;
    state.pause_hotkey_str = None;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    println!("[HOOK] Pause hotkey cleared");
}

pub fn get_engine_status() -> Value {
    let state = engine_state().lock().unwrap();
    serde_json::json!({
        "uiohookAvailable": HOOKS_RUNNING.load(Ordering::SeqCst),
        "nutjsAvailable": false,
        "macrosEnabled": MACROS_ENABLED.load(Ordering::SeqCst),
        "activeProfile": state.active_profile,
        "globalPauseToggleKey": state.pause_hotkey_str,
        "isDemoMode": false,
    })
}
