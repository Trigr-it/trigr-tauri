use log::{error, info};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
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

/// HWND of the fill-in window while it is visible. Set by expansions.rs, read by hook callback.
/// When the fill-in window is foreground, keystrokes pass through without buffering.
pub static FILLIN_HWND: AtomicIsize = AtomicIsize::new(0);

/// When true, a fill-in prompt is active — prevents concurrent fill-in invocations.
pub static FILL_IN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Keystroke captured during injection for later replay.
pub struct BufferedKey {
    pub vk_code: u32,
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

/// Rebuild the suppress key set from current assignments.
/// Must be called while holding the engine_state lock — overlay_hotkey is read from the state.
fn rebuild_suppress_keys(assignments: &HashMap<String, Value>, profile: &str, profile_settings: &HashMap<String, Value>) {
    let mut set = HashSet::new();
    let prefix = format!("{}::", profile);
    // Bare keys only suppress when the active profile is app-linked
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
        if parts.last() == Some(&"double") { continue; }
        if combo_str == "BARE" {
            if is_linked {
                let key_id = parts[2];
                if let Some(vk) = key_id_to_vk(key_id) {
                    set.insert((0u8, vk));
                }
            }
            continue;
        }
        let key_id = parts[2];
        if let Some(vk) = key_id_to_vk(key_id) {
            let bits = modifier_string_to_bits(combo_str);
            if bits != 0 {
                set.insert((bits, vk));
            }
        }
    }
    println!("[HOOK] Rebuilt suppress set: {} combos (before overlay)", set.len());
    if let Ok(mut w) = suppress_keys().write() {
        *w = set;
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
        "Backquote" => Some(0xC0), "Backslash" => Some(0xDC),
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
            pending_is_bare: false,
            capture_sole_modifier: None,
            overlay_hotkey: Some((1, 0x20)), // Default: Ctrl+Space (bits=1=Ctrl, vk=0x20=Space)
            pause_hotkey: None, // Set via set_global_pause_key command
            pause_hotkey_str: None,
            global_input_method: "direct".to_string(),
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

// ── Character map for text expansion buffer ─────────────────────────────────

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
                    buf.push(BufferedKey { vk_code: kb.vkCode, is_keydown });
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
                            return 1;
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
        match w_param as u32 {
            WM_LBUTTONDOWN => send_event(HookEvent::MouseDown {
                button: MouseButton::Left,
            }),
            WM_LBUTTONUP => send_event(HookEvent::MouseUp {
                button: MouseButton::Left,
            }),
            WM_RBUTTONDOWN => send_event(HookEvent::MouseDown {
                button: MouseButton::Right,
            }),
            WM_RBUTTONUP => send_event(HookEvent::MouseUp {
                button: MouseButton::Right,
            }),
            WM_MBUTTONDOWN => send_event(HookEvent::MouseDown {
                button: MouseButton::Middle,
            }),
            WM_MBUTTONUP => send_event(HookEvent::MouseUp {
                button: MouseButton::Middle,
            }),
            WM_XBUTTONDOWN => {
                let ms = &*(l_param as *const MSLLHOOKSTRUCT);
                let xbutton = ((ms.mouseData >> 16) & 0xFFFF) as u16;
                let button = if xbutton == 1 {
                    MouseButton::Side1
                } else {
                    MouseButton::Side2
                };
                send_event(HookEvent::MouseDown { button });
            }
            WM_XBUTTONUP => {
                let ms = &*(l_param as *const MSLLHOOKSTRUCT);
                let xbutton = ((ms.mouseData >> 16) & 0xFFFF) as u16;
                let button = if xbutton == 1 {
                    MouseButton::Side1
                } else {
                    MouseButton::Side2
                };
                send_event(HookEvent::MouseUp { button });
            }
            WM_MOUSEWHEEL => {
                let ms = &*(l_param as *const MSLLHOOKSTRUCT);
                let delta = (ms.mouseData >> 16) as i16;
                send_event(HookEvent::MouseWheel { delta });
            }
            _ => {}
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
                    if let HookEvent::KeyDown { vk_code, .. } = &event {
                        if !is_modifier_vk(*vk_code) && has_any_modifier() {
                            if let Ok(state) = engine_state().try_lock() {
                                if let Some((mod_bits, vk)) = state.pause_hotkey {
                                    if modifier_bits() == mod_bits && key_id_to_vk(vk_to_key_id(*vk_code).unwrap_or("")).map(|v| v as u32) == Some(vk) {
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
                    HookEvent::KeyDown { vk_code, .. } => {
                        if !is_modifier_vk(*vk_code) {
                            if let Some(id) = vk_to_key_id(*vk_code) {
                                let display = key_id_to_display(id).to_string();
                                forward_to_waiter(&WaitEvent::KeyDown { key_id: display });
                            }
                        }
                    }
                    HookEvent::KeyUp { vk_code, .. } => {
                        if !is_modifier_vk(*vk_code) {
                            if let Some(id) = vk_to_key_id(*vk_code) {
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
                    HookEvent::KeyDown { vk_code, .. } => handle_keydown(vk_code, &app),
                    HookEvent::KeyUp { vk_code, .. } => handle_keyup(vk_code, &app),
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

fn handle_keydown(vk: u32, app: &AppHandle) {
    let key_id = match vk_to_key_id(vk) {
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

    // ── Release any held key on physical keypress ───────────────────────
    if crate::actions::is_key_held() {
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

    // ── Global pause hotkey check (works even when paused) ────────────
    if has_any_modifier() {
        let state = engine_state().lock().unwrap();
        if let Some((mod_bits, vk)) = state.pause_hotkey {
            let current_bits = modifier_bits();
            let key_vk = key_id_to_vk(key_id);
            if current_bits == mod_bits && key_vk == Some(vk) {
                drop(state);
                let was_enabled = MACROS_ENABLED.load(Ordering::SeqCst);
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
        // Bare key — check app-linked profile assignments first
        let profile = state.active_profile.clone();
        let linked = state
            .profile_settings
            .get(&profile)
            .and_then(|s| s.get("linkedApp"))
            .and_then(|v| v.as_str())
            .is_some();

        if linked {
            let bare_key = format!("{}::BARE::{}", profile, key_id);
            if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                crate::expansions::buffer_clear();
                state.pending_macro = Some(macro_val);
                state.pending_storage_key = Some(bare_key);
                state.pending_is_bare = true;
                return;
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
        } else if let Some(ch) = vk_to_char(vk) {
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
                fire_macro(macro_clone, false, &app_clone);
            });
            // Don't set pending_macro — timer handles firing
            return;
        } else {
            // No double variant — fire directly at keyup
            state.pending_macro = Some(macro_val);
            state.pending_storage_key = None;
        }
        state.pending_is_bare = false;
    }
}

// ── Keyup handler ───────────────────────────────────────────────────────────

fn handle_keyup(vk: u32, app: &AppHandle) {
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
            let is_bare = state.pending_is_bare;
            state.pending_is_bare = false;

            // Drop state lock before dispatching
            drop(state);

            if let Some(sk) = storage_key {
                // Has a storage key → go through double-tap dispatch
                dispatch_with_double_tap(&sk, macro_val, app);
            } else {
                // No storage key (double-tap already resolved at keydown, or no double variant)
                fire_macro(macro_val, is_bare, app);
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

    if !has_any_modifier() {
        // Bare mouse — only middle, side1, side2 allowed bare
        let bare_allowed = matches!(button, MouseButton::Middle | MouseButton::Side1 | MouseButton::Side2);
        if !bare_allowed {
            return;
        }

        let state = engine_state().lock().unwrap();
        let profile = state.active_profile.clone();
        let linked = state
            .profile_settings
            .get(&profile)
            .and_then(|s| s.get("linkedApp"))
            .and_then(|v| v.as_str())
            .is_some();

        if linked {
            let bare_key = format!("{}::BARE::{}", profile, mouse_id);
            if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                drop(state);
                dispatch_with_double_tap(&bare_key, macro_val, app);
            }
        }
        return;
    }

    // Modified mouse button
    let combo = build_modifier_combo();
    let state = engine_state().lock().unwrap();
    let profile = state.active_profile.clone();
    let storage_key = format!("{}::{}::{}", profile, combo, mouse_id);

    if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
        drop(state);
        // Mouse buttons fire immediately (no deferred-to-keyup)
        dispatch_with_double_tap(&storage_key, macro_val, app);
    }
}

fn handle_mouse_up(_button: MouseButton, _app: &AppHandle) {
    // Mouse up doesn't trigger actions — macros fire on mousedown
}

fn handle_mouse_wheel(delta: i16, app: &AppHandle) {
    if APP_INPUT_FOCUSED.load(Ordering::SeqCst) || !has_any_modifier() {
        return;
    }

    let wheel_id = if delta > 0 {
        "MOUSE_SCROLL_UP"
    } else {
        "MOUSE_SCROLL_DOWN"
    };

    let combo = build_modifier_combo();
    let state = engine_state().lock().unwrap();
    let profile = state.active_profile.clone();
    let storage_key = format!("{}::{}::{}", profile, combo, wheel_id);

    if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
        drop(state);
        // Scroll fires immediately
        fire_macro(macro_val, false, app);
    }
}

// ── Double-tap dispatch ─────────────────────────────────────────────────────

fn dispatch_with_double_tap(storage_key: &str, macro_val: Value, app: &AppHandle) {
    let mut state = engine_state().lock().unwrap();
    let double_key = format!("{}::double", storage_key);
    let double_macro = state.assignments.get(&double_key).cloned();

    if double_macro.is_none() {
        // No double-tap variant — fire immediately
        drop(state);
        fire_macro(macro_val, false, app);
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
            fire_macro(dm, false, app);
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
        fire_macro(macro_clone, false, &app_clone);
    });
}

// ── Fire macro — execute action + notify frontend ───────────────────────────

fn fire_macro(macro_val: Value, is_bare: bool, app: &AppHandle) {
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
        crate::actions::execute_action(&macro_clone, is_bare, target_hwnd, is_altgr, &app_clone);

        // Log analytics
        let action_type = macro_clone.get("type").and_then(|v| v.as_str()).unwrap_or("hotkey");
        let analytics_type = if action_type == "macro" { "macro" } else { "hotkey" };
        crate::analytics::log_action(analytics_type, 0);

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
                        println!("[HOOK] Hooks reinstalled");
                        thread::sleep(Duration::from_secs(5));
                        last_heartbeat = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                        continue;
                    }
                }
                last_heartbeat = current;
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
    println!("[ENGINE] Assignments stored: {} entries", state.assignments.len());
}

pub fn set_active_profile(profile: String) {
    let mut state = engine_state().lock().unwrap();
    state.active_profile = profile.clone();
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
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
        println!("[HOOK] Pause hotkey set: {} → bits={} vk=0x{:02X}", combo, parsed.0, parsed.1);
    }
}

pub fn clear_pause_hotkey() {
    let mut state = engine_state().lock().unwrap();
    state.pause_hotkey = None;
    state.pause_hotkey_str = None;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
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
