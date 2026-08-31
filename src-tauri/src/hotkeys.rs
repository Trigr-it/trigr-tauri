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
    KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSLLHOOKSTRUCT, MSG, PM_REMOVE,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP, WM_MOUSEMOVE, WM_QUIT,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEWHEEL,
    WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
};

// ── Global state ────────────────────────────────────────────────────────────

static KB_HOOK: AtomicIsize = AtomicIsize::new(0);
static MOUSE_HOOK: AtomicIsize = AtomicIsize::new(0);
static HOOK_THREAD_ID: AtomicIsize = AtomicIsize::new(0);
static HOOKS_RUNNING: AtomicBool = AtomicBool::new(false);

/// Handle to the currently-running hook thread. Reinstall must join the old
/// thread before spawning a new one — otherwise a lingering old thread can
/// (a) leave its LL hooks temporarily co-installed with the new thread's,
/// causing duplicate KeyDown/KeyUp dispatch, and (b) run its own cleanup
/// block AFTER the new thread has already written its handles into KB_HOOK
/// / MOUSE_HOOK / HOOKS_RUNNING, clobbering them and silently disabling the
/// watchdog for the rest of the session.
static HOOK_THREAD_HANDLE: OnceLock<Mutex<Option<thread::JoinHandle<()>>>> = OnceLock::new();
fn hook_thread_handle() -> &'static Mutex<Option<thread::JoinHandle<()>>> {
    HOOK_THREAD_HANDLE.get_or_init(|| Mutex::new(None))
}

/// True when the LL mouse hook has been intentionally uninstalled because the
/// foreground window is fullscreen / borderless-fullscreen / chrome-less (e.g.
/// World of Warcraft). The hook adds per-event latency that disrupts games'
/// SetCursorPos recentering loops used for infinite camera rotation. The
/// foreground watcher in foreground.rs sets this flag on transition + posts
/// WM_KEYFIRE_MOUSE_HOOK_PAUSE / RESUME to the hook thread. Watchdog reads it to
/// skip its heartbeat-stale reinstall (which would otherwise undo our pause).
/// Runtime only — never persisted to config.
pub static MOUSE_HOOK_PAUSED: AtomicBool = AtomicBool::new(false);

/// Custom pump messages posted by the foreground watcher into the hook thread
/// to selectively uninstall / reinstall only the LL mouse hook (keyboard hook
/// stays installed). WM_USER range is Windows-reserved for app-private use and
/// won't collide with WM_QUIT or any system message.
pub const WM_KEYFIRE_MOUSE_HOOK_PAUSE: u32 = 0x0400 + 1;  // WM_USER + 1
pub const WM_KEYFIRE_MOUSE_HOOK_RESUME: u32 = 0x0400 + 2; // WM_USER + 2

/// Hold trigger (v0.5, Pro): paused alongside MOUSE_HOOK_PAUSED by the
/// foreground watcher's fullscreen detector. While true, keydowns don't arm
/// hold timers (keys with ::hold variants behave as plain single/double) and
/// the watcher doesn't fire armed entries. Runtime only — never persisted.
pub static HOLD_DETECTION_PAUSED: AtomicBool = AtomicBool::new(false);
pub(crate) static MACROS_ENABLED: AtomicBool = AtomicBool::new(true);
static IS_RECORDING_HOTKEY: AtomicBool = AtomicBool::new(false);
static IS_CAPTURING_KEY: AtomicBool = AtomicBool::new(false);
/// Set the first time the ISO-only key (scancode 0x56, beside left Shift) is
/// pressed this run; the frontend is told once so the on-screen keyboard can
/// switch to the ISO shape.
static ISO_KEY_SEEN: AtomicBool = AtomicBool::new(false);
// Wait for Pixel eyedropper: while true, the next left click anywhere picks
// that screen point (suppressed + emitted to the editor); right click or ESC
// cancels. Self-clearing — the first L/R click or ESC always resets it, so a
// lost frontend can cost at most one swallowed click. Three sites must stay
// in sync: handle_mouse_down branch, mouse_hook_proc suppression mirror,
// handle_keydown ESC branch + keyboard_hook_proc ESC swallow.
static PIXEL_PICK_ACTIVE: AtomicBool = AtomicBool::new(false);
// Post-pick settle window (~450ms): the sampler thread nudges the cursor off
// the picked point, waits for hover-out transitions, samples the rest-state
// colour, then emits. L/R clicks stay suppressed for the whole window (hook
// mirror ORs this with PIXEL_PICK_ACTIVE) so an impatient second click can't
// leak to the app the instant ACTIVE clears — that exact leak was a dev-test
// bug (2026-08-17).
static PIXEL_PICK_SAMPLING: AtomicBool = AtomicBool::new(false);
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

/// When true, the clipboard overlay is visible and keyboard input is routed to it
/// via the LL hook instead of DOM events (the overlay uses WS_EX_NOACTIVATE so it
/// never steals focus from the active app).
pub static CLIPBOARD_OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);

/// True when the clipboard overlay was opened from the fill-in webview
/// (show_clipboard_overlay_for_fillin) rather than the standard LL-hook
/// Ctrl+Shift+V path. Empirically established 2026-07-03: LL hooks do NOT
/// fire for keys while a Trigr WebView2 window has focus, so the normal
/// hotkey path is unreachable from inside a fill-in — the popup has to be
/// invoked from the fill-in's own DOM keydown listener.
///
/// Two runtime differences when this flag is true:
///   1. Show path activates the popup (no WS_EX_NOACTIVATE) so its DOM
///      handles keys directly. The clipboard-overlay-key LL-hook routing
///      block below skips.
///   2. Paste path emits `fillin-insert-text` back to the fill-in webview
///      instead of running Ctrl+V injection (WebView2 → WebView2 injection
///      is unreliable per [[feedback_webview2_input_injection]]).
pub static CLIPBOARD_OVERLAY_FOR_FILLIN: AtomicBool = AtomicBool::new(false);

/// HWNDs of the search/voice overlay and clipboard overlay while visible. Set by lib.rs
/// in show_overlay / show_clipboard_overlay, cleared by the corresponding hide_*.
/// Used by handle_mouse_down to detect click-outside-bounds dismissal — the blur-based
/// auto-close path doesn't fire when the window never grabs OS focus on initial show
/// (clipboard uses WS_EX_NOACTIVATE; search's set_focus can fail silently per Win32
/// foreground-stealing rules).
pub static SEARCH_OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);
pub static CLIPBOARD_OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);

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

/// Tracks whether the overlay was just opened (not toggled off).
static OVERLAY_JUST_OPENED: AtomicBool = AtomicBool::new(false);

/// Tracks whether voice mode is active — allows bare action-key presses to be
/// routed as voice events (modifiers were cleared on the initial keydown).
static VOICE_ACTIVE: AtomicBool = AtomicBool::new(false);
/// The VK code of the voice hotkey's action key (e.g., Space = 0x20).
static VOICE_ACTION_VK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Tracks whether the voice action key is physically held (to suppress key repeat).
static VOICE_KEY_HELD: AtomicBool = AtomicBool::new(false);

/// Tracks whether the most recent Menu key (VK_APPS, 0x5D) keydown was
/// suppressed. The matching keyup must also be suppressed, otherwise
/// DefWindowProc translates the keyup into WM_CONTEXTMENU and the OS context
/// menu opens alongside the user's mapped action. Set on keydown suppression,
/// consumed on keyup. Targeted to Menu key only — other keys don't have this
/// keyup-driven OS behaviour.
static MENU_KEYDOWN_SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// Tracks whether the radial menu overlay is open (for hold-to-select release detection).
static RADIAL_MENU_OPEN: AtomicBool = AtomicBool::new(false);
/// Tracks whether the radial action key is physically held (to suppress key repeat).
static RADIAL_KEY_HELD: AtomicBool = AtomicBool::new(false);
/// The VK code of the radial menu hotkey's action key (e.g., Space = 0x20).
static RADIAL_ACTION_VK: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

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

/// Set of (modifier bits, mouse suppress ID) pairs whose clicks must be
/// swallowed by the hook. Populated by rebuild_suppress_keys in two cases:
///   (1) modified mouse combos with a ::hold variant. A hold-armed press must
///       not leak its click to the app while the watcher waits for the
///       threshold (mirror of the keyboard ::hold rule).
///   (2) modified scroll assignments (Alt+MOUSE_SCROLL_DOWN etc.). Apps almost
///       always attach their own action to modified scroll — browser
///       text-size on Alt+Scroll, zoom on Ctrl+Scroll — so a user who
///       assigned a macro to it doesn't want both to fire.
/// Plain modified mouse BUTTON singles/doubles deliberately do NOT suppress —
/// that's shipped behaviour (the click still lands).
static SUPPRESS_MOD_MOUSE: OnceLock<RwLock<HashSet<(u8, u8)>>> = OnceLock::new();

fn suppress_mod_mouse() -> &'static RwLock<HashSet<(u8, u8)>> {
    SUPPRESS_MOD_MOUSE.get_or_init(|| RwLock::new(HashSet::new()))
}

/// Global map: mouse button suppress ID → set of linked profile names that have
/// a bare assignment for that button.  Built across ALL linked profiles (not just
/// the active one) so the hook can suppress mouse events during click-to-refocus
/// before the profile has switched.
static ALL_LINKED_MOUSE: OnceLock<RwLock<HashMap<u8, HashSet<String>>>> = OnceLock::new();

fn all_linked_mouse() -> &'static RwLock<HashMap<u8, HashSet<String>>> {
    ALL_LINKED_MOUSE.get_or_init(|| RwLock::new(HashMap::new()))
}

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
/// F13-F24: character-less extra function keys (Stream Deck / macropad triggers).
fn is_extra_f_key(key_id: &str) -> bool {
    matches!(key_id, "F13" | "F14" | "F15" | "F16" | "F17" | "F18" | "F19" | "F20" | "F21" | "F22" | "F23" | "F24")
}

/// Matches STATIC_BARE_ALLOWED in keyboardLayout.jsx.
fn is_static_bare_allowed(key_id: &str) -> bool {
    matches!(key_id,
        "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
        | "F13" | "F14" | "F15" | "F16" | "F17" | "F18" | "F19" | "F20" | "F21" | "F22" | "F23" | "F24"
        | "Insert" | "Home" | "End" | "Delete" | "PageUp" | "PageDown"
        | "PrintScreen" | "ScrollLock" | "Pause"
        | "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight"
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
    let mut mod_mouse_set: HashSet<(u8, u8)> = HashSet::new();
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
        // Unassigned library entries ("{Profile}::UNASSIGNED::{uuid}") have no
        // trigger — nothing to suppress. Provably a no-op even without this
        // (the uuid never resolves to a vk and UNASSIGNED maps to modifier
        // bits 0), but skip explicitly so nobody has to re-prove it.
        if combo_str == "UNASSIGNED" { continue; }
        // Skip ::double entries from the suppress set — double-only keys should
        // let the single press pass through to the app. When both single+double
        // exist, the single entry already adds the key to the suppress set.
        if parts.last() == Some(&"double") { continue; }
        // ::hold entries DO suppress (unlike ::double): a hold-armed key must
        // not leak its keystroke to the app while the watcher waits for the
        // threshold. Pro-gated — for free users hold mappings are inert, and
        // suppressing would leave a dead key.
        if parts.last() == Some(&"hold") && !crate::licence::is_pro() { continue; }
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
        } else if matches!(key_id, "MOUSE_SCROLL_UP" | "MOUSE_SCROLL_DOWN") {
            // Modified scroll (e.g. Alt+Scroll Down → Volume Down). Unlike
            // modified mouse BUTTONS — which deliberately pass through so the
            // app still receives the click — modified scrolls suppress when
            // assigned, because apps almost always attach their own action to
            // modified scroll (browser text-size on Alt+Scroll, zoom on
            // Ctrl+Scroll). See the suppress_mod_mouse comment for the design.
            if let Some(mouse_id) = mouse_key_id_to_suppress(key_id) {
                let bits = modifier_string_to_bits(combo_str);
                if bits != 0 {
                    mod_mouse_set.insert((bits, mouse_id));
                }
            }
        } else if parts.last() == Some(&"hold") {
            // Modified mouse combo with a ::hold variant (Pro — free users
            // already skipped above). Suppress the click so the hold watcher
            // owns the press cycle; early release synthesizes the click back.
            if let Some(mouse_id) = mouse_key_id_to_suppress(key_id) {
                let bits = modifier_string_to_bits(combo_str);
                if bits != 0 {
                    mod_mouse_set.insert((bits, mouse_id));
                }
            }
        }
    }
    log::info!("[HOOK] Rebuilt suppress set: {} key combos, {} bare mouse, {} mod-mouse hold (before overlay)", set.len(), mouse_set.len(), mod_mouse_set.len());
    if let Ok(mut w) = suppress_keys().write() {
        *w = set;
    }
    if let Ok(mut w) = suppress_bare_mouse().write() {
        *w = mouse_set;
    }
    if let Ok(mut w) = suppress_mod_mouse().write() {
        *w = mod_mouse_set;
    }
}

/// Rebuild the global map of mouse buttons → linked profiles.  Scans ALL linked
/// profiles (not just the active one) so the hook can suppress during refocus.
/// Called while engine_state is held — must NOT re-lock it.
fn rebuild_all_linked_mouse(assignments: &HashMap<String, Value>, profile_settings: &HashMap<String, Value>) {
    let mut map: HashMap<u8, HashSet<String>> = HashMap::new();
    for (profile, settings) in profile_settings.iter() {
        let is_linked = settings
            .get("linkedApp")
            .and_then(|v| v.as_str())
            .is_some();
        if !is_linked { continue; }
        let prefix = format!("{}::BARE::", profile);
        for key in assignments.keys() {
            if !key.starts_with(&prefix) { continue; }
            // Skip double entries (single lets the press pass; when both exist
            // the single entry covers suppression). Hold entries DO count —
            // a hold-armed bare button must suppress during click-to-refocus
            // too — but only for Pro (free-tier holds are inert).
            if key.ends_with("::double") { continue; }
            if key.ends_with("::hold") && !crate::licence::is_pro() { continue; }
            let key_id = key[prefix.len()..].split("::").next().unwrap_or("");
            if let Some(mouse_id) = mouse_key_id_to_suppress(key_id) {
                map.entry(mouse_id).or_default().insert(profile.clone());
            }
        }
    }
    if let Ok(mut w) = all_linked_mouse().write() {
        *w = map;
    }
}

/// Insert the overlay hotkey into the suppress set. Called separately because
/// rebuild_suppress_keys runs while holding engine_state, so it can't re-lock to read overlay_hotkey.
fn add_overlay_to_suppress(overlay: Option<(u8, u32)>) {
    if let Some(combo) = overlay {
        if let Ok(mut w) = suppress_keys().write() {
            w.insert(combo);
            log::info!("[HOOK] Overlay hotkey added to suppress set: bits={} vk=0x{:02X} (total {} combos)", combo.0, combo.1, w.len());
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

/// Insert the voice hotkey into the suppress set.
fn add_voice_to_suppress(voice: Option<(u8, u32)>) {
    if let Some(combo) = voice {
        if let Ok(mut w) = suppress_keys().write() {
            w.insert(combo);
        }
    }
}

/// Insert the clipboard paste hotkey into the suppress set.
///
/// Skipped entirely when clipboard capture is disabled, so the combo
/// (default Ctrl+Shift+V) passes through to the OS instead of being
/// hijacked by Keyfire. The suppress add is re-applied automatically when
/// capture is re-enabled via `refresh_clipboard_paste_suppress`.
fn add_clipboard_paste_to_suppress(combo: Option<(u8, u32)>) {
    if !crate::clipboard::is_capture_enabled() {
        return;
    }
    if let Some(combo) = combo {
        if let Ok(mut w) = suppress_keys().write() {
            w.insert(combo);
        }
    }
}

/// Remove the clipboard paste hotkey from the suppress set. Used when
/// clipboard capture is toggled off so the combo passes through to the OS.
fn remove_clipboard_paste_from_suppress(combo: Option<(u8, u32)>) {
    if let Some(combo) = combo {
        if let Ok(mut w) = suppress_keys().write() {
            w.remove(&combo);
        }
    }
}

/// Re-sync the clipboard paste hotkey in the suppress set against the
/// current capture-enabled state. Called from `clipboard::set_capture_enabled`
/// whenever the toggle flips, so the hotkey is atomically freed (when
/// disabled) or reclaimed (when re-enabled) without restarting hooks.
pub fn refresh_clipboard_paste_suppress() {
    let combo = engine_state_lock().clipboard_paste_hotkey;
    if crate::clipboard::is_capture_enabled() {
        if let Some(c) = combo {
            if let Ok(mut w) = suppress_keys().write() {
                w.insert(c);
            }
        }
    } else {
        remove_clipboard_paste_from_suppress(combo);
    }
}

/// Insert the radial menu hotkey into the suppress set.
fn add_radial_menu_to_suppress(combo: Option<(u8, u32)>) {
    if let Some(combo) = combo {
        if let Ok(mut w) = suppress_keys().write() {
            w.insert(combo);
        }
    }
}

/// Map Keyfire key ID back to VK code (reverse of vk_to_key_id).
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
        // F13-F24: no physical keyboard has them, which makes them ideal
        // dedicated triggers for Stream Decks, macropads and remapped keys.
        "F13" => Some(0x7C), "F14" => Some(0x7D), "F15" => Some(0x7E), "F16" => Some(0x7F),
        "F17" => Some(0x80), "F18" => Some(0x81), "F19" => Some(0x82), "F20" => Some(0x83),
        "F21" => Some(0x84), "F22" => Some(0x85), "F23" => Some(0x86), "F24" => Some(0x87),
        "ArrowLeft" => Some(0x25), "ArrowUp" => Some(0x26),
        "ArrowRight" => Some(0x27), "ArrowDown" => Some(0x28),
        "Home" => Some(0x24), "End" => Some(0x23),
        "PageUp" => Some(0x21), "PageDown" => Some(0x22),
        "Insert" => Some(0x2D), "Delete" => Some(0x2E),
        "Escape" => Some(0x1B), "Enter" => Some(0x0D), "Tab" => Some(0x09),
        "Space" => Some(0x20), "Backspace" => Some(0x08),
        "ContextMenu" => Some(0x5D),
        "Minus" => Some(0xBD), "Equal" => Some(0xBB),
        "BracketLeft" => Some(0xDB), "BracketRight" => Some(0xDD),
        "Semicolon" => Some(0xBA), "Quote" => Some(0xDE),
        "Backquote" => Some(0xC0),
        "Backslash" => Some(0xDC),
        "IntlBackslash" => Some(0xE2), // VK_OEM_102, ISO key beside left Shift
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

/// Lock the engine state, tolerating a poisoned mutex. A panic on any thread
/// that held this lock used to poison it, after which every `lock().unwrap()`
/// on the processor thread panicked too: the LL hook kept swallowing bound
/// keys while nothing dispatched them — a silently dead engine with a green
/// tray icon. The state is plain data, so recovering the guard is safe.
pub(crate) fn engine_state_lock() -> std::sync::MutexGuard<'static, EngineState> {
    engine_state().lock().unwrap_or_else(|p| p.into_inner())
}

/// True while both LL hooks are installed.
pub fn hooks_running() -> bool {
    HOOKS_RUNNING.load(Ordering::SeqCst)
}

/// Called by the foreground watcher when the session locks (Win+L, secure
/// desktop, sleep). Keyups delivered while the secure desktop is up never
/// reach the LL hook, so anything Keyfire was holding stays held: a repeat
/// kept spamming into the lock screen and resumed after unlock, a Hold-mode
/// key or bare-key remap stayed logically DOWN, and KEYS_HELD_DOWN treated the
/// first press after unlock as auto-repeat and dropped it.
pub fn on_session_locked() {
    crate::actions::stop_repeating_key();
    crate::actions::release_held_key();
    crate::actions::release_all_bare_remaps();
    if let Some(set) = KEYS_HELD_DOWN.get() {
        if let Ok(mut w) = set.write() {
            w.clear();
        }
    }
    sync_modifier_state_from_os();
    info!("[Keyfire] Session locked — released held/repeating input state");
}

/// Push the current engine status to the main window (status-bar chips).
/// Same payload shape as the pause-toggle and profile-switch emits.
pub fn emit_engine_status(app: &AppHandle) {
    let (profile, pause_str) = {
        let state = engine_state_lock();
        (state.active_profile.clone(), state.pause_hotkey_str.clone())
    };
    let _ = app.emit(
        "engine-status",
        serde_json::json!({
            "uiohookAvailable": HOOKS_RUNNING.load(Ordering::SeqCst),
            "nutjsAvailable": false,
            "macrosEnabled": MACROS_ENABLED.load(Ordering::SeqCst),
            "activeProfile": profile,
            "globalPauseToggleKey": pause_str,
            "isDemoMode": false,
        }),
    );
}

// ── Hold trigger state machine (v0.5, Pro) ──────────────────────────────────
//
// A key with a `::hold` variant defers ALL dispatch away from keydown:
//   keydown      → arm a timer entry here (and swallow auto-repeat keydowns)
//   watcher      → at threshold, fire the ::hold assignment while still held,
//                  cancel any double-tap bookkeeping for the key
//   keyup early  → release before threshold re-injects the deferred single /
//                  double through the existing pending_macro machinery, so
//                  modifier-release timing and double-tap dispatch are reused
//   keyup late   → entry.fired == true → everything suppressed
//
// Map is keyed by raw VK (one physical key = one hold cycle), which keeps
// keyup matching correct regardless of modifier release order. ONE watcher
// thread total — never a thread per keypress.

struct HoldEntry {
    /// Base storage key, no suffix (e.g. "Default::Ctrl+Shift::F12").
    storage_key: String,
    fire_at: Instant,
    inserted_at: Instant,
    fired: bool,
    hold_macro: Value,
    /// The base (single-press) assignment, if one exists — re-injected at
    /// early release.
    single_macro: Option<Value>,
    has_double: bool,
    is_bare: bool,
}

static HOLD_TIMERS: OnceLock<Mutex<HashMap<u32, HoldEntry>>> = OnceLock::new();

fn hold_timers() -> &'static Mutex<HashMap<u32, HoldEntry>> {
    HOLD_TIMERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Arm a hold timer for a fresh keydown. Returns silently when the vk already
/// has a live entry — that's an OS auto-repeat keydown, which must be
/// swallowed for the whole hold cycle. A stale entry (>10s, keyup lost to a
/// hook reinstall) is replaced rather than treated as a repeat.
fn arm_hold_timer(
    vk: u32,
    storage_key: String,
    hold_macro: Value,
    single_macro: Option<Value>,
    has_double: bool,
    is_bare: bool,
    threshold_ms: u64,
) {
    let mut timers = hold_timers().lock().unwrap();
    if let Some(existing) = timers.get(&vk) {
        if existing.inserted_at.elapsed() < Duration::from_secs(10) {
            return; // auto-repeat while held — swallow
        }
    }
    let now = Instant::now();
    timers.insert(
        vk,
        HoldEntry {
            storage_key,
            fire_at: now + Duration::from_millis(threshold_ms),
            inserted_at: now,
            fired: false,
            hold_macro,
            single_macro,
            has_double,
            is_bare,
        },
    );
}

/// Drop all armed hold timers. Called on assignment updates (entries hold
/// clones of old macros) and hook reinstall.
pub(crate) fn clear_hold_timers() {
    if let Some(m) = HOLD_TIMERS.get() {
        if let Ok(mut timers) = m.lock() {
            timers.clear();
        }
    }
}

// ── Auto-repeat tracking ────────────────────────────────────────────────
// Set of non-modifier VKs whose first WM_KEYDOWN has already been
// processed for the current physical press. Every subsequent WM_KEYDOWN
// for the same vk before its keyup is a Windows OS auto-repeat and must
// NOT re-enter the hotkey dispatch path. Without this guard, a foreground
// watcher mid-press profile switch lets the NEW active_profile's hold or
// double assignments fire under the same physical hold gesture — the
// storage_key is rebuilt per invocation using the current active_profile.
//
// Modifier VKs return early in handle_keydown so they never enter the set.
// Cleared on keyup, hook reinstall, and assignment updates.
static KEYS_HELD_DOWN: OnceLock<RwLock<HashSet<u32>>> = OnceLock::new();

fn keys_held_down() -> &'static RwLock<HashSet<u32>> {
    KEYS_HELD_DOWN.get_or_init(|| RwLock::new(HashSet::new()))
}

/// True if this vk is already held (auto-repeat). False = first press
/// of a new physical gesture; inserts the vk into the set.
fn record_keydown_and_check_repeat(vk: u32) -> bool {
    let mut held = keys_held_down().write().unwrap();
    if held.contains(&vk) {
        true
    } else {
        held.insert(vk);
        false
    }
}

pub(crate) fn clear_held_keys() {
    if let Some(set) = KEYS_HELD_DOWN.get() {
        if let Ok(mut held) = set.write() {
            held.clear();
        }
    }
}

static HOLD_WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);

/// The single trigr-hold-watcher thread: 16ms tick, fires entries whose
/// threshold has passed. Firing happens here (not in any hook callback) so
/// the hook latency budget is untouched.
fn spawn_hold_watcher(app: AppHandle) {
    if HOLD_WATCHER_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::Builder::new()
        .name("trigr-hold-watcher".to_string())
        .spawn(move || {
            loop {
                thread::sleep(Duration::from_millis(16));
                if HOLD_DETECTION_PAUSED.load(Ordering::SeqCst) {
                    continue;
                }
                // Collect expired entries under the lock, fire after releasing it.
                let mut to_fire: Vec<(String, Value, bool)> = Vec::new();
                {
                    let mut timers = hold_timers().lock().unwrap();
                    if timers.is_empty() {
                        continue;
                    }
                    let now = Instant::now();
                    for entry in timers.values_mut() {
                        if !entry.fired && now >= entry.fire_at {
                            entry.fired = true;
                            to_fire.push((
                                entry.storage_key.clone(),
                                entry.hold_macro.clone(),
                                entry.is_bare,
                            ));
                        }
                    }
                }
                for (sk, hold_macro, is_bare) in to_fire {
                    // Threshold reached → hold wins this press cycle: cancel the
                    // double window, any pending single timer, and any deferred
                    // pending macro for this key.
                    {
                        let mut state = engine_state_lock();
                        if let Some(cancel) = state.pending_single_cancel.remove(&sk) {
                            cancel.store(true, Ordering::SeqCst);
                        }
                        state.last_hotkey_time.remove(&sk);
                        if state.pending_trigger_key.as_deref() == Some(sk.as_str()) {
                            state.pending_macro = None;
                            state.pending_storage_key = None;
                            state.pending_trigger_key = None;
                            state.pending_is_bare = false;
                        }
                    }
                    let hold_trigger = format!("{}::hold", sk);
                    info!("[Keyfire] [HOLD] fired: {}", hold_trigger);
                    fire_macro(hold_macro, is_bare, Some(hold_trigger), &app);
                }
            }
        })
        .expect("Failed to spawn hold watcher thread");
}

/// Outcome of the mouse hold-arm check (see mouse_hold_check).
enum MouseHoldOutcome {
    /// No ::hold variant (or Free tier / detection paused) — caller runs
    /// its normal single/double dispatch.
    NotArmed,
    /// Hold timer armed (or a live entry swallowed the press) — the press
    /// cycle is consumed; handle_mouse_up resolves it.
    Consumed,
    /// Second tap of a double landed while hold-armed — caller drops the
    /// state lock and fires this double immediately.
    FireDouble(Value),
}

/// Mouse twin of the keyboard ::hold branch in handle_keydown. A ::hold
/// variant takes over the whole press cycle: arm the shared watcher timer
/// and consume the press; handle_mouse_up re-injects the deferred single /
/// double (or synthesizes a passthrough click) on early release; the watcher
/// fires the hold at threshold. Differences from keyboard: no OS auto-repeat,
/// and deferred dispatch fires immediately at mouse-up (mouse has no
/// modifier-release deferral). Timer map is keyed by the real button VK
/// (mouse_button_to_vk). Free users fall through — ::hold mappings inert.
fn mouse_hold_check(
    state: &mut EngineState,
    storage_key: &str,
    vk: u32,
    is_bare: bool,
) -> MouseHoldOutcome {
    let hold_key = format!("{}::hold", storage_key);
    if !crate::licence::is_pro()
        || !state.assignments.contains_key(&hold_key)
        || HOLD_DETECTION_PAUSED.load(Ordering::SeqCst)
    {
        return MouseHoldOutcome::NotArmed;
    }

    // Live-entry swallow: mouse has no auto-repeat, but a mouse-up lost to a
    // hook reinstall could leave a stale entry — mirror the keyboard guard
    // (>10s stale entries get replaced inside arm_hold_timer).
    // Lock order state→timers is safe: no thread nests timers→state.
    {
        let timers = hold_timers().lock().unwrap();
        if let Some(existing) = timers.get(&vk) {
            if existing.inserted_at.elapsed() < Duration::from_secs(10) {
                return MouseHoldOutcome::Consumed;
            }
        }
    }

    let hold_macro = state.assignments.get(&hold_key).cloned().unwrap_or(Value::Null);
    let single_macro = state.assignments.get(storage_key).cloned();
    let double_key = format!("{}::double", storage_key);
    let has_double = state.assignments.contains_key(&double_key);
    let threshold = state.hold_threshold_ms;

    // Double-tap detection stays at button-down (tap-to-tap timing), same as
    // the keyboard hold branch. Per the conflict matrix, hold is NOT armed on
    // the second press.
    if has_double {
        let now = Instant::now();
        let dtw = state.double_tap_window_ms;
        if let Some(last) = state.last_hotkey_time.get(storage_key) {
            if now.duration_since(*last).as_millis() < dtw as u128 {
                if let Some(cancel) = state.pending_single_cancel.remove(storage_key) {
                    cancel.store(true, Ordering::SeqCst);
                }
                state.last_hotkey_time.remove(storage_key);
                info!("[Keyfire] x2 Mouse double-tap (hold-armed button): {}", storage_key);
                // fired=true sentinel keeps the watcher and the mouse-up
                // resolution inert for this press cycle.
                hold_timers().lock().unwrap().insert(vk, HoldEntry {
                    storage_key: storage_key.to_string(),
                    fire_at: now,
                    inserted_at: now,
                    fired: true,
                    hold_macro: Value::Null,
                    single_macro: None,
                    has_double: true,
                    is_bare,
                });
                let dm = state.assignments.get(&double_key).cloned().unwrap_or(Value::Null);
                return MouseHoldOutcome::FireDouble(dm);
            }
        }
        // First tap — record for the second-tap check above.
        state.last_hotkey_time.insert(storage_key.to_string(), now);
    }

    arm_hold_timer(vk, storage_key.to_string(), hold_macro, single_macro, has_double, is_bare, threshold);
    MouseHoldOutcome::Consumed
}

pub(crate) struct EngineState {
    pub(crate) active_profile: String,
    pub(crate) assignments: HashMap<String, Value>,
    pub(crate) profile_settings: HashMap<String, Value>,
    double_tap_window_ms: u64,
    // Hold trigger threshold (v0.5, Pro) — global, clamped 200–700ms.
    hold_threshold_ms: u64,
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
    pub(crate) overlay_hotkey: Option<(u8, u32)>,
    // Global pause hotkey — parsed as (modifier_bits, vk_code)
    pub(crate) pause_hotkey: Option<(u8, u32)>,
    pub(crate) pause_hotkey_str: Option<String>,
    // Global input method — resolved when per-assignment method is "global" or absent
    pub(crate) global_input_method: String,
    // Macro speed preset — "safe" | "fast" | "instant" | "custom"
    pub(crate) macro_speed: String,
    // Fire single-only assignments at keydown instead of deferring to keyup
    // (opt-in, "Fire on key press" in Settings). Only applies to keys with no
    // ::double and no ::hold variant — those gestures need the deferred paths.
    pub(crate) fire_on_press: bool,
    // Custom speed slider values (only used when macro_speed == "custom")
    pub(crate) custom_keystroke_delay: u64,
    pub(crate) custom_pre_execution_delay: u64,
    // Clipboard quick-paste hotkey — parsed as (modifier_bits, vk_code)
    pub(crate) clipboard_paste_hotkey: Option<(u8, u32)>,
    // Voice trigger hotkey — parsed as (modifier_bits, vk_code)
    pub(crate) voice_hotkey: Option<(u8, u32)>,
    // Radial menu hotkey — parsed as (modifier_bits, vk_code)
    pub(crate) radial_menu_hotkey: Option<(u8, u32)>,
    // Quick Record (temp macro) hotkeys — parsed as (modifier_bits, vk_code).
    // Record toggle: start-if-idle / stop-if-recording. Play: replay the
    // most recently saved temp macro (no-op if empty). Both configurable via
    // Settings → Quick Record; defaults Ctrl+Alt+R / Ctrl+Alt+P. Shorthand
    // alternatives to ship: store None when user removes them.
    pub(crate) temp_macro_record_hotkey: Option<(u8, u32)>,
    pub(crate) temp_macro_record_hotkey_str: Option<String>,
    pub(crate) temp_macro_play_hotkey: Option<(u8, u32)>,
    pub(crate) temp_macro_play_hotkey_str: Option<String>,
    // Continuous-replay loop hotkey — press once to start, press again to
    // stop. Esc also stops via actions::esc_requested per
    // [[feedback_esc_global_macro_cancel]]. None when unset/disabled.
    pub(crate) temp_macro_loop_hotkey: Option<(u8, u32)>,
    pub(crate) temp_macro_loop_hotkey_str: Option<String>,
    // Cached temp macro events + capture timestamp. Persisted to config so
    // the slot survives restart. Play hotkey reads directly from here without
    // a disk round-trip. Cleared via Settings → Quick Record → Clear.
    pub(crate) temp_macro_events: Option<Vec<crate::recorder::RecordedEvent>>,
    pub(crate) temp_macro_captured_at: Option<String>,
    // Default date format for bare {date} token and unformatted Date Math tokens.
    // One of: "DD/MM/YYYY" | "MM/DD/YYYY" | "YYYY-MM-DD". Existing v0.4.6 users
    // with no config field land on DD/MM/YYYY (the prior implicit default).
    pub(crate) default_date_format: String,
}

use std::sync::Arc;

impl Default for EngineState {
    fn default() -> Self {
        Self {
            active_profile: "Default".to_string(),
            assignments: HashMap::new(),
            profile_settings: HashMap::new(),
            double_tap_window_ms: 300,
            hold_threshold_ms: 350,
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
            fire_on_press: false,
            custom_keystroke_delay: 30,
            custom_pre_execution_delay: 150,
            voice_hotkey: None, // Voice ships unmapped; user picks a hotkey when they enable voice (Pro-gated).
            clipboard_paste_hotkey: Some((3, 0x56)), // Default: Ctrl+Shift+V (bits=3, vk=0x56)
            radial_menu_hotkey: None, // Set via set_radial_menu_hotkey command
            // OFF by default — Quick Record captures keystrokes, a privacy
            // implication users should opt into via Settings → Quick Record.
            // Suggested combos when they enable: Ctrl+Alt+R / Ctrl+Alt+P
            // (avoiding Ctrl+Shift+R / Ctrl+Shift+T which collide with browser
            // hard-refresh + reopen-closed-tab on every Chromium app).
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

// ── Hook events ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum HookEvent {
    KeyDown { vk_code: u32, scan_code: u32 },
    KeyUp { vk_code: u32, scan_code: u32 },
    MouseDown { button: MouseButton },
    MouseUp { button: MouseButton },
    MouseWheel { delta: i16 },
    // Recorder stop hotkey detected in the keyboard hook. Processor thread
    // sees this and emits a Tauri event so the frontend retrieves the buffer.
    RecorderStopRequested,
    // Quick Record: user pressed the record hotkey while NOT recording. The
    // processor checks recorder::TEMP_RECORDING_ACTIVE — when false, starts
    // a temp recording (sets the flag, calls recorder::start, emits a "saved"
    // toast on the next stop). Always handled by the processor, never the hook.
    TempMacroRecordRequested,
    // Quick Record: user pressed the play hotkey. Processor checks the engine's
    // cached temp_macro_events; if non-empty, spawns a replay thread.
    TempMacroPlayRequested,
    // Quick Record: user pressed the loop hotkey. Processor toggles
    // recorder::TEMP_MACRO_LOOP_ACTIVE — if false, spawns a thread running
    // replay_recorded_events_loop until the flag flips or Esc fires. If true,
    // sets the flag to false; the in-flight thread observes at its next
    // checkpoint and exits.
    TempMacroLoopRequested,
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

// ── VK code → Keyfire key ID mapping ──────────────────────────────────────────

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
        0x7C => Some("F13"), 0x7D => Some("F14"), 0x7E => Some("F15"), 0x7F => Some("F16"),
        0x80 => Some("F17"), 0x81 => Some("F18"), 0x82 => Some("F19"), 0x83 => Some("F20"),
        0x84 => Some("F21"), 0x85 => Some("F22"), 0x86 => Some("F23"), 0x87 => Some("F24"),
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
        0x5D => Some("ContextMenu"),
        // Symbols
        0xBD => Some("Minus"),
        0xBB => Some("Equal"),
        0xDB => Some("BracketLeft"),
        0xDD => Some("BracketRight"),
        0xBA => Some("Semicolon"),
        0xDE => Some("Quote"),
        0xC0 => Some("Backquote"),
        0xDC => Some("Backslash"),
        0xE2 => Some("IntlBackslash"),
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

/// Real Win32 button VKs — used as HOLD_TIMERS keys for mouse hold cycles.
/// No collision with keyboard entries: the keyboard hook never reports
/// vk 0x01-0x06, so handle_keyup can't touch mouse entries and vice versa.
fn mouse_button_to_vk(button: MouseButton) -> u32 {
    match button {
        MouseButton::Left => 0x01,   // VK_LBUTTON
        MouseButton::Right => 0x02,  // VK_RBUTTON
        MouseButton::Middle => 0x04, // VK_MBUTTON
        MouseButton::Side1 => 0x05,  // VK_XBUTTON1
        MouseButton::Side2 => 0x06,  // VK_XBUTTON2
    }
}

/// Button name in the form actions::replay_mouse_button expects — used by the
/// hold early-release click passthrough.
fn mouse_button_to_replay_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "Left",
        MouseButton::Right => "Right",
        MouseButton::Middle => "Middle",
        MouseButton::Side1 => "Side1",
        MouseButton::Side2 => "Side2",
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

/// Fallback check for the "click to refocus" case: the linked app is NOT the
/// foreground, but the cursor IS over one of its windows.  Uses the PID cache
/// built by the foreground watcher.  Returns the matched profile name so the
/// caller can switch profiles inline.
///
/// SAFETY: only fast kernel calls (GetCursorPos, WindowFromPoint,
/// GetWindowThreadProcessId) — safe from the LL hook thread.
fn cursor_over_unfocused_linked_app() -> Option<String> {
    unsafe {
        let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
        let cursor_wnd = windows_sys::Win32::UI::WindowsAndMessaging::WindowFromPoint(pt);
        if cursor_wnd.is_null() { return None; }
        let mut cursor_pid: u32 = 0;
        windows_sys::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(cursor_wnd, &mut cursor_pid);
        if cursor_pid == 0 { return None; }
        crate::foreground::linked_profile_for_pid(cursor_pid)
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

// ── Live key legends from the Windows layout ─────────────────────────────────
// The on-screen keyboard is drawn from fixed physical positions ("slots",
// named after their US-QWERTY key id). For each slot we ask the active input
// layout which VK sits there and what it types, plain and shifted, and run
// that VK through resolve_key_id so the slot shows the SAME key id the hook
// will report when the user presses it. AZERTY/QWERTZ letters therefore land
// in the right place, and UK/DE/FR/… symbols carry their real legends.

/// One drawn key position and what the current layout puts on it.
#[derive(serde::Serialize)]
pub struct KeyLegend {
    /// US-QWERTY id of the physical position (matches keyboardLayout.jsx).
    pub slot: &'static str,
    /// Key id the hook reports for a press at this position on this layout.
    pub key_id: String,
    pub base: String,
    pub shift: String,
}

/// Set-1 scancodes of every character slot the canvas draws.
const SLOT_SCANCODES: &[(&str, u32)] = &[
    ("Backquote", 0x29),
    ("Digit1", 0x02), ("Digit2", 0x03), ("Digit3", 0x04), ("Digit4", 0x05), ("Digit5", 0x06),
    ("Digit6", 0x07), ("Digit7", 0x08), ("Digit8", 0x09), ("Digit9", 0x0A), ("Digit0", 0x0B),
    ("Minus", 0x0C), ("Equal", 0x0D),
    ("KeyQ", 0x10), ("KeyW", 0x11), ("KeyE", 0x12), ("KeyR", 0x13), ("KeyT", 0x14),
    ("KeyY", 0x15), ("KeyU", 0x16), ("KeyI", 0x17), ("KeyO", 0x18), ("KeyP", 0x19),
    ("BracketLeft", 0x1A), ("BracketRight", 0x1B),
    ("KeyA", 0x1E), ("KeyS", 0x1F), ("KeyD", 0x20), ("KeyF", 0x21), ("KeyG", 0x22),
    ("KeyH", 0x23), ("KeyJ", 0x24), ("KeyK", 0x25), ("KeyL", 0x26),
    ("Semicolon", 0x27), ("Quote", 0x28), ("Backslash", 0x2B),
    ("KeyZ", 0x2C), ("KeyX", 0x2D), ("KeyC", 0x2E), ("KeyV", 0x2F), ("KeyB", 0x30),
    ("KeyN", 0x31), ("KeyM", 0x32),
    ("Comma", 0x33), ("Period", 0x34), ("Slash", 0x35),
    ("IntlBackslash", 0x56),
];

/// What a VK types on `hkl` with the given Shift state. Dead keys come back
/// as a negative count with the character still in the buffer, so the sign is
/// dropped. Flag 0x4 asks ToUnicodeEx not to disturb the real keyboard state
/// (Windows 10 1607+), which matters because a dead key would otherwise
/// linger and mangle the user's next real keystroke.
unsafe fn layout_char(vk: u32, scan: u32, shift: bool, hkl: windows_sys::Win32::UI::Input::KeyboardAndMouse::HKL) -> String {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::ToUnicodeEx;
    let mut state = [0u8; 256];
    if shift {
        state[0x10] = 0x80; // VK_SHIFT down
    }
    let mut buf = [0u16; 8];
    let n = ToUnicodeEx(vk, scan, state.as_ptr(), buf.as_mut_ptr(), buf.len() as i32, 0x4, hkl);
    let n = (n.unsigned_abs() as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..n])
        .chars()
        .filter(|c| !c.is_control() && !c.is_whitespace())
        .collect()
}

/// Legends for every character slot on the calling thread's input layout.
pub fn keyboard_legends() -> Vec<KeyLegend> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, MapVirtualKeyExW};
    const MAPVK_VSC_TO_VK_EX: u32 = 3;
    let mut out = Vec::with_capacity(SLOT_SCANCODES.len());
    unsafe {
        let hkl = GetKeyboardLayout(0);
        for &(slot, scan) in SLOT_SCANCODES {
            let vk = MapVirtualKeyExW(scan, MAPVK_VSC_TO_VK_EX, hkl);
            if vk == 0 {
                continue;
            }
            let key_id = resolve_key_id(vk, scan).unwrap_or(slot);
            out.push(KeyLegend {
                slot,
                key_id: key_id.to_string(),
                base: layout_char(vk, scan, false, hkl),
                shift: layout_char(vk, scan, true, hkl),
            });
        }
    }
    out
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

/// Resolve character with Shift state — US/UK layout only. Used by the
/// expansion buffer so triggers requiring Shift (`:`, `?`, `>`, `<`, `"`,
/// uppercase letters, shifted digits) match correctly. Non-US layouts still
/// produce US/UK characters here; a future fix will use ToUnicodeEx.
pub(crate) fn resolve_char_with_shift(vk: u32, scan: u32, shift: bool) -> Option<char> {
    // Letters: the case that lands on screen is Shift XOR Caps Lock. Without
    // the Caps side, Caps-on typing stores a case-inverted buffer ("tHE" on
    // screen buffered as "The"), so the Caps Lock autocorrect could never see
    // the exact shape it exists to fix.
    if (0x41..=0x5A).contains(&vk) {
        let upper = shift ^ crate::expansions::caps_lock_on();
        let base = if upper { b'A' } else { b'a' };
        return Some((base + (vk - 0x41) as u8) as char);
    }
    if !shift {
        return resolve_char(vk, scan);
    }
    match vk {
        // Top-row digits → US/UK shifted symbols
        0x31 => Some('!'),
        0x32 => Some('@'),
        0x33 => Some('#'),
        0x34 => Some('$'),
        0x35 => Some('%'),
        0x36 => Some('^'),
        0x37 => Some('&'),
        0x38 => Some('*'),
        0x39 => Some('('),
        0x30 => Some(')'),
        // OEM punctuation (shared between US and UK QWERTY)
        0xBD => Some('_'),  // - → _
        0xBB => Some('+'),  // = → +
        0xDB => Some('{'),  // [ → {
        0xDD => Some('}'),  // ] → }
        0xBA => Some(':'),  // ; → :
        0xDE => Some('"'),  // ' → " (US) / @ on UK — accept US default
        0xC0 => Some('~'),  // ` → ~
        0xDC => Some('|'),  // \ → |
        0xBC => Some('<'),  // , → <
        0xBE => Some('>'),  // . → >
        0xBF => Some('?'),  // / → ?
        _ => None,
    }
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

/// True if the key being pressed right now (key_id + currently held
/// modifiers) is the trigger described by `storage_key`
/// ("Profile::Combo::KeyId[::double|::hold]"; Combo is "BARE" or the
/// build_modifier_combo string).
fn press_matches_trigger(storage_key: &str, key_id: &str) -> bool {
    let mut parts = storage_key.split("::");
    let (Some(_profile), Some(combo), Some(trigger_key)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    if trigger_key != key_id {
        return false;
    }
    let held = build_modifier_combo();
    if combo == "BARE" { held.is_empty() } else { combo == held }
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

/// Physical modifier check straight from the OS, bypassing the tracked MOD_*
/// atomics. Those are fed by hook keyboard events, which don't arrive while
/// Keyfire's own WebView2 has focus — stale-false exactly when the user is
/// interacting with our UI (e.g. recording a trigger). GetAsyncKeyState is a
/// fast non-blocking syscall, safe in hook callbacks.
fn os_any_modifier_down() -> bool {
    unsafe {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        GetAsyncKeyState(0xA2) < 0 || GetAsyncKeyState(0xA3) < 0   // Ctrl
            || GetAsyncKeyState(0xA0) < 0 || GetAsyncKeyState(0xA1) < 0   // Shift
            || GetAsyncKeyState(0xA4) < 0 || GetAsyncKeyState(0xA5) < 0   // Alt
            || GetAsyncKeyState(0x5B) < 0 || GetAsyncKeyState(0x5C) < 0   // Win
    }
}

/// True when Shift is the only modifier held — used to route Shift+printable
/// keystrokes (`:`, `?`, `"`, etc.) into the text-expansion buffer alongside
/// the unmodified path. Ctrl/Alt/Win combos still go through the modified-
/// hotkey path only.
fn has_only_shift() -> bool {
    MOD_SHIFT.load(Ordering::SeqCst)
        && !MOD_CTRL.load(Ordering::SeqCst)
        && !MOD_ALT.load(Ordering::SeqCst)
        && !MOD_META.load(Ordering::SeqCst)
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

    // Esc → cancel whatever is running right now (loop, one-shot, Type Text,
    // Wait step, Record Macro replay, repeat). One lock-free timestamp store
    // per real Esc keydown; runs compare it against their own start time
    // (actions::esc_requested), so an Esc pressed while nothing is running
    // is simply older than the next run and cannot block it. We do NOT
    // suppress Esc — the target app should still see it so any open modal
    // closes too.
    if n_code >= 0 && matches!(w_param as u32, WM_KEYDOWN | WM_SYSKEYDOWN) {
        let kb = &*(l_param as *const KBDLLHOOKSTRUCT);
        if kb.vkCode == 0x1B /* VK_ESCAPE */ && (kb.flags & LLKHF_INJECTED) == 0 {
            crate::actions::esc_stamp();
        }
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
    if n_code >= 0 && SUPPRESS_SIMULATED.load(Ordering::SeqCst) {
        // Mid-injection: our synthetic events pass through, but a real user keypress
        // for a suppressed key must still be blocked — otherwise it leaks to the game
        // as raw input (e.g. "I" reaching the game instead of being consumed by Keyfire).
        //
        // LLKHF_INJECTED distinguishes synthetic (SendInput) events from real
        // hardware input. Required for the hold-only passthrough path: when we
        // SendInput a suppressed VK (e.g. F8 with ::hold mapped), this hook would
        // otherwise treat it as a real press and block it.
        let kb = &*(l_param as *const KBDLLHOOKSTRUCT);
        let is_real = (kb.flags & LLKHF_INJECTED) == 0;
        let is_down = matches!(w_param as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let is_up = matches!(w_param as u32, WM_KEYUP | WM_SYSKEYUP);
        // Real MODIFIER transitions must still reach the processor so the
        // MOD_* atomics track the physical state. Repeat mode keeps this flag
        // up for ~30ms of every iteration, so without this a held/released
        // Alt inside that window left modifier_bits() stale and the next
        // combo mis-matched. Pass through to the OS unchanged.
        if is_real && is_modifier_vk(kb.vkCode) && (is_down || is_up) {
            if is_down {
                send_event(HookEvent::KeyDown { vk_code: kb.vkCode, scan_code: kb.scanCode });
            } else {
                send_event(HookEvent::KeyUp { vk_code: kb.vkCode, scan_code: kb.scanCode });
            }
        }
        if is_down
            && is_real
            && !is_modifier_vk(kb.vkCode)
            && MACROS_ENABLED.load(Ordering::SeqCst)
        {
            let bits = modifier_bits();
            if let Ok(set) = suppress_keys().try_read() {
                if set.contains(&(bits, kb.vkCode)) && !(bits == 0 && is_foreground_dialog()) {
                    if kb.vkCode == 0x5D {
                        MENU_KEYDOWN_SUPPRESSED.store(true, Ordering::SeqCst);
                    }
                    // Swallowed from the app, but the processor MUST still
                    // hear it: this is a bound combo the user pressed on
                    // purpose. Previously it was eaten silently here, so a
                    // repeat-mode trigger re-pressed inside the suppression
                    // window never reached the same-trigger stop check —
                    // the "Alt+1 sometimes doesn't stop the spam" bug.
                    send_event(HookEvent::KeyDown { vk_code: kb.vkCode, scan_code: kb.scanCode });
                    return 1;
                }
            }
        }
    }
    if n_code >= 0 && !SUPPRESS_SIMULATED.load(Ordering::SeqCst) {
        let kb = &*(l_param as *const KBDLLHOOKSTRUCT);

        // ── Macro recorder ingestion ────────────────────────────────────────
        // When a recording is in progress, observe every real (non-injected)
        // keystroke. Stop-hotkey detection on KEYDOWN suppresses the keystroke
        // entirely (don't leak the stop combo to the target app) and signals
        // the processor to emit a Tauri event. All other keystrokes fall
        // through to the normal flow — recording is a side observation, the
        // user's keys must still reach the target app.
        if crate::recorder::IS_RECORDING_MACRO.load(Ordering::SeqCst)
            && (kb.flags & LLKHF_INJECTED) == 0
        {
            let is_down = matches!(w_param as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
            let is_up = matches!(w_param as u32, WM_KEYUP | WM_SYSKEYUP);
            if is_down {
                let bits = modifier_bits();
                // Hardcoded stop combo Ctrl+Alt+R (bits=5=Ctrl+Alt, vk=0x52) as
                // a fallback when the user has NOT configured a Quick Record
                // hotkey. Only active while recording — outside a recording
                // Ctrl+Alt+R still passes through normally so it doesn't
                // hijack macros that use the combo. This exists because the
                // macro-editor Record button doesn't require Quick Record to
                // be enabled; a fresh profile would otherwise have no way to
                // stop-via-hotkey and could only click the pill's Stop.
                let is_hardcoded_stop = kb.vkCode == 0x52
                    && bits == 5
                    && crate::recorder::TEMP_MACRO_RECORD_VK.load(Ordering::SeqCst) == 0;
                if crate::recorder::matches_record_hotkey(kb.vkCode, bits) || is_hardcoded_stop {
                    // Flip the flag IMMEDIATELY so modifier keyups that
                    // follow don't leak into the buffer. The processor
                    // event routes to the appropriate stop handler based
                    // on TEMP_RECORDING_ACTIVE (editor vs global flow).
                    crate::recorder::IS_RECORDING_MACRO.store(false, Ordering::SeqCst);
                    send_event(HookEvent::RecorderStopRequested);
                    return 1;
                }
            }
            if is_down || is_up {
                crate::recorder::push_key(kb.vkCode, kb.scanCode, is_down);
            }
        } else if (kb.flags & LLKHF_INJECTED) == 0
            && matches!(w_param as u32, WM_KEYDOWN | WM_SYSKEYDOWN)
        {
            // Not currently recording — check the Quick Record start + play
            // hotkeys. Both must be suppressed so they don't leak to the
            // underlying app even on subsequent keyup. Only fires when
            // MACROS_ENABLED so a paused engine doesn't trigger record/play.
            if MACROS_ENABLED.load(Ordering::SeqCst) {
                let bits = modifier_bits();
                if !is_modifier_vk(kb.vkCode) && bits != 0 {
                    if crate::recorder::matches_record_hotkey(kb.vkCode, bits) {
                        send_event(HookEvent::TempMacroRecordRequested);
                        return 1;
                    }
                    if crate::recorder::matches_play_hotkey(kb.vkCode, bits) {
                        send_event(HookEvent::TempMacroPlayRequested);
                        return 1;
                    }
                    if crate::recorder::matches_loop_hotkey(kb.vkCode, bits) {
                        send_event(HookEvent::TempMacroLoopRequested);
                        return 1;
                    }
                }
            }
        }

        match w_param as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                // CRITICAL: for Space pre-swallow, evaluate the swallow decision
                // and set SPACE_PRE_SWALLOWED *before* send_event posts the
                // KeyDown event to the processor channel. Otherwise the
                // processor can race ahead and run check_space_trigger before
                // the atomic is stored, causing it to take the legacy +1-
                // backspace path and corrupt the character before the trigger.
                let space_swallow = if kb.vkCode == 0x20 /* VK_SPACE */ {
                    // AUTOCORRECT_PENDING joins the space pre-swallow: a buffer
                    // matching a misspelling needs the exact same race fix as a
                    // space-mode trigger. check_space_trigger consumes the same
                    // SPACE_PRE_SWALLOWED latch for both.
                    let should_swallow = (crate::expansions::EXPANSION_PENDING_SPACE.load(Ordering::SeqCst)
                        || crate::expansions::AUTOCORRECT_PENDING.load(Ordering::SeqCst))
                        && modifier_bits() == 0
                        && MACROS_ENABLED.load(Ordering::SeqCst)
                        && !APP_INPUT_FOCUSED.load(Ordering::SeqCst)
                        && !IS_RECORDING_HOTKEY.load(Ordering::SeqCst)
                        && !IS_CAPTURING_KEY.load(Ordering::SeqCst)
                        && !CLIPBOARD_OVERLAY_VISIBLE.load(Ordering::SeqCst)
                        && FILLIN_HWND.load(Ordering::SeqCst) == 0;
                    if should_swallow {
                        crate::expansions::SPACE_PRE_SWALLOWED.store(true, Ordering::SeqCst);
                    }
                    should_swallow
                } else {
                    false
                };

                // Autocorrect terminator pre-swallow: Enter, Tab, and the
                // unshifted punctuation OEM keys complete a word the same way
                // Space does. Swallow only when the buffer already resolves a
                // correction (AUTOCORRECT_PENDING) under the exact guard set
                // the Space branch uses — MUST stay mirrored with it. Shifted
                // punctuation ('!', '?') is deliberately NOT swallowed: shifted
                // combos can be hotkey territory, so those fall back to the
                // +1-backspace path in check_char_terminator. The processor
                // re-injects any swallowed key that doesn't end up firing.
                // Backspace pre-swallow for the one-shot autocorrect undo:
                // while AC_UNDO_ARMED, a bare Backspace is consumed here and
                // try_undo_autocorrect reverts the whole correction. Guard set
                // mirrors the terminator swallows. A swallow on a stale armed
                // flag is safe — try_undo re-injects the Backspace.
                let ac_swallow = if !space_swallow
                    && kb.vkCode == 0x08 /* BACKSPACE */
                    && crate::expansions::AC_UNDO_ARMED.load(Ordering::SeqCst)
                {
                    let should_swallow = modifier_bits() == 0
                        && MACROS_ENABLED.load(Ordering::SeqCst)
                        && !APP_INPUT_FOCUSED.load(Ordering::SeqCst)
                        && !IS_RECORDING_HOTKEY.load(Ordering::SeqCst)
                        && !IS_CAPTURING_KEY.load(Ordering::SeqCst)
                        && !CLIPBOARD_OVERLAY_VISIBLE.load(Ordering::SeqCst)
                        && FILLIN_HWND.load(Ordering::SeqCst) == 0;
                    if should_swallow {
                        crate::expansions::AC_BS_PRE_SWALLOWED.store(true, Ordering::SeqCst);
                    }
                    should_swallow
                } else if !space_swallow
                    && matches!(
                        kb.vkCode,
                        0x0D /* RETURN */ | 0x09 /* TAB */
                        | 0xBE /* OEM_PERIOD . */ | 0xBC /* OEM_COMMA , */
                        | 0xBA /* OEM_1 ; */
                    )
                {
                    let should_swallow = crate::expansions::AUTOCORRECT_PENDING.load(Ordering::SeqCst)
                        && modifier_bits() == 0
                        && MACROS_ENABLED.load(Ordering::SeqCst)
                        && !APP_INPUT_FOCUSED.load(Ordering::SeqCst)
                        && !IS_RECORDING_HOTKEY.load(Ordering::SeqCst)
                        && !IS_CAPTURING_KEY.load(Ordering::SeqCst)
                        && !CLIPBOARD_OVERLAY_VISIBLE.load(Ordering::SeqCst)
                        && FILLIN_HWND.load(Ordering::SeqCst) == 0;
                    if should_swallow {
                        crate::expansions::AC_KEY_PRE_SWALLOWED.store(true, Ordering::SeqCst);
                    }
                    should_swallow
                } else {
                    false
                };

                send_event(HookEvent::KeyDown {
                    vk_code: kb.vkCode,
                    scan_code: kb.scanCode,
                });
                // Pixel-pick eyedropper: swallow ESC so the cancel gesture
                // doesn't also reach the foreground app. MIRROR of the
                // PIXEL_PICK ESC branch in handle_keydown — keep in sync.
                if PIXEL_PICK_ACTIVE.load(Ordering::SeqCst)
                    && kb.vkCode == 0x1B
                    && (kb.flags & LLKHF_INJECTED) == 0
                {
                    return 1;
                }
                // Clipboard popup open (WS_EX_NOACTIVATE — the target app still
                // owns keyboard focus): the processor routes this keydown to the
                // popup's search via 'clipboard-overlay-key', so it must be
                // blocked here or it ALSO types into the target app. Mirror of
                // the routing condition in handle_keydown: modifier keys and
                // Ctrl/Alt/Win combos pass through, injected events (our own
                // paste Ctrl+V) pass through. Keyups are deliberately not
                // suppressed — same orphan-keyup convention as the suppress-set
                // path, and it avoids stuck keys when a key was already held
                // down before the popup opened. Fill-in mode is excluded: that
                // popup takes real focus and its DOM owns the keys.
                if CLIPBOARD_OVERLAY_VISIBLE.load(Ordering::SeqCst)
                    && !CLIPBOARD_OVERLAY_FOR_FILLIN.load(Ordering::SeqCst)
                    && (kb.flags & LLKHF_INJECTED) == 0
                    && !is_modifier_vk(kb.vkCode)
                    && !MOD_CTRL.load(Ordering::SeqCst)
                    && !MOD_ALT.load(Ordering::SeqCst)
                    && !MOD_META.load(Ordering::SeqCst)
                {
                    return 1;
                }
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
                                if kb.vkCode == 0x5D {
                                    MENU_KEYDOWN_SUPPRESSED.store(true, Ordering::SeqCst);
                                }
                                return 1;
                            }
                        }
                    }
                }
                if space_swallow || ac_swallow {
                    return 1;
                }
            }
            WM_KEYUP | WM_SYSKEYUP => {
                send_event(HookEvent::KeyUp {
                    vk_code: kb.vkCode,
                    scan_code: kb.scanCode,
                });
                // Menu key (VK_APPS, 0x5D) opens the OS context menu via
                // DefWindowProc on WM_KEYUP — not WM_KEYDOWN. Consume the
                // matching-keydown flag so the keyup is suppressed ONLY when
                // we actually suppressed the keydown. Bare Menu presses that
                // weren't mapped still flow through and open the OS menu.
                if kb.vkCode == 0x5D
                    && MENU_KEYDOWN_SUPPRESSED.swap(false, Ordering::SeqCst)
                {
                    return 1;
                }
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
    // Mid-injection (SUPPRESS_SIMULATED): synthetic events must not re-enter
    // the engine, but REAL button presses still have to reach the processor.
    // Repeat mode holds the flag for ~30ms of every iteration, so a mouse
    // trigger re-pressed inside that window was previously dropped here and
    // the same-trigger stop check never ran (keyboard twin: the suppressed-
    // combo forward in keyboard_hook_proc). Forward only — the suppress
    // decision below is untouched, so this window's click still passes to
    // the app exactly as it did before.
    if n_code >= 0 && SUPPRESS_SIMULATED.load(Ordering::SeqCst) {
        let ms = &*(l_param as *const MSLLHOOKSTRUCT);
        if (ms.flags & LLMHF_INJECTED) == 0 {
            let xbutton = || -> MouseButton {
                if ((ms.mouseData >> 16) & 0xFFFF) as u16 == 1 { MouseButton::Side1 } else { MouseButton::Side2 }
            };
            match w_param as u32 {
                WM_LBUTTONDOWN => send_event(HookEvent::MouseDown { button: MouseButton::Left }),
                WM_LBUTTONUP   => send_event(HookEvent::MouseUp   { button: MouseButton::Left }),
                WM_RBUTTONDOWN => send_event(HookEvent::MouseDown { button: MouseButton::Right }),
                WM_RBUTTONUP   => send_event(HookEvent::MouseUp   { button: MouseButton::Right }),
                WM_MBUTTONDOWN => send_event(HookEvent::MouseDown { button: MouseButton::Middle }),
                WM_MBUTTONUP   => send_event(HookEvent::MouseUp   { button: MouseButton::Middle }),
                WM_XBUTTONDOWN => send_event(HookEvent::MouseDown { button: xbutton() }),
                WM_XBUTTONUP   => send_event(HookEvent::MouseUp   { button: xbutton() }),
                _ => {}
            }
        }
    }
    if n_code >= 0 && !SUPPRESS_SIMULATED.load(Ordering::SeqCst) {
        let mut suppress_id: Option<u8> = None;
        let mut is_button_down = false;

        // ── Macro recorder ingestion ────────────────────────────────────────
        // Capture every real mouse event (clicks, wheel, throttled moves) while
        // a recording is active. Synthetic events (LLMHF_INJECTED) are skipped
        // so Keyfire's own SendInput doesn't loop back into the recording buffer.
        if crate::recorder::IS_RECORDING_MACRO.load(Ordering::SeqCst) {
            let ms = &*(l_param as *const MSLLHOOKSTRUCT);
            if (ms.flags & LLMHF_INJECTED) == 0 {
                let mx = ms.pt.x;
                let my = ms.pt.y;
                match w_param as u32 {
                    WM_LBUTTONDOWN => crate::recorder::push_mouse_button("Left", mx, my, true),
                    WM_LBUTTONUP   => crate::recorder::push_mouse_button("Left", mx, my, false),
                    WM_RBUTTONDOWN => crate::recorder::push_mouse_button("Right", mx, my, true),
                    WM_RBUTTONUP   => crate::recorder::push_mouse_button("Right", mx, my, false),
                    WM_MBUTTONDOWN => crate::recorder::push_mouse_button("Middle", mx, my, true),
                    WM_MBUTTONUP   => crate::recorder::push_mouse_button("Middle", mx, my, false),
                    WM_XBUTTONDOWN => {
                        let xbutton = ((ms.mouseData >> 16) & 0xFFFF) as u16;
                        let name = if xbutton == 1 { "Side1" } else { "Side2" };
                        crate::recorder::push_mouse_button(name, mx, my, true);
                    }
                    WM_XBUTTONUP => {
                        let xbutton = ((ms.mouseData >> 16) & 0xFFFF) as u16;
                        let name = if xbutton == 1 { "Side1" } else { "Side2" };
                        crate::recorder::push_mouse_button(name, mx, my, false);
                    }
                    WM_MOUSEWHEEL => {
                        let delta = (ms.mouseData >> 16) as i16;
                        crate::recorder::push_wheel(delta as i32, mx, my);
                    }
                    WM_MOUSEMOVE => crate::recorder::push_mouse_move(mx, my),
                    _ => {}
                }
            }
        }

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
        // ── Trigger-recording capture suppression ───────────────────────────
        // While hotkey recording is active, a capturable click (any button
        // with a modifier held, or a bare Middle/Side1/Side2) is consumed as
        // the recorded trigger by handle_mouse_down on the processor thread —
        // suppress it here so it doesn't ALSO fire in the app under the
        // cursor (e.g. Alt+Right Click opening a context menu mid-record).
        // MIRROR of the capture condition in handle_mouse_down — keep in
        // sync. Bare Left/Right pass through so the user can still click
        // Keyfire's own UI (Recording button to cancel). The matching UP is
        // suppressed by the paired-bit check below when the engine is
        // enabled; with macros paused the UP passes through orphaned — same
        // convention as keyboard suppress-set keyups.
        if IS_RECORDING_HOTKEY.load(Ordering::SeqCst) {
            if let Some(btn_id) = suppress_id {
                // OS-state modifier check, NOT the MOD_* atomics — those are
                // stale-false while Keyfire's WebView2 has focus (see
                // os_any_modifier_down). The processor-side capture branches
                // resync from the OS too, so both sides agree.
                if is_button_down {
                    let capturable = os_any_modifier_down()
                        || matches!(btn_id, SUPPRESS_MOUSE_MIDDLE | SUPPRESS_MOUSE_SIDE1 | SUPPRESS_MOUSE_SIDE2);
                    if capturable {
                        if let Some(bit) = suppress_btn_bit(btn_id) {
                            MOUSE_DOWN_SUPPRESSED.fetch_or(bit, Ordering::SeqCst);
                        }
                        return 1;
                    }
                } else if matches!(btn_id, SUPPRESS_MOUSE_SCROLL_UP | SUPPRESS_MOUSE_SCROLL_DOWN)
                    && os_any_modifier_down()
                {
                    // Wheel capture (modifier required — mirror of the
                    // handle_mouse_wheel condition). Standalone event, no
                    // down/up pairing needed. Button UPs don't match the
                    // scroll ids and fall through to the paired-bit path.
                    return 1;
                }
            }
        }

        // ── PIXEL_PICK suppression (Wait for Pixel eyedropper) ─────────────
        // The picking click must never reach the app under the cursor — it
        // would activate the very button being sampled. Left = pick, Right =
        // cancel; the paired-bit path suppresses the matching UP. SAMPLING
        // keeps the shield up through the post-pick settle so a rapid second
        // click can't leak either. MIRROR of the PIXEL_PICK branches in
        // handle_mouse_down — keep in sync.
        if PIXEL_PICK_ACTIVE.load(Ordering::SeqCst) || PIXEL_PICK_SAMPLING.load(Ordering::SeqCst) {
            if let Some(btn_id) = suppress_id {
                if is_button_down
                    && matches!(btn_id, SUPPRESS_MOUSE_LEFT | SUPPRESS_MOUSE_RIGHT)
                {
                    if let Some(bit) = suppress_btn_bit(btn_id) {
                        MOUSE_DOWN_SUPPRESSED.fetch_or(bit, Ordering::SeqCst);
                    }
                    return 1;
                }
            }
        }

        // ── CAPTURE_KEY suppression (action-editor mouse capture) ──────────
        // Tighter rules than the trigger recorder: L/R/M only (send_mouse_click
        // doesn't output side buttons), so Side1/Side2 clicks fall through and
        // work normally during capture. MIRROR of the CAPTURE_KEY branch in
        // handle_mouse_down — keep in sync.
        if IS_CAPTURING_KEY.load(Ordering::SeqCst) {
            if let Some(btn_id) = suppress_id {
                if is_button_down
                    && matches!(btn_id, SUPPRESS_MOUSE_LEFT | SUPPRESS_MOUSE_RIGHT | SUPPRESS_MOUSE_MIDDLE)
                {
                    let capturable = os_any_modifier_down()
                        || btn_id == SUPPRESS_MOUSE_MIDDLE;
                    if capturable {
                        if let Some(bit) = suppress_btn_bit(btn_id) {
                            MOUSE_DOWN_SUPPRESSED.fetch_or(bit, Ordering::SeqCst);
                        }
                        return 1;
                    }
                }
            }
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
                        let mut suppressed = false;
                        // Primary path: active profile suppress set + linked app is foreground
                        if let Ok(set) = suppress_bare_mouse().try_read() {
                            if set.contains(&btn_id) {
                                if is_cursor_over_linked_app() && !is_foreground_dialog() {
                                    suppressed = true;
                                }
                            }
                        }
                        // Fallback: linked app is NOT foreground but cursor IS over it
                        // (click-to-refocus scenario). Check the global linked-mouse map,
                        // verifying the specific profile under cursor has this button assigned.
                        // Pro gate mirrors the dispatch-side refocus check (~line 2023):
                        // without this, Free users get clicks swallowed but no remap fires.
                        if !suppressed && crate::licence::is_pro() {
                            if let Ok(map) = all_linked_mouse().try_read() {
                                if let Some(profiles) = map.get(&btn_id) {
                                    if let Some(profile_name) = cursor_over_unfocused_linked_app() {
                                        if profiles.contains(&profile_name) {
                                            suppressed = true;
                                        }
                                    }
                                }
                            }
                        }
                        // Modified-mouse hold: a (modifier, button) pair with a
                        // ::hold variant must not leak its click while the hold
                        // watcher waits (set built in rebuild_suppress_keys).
                        // Bits come from the tracked atomics — stale-false while
                        // our own UI is focused, which conveniently skips
                        // suppression exactly where dispatch is skipped too.
                        if !suppressed {
                            let bits = modifier_bits();
                            if bits != 0 {
                                if let Ok(set) = suppress_mod_mouse().try_read() {
                                    if set.contains(&(bits, btn_id)) {
                                        suppressed = true;
                                    }
                                }
                            }
                        }
                        if suppressed {
                            MOUSE_DOWN_SUPPRESSED.fetch_or(bit, Ordering::SeqCst);
                            return 1;
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
                    // Scroll event — no pairing needed, standalone check.
                    // Bare-scroll suppression only in linked profiles; modified
                    // scroll suppression is global (the app almost always uses
                    // modified scroll for its own action — Alt+Scroll browser
                    // text-size, Ctrl+Scroll zoom — and a user who bound it to
                    // a macro doesn't want both to fire).
                    if let Ok(set) = suppress_bare_mouse().try_read() {
                        if set.contains(&btn_id) {
                            if is_cursor_over_linked_app() && !is_foreground_dialog() {
                                return 1;
                            }
                        }
                    }
                    let bits = modifier_bits();
                    if bits != 0 {
                        if let Ok(set) = suppress_mod_mouse().try_read() {
                            if set.contains(&(bits, btn_id)) {
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
            log::info!("[PROC] Event processor started");
            info!("[Keyfire] Event processor started");
            // Supervised: a panic inside any handler used to unwind out of
            // this closure, drop the receiver and leave the LL hook feeding a
            // dead channel forever (heartbeat still ticked, so the watchdog
            // never noticed). Catch, log, and re-enter the loop.
            let mut panic_count: u32 = 0;
            loop {
            let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut last_heartbeat_count: isize = 0;
            while let Ok(event) = receiver.recv() {
                // Periodic heartbeat — log every 500 hook events
                let count = HOOK_EVENT_COUNT.load(Ordering::SeqCst);
                if count - last_heartbeat_count >= 500 {
                    info!("[Keyfire] Hook heartbeat: {} events processed", count);
                    last_heartbeat_count = count;
                }
                // Log if hook callback received nCode < 0
                if HOOK_NCODE_NEGATIVE.swap(false, Ordering::SeqCst) {
                    info!("[Keyfire] Hook nCode<0 received — hook may be dying");
                }
                // Recorder stop-hotkey signal — hook already suppressed the
                // keystroke; emit to the frontend so it retrieves the buffer
                // and clears IS_RECORDING_MACRO. Handled BEFORE the
                // macros-disabled / pause-hotkey branch so stop still works
                // when the user has paused macros mid-recording.
                if matches!(event, HookEvent::RecorderStopRequested) {
                    let (count, dur) = crate::recorder::status_snapshot();
                    // Branch on TEMP_RECORDING_ACTIVE: editor flow lets the
                    // frontend retrieve events via stop_macro_recording;
                    // global flow finalises here without a UI round-trip.
                    if crate::recorder::TEMP_RECORDING_ACTIVE.load(Ordering::SeqCst) {
                        crate::recorder::TEMP_RECORDING_ACTIVE.store(false, Ordering::SeqCst);
                        let events = crate::recorder::stop();
                        let captured_at = chrono::Local::now().to_rfc3339();
                        // Cache in engine state for fast play + persist to disk.
                        if let Ok(mut state) = engine_state().lock() {
                            state.temp_macro_events = Some(events.clone());
                            state.temp_macro_captured_at = Some(captured_at.clone());
                        }
                        crate::persist_temp_macro(&events, &captured_at);
                        // Hide the recording bar — global flow has no UI round-trip
                        // through the frontend, so Rust hides directly.
                        crate::hide_recorder_bar(app.clone());
                        let _ = app.emit(
                            "temp-macro-saved",
                            serde_json::json!({
                                "count": events.len(),
                                "durationMs": dur,
                                "capturedAt": captured_at,
                            }),
                        );
                        log::info!("[RECORDER] Temp macro saved ({} events, {}ms)", events.len(), dur);
                    } else {
                        let _ = app.emit(
                            "recorder-stop-requested",
                            serde_json::json!({ "count": count, "durationMs": dur }),
                        );
                        log::info!("[RECORDER] Stop hotkey relayed to frontend");
                    }
                    continue;
                }
                if matches!(event, HookEvent::TempMacroRecordRequested) {
                    // Ignore record press while the Loop is running — user
                    // must stop the loop first (Loop hotkey again or Esc).
                    // Otherwise the new recording's user input would arrive
                    // mid-replay, mixing the two streams.
                    if crate::recorder::TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst) {
                        log::info!("[RECORDER] Quick Record press ignored — Quick Loop is running");
                        crate::emit_user_toast(&app, "info", "Stop the running Quick Loop first (loop hotkey again or Esc).");
                        continue;
                    }
                    crate::recorder::TEMP_RECORDING_ACTIVE.store(true, Ordering::SeqCst);
                    // show_recorder_bar shows the bottom-centre recording bar
                    // AND calls recorder::start internally — reuse the same
                    // pill the editor flow uses so the user gets identical
                    // visual feedback whether they hit Record in-app or via
                    // the global hotkey.
                    crate::show_recorder_bar(app.clone());
                    let _ = app.emit("temp-macro-recording-started", serde_json::json!({}));
                    log::info!("[RECORDER] Quick Record: recording started via global hotkey");
                    continue;
                }
                if matches!(event, HookEvent::TempMacroPlayRequested) {
                    // While the Loop is running, ignore single-Play presses —
                    // user must stop the loop first (Loop hotkey again or Esc).
                    // Avoids double-replay collisions on the same input queue.
                    if crate::recorder::TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst) {
                        log::info!("[RECORDER] Quick Replay press ignored — Quick Loop is running");
                        continue;
                    }
                    let snapshot: Option<(Vec<crate::recorder::RecordedEvent>, String)> = engine_state()
                        .lock()
                        .ok()
                        .and_then(|s| {
                            match (&s.temp_macro_events, &s.temp_macro_captured_at) {
                                (Some(ev), Some(ts)) if !ev.is_empty() => Some((ev.clone(), ts.clone())),
                                _ => None,
                            }
                        });
                    match snapshot {
                        Some((events, captured_at)) => {
                            let _ = app.emit(
                                "temp-macro-replay-started",
                                serde_json::json!({ "count": events.len(), "capturedAt": captured_at }),
                            );
                            std::thread::spawn(move || {
                                // Quick Replay bypasses execute_action, so it
                                // marks its own cancellable run (Esc from
                                // before this instant cannot abort it).
                                crate::actions::begin_cancellable_run();
                                let duration = crate::recorder::events_duration_secs(&events);
                                crate::actions::replay_recorded_events(&events, "Quick Replay");
                                crate::analytics::log_replay_fired(
                                    "GLOBAL::QUICKRECORD::replay",
                                    "Quick Record Replay",
                                    duration.max(1.0),
                                );
                            });
                        }
                        None => {
                            let _ = app.emit("temp-macro-replay-empty", serde_json::json!({}));
                            log::info!("[RECORDER] Quick Replay: no temp macro saved");
                        }
                    }
                    continue;
                }
                if matches!(event, HookEvent::TempMacroLoopRequested) {
                    // Toggle behaviour: if the loop is already running, the
                    // press is a stop request — flip the flag and the
                    // in-flight thread observes at its next checkpoint.
                    if crate::recorder::TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst) {
                        crate::recorder::TEMP_MACRO_LOOP_ACTIVE.store(false, Ordering::SeqCst);
                        let _ = app.emit("temp-macro-loop-stopped", serde_json::json!({}));
                        log::info!("[RECORDER] Quick Loop: stop requested via hotkey");
                        continue;
                    }
                    let snapshot: Option<(Vec<crate::recorder::RecordedEvent>, String)> = engine_state()
                        .lock()
                        .ok()
                        .and_then(|s| {
                            match (&s.temp_macro_events, &s.temp_macro_captured_at) {
                                (Some(ev), Some(ts)) if !ev.is_empty() => Some((ev.clone(), ts.clone())),
                                _ => None,
                            }
                        });
                    match snapshot {
                        Some((events, captured_at)) => {
                            let _ = app.emit(
                                "temp-macro-loop-started",
                                serde_json::json!({ "count": events.len(), "capturedAt": captured_at }),
                            );
                            std::thread::spawn(move || {
                                // Same as Quick Replay: own cancellable run.
                                crate::actions::begin_cancellable_run();
                                let duration = crate::recorder::events_duration_secs(&events);
                                let iters = crate::actions::replay_recorded_events_loop(&events, "Quick Loop");
                                if iters > 0 {
                                    // One row per loop session; credit = iterations × duration.
                                    crate::analytics::log_replay_fired(
                                        "GLOBAL::QUICKRECORD::loop",
                                        "Quick Record Loop",
                                        (iters as f64) * duration.max(1.0),
                                    );
                                }
                            });
                        }
                        None => {
                            let _ = app.emit("temp-macro-replay-empty", serde_json::json!({}));
                            log::info!("[RECORDER] Quick Loop: no temp macro saved");
                            crate::emit_user_toast(&app, "info", "Nothing recorded yet. Press the Quick Record hotkey first.");
                        }
                    }
                    continue;
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
                                        log::info!("[PAUSE] Unpaused via hotkey");
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
                    // Already handled above via `continue`. Compiler exhaustiveness.
                    HookEvent::RecorderStopRequested => {}
                    HookEvent::TempMacroRecordRequested => {}
                    HookEvent::TempMacroPlayRequested => {}
                    HookEvent::TempMacroLoopRequested => {}
                }
            }
            }));
            match run {
                Ok(()) => break,
                Err(_) => {
                    panic_count += 1;
                    error!("[Keyfire] Event processor recovered from a panic (#{}) — see the [PANIC] line above", panic_count);
                    // Drop stale suppression state so a half-finished dispatch
                    // can't leave the keyboard swallowed.
                    SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                    INJECTION_IN_PROGRESS.store(false, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(50));
                    continue;
                }
            }
            }
            info!("[Keyfire] Event processor stopped");
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

/// Drive the text-expansion buffer for a bare or Shift-only printable
/// keystroke. Called once any hotkey-matching is known to have NOT matched.
/// Skips work if the fill-in window has focus (those keystrokes belong to it).
fn process_expansion_keystroke(key_id: &str, vk: u32, scan: u32) {
    if FILLIN_HWND.load(Ordering::SeqCst) != 0 {
        return;
    }
    // Any input other than an immediate Backspace invalidates the one-shot
    // undo. Runs BEFORE handling so a fire during handling re-arms cleanly.
    if key_id != "Backspace" {
        crate::expansions::disarm_undo();
    }

    if key_id == "Backspace" {
        // One-shot undo: Backspace as the very next input after a correction
        // reverts it (the hook pre-swallowed the keystroke when armed).
        if crate::expansions::try_undo_autocorrect() {
            return;
        }
        crate::expansions::buffer_pop();
    } else if key_id == "Space" {
        crate::expansions::check_space_trigger();
        crate::expansions::buffer_clear();
    } else if key_id == "Enter" || key_id == "Tab" {
        // Word terminators for autocorrect (hook may have pre-swallowed the
        // key — check_key_terminator re-injects it when nothing fires).
        crate::expansions::check_key_terminator(vk as u16);
        crate::expansions::buffer_clear();
    } else if key_id == "Escape" {
        crate::expansions::on_caret_moved();
        crate::expansions::buffer_clear();
    } else if matches!(key_id, "ArrowLeft" | "ArrowRight" | "ArrowUp" | "ArrowDown" | "Home" | "End" | "Delete") {
        // Caret moved — the buffer no longer reflects what's left of the
        // caret, so triggers, autocorrect, and sentence context must reset.
        crate::expansions::on_caret_moved();
        crate::expansions::buffer_clear();
    } else {
        let shift = MOD_SHIFT.load(Ordering::SeqCst);
        if let Some(ch) = resolve_char_with_shift(vk, scan, shift) {
            // Punctuation terminators check the buffer BEFORE the char joins
            // it. Fired → the batch already emitted the char, don't push.
            if crate::expansions::check_char_terminator(ch) {
                return;
            }
            crate::expansions::buffer_push(ch);
            crate::expansions::check_immediate_triggers();
        } else {
            // Dead key or unresolvable — if the hook pre-swallowed it for
            // autocorrect, give the keystroke back.
            crate::expansions::reinject_if_swallowed(vk as u16);
        }
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

    // First sighting of the ISO-only key proves the physical board is ISO.
    // Processor thread, not the hook proc, so an emit here is fine.
    if key_id == "IntlBackslash" && !ISO_KEY_SEEN.swap(true, Ordering::SeqCst) {
        let _ = app.emit("iso-key-detected", ());
    }

    // Update modifier state
    if is_modifier_vk(vk) {
        update_modifier_state(vk, true);
        // Clear expansion buffer on Ctrl/Alt/Win press (ARM64 timing safety —
        // those modifiers precede hotkey combos, never text). SHIFT is exempt:
        // it's a TEXT modifier, and resolve_char_with_shift exists precisely
        // so shifted chars join the buffer. Clearing on Shift broke every
        // trigger with a shifted char mid-word — "(eur)" lost its "(eur"
        // prefix to the Shift pressed for ')', "->" lost its '-' to the
        // Shift for '>'. Word-start Shift ("HEllo", "^2") only survived
        // because the buffer was empty when Shift went down.
        // Undo disarm stays for ALL modifiers — any modifier press means the
        // next Backspace isn't the "revert that" gesture.
        if !matches!(vk, 0xA0 | 0xA1) {
            crate::expansions::buffer_clear();
        }
        crate::expansions::disarm_undo();

        // Track sole modifier for key capture mode
        if IS_CAPTURING_KEY.load(Ordering::SeqCst) {
            let mut state = engine_state_lock();
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

    // ── Auto-repeat detection ────────────────────────────────────────────
    // True for every WM_KEYDOWN after the first one of this physical press
    // (Windows OS key-repeat at ~30 Hz). Skips hotkey dispatch only — the
    // expansion buffer fall-through still receives auto-repeats so held
    // character keys feed triggers like ":kr" normally. See KEYS_HELD_DOWN.
    let is_auto_repeat = record_keydown_and_check_repeat(vk);

    // ── Foreground sync ─────────────────────────────────────────────────
    // On the first press of a new physical gesture, eliminate the 1500ms
    // foreground-watcher poll race: if the foreground HWND has changed since
    // the watcher last recorded it, switch the active profile inline so the
    // storage_key we build below resolves against the actual foreground app.
    // Fast-path ~2µs (GetForegroundWindow + atomic compare); first press
    // after a focus change pays ~50µs (OpenProcess + path query + switch).
    // Auto-repeats skip — they don't dispatch hotkeys either.
    // MUST run before any engine_state lock below (lock order: fg_state →
    // engine_state, set by check_and_switch_if_stale's internal callees).
    if !is_auto_repeat {
        crate::foreground::check_and_switch_if_stale(app);
    }

    // ── Hold / repeat "stops when" ──────────────────────────────────────
    // Esc always releases a held hotkey. Beyond that the action's `stopOn`
    // policy decides (actions::stop_on_any_key): "anyKey" stops on any real
    // NON-modifier key that is not the trigger itself — modifiers are chord
    // parts, and the trigger's own re-press must reach the executor so it
    // toggles the hold/repeat off cleanly; "escOrTrigger" leaves other keys
    // alone. Defaults: hold = anyKey, repeat = escOrTrigger (autoclickers
    // keep going while the user types), both changeable in the editor.
    if vk == 0x1B && crate::actions::is_key_held() {
        crate::actions::release_held_key();
        crate::tray::update_tray_icon_normal(app);
    } else if !is_modifier_vk(vk) && !is_auto_repeat {
        if let Some(policy) = crate::actions::held_any_key_stop() {
            if !policy.trigger.as_deref().map_or(false, |t| press_matches_trigger(t, &key_id)) {
                log::debug!("[DEBUG] HELD RELEASE (any key): key_id={}", key_id);
                crate::actions::release_held_key();
                crate::tray::update_tray_icon_normal(app);
            }
        }
        if let Some(policy) = crate::actions::repeat_any_key_stop() {
            if !policy.trigger.as_deref().map_or(false, |t| press_matches_trigger(t, &key_id)) {
                crate::actions::stop_repeating_key();
                crate::tray::update_tray_icon_normal(app);
            }
        }
    }

    // ── Pixel-pick eyedropper: ESC cancels ──────────────────────────────
    // Mirror of the PIXEL_PICK ESC swallow in keyboard_hook_proc — keep in
    // sync. Other keys pass through untouched while picking.
    if PIXEL_PICK_ACTIVE.load(Ordering::SeqCst) && vk == 0x1B {
        PIXEL_PICK_ACTIVE.store(false, Ordering::SeqCst);
        let _ = app.emit("pixel-pick-cancelled", serde_json::json!({}));
        return;
    }

    // ── Recording mode: capture combo and send to frontend ──────────────
    // Must run BEFORE APP_INPUT_FOCUSED check — recording works while Keyfire UI is focused.
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
    // Must run BEFORE APP_INPUT_FOCUSED check — capture works while Keyfire UI is focused.
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

    // ── Overlay hotkey check (works even when Keyfire is focused) ───────
    if MACROS_ENABLED.load(Ordering::SeqCst) && has_any_modifier() {
        let state = engine_state_lock();
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
                OVERLAY_JUST_OPENED.store(true, Ordering::SeqCst);
                let _ = app.emit("toggle-overlay", Value::Null);
                return;
            }
        }
        drop(state);
    }

    // ── Voice trigger hotkey check ────────────────────────────────────
    // Full combo match (e.g., Ctrl+Alt+W): emit voice-open on first press,
    // voice-keydown on subsequent presses (voice already active).
    if MACROS_ENABLED.load(Ordering::SeqCst) && has_any_modifier() {
        let state = engine_state_lock();
        if let Some((mod_bits, vk)) = state.voice_hotkey {
            let current_bits = modifier_bits();
            let key_vk = key_id_to_vk(key_id);
            if current_bits == mod_bits && key_vk == Some(vk) {
                drop(state);
                // Always release held modifiers on any voice hotkey press
                MOD_CTRL.store(false, Ordering::SeqCst);
                MOD_SHIFT.store(false, Ordering::SeqCst);
                MOD_ALT.store(false, Ordering::SeqCst);
                MOD_META.store(false, Ordering::SeqCst);
                SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                crate::actions::release_held_modifiers();
                SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                if VOICE_ACTIVE.load(Ordering::SeqCst) {
                    // Overlay already open — close it.
                    // Key-repeat guard: only fire once per physical press.
                    let was_held = VOICE_KEY_HELD.swap(true, Ordering::SeqCst);
                    if !was_held {
                        info!("[Keyfire] Voice hotkey while active — closing overlay");
                        let _ = app.emit("voice-keydown", Value::Null);
                    }
                } else {
                    // Fresh press — open overlay.
                    // No key-repeat guard here: VOICE_ACTIVE=true immediately so any
                    // repeat keydown falls into the close branch above and is guarded there.
                    VOICE_ACTIVE.store(true, Ordering::SeqCst);
                    VOICE_ACTION_VK.store(vk, Ordering::SeqCst);
                    VOICE_KEY_HELD.store(true, Ordering::SeqCst);
                    info!("[Keyfire] Voice hotkey first press — emitting voice-open");
                    let _ = app.emit("voice-open", Value::Null);
                }
                return;
            }
        }
        drop(state);
    }
    // Bare action-key while voice is active (modifiers were cleared on first press,
    // so the combo check above won't match — this path catches the bare key).
    if VOICE_ACTIVE.load(Ordering::SeqCst) {
        let vk = VOICE_ACTION_VK.load(Ordering::SeqCst);
        if vk != 0 && key_id_to_vk(key_id) == Some(vk) {
            // Suppress keyboard repeat — only emit on fresh press (after keyup)
            if !VOICE_KEY_HELD.swap(true, Ordering::SeqCst) {
                info!("[Keyfire] Voice bare-key press — emitting voice-keydown");
                let _ = app.emit("voice-keydown", Value::Null);
            }
            return;
        }
    }

    // ── Clipboard quick-paste hotkey check ─────────────────────────────
    // Gated on `clipboard::is_capture_enabled` so the popup never fires when
    // the user has disabled clipboard, even if the combo somehow slips past
    // the suppress set. The suppress set is also refreshed on the toggle in
    // `clipboard::set_capture_enabled`, so this is defence-in-depth.
    if MACROS_ENABLED.load(Ordering::SeqCst)
        && has_any_modifier()
        && crate::clipboard::is_capture_enabled()
    {
        let state = engine_state_lock();
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
                // Fill-in mode: when a fill-in is open, route to the fill-in
                // popup path regardless of which window has actual DOM focus.
                // We can't rely on the FillInWindow's DOM keydown handler to
                // catch this — the LL hook has already eaten the combo via
                // suppress_keys before it reaches any window's DOM, so the
                // fill-in never sees it. Emit a dedicated event that lib.rs
                // handles by calling show_clipboard_overlay_for_fillin.
                if FILLIN_HWND.load(Ordering::SeqCst) != 0 {
                    let _ = app.emit("toggle-clipboard-overlay-for-fillin", Value::Null);
                } else {
                    let _ = app.emit("toggle-clipboard-overlay", Value::Null);
                }
                return;
            }
        }
        drop(state);
    }

    // ── Radial menu hotkey check ─────────────────────────────────────
    if MACROS_ENABLED.load(Ordering::SeqCst) && has_any_modifier() {
        let state = engine_state_lock();
        if let Some((mod_bits, vk)) = state.radial_menu_hotkey {
            let current_bits = modifier_bits();
            let key_vk = key_id_to_vk(key_id);
            if current_bits == mod_bits && key_vk == Some(vk) {
                // Key-repeat guard: if action key is still physically held, suppress
                if RADIAL_KEY_HELD.swap(true, Ordering::SeqCst) {
                    drop(state);
                    return;
                }
                drop(state);
                MOD_CTRL.store(false, Ordering::SeqCst);
                MOD_SHIFT.store(false, Ordering::SeqCst);
                MOD_ALT.store(false, Ordering::SeqCst);
                MOD_META.store(false, Ordering::SeqCst);
                SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                crate::actions::release_held_modifiers();
                SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                // Track the action key VK for hold-to-select release detection
                RADIAL_ACTION_VK.store(vk, Ordering::SeqCst);
                RADIAL_MENU_OPEN.store(true, Ordering::SeqCst);
                let _ = app.emit("toggle-radial-menu", Value::Null);
                return;
            }
        }
        drop(state);
    }
    // Bare action-key while radial key is held (modifiers were cleared on first press,
    // so the combo check above won't match on repeat — catch bare repeats here).
    if RADIAL_KEY_HELD.load(Ordering::SeqCst) {
        let radial_vk = RADIAL_ACTION_VK.load(Ordering::SeqCst);
        if radial_vk != 0 && key_id_to_vk(key_id) == Some(radial_vk) {
            return;
        }
    }

    // ── Global pause hotkey check (works even when paused) ────────────
    if has_any_modifier() {
        let state = engine_state_lock();
        if let Some((mod_bits, vk)) = state.pause_hotkey {
            let current_bits = modifier_bits();
            let key_vk = key_id_to_vk(key_id);
            log::debug!("[DEBUG] PAUSE CHECK: has_any_modifier=true, current_bits={}, mod_bits={}, key_vk={:?}, vk={}, key_id={}", current_bits, mod_bits, key_vk, vk, key_id);
            if current_bits == mod_bits && key_vk == Some(vk) {
                log::debug!("[DEBUG] PAUSE MATCH: firing pause");
                drop(state);
                let was_enabled = MACROS_ENABLED.load(Ordering::SeqCst);
                if was_enabled {
                    crate::actions::stop_repeating_key();
                }
                MACROS_ENABLED.store(!was_enabled, Ordering::SeqCst);
                let now_enabled = !was_enabled;
                log::info!("[PAUSE] Global pause toggled: macros={}", now_enabled);
                // Rebuild tray menu and notify frontend
                crate::tray::rebuild_tray_menu(app);
                crate::tray::update_tray_icon(app, now_enabled);
                {
                    let st = engine_state_lock();
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
    // If moved below, capture will silently fail when Keyfire has focus.
    // Skip if Keyfire input field is focused (normal hotkey matching suppressed)
    if APP_INPUT_FOCUSED.load(Ordering::SeqCst) {
        return;
    }

    // ── Clipboard overlay keyboard routing ─────────────────────────────────
    // The overlay uses WS_EX_NOACTIVATE so it never activates the window,
    // preventing focus-sensitive apps (SnagIt, drawing tools) from losing
    // their text cursor when the overlay opens. All keyboard input is routed
    // here instead of via DOM events.
    // Note: Ctrl+Shift+V (toggle) is handled above and has already returned.
    // Fill-in mode is the exception: the popup is activated with real OS focus
    // so its own DOM handlers own the search + arrow-nav + Enter keys. Routing
    // via this LL path would double-fire and desync the selected index.
    if CLIPBOARD_OVERLAY_VISIBLE.load(Ordering::SeqCst)
        && !CLIPBOARD_OVERLAY_FOR_FILLIN.load(Ordering::SeqCst)
    {
        if !is_modifier_vk(vk) {
            // Route bare or shift-modified keys (search input + navigation).
            // Ctrl/Alt/Win combos are not routed — avoids firing hotkeys or
            // typing shortcut letters into search while the overlay is open.
            // MUST stay mirrored with the hook-level suppression branch in
            // keyboard_hook_proc: a routed key is always suppressed from the
            // target app, a non-routed key always passes through to it.
            if !MOD_CTRL.load(Ordering::SeqCst)
                && !MOD_ALT.load(Ordering::SeqCst)
                && !MOD_META.load(Ordering::SeqCst)
            {
                let shift = MOD_SHIFT.load(Ordering::SeqCst);
                let _ = app.emit("clipboard-overlay-key", serde_json::json!({ "vk": vk, "shift": shift }));
            }
        }
        return;
    }

    // ── Normal hotkey matching ──────────────────────────────────────────
    let mut state = engine_state_lock();

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

        if bare_allowed && !is_auto_repeat {
            let bare_key = format!("{}::BARE::{}", profile, key_id);
            // Stop repeat if this key is the repeat trigger
            if crate::actions::is_repeating() {
                if let Some(trigger) = crate::actions::get_repeating_trigger() {
                    log::debug!("[DEBUG] REPEAT STOP CHECK (bare): incoming={}, trigger={}", bare_key, trigger);
                    if trigger == bare_key {
                        crate::actions::stop_repeating_key();
                        crate::tray::update_tray_icon_normal(app);
                        return;
                    }
                }
            }
            // ── Hold trigger (v0.5, Pro) on bare keys ───────────────────
            // Mirrors the modified-key hold branch below. Note: this runs
            // BEFORE the bare "hotkey" remap passthrough, so a hold-marked
            // bare key deliberately loses AHK-style direct passthrough — its
            // single fires at early release via pending_macro instead.
            let bare_hold_key = format!("{}::hold", bare_key);
            if crate::licence::is_pro()
                && state.assignments.contains_key(&bare_hold_key)
                && !HOLD_DETECTION_PAUSED.load(Ordering::SeqCst)
            {
                // Auto-repeat swallow — see the modified-key hold branch.
                {
                    let timers = hold_timers().lock().unwrap();
                    if let Some(existing) = timers.get(&vk) {
                        if existing.inserted_at.elapsed() < Duration::from_secs(10) {
                            return;
                        }
                    }
                }
                crate::expansions::buffer_clear();
                let hold_macro = state.assignments.get(&bare_hold_key).cloned().unwrap_or(Value::Null);
                let single_macro = state.assignments.get(&bare_key).cloned();
                let double_key = format!("{}::double", bare_key);
                let has_double = state.assignments.contains_key(&double_key);
                let threshold = state.hold_threshold_ms;

                // Keydown-time double-tap detection — see the modified-key
                // hold branch for why this can't live at keyup.
                if has_double {
                    let now = Instant::now();
                    let dtw = state.double_tap_window_ms;
                    if let Some(last) = state.last_hotkey_time.get(&bare_key) {
                        if now.duration_since(*last).as_millis() < dtw as u128 {
                            if let Some(cancel) = state.pending_single_cancel.remove(&bare_key) {
                                cancel.store(true, Ordering::SeqCst);
                            }
                            state.last_hotkey_time.remove(&bare_key);
                            info!("[Keyfire] x2 Keydown double-tap (hold-armed bare key): {}", bare_key);
                            // Sentinel so this press's auto-repeats hit the
                            // swallow check above — see the modified branch.
                            hold_timers().lock().unwrap().insert(vk, HoldEntry {
                                storage_key: bare_key.clone(),
                                fire_at: now,
                                inserted_at: now,
                                fired: true,
                                hold_macro: Value::Null,
                                single_macro: None,
                                has_double: true,
                                is_bare: true,
                            });
                            state.pending_macro = state.assignments.get(&double_key).cloned();
                            state.pending_storage_key = None;
                            state.pending_trigger_key = Some(bare_key);
                            state.pending_is_bare = true;
                            return;
                        }
                    }
                    state.last_hotkey_time.insert(bare_key.clone(), now);
                }

                drop(state);
                arm_hold_timer(vk, bare_key, hold_macro, single_macro, has_double, true, threshold);
                return;
            }

            if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                crate::expansions::buffer_clear();
                let action_type = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let double_key_str = format!("{}::double", bare_key);
                // Pro gate: Free users ignore double-tap mappings so single-press fires normally.
                let has_double = crate::licence::is_pro() && state.assignments.contains_key(&double_key_str);

                // Hotkey actions on bare keys: AHK-style direct passthrough.
                // keydown → send target keydown only (no up yet).
                // keyup  → remap_key_release sends target keyup (see handle_keyup).
                // This makes hold, tap, and OS key-repeat all feel identical to
                // pressing the target key directly.
                // Falls back to fire_macro for mouse, hold, repeat, or unknown key.
                if action_type == "hotkey" && !has_double {
                    if let Some(data) = macro_val.get("data") {
                        // Mirror-hold chord (holdMode + holdUntilRelease):
                        // outputs press here at keydown and release when THIS
                        // key's keyup reaches remap_key_release. Must run
                        // before remap_key_press (which rejects holdMode and
                        // would fall through to the keyup-deferred path).
                        if crate::actions::hold_chord_press(vk as u16, data) {
                            drop(state);
                            return;
                        }
                        if crate::actions::remap_key_press(vk as u16, data) {
                            drop(state);
                            return;
                        }
                    }
                    let trigger = bare_key.clone();
                    drop(state);
                    fire_macro(macro_val, false, Some(trigger), app);
                    return;
                }

                // Fire on key press (opt-in): bare single-only assignments fire
                // at keydown instead of keyup. Keys with a double variant keep
                // the deferred path — dispatch_with_double_tap owns the tap
                // window at keyup. is_bare stays false to match that path (the
                // assigned bare key was hook-suppressed, nothing leaked).
                if state.fire_on_press && !has_double {
                    let trigger = bare_key.clone();
                    drop(state);
                    info!("[Keyfire] Fire on press (bare): {}", trigger);
                    fire_macro_on_press(macro_val, Some(trigger), app);
                    return;
                }

                state.pending_macro = Some(macro_val);
                state.pending_storage_key = Some(bare_key.clone());
                state.pending_trigger_key = Some(bare_key);
                state.pending_is_bare = true;
                return;
            }

            // No single-press — check for double-only bare key
            // Pro gate: Free users skip double-only entirely (config preserved for upgrade).
            let double_key = format!("{}::double", bare_key);
            if crate::licence::is_pro() && state.assignments.contains_key(&double_key) {
                crate::expansions::buffer_clear();
                let now = Instant::now();
                let dtw = state.double_tap_window_ms;
                if let Some(last) = state.last_hotkey_time.get(&bare_key) {
                    if now.duration_since(*last).as_millis() < dtw as u128 {
                        state.last_hotkey_time.remove(&bare_key);
                        info!("[Keyfire] x2 Double-only bare: {}", bare_key);
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
        process_expansion_keystroke(&key_id, vk, scan);
        return;
    }

    // Build storage key from held modifiers
    let combo = build_modifier_combo();
    let profile = state.active_profile.clone();
    let storage_key = format!("{}::{}::{}", profile, combo, key_id);

    // Stop repeat if this key is the repeat trigger
    if crate::actions::is_repeating() {
        if let Some(trigger) = crate::actions::get_repeating_trigger() {
            log::debug!("[DEBUG] REPEAT STOP CHECK (modified): incoming={}, trigger={}", storage_key, trigger);
            if trigger == storage_key {
                crate::actions::stop_repeating_key();
                crate::tray::update_tray_icon_normal(app);
                return;
            }
        }
    }

    // Track whether a modified hotkey matched. If not, and Shift was the only
    // modifier held, the keystroke should still drive the expansion buffer
    // (so triggers like ":kr" work).
    let mut hotkey_matched = false;

    // Auto-repeat presses skip the hotkey dispatch — the original physical
    // press already made its decisions. Stops mid-press profile switches
    // from arming the new profile's hold/double under the same gesture.
    if !is_auto_repeat {
    // ── Hold trigger (v0.5, Pro) ────────────────────────────────────────
    // A ::hold variant takes over the whole press cycle: arm the watcher
    // timer and return (auto-repeat keydowns are swallowed inside
    // arm_hold_timer). Early release re-injects the deferred single/double
    // from handle_keyup; reaching threshold fires the hold from the watcher.
    // Free users fall through — their ::hold mappings stay stored but inert.
    let hold_key = format!("{}::hold", storage_key);
    if crate::licence::is_pro()
        && state.assignments.contains_key(&hold_key)
        && !HOLD_DETECTION_PAUSED.load(Ordering::SeqCst)
    {
        // Auto-repeat swallow — MUST run before the double-tap window logic
        // below: a held key repeats WM_KEYDOWN at ~30Hz and each repeat would
        // otherwise re-enter the bookkeeping (alternately re-arming hold and
        // re-detecting "second tap" — seen as x2 log spam in dev 2026-06-11).
        // Covers armed holds, post-fire holds, and the second-tap sentinel.
        // Lock order state→timers is safe: no thread nests timers→state.
        {
            let timers = hold_timers().lock().unwrap();
            if let Some(existing) = timers.get(&vk) {
                if existing.inserted_at.elapsed() < Duration::from_secs(10) {
                    return;
                }
            }
        }
        crate::expansions::buffer_clear();
        let hold_macro = state.assignments.get(&hold_key).cloned().unwrap_or(Value::Null);
        let single_macro = state.assignments.get(&storage_key).cloned();
        let double_key = format!("{}::double", storage_key);
        let has_double = state.assignments.contains_key(&double_key);
        let threshold = state.hold_threshold_ms;

        // Double-tap detection must stay at KEYDOWN (tap-to-tap timing).
        // Deferring it to keyup breaks modified combos: both taps' keyups
        // overwrite the same pending_macro slot while the modifiers stay
        // held, so only one dispatch ever happens and the double resolves
        // as a single.
        if has_double {
            let now = Instant::now();
            let dtw = state.double_tap_window_ms;
            if let Some(last) = state.last_hotkey_time.get(&storage_key) {
                if now.duration_since(*last).as_millis() < dtw as u128 {
                    // Second tap inside the window — resolves as a double.
                    // Per the conflict matrix, hold is NOT armed on the
                    // second press.
                    if let Some(cancel) = state.pending_single_cancel.remove(&storage_key) {
                        cancel.store(true, Ordering::SeqCst);
                    }
                    state.last_hotkey_time.remove(&storage_key);
                    info!("[Keyfire] x2 Keydown double-tap (hold-armed key): {}", storage_key);
                    // Sentinel so this press's auto-repeats hit the swallow
                    // check above. fired=true keeps the watcher and the keyup
                    // re-injection inert; the pending double set below still
                    // fires at keyup as normal.
                    hold_timers().lock().unwrap().insert(vk, HoldEntry {
                        storage_key: storage_key.clone(),
                        fire_at: now,
                        inserted_at: now,
                        fired: true,
                        hold_macro: Value::Null,
                        single_macro: None,
                        has_double: true,
                        is_bare: false,
                    });
                    state.pending_macro = state.assignments.get(&double_key).cloned();
                    state.pending_storage_key = None;
                    state.pending_trigger_key = Some(storage_key);
                    state.pending_is_bare = false;
                    return;
                }
            }
            // First tap — record for the second-tap check above.
            state.last_hotkey_time.insert(storage_key.clone(), now);
        }

        drop(state);
        arm_hold_timer(vk, storage_key, hold_macro, single_macro, has_double, false, threshold);
        return;
    }

    if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
        hotkey_matched = true;
        crate::expansions::buffer_clear();
        // Check for double-tap variant
        // Pro gate: Free users ignore double-tap mappings so single-press fires normally.
        let double_key = format!("{}::double", storage_key);
        let has_double = crate::licence::is_pro() && state.assignments.contains_key(&double_key);

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
                    info!("[Keyfire] x2 Keydown double-tap: {}", storage_key);
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

            info!("[Keyfire] x1 First tap: {} — waiting {}ms", storage_key, dtw);

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
                    let mut state = engine_state_lock();
                    state.pending_single_cancel.remove(&sk);
                    state.last_hotkey_time.remove(&sk);
                }
                info!("[Keyfire] x1 Single confirmed: {}", sk);
                fire_macro(macro_clone, false, Some(sk), &app_clone);
            });
            // Don't set pending_macro — timer handles firing
            return;
        } else {
            // No double variant.
            // Hotkey actions: fire inline at keydown (no thread, no deferred wait).
            // Everything else fires at keyup via pending_macro (needs clean modifier state),
            // unless Fire on key press is enabled — then single-only assignments
            // dispatch here at keydown for AHK-parity latency. The injection
            // handlers release still-held physical modifiers themselves.
            let action_type = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if action_type == "hotkey" {
                if let Some(data) = macro_val.get("data") {
                    // Mirror-hold chord for MODIFIED triggers (Ctrl+F10 etc):
                    // same contract as the bare-key site — press at keydown,
                    // release on the action key's keyup via remap_key_release.
                    // Modifier keyups don't end the hold; only the action key.
                    if crate::actions::hold_chord_press(vk as u16, data) {
                        drop(state);
                        return;
                    }
                    if crate::actions::execute_hotkey_inline(data, app) {
                        drop(state);
                        return;
                    }
                }
            }
            if state.fire_on_press {
                drop(state);
                info!("[Keyfire] Fire on press: {}", storage_key);
                fire_macro_on_press(macro_val, Some(storage_key), app);
                return;
            }
            state.pending_macro = Some(macro_val);
            state.pending_storage_key = None;
            state.pending_trigger_key = Some(storage_key);
        }
        state.pending_is_bare = false;
    } else {
        // No single-press — check for double-only
        // Pro gate: Free users skip double-only entirely (config preserved for upgrade).
        let double_key = format!("{}::double", storage_key);
        if crate::licence::is_pro() && state.assignments.contains_key(&double_key) {
            hotkey_matched = true;
            crate::expansions::buffer_clear();
            let now = Instant::now();
            let dtw = state.double_tap_window_ms;
            if let Some(last) = state.last_hotkey_time.get(&storage_key) {
                if now.duration_since(*last).as_millis() < dtw as u128 {
                    state.last_hotkey_time.remove(&storage_key);
                    info!("[Keyfire] x2 Double-only: {}", storage_key);
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
    } // end of `if !is_auto_repeat` (modified-key hotkey dispatch)

    // ── Shift-only fallthrough for text-expansion buffer ────────────────
    // If no modified hotkey matched and Shift is the only modifier held,
    // route the keystroke through the expansion buffer so triggers
    // requiring Shift (":kr", "?help", uppercase letters, etc.) work.
    // Ctrl/Alt/Win combos do NOT fall through — those are hotkey territory.
    if !hotkey_matched && has_only_shift() {
        drop(state);
        process_expansion_keystroke(&key_id, vk, scan);
    }
}

/// Send a synthetic keydown + 15ms hold + keyup pair for the given VK so
/// the OS / target app sees a clean tap. Used in the hold-armed early-
/// release path when no single mapping exists — passthrough behaviour so
/// the app's native handling of the key (F8 ortho, F5 refresh, arrow nav)
/// still works after the LL hook suppressed the original physical keydown.
/// Any modifiers still held during the synthesis are naturally included in
/// the app's view of the combo.
///
/// PIPELINE_KEY_HOLD_MS hold between keydown and keyup is the
/// [[feedback_synthetic_key_hold_time]] invariant — fused single-batch
/// SendInput is invisible to per-frame game polling. Uses the shorter
/// pipeline hold (not the 50ms one-shot KEY_HOLD_MS) because this runs on
/// the processor thread, where sleeping delays all queued input. Caller MUST
/// wrap with SUPPRESS_SIMULATED so our own LL hook ignores these synthetic
/// events.
fn send_synthetic_tap(vk: u32) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        MapVirtualKeyW, SendInput, INPUT, INPUT_KEYBOARD, INPUT_0, KEYBDINPUT,
        KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE,
    };
    unsafe {
        // Resolve hardware scancode for this VK on the current keyboard layout.
        // MAPVK_VK_TO_VSC = 0. Scancode-mode SendInput reaches DirectInput / Raw
        // Input games that read the hardware keyboard buffer directly and ignore
        // the cooked Win32 message stream (the v0.5.0 wVk-only synthesis worked
        // in BricsCAD/notepad but was invisible to FPS engines — the W-key
        // movement repro that drove this fix).
        let scan = MapVirtualKeyW(vk, 0) as u16;

        // Extended-key flag is mandatory for arrows, INS/DEL/HOME/END/PGUP/PGDN,
        // right Alt/Ctrl, NumLock, Print Screen, and the Windows keys — without
        // it those keys also fail to register in scancode-reading apps. Same
        // pattern as the Chromium-terminal Shift+Insert paste fix.
        let is_extended = matches!(vk,
            0x21..=0x28 | 0x2C..=0x2E | 0x5B | 0x5C | 0x90 | 0xA3 | 0xA5
        );

        // Fall back to VK-only mode if MapVirtualKeyW couldn't resolve a scan
        // (rare: some media-key VKs have no hardware mapping). VK-only mode is
        // the v0.5.0 behaviour — still works for standard Win32 apps even if
        // it doesn't reach raw-input games.
        let (w_vk, w_scan, base_flags) = if scan != 0 {
            let mut f = KEYEVENTF_SCANCODE;
            if is_extended { f |= KEYEVENTF_EXTENDEDKEY; }
            (0u16, scan, f)
        } else {
            (vk as u16, 0u16, 0)
        };

        let down = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: w_vk, wScan: w_scan, dwFlags: base_flags, time: 0, dwExtraInfo: 0,
                },
            },
        };
        let up = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: w_vk, wScan: w_scan, dwFlags: base_flags | KEYEVENTF_KEYUP, time: 0, dwExtraInfo: 0,
                },
            },
        };
        SendInput(1, &down as *const _, std::mem::size_of::<INPUT>() as i32);
        thread::sleep(Duration::from_millis(crate::actions::PIPELINE_KEY_HOLD_MS));
        SendInput(1, &up as *const _, std::mem::size_of::<INPUT>() as i32);
    }
}

// ── Keyup handler ───────────────────────────────────────────────────────────

fn handle_keyup(vk: u32, scan: u32, app: &AppHandle) {
    // Clear physical-press tracking for this vk. Safe for any vk — no-op
    // if not present, and modifier VKs are never tracked.
    if let Some(set) = KEYS_HELD_DOWN.get() {
        if let Ok(mut held) = set.write() {
            held.remove(&vk);
        }
    }

    // Normalize VK through scan-code resolution so OEM keys match on all layouts.
    let normalised_vk = resolve_key_id(vk, scan)
        .and_then(key_id_to_vk)
        .unwrap_or(vk);

    // Voice action-key release tracking
    if VOICE_KEY_HELD.load(Ordering::SeqCst) {
        let voice_vk = VOICE_ACTION_VK.load(Ordering::SeqCst);
        if voice_vk != 0 && normalised_vk == voice_vk {
            VOICE_KEY_HELD.store(false, Ordering::SeqCst);
        }
    }

    // Radial menu: clear held flag on action key release
    {
        let held = RADIAL_KEY_HELD.load(Ordering::SeqCst);
        let radial_vk = RADIAL_ACTION_VK.load(Ordering::SeqCst);
        if held && radial_vk != 0 && (vk == radial_vk || normalised_vk == radial_vk) {
            RADIAL_KEY_HELD.store(false, Ordering::SeqCst);
            RADIAL_MENU_OPEN.store(false, Ordering::SeqCst);
        }
    }

    // Update modifier state
    if is_modifier_vk(vk) {
        update_modifier_state(vk, false);

        // Key capture: bare modifier release
        if IS_CAPTURING_KEY.load(Ordering::SeqCst) && no_modifiers_held() {
            let state = engine_state_lock();
            if let Some(ref sole) = state.capture_sole_modifier {
                IS_CAPTURING_KEY.store(false, Ordering::SeqCst);
                let _ = app.emit("key-captured", Value::String(sole.clone()));
            }
        }
    }

    // Release active bare-key remap — sends target keyup if this trigger was remapped.
    // Must come after modifier update so modifier state is accurate for the release.
    if crate::actions::remap_key_release(vk as u16) {
        return;
    }

    // ── Hold trigger: trigger-key release ends the hold cycle ──────────────
    // fired == true → the watcher already fired the hold; suppress everything.
    // fired == false → released before threshold; re-inject the dispatch that
    // keydown deferred (single via pending_macro, double via the storage-key
    // route into dispatch_with_double_tap, double-only via the same window
    // bookkeeping the keydown path uses). The pending block below then fires
    // it once all modifiers are released.
    {
        let removed = {
            let mut timers = hold_timers().lock().unwrap();
            timers.remove(&vk)
        };
        if let Some(entry) = removed {
            if !entry.fired {
                if let Some(single) = entry.single_macro {
                    if entry.has_double {
                        // Single + double + hold, released before threshold:
                        // the single must wait out the double window. Spawn
                        // the same cancel-able timer the non-hold keydown
                        // path uses — a second tap cancels it via
                        // pending_single_cancel in the keydown hold branch.
                        let sk = entry.storage_key.clone();
                        let is_bare = entry.is_bare;
                        let mut state = engine_state_lock();
                        let dtw = state.double_tap_window_ms;
                        if let Some(old_cancel) = state.pending_single_cancel.remove(&sk) {
                            old_cancel.store(true, Ordering::SeqCst);
                        }
                        let cancel_flag = Arc::new(AtomicBool::new(false));
                        state.pending_single_cancel.insert(sk.clone(), cancel_flag.clone());
                        drop(state);
                        let app_clone = app.clone();
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(dtw));
                            if cancel_flag.load(Ordering::SeqCst) {
                                return; // second tap arrived — double fired instead
                            }
                            {
                                let mut state = engine_state_lock();
                                state.pending_single_cancel.remove(&sk);
                                state.last_hotkey_time.remove(&sk);
                            }
                            info!("[Keyfire] x1 Single confirmed (hold-deferred): {}", sk);
                            fire_macro(single, is_bare, Some(sk), &app_clone);
                        });
                    } else {
                        // Single + hold only: fire through the pending route so
                        // injection waits for clean modifier state as usual.
                        let mut state = engine_state_lock();
                        state.pending_macro = Some(single);
                        state.pending_storage_key = None;
                        state.pending_trigger_key = Some(entry.storage_key.clone());
                        state.pending_is_bare = entry.is_bare;
                    }
                } else if entry.has_double {
                    // Hold + double, NO single — defer the passthrough through the
                    // dtw window. If a second tap arrives the keydown hold branch
                    // sets pending_macro for double and cancels this; otherwise
                    // synthesize a clean tap so the app's native key behaviour
                    // fires (the LL hook suppressed the original physical keydown).
                    let sk = entry.storage_key.clone();
                    let key_vk = normalised_vk;
                    let mut state = engine_state_lock();
                    let dtw = state.double_tap_window_ms;
                    if let Some(old_cancel) = state.pending_single_cancel.remove(&sk) {
                        old_cancel.store(true, Ordering::SeqCst);
                    }
                    let cancel_flag = Arc::new(AtomicBool::new(false));
                    state.pending_single_cancel.insert(sk.clone(), cancel_flag.clone());
                    drop(state);
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(dtw));
                        if cancel_flag.load(Ordering::SeqCst) {
                            return; // second tap arrived → double fired instead
                        }
                        {
                            let mut state = engine_state_lock();
                            state.pending_single_cancel.remove(&sk);
                            state.last_hotkey_time.remove(&sk);
                        }
                        info!("[Keyfire] [HOLD] tap passthrough (hold+double, no single): {}", sk);
                        SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        send_synthetic_tap(key_vk);
                        thread::sleep(Duration::from_millis(5));
                        SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                    });
                } else {
                    // Hold-only, no single, no double — immediate passthrough so
                    // the app's native key behaviour fires (the LL hook suppressed
                    // the user's physical keydown). Modifiers held during this
                    // synthesis are naturally included in the app's view.
                    info!("[Keyfire] [HOLD] tap passthrough (hold-only): {}", entry.storage_key);
                    SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                    send_synthetic_tap(normalised_vk);
                    thread::sleep(Duration::from_millis(5));
                    SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                }
            }
        }
    }

    // Fire pending macro once all modifiers released (or immediately for bare keys)
    if no_modifiers_held() {
        let mut state = engine_state_lock();
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

fn check_overlay_outside_click(app: &AppHandle) {
    use windows_sys::Win32::Foundation::{POINT, RECT};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetCursorPos, GetWindowRect};

    let search_hwnd = SEARCH_OVERLAY_HWND.load(Ordering::SeqCst);
    let clipboard_hwnd = CLIPBOARD_OVERLAY_HWND.load(Ordering::SeqCst);

    if search_hwnd == 0 && clipboard_hwnd == 0 {
        return;
    }

    let mut pt = POINT { x: 0, y: 0 };
    unsafe { GetCursorPos(&mut pt); }

    let read_rect = |hwnd: isize| -> Option<RECT> {
        if hwnd == 0 { return None; }
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            if GetWindowRect(hwnd as _, &mut rect) == 0 {
                return None;
            }
        }
        Some(rect)
    };
    let cursor_outside = |rect: &RECT| -> bool {
        pt.x < rect.left || pt.x >= rect.right || pt.y < rect.top || pt.y >= rect.bottom
    };

    let voice = is_voice_active();

    // Search overlay: skip dismissal while voice recognition is active (the WinRT
    // recognizer briefly steals focus/cursor in ways that can mimic an outside click).
    if search_hwnd != 0 {
        if let Some(rect) = read_rect(search_hwnd) {
            if !voice && cursor_outside(&rect) {
                let _ = app.emit("close-overlay-outside-click", Value::Null);
            }
        }
    }
    if clipboard_hwnd != 0 {
        if let Some(rect) = read_rect(clipboard_hwnd) {
            if cursor_outside(&rect) {
                let _ = app.emit("close-clipboard-overlay-outside-click", Value::Null);
            }
        }
    }
}

fn handle_mouse_down(button: MouseButton, app: &AppHandle) {
    // Outside-click dismissal for the search/clipboard overlays. The blur-based path
    // doesn't fire on the first click outside when the overlay never grabbed OS focus
    // (clipboard is WS_EX_NOACTIVATE; search's set_focus can fail per Win32 rules).
    // Runs before the input-focus early return so this still fires when the user
    // hasn't yet clicked into the overlay.
    check_overlay_outside_click(app);
    // Same OS resync the keydown path does. After a missed modifier keyup
    // (secure desktop, Alt+Tab into Keyfire's own window) the MOD_* atomics
    // were stale here, so a plain click or scroll fired the Alt/Ctrl-modified
    // binding until the next keyboard keydown corrected them.
    sync_modifier_state_from_os();

    // A click moves the caret (or focus) — the expansion buffer no longer
    // reflects what's left of the caret, so expansion triggers, autocorrect,
    // the one-shot undo, and sentence context must all reset.
    crate::expansions::on_caret_moved();
    crate::expansions::buffer_clear();

    // ── Pixel-pick eyedropper (Wait for Pixel editor) ────────────────────
    // While active the next left click anywhere picks that screen point:
    // the click is consumed (PIXEL_PICK suppression mirror in
    // mouse_hook_proc — keep in sync), the pixel colour read, and both
    // handed to the editor. Right click cancels. Runs BEFORE the recording
    // and capture branches — the pick is a modal editor flow and nothing
    // else may see the click. Colour is the HOVER state of whatever was
    // clicked; the editor re-samples after the cursor moves away.
    // Post-pick settle window: the sampler thread below is still working.
    // The hook mirror suppresses these clicks; swallow them here too so a
    // double-click can't re-arm anything or fall through to dispatch.
    if PIXEL_PICK_SAMPLING.load(Ordering::SeqCst)
        && matches!(button, MouseButton::Left | MouseButton::Right)
    {
        return;
    }

    if PIXEL_PICK_ACTIVE.load(Ordering::SeqCst) {
        match button {
            MouseButton::Left => {
                PIXEL_PICK_ACTIVE.store(false, Ordering::SeqCst);
                PIXEL_PICK_SAMPLING.store(true, Ordering::SeqCst);
                let mut point = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point);
                }
                // Colour under the cursor right now is the HOVER state of
                // whatever was clicked. Sample it as a fallback, then hand
                // off to a worker: nudge the cursor off the point so hover
                // styling drops, settle 400ms for fade-out transitions,
                // sample the rest-state colour, put the cursor back, emit.
                // MUST NOT sleep on this (processor) thread.
                let hover = crate::actions::read_screen_pixel(point.x, point.y);
                let app2 = app.clone();
                std::thread::spawn(move || {
                    use windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos;
                    let dx = if point.x < 400 { 250 } else { -250 };
                    let dy = if point.y < 400 { 250 } else { -250 };
                    unsafe { SetCursorPos(point.x + dx, point.y + dy); }
                    std::thread::sleep(std::time::Duration::from_millis(400));
                    let rest = crate::actions::read_screen_pixel(point.x, point.y);
                    unsafe { SetCursorPos(point.x, point.y); }
                    let color = rest
                        .or(hover)
                        .map(|(r, g, b)| format!("#{:02x}{:02x}{:02x}", r, g, b));
                    PIXEL_PICK_SAMPLING.store(false, Ordering::SeqCst);
                    let _ = app2.emit(
                        "pixel-pick-result",
                        serde_json::json!({ "x": point.x, "y": point.y, "color": color }),
                    );
                });
                return;
            }
            MouseButton::Right => {
                PIXEL_PICK_ACTIVE.store(false, Ordering::SeqCst);
                let _ = app.emit("pixel-pick-cancelled", serde_json::json!({}));
                return;
            }
            _ => {}
        }
    }

    // ── Recording mode: capture mouse trigger and send to frontend ──────
    // Mirror of the keyboard recording branch in handle_keydown, and of the
    // hook-level suppression condition in mouse_hook_proc — keep both in
    // sync. Must run BEFORE the APP_INPUT_FOCUSED check so recording works
    // while the Keyfire UI is focused. Bare Left/Right never capture: the
    // user must be able to click Keyfire's own UI (Recording button to
    // cancel) and anything else on screen while recording. Those two stay
    // assignable via the mouse canvas only.
    if IS_RECORDING_HOTKEY.load(Ordering::SeqCst) {
        // The MOD_* atomics are fed by hook keyboard events, which don't
        // arrive while Keyfire's own WebView2 has focus (the JS keydown path
        // owns keyboard capture there) — and recording is nearly always done
        // with our UI focused. Resync from physical OS state before reading,
        // or modifier+click captures degrade to bare.
        sync_modifier_state_from_os();
        let capturable = has_any_modifier()
            || matches!(button, MouseButton::Middle | MouseButton::Side1 | MouseButton::Side2);
        if capturable {
            IS_RECORDING_HOTKEY.store(false, Ordering::SeqCst);

            let mut mods = Vec::new();
            if MOD_CTRL.load(Ordering::SeqCst) { mods.push("Ctrl"); }
            if MOD_SHIFT.load(Ordering::SeqCst) { mods.push("Shift"); }
            if MOD_ALT.load(Ordering::SeqCst) { mods.push("Alt"); }
            if MOD_META.load(Ordering::SeqCst) { mods.push("Win"); }

            let _ = app.emit(
                "hotkey-recorded",
                serde_json::json!({ "modifiers": mods, "keyId": mouse_button_to_key_id(button) }),
            );
            return;
        }
    }

    // ── Action-editor key-capture mode: capture mouse combo for Send Hotkey ──
    // Mirror of the trigger recorder branch above and of the keyboard
    // IS_CAPTURING_KEY branch in handle_keydown — keep all three in sync.
    // Restricted to L/R/M buttons because the executor (send_mouse_click)
    // only supports LButton/RButton/MButton output — Side1/Side2 would
    // capture cleanly but never fire on playback, which is a worse UX than
    // just letting the side click through here. Emits the pill-picker
    // naming convention ("LButton" etc, matching MOUSE_CLICK_OPTIONS) so a
    // captured "Shift+LButton" saves identically to a modifier+pill combo
    // and executes through the existing execute_send_hotkey mouse path.
    if IS_CAPTURING_KEY.load(Ordering::SeqCst) {
        let output_name = match button {
            MouseButton::Left => "LButton",
            MouseButton::Right => "RButton",
            MouseButton::Middle => "MButton",
            _ => "",
        };
        if !output_name.is_empty() {
            sync_modifier_state_from_os();
            // Modifier required for L/R; bare Middle allowed. Bare L/R would
            // eat clicks on Keyfire's own Cancel button during capture.
            let capturable = has_any_modifier() || matches!(button, MouseButton::Middle);
            if capturable {
                IS_CAPTURING_KEY.store(false, Ordering::SeqCst);

                let mut parts = Vec::new();
                if MOD_CTRL.load(Ordering::SeqCst) { parts.push("Ctrl".to_string()); }
                if MOD_SHIFT.load(Ordering::SeqCst) { parts.push("Shift".to_string()); }
                if MOD_ALT.load(Ordering::SeqCst) { parts.push("Alt".to_string()); }
                if MOD_META.load(Ordering::SeqCst) { parts.push("Win".to_string()); }
                parts.push(output_name.to_string());

                let combo = parts.join("+");
                let _ = app.emit("key-captured", Value::String(combo));
                return;
            }
        }
    }

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

    // Refocus fallback: if the linked app is NOT foreground but the cursor IS
    // over it, detect the profile so we can still fire the remap.
    // Pro gate: app-specific profile switching is Pro-only — Free users never
    // get the refocus switch even when cursor is over a linked app.
    // Recorder gate: suppressed entirely while a recording flow is active
    // (main hidden + countdown showing). A refocus-switch mid-flow fires
    // profile-switched → main clears selectedKey → ReplayRecordingValue
    // unmounts → cleanup discards the recording.
    let refocus_profile = if !cursor_over_app
        && !in_dialog
        && crate::licence::is_pro()
        && !crate::recorder::RECORDER_FLOW_ACTIVE.load(Ordering::SeqCst)
    {
        cursor_over_unfocused_linked_app()
    } else {
        None
    };

    if !has_any_modifier() {
        // Bare mouse — all buttons allowed in app-linked profiles

        // If we're in a refocus scenario, release any held keys from the
        // previous profile before switching (matches foreground watcher behavior).
        if refocus_profile.is_some() {
            crate::actions::release_held_key();
            crate::actions::stop_repeating_key();
        }

        let mut state = engine_state_lock();

        // If we're in a refocus scenario, switch to the linked profile now
        // so the assignment lookup uses the correct profile.
        if let Some(ref rp) = refocus_profile {
            if state.active_profile != *rp {
                state.active_profile = rp.clone();
                rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
                rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
                add_overlay_to_suppress(state.overlay_hotkey);
                add_pause_to_suppress(state.pause_hotkey);
                add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
                add_voice_to_suppress(state.voice_hotkey);
                add_radial_menu_to_suppress(state.radial_menu_hotkey);
                MOUSE_DOWN_SUPPRESSED.store(0, Ordering::SeqCst);
                info!("[Keyfire] Refocus-switched to profile \"{}\"", rp);
                let profile_name = rp.clone();
                let app2 = app.clone();
                // Notify frontend asynchronously (we hold state lock)
                std::thread::spawn(move || {
                    let _ = app2.emit("profile-switched", serde_json::json!({ "profile": profile_name }));
                });
            }
        }

        let profile = state.active_profile.clone();
        let linked = state
            .profile_settings
            .get(&profile)
            .and_then(|s| s.get("linkedApp"))
            .and_then(|v| v.as_str())
            .is_some();

        if linked && !in_dialog && (cursor_over_app || refocus_profile.is_some()) {
            let bare_key = format!("{}::BARE::{}", profile, mouse_id);
            // Hold trigger — a ::hold variant takes over the press cycle.
            match mouse_hold_check(&mut state, &bare_key, mouse_button_to_vk(button), true) {
                MouseHoldOutcome::Consumed => return,
                MouseHoldOutcome::FireDouble(dm) => {
                    drop(state);
                    fire_macro(dm, true, Some(bare_key), app);
                    return;
                }
                MouseHoldOutcome::NotArmed => {}
            }
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
    let mut state = engine_state_lock();
    let profile = state.active_profile.clone();
    let storage_key = format!("{}::{}::{}", profile, combo, mouse_id);

    // Hold trigger — a ::hold variant takes over the press cycle. The hook
    // already suppressed this click via suppress_mod_mouse.
    match mouse_hold_check(&mut state, &storage_key, mouse_button_to_vk(button), false) {
        MouseHoldOutcome::Consumed => return,
        MouseHoldOutcome::FireDouble(dm) => {
            drop(state);
            fire_macro(dm, false, Some(storage_key), app);
            return;
        }
        MouseHoldOutcome::NotArmed => {}
    }

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
        // Bare hold fallback (modifiers physically held pass through, same
        // full-button-replacement model as bare singles).
        match mouse_hold_check(&mut state, &bare_key, mouse_button_to_vk(button), true) {
            MouseHoldOutcome::Consumed => return,
            MouseHoldOutcome::FireDouble(dm) => {
                drop(state);
                fire_macro(dm, true, Some(bare_key), app);
                return;
            }
            MouseHoldOutcome::NotArmed => {}
        }
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
    // Release held key if this mouse button was the trigger (press-hold mirroring).
    // The pending-release fallback (for the fast-click race where mouse-up beats
    // the hold thread's setup) is only allowed for buttons that actually have a
    // hold-mode assignment — otherwise every ordinary click would record a
    // pending release, spamming the log and clobbering the slot a genuinely
    // hold-mapped button may be relying on.
    let mouse_id = mouse_button_to_key_id(button);

    // ── Hold trigger: button release ends the hold cycle ────────────────
    // Mirror of the keyboard block in handle_keyup. fired == true → the
    // watcher already fired the hold (or a double resolved this cycle);
    // suppress everything. fired == false → released before threshold;
    // dispatch what the button-down deferred. Mouse dispatch fires
    // immediately (no modifier-release deferral), and "passthrough" means
    // synthesizing the click the hook suppressed.
    {
        let removed = {
            let mut timers = hold_timers().lock().unwrap();
            timers.remove(&mouse_button_to_vk(button))
        };
        if let Some(entry) = removed {
            if !entry.fired {
                if let Some(single) = entry.single_macro {
                    if entry.has_double {
                        // Single + double + hold: the single waits out the
                        // double window on a cancel-able timer — a second tap
                        // cancels it via pending_single_cancel in
                        // mouse_hold_check.
                        let sk = entry.storage_key.clone();
                        let is_bare = entry.is_bare;
                        let mut state = engine_state_lock();
                        let dtw = state.double_tap_window_ms;
                        if let Some(old_cancel) = state.pending_single_cancel.remove(&sk) {
                            old_cancel.store(true, Ordering::SeqCst);
                        }
                        let cancel_flag = Arc::new(AtomicBool::new(false));
                        state.pending_single_cancel.insert(sk.clone(), cancel_flag.clone());
                        drop(state);
                        let app_clone = app.clone();
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(dtw));
                            if cancel_flag.load(Ordering::SeqCst) {
                                return; // second tap arrived — double fired instead
                            }
                            {
                                let mut state = engine_state_lock();
                                state.pending_single_cancel.remove(&sk);
                                state.last_hotkey_time.remove(&sk);
                            }
                            info!("[Keyfire] x1 Mouse single confirmed (hold-deferred): {}", sk);
                            fire_macro(single, is_bare, Some(sk), &app_clone);
                        });
                    } else {
                        // Single + hold only: fire the deferred single now.
                        info!("[Keyfire] x1 Mouse single (hold-deferred): {}", entry.storage_key);
                        fire_macro(single, entry.is_bare, Some(entry.storage_key.clone()), app);
                    }
                } else if entry.has_double {
                    // Hold + double, NO single — defer the passthrough click
                    // through the dtw window. A second tap cancels it (double
                    // fires from mouse_hold_check); otherwise synthesize the
                    // click the hook suppressed.
                    let sk = entry.storage_key.clone();
                    let btn_name = mouse_button_to_replay_name(button);
                    let mut state = engine_state_lock();
                    let dtw = state.double_tap_window_ms;
                    if let Some(old_cancel) = state.pending_single_cancel.remove(&sk) {
                        old_cancel.store(true, Ordering::SeqCst);
                    }
                    let cancel_flag = Arc::new(AtomicBool::new(false));
                    state.pending_single_cancel.insert(sk.clone(), cancel_flag.clone());
                    drop(state);
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(dtw));
                        if cancel_flag.load(Ordering::SeqCst) {
                            return; // second tap arrived → double fired instead
                        }
                        {
                            let mut state = engine_state_lock();
                            state.pending_single_cancel.remove(&sk);
                            state.last_hotkey_time.remove(&sk);
                        }
                        info!("[Keyfire] [HOLD] mouse click passthrough (hold+double, no single): {}", sk);
                        crate::actions::send_passthrough_click(btn_name);
                    });
                } else {
                    // Hold-only — immediate passthrough so the app still gets
                    // its native click (the hook suppressed the physical down).
                    info!("[Keyfire] [HOLD] mouse click passthrough (hold-only): {}", entry.storage_key);
                    crate::actions::send_passthrough_click(mouse_button_to_replay_name(button));
                }
            }
        }
    }

    // Release held key if this mouse button was the trigger (press-hold
    // mirroring — the holdMode ACTION concept, independent of ::hold triggers).
    let allow_pending = button_has_hold_assignment(mouse_id);
    if let Some(label) = crate::actions::release_held_if_mouse_trigger(mouse_id, allow_pending) {
        crate::tray::update_tray_icon_normal(app);
        info!("[Keyfire] Mouse-up released hold: {}", label);
    }
}

/// True if any assignment (any profile, any modifier combo, incl. ::double)
/// is triggered by this mouse button with holdMode enabled. Cheap map scan on
/// the processor thread — NOT called from the hook callbacks.
fn button_has_hold_assignment(mouse_id: &str) -> bool {
    let single_suffix = format!("::{}", mouse_id);
    let double_suffix = format!("::{}::double", mouse_id);
    let state = engine_state_lock();
    state.assignments.iter().any(|(k, v)| {
        (k.ends_with(&single_suffix) || k.ends_with(&double_suffix))
            && v.get("data")
                .and_then(|d| d.get("holdMode"))
                .and_then(|h| h.as_bool())
                .unwrap_or(false)
    })
}

fn handle_mouse_wheel(delta: i16, app: &AppHandle) {
    sync_modifier_state_from_os(); // see handle_mouse_down
    // ── Recording mode: capture scroll trigger and send to frontend ─────
    // Mirror of the mouse-button capture branch in handle_mouse_down and of
    // the wheel suppression condition in mouse_hook_proc — keep in sync.
    // Modifier REQUIRED: a bare scroll would capture the instant the user
    // scrolls anything (including our own UI) while recording. Bare scroll
    // stays assignable via the mouse canvas only. Same OS-state resync as
    // the button branch — the MOD_* atomics are stale while our UI has focus.
    if IS_RECORDING_HOTKEY.load(Ordering::SeqCst) {
        sync_modifier_state_from_os();
        if has_any_modifier() {
            IS_RECORDING_HOTKEY.store(false, Ordering::SeqCst);

            let mut mods = Vec::new();
            if MOD_CTRL.load(Ordering::SeqCst) { mods.push("Ctrl"); }
            if MOD_SHIFT.load(Ordering::SeqCst) { mods.push("Shift"); }
            if MOD_ALT.load(Ordering::SeqCst) { mods.push("Alt"); }
            if MOD_META.load(Ordering::SeqCst) { mods.push("Win"); }

            let key_id = if delta > 0 { "MOUSE_SCROLL_UP" } else { "MOUSE_SCROLL_DOWN" };
            let _ = app.emit(
                "hotkey-recorded",
                serde_json::json!({ "modifiers": mods, "keyId": key_id }),
            );
            return;
        }
    }

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
        let state = engine_state_lock();
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
    let state = engine_state_lock();
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
    // Pro gate: Free users never fire double-only assignments. Config is preserved
    // so the action returns when the user upgrades.
    if !crate::licence::is_pro() {
        return;
    }
    let mut state = engine_state_lock();
    let now = Instant::now();
    let dtw = state.double_tap_window_ms;

    if let Some(last) = state.last_hotkey_time.get(storage_key) {
        if now.duration_since(*last).as_millis() < dtw as u128 {
            state.last_hotkey_time.remove(storage_key);
            info!("[Keyfire] x2 Double-only: {}", storage_key);
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
    let mut state = engine_state_lock();
    let double_key = format!("{}::double", storage_key);
    // Pro gate: Free users get single-press only. Double-tap assignments from
    // a lapsed trial stay in config (data preserved) but never fire until upgrade.
    let double_macro = if crate::licence::is_pro() {
        state.assignments.get(&double_key).cloned()
    } else {
        None
    };

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
            info!("[Keyfire] x2 Double-tap: {}", storage_key);
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

    info!("[Keyfire] x1 First tap: {} — waiting {}ms", storage_key, dtw);

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
            let mut state = engine_state_lock();
            state.pending_single_cancel.remove(&sk);
            state.last_hotkey_time.remove(&sk);
        }
        info!("[Keyfire] x1 Single confirmed: {}", sk);
        fire_macro(macro_clone, false, Some(sk), &app_clone);
    });
}

// ── Fire macro — execute action + notify frontend ───────────────────────────

fn fire_macro(macro_val: Value, is_bare: bool, trigger_key: Option<String>, app: &AppHandle) {
    fire_macro_impl(macro_val, is_bare, trigger_key, app, false)
}

/// Fire-on-press variant: dispatches at keydown while the trigger key (and any
/// modifiers) are still physically held. Skips the AltGr dead-character erase:
/// the trigger keydown was hook-suppressed so nothing leaked, but the live
/// Ctrl+Alt state would read as AltGr and the erase would eat a real character
/// from the target app. Injection paths handle the still-held modifiers via
/// release_held_modifiers (physical state read through GetAsyncKeyState).
fn fire_macro_on_press(macro_val: Value, trigger_key: Option<String>, app: &AppHandle) {
    fire_macro_impl(macro_val, false, trigger_key, app, true)
}

fn fire_macro_impl(macro_val: Value, is_bare: bool, trigger_key: Option<String>, app: &AppHandle, skip_altgr_erase: bool) {
    // Any assignment firing breaks the typed-word context — the action may
    // inject text, paste, or switch windows, so whatever half-word sits in
    // the expansion buffer no longer reflects what's left of the caret.
    // (Previously covered incidentally by the buffer clear on every modifier
    // press; Shift no longer clears — see the Shift exemption in
    // handle_keydown — so the fire path clears explicitly.)
    crate::expansions::buffer_clear();

    // Re-press cancel — if a loop is already running for this trigger, the user
    // pressing it again is the canonical stop gesture. Set the cancel flag and
    // bail before any thread spawn / clipboard work happens. The running loop
    // observes the flag at its next per-iter or inter-step check and exits.
    if let Some(ref key) = trigger_key {
        if crate::actions::cancel_loop_if_running(key) {
            log::info!("[Keyfire] Loop cancel signal: {}", key);
            return;
        }
    }

    // H1 re-entrancy guard — same trigger mid-flight. Previously dropped
    // the new fire outright (H1 re-entrancy guard) to prevent the
    // BricsCAD-style race across clipboard snapshot/restore + SUPPRESS_SIMULATED.
    // v0.8.6: on re-press we now signal cancel (esc_stamp, same signal as a
    // real Esc), wait for the running thread to release its guard, and
    // acquire fresh. Enables re-pressing the trigger to abort a stuck Wait
    // for Text / Wait for Pixel and try again — the specific ask from
    // OCR-diagnosis testing. The 250ms wait ceiling is a safety net; if the
    // old thread is stuck harder than the cancel signal can reach, we drop
    // the fire rather than race it. Caveat: the cancel signal is global, so
    // concurrent macros that happen to be inside a Wait step at this instant
    // will also abort — accepted trade-off; restarting a single macro is the
    // far more common intent. The new run starts after the stamp, so it is
    // unaffected by it (no clear needed).
    let macro_guard = if let Some(ref key) = trigger_key {
        match crate::actions::MacroRunningGuard::try_acquire(key) {
            Some(g) => Some(g),
            None => {
                log::info!(
                    "[Keyfire] Re-fire on active trigger {}: cancelling current run + restarting",
                    key
                );
                crate::actions::esc_stamp();
                let mut acquired = None;
                for _ in 0..25 {
                    thread::sleep(std::time::Duration::from_millis(10));
                    if let Some(g) = crate::actions::MacroRunningGuard::try_acquire(key) {
                        acquired = Some(g);
                        break;
                    }
                }
                match acquired {
                    Some(g) => Some(g),
                    None => {
                        log::warn!(
                            "[Keyfire] Re-fire cancel timed out (>250ms) — dropped: {}",
                            key
                        );
                        return;
                    }
                }
            }
        }
    } else {
        None
    };

    // Capture the target window HWND NOW, before any async delay.
    let target_hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };

    // Detect AltGr (Ctrl+Alt held simultaneously) — snapshot now, modifiers
    // will be cleared by the time execute_action runs. Fire-on-press dispatch
    // skips this: Ctrl+Alt is legitimately still held at keydown-fire time.
    // The erase exists for a Ctrl+Alt (AltGr) keydown that LEAKED a dead
    // character into the app. A bound combo is in the suppress set, so its
    // keydown never reached the app and there is nothing to erase — yet the
    // Backspace was still sent, deleting one real character from the user's
    // document on every Ctrl+Alt hold / double-tap / mouse trigger. Mouse
    // triggers can't type a character at all.
    let trigger_reached_app = trigger_key.as_deref().map(|sk| {
        let parts: Vec<&str> = sk.split("::").collect();
        if parts.len() < 3 { return true; }
        let key_id = parts[2];
        if key_id.starts_with("MOUSE_") { return false; }
        match key_id_to_vk(key_id) {
            Some(vk) => {
                let bits = modifier_bits();
                !suppress_keys().try_read().map(|set| set.contains(&(bits, vk))).unwrap_or(false)
            }
            None => true,
        }
    }).unwrap_or(true);
    let is_altgr = !skip_altgr_erase
        && trigger_reached_app
        && MOD_CTRL.load(Ordering::SeqCst) && MOD_ALT.load(Ordering::SeqCst);
    // F13-F24 never produce a character (no physical keyboard has them; a
    // Stream Deck or macropad press arrives as a bare VK 0x7C-0x87), so the
    // bare-key leaked-character erase has nothing to undo. Skip it, or every
    // deck press would delete one real character in the target app.
    let is_bare = is_bare && !trigger_key.as_deref()
        .and_then(|sk| sk.split("::").nth(2))
        .map(is_extra_f_key)
        .unwrap_or(false);
    if is_altgr {
        log::info!("[FIRE] AltGr combo detected — will erase dead character");
    }
    log::info!("[FIRE] Captured target HWND: 0x{:X}", target_hwnd);

    // Execute the action on a separate thread to avoid blocking the event processor
    let macro_clone = macro_val.clone();
    let app_clone = app.clone();
    thread::spawn(move || {
        // Guard moved into the thread — drops on exit, releasing the storage_key
        // from ACTIVE_MACRO_KEYS so future fires can proceed.
        let _macro_guard = macro_guard;

        crate::actions::LAST_ACTION_FAILED.store(false, Ordering::SeqCst);
        crate::actions::execute_action(&macro_clone, is_bare, target_hwnd, is_altgr, trigger_key.as_deref(), &app_clone);
        let action_failed = crate::actions::LAST_ACTION_FAILED.swap(false, Ordering::SeqCst);

        // Log analytics — log_assignment_fired computes the time-saved credit
        // from the assignment's own data (steps, repeats, recording durations).
        let label = macro_clone.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let trigger = trigger_key.as_deref().unwrap_or("");
        crate::analytics::log_assignment_fired(trigger, label, &macro_clone);

        // Notify frontend for visual feedback
        let _ = app_clone.emit(
            "macro-fired",
            serde_json::json!({
                "label": macro_clone.get("label").and_then(|v| v.as_str()).unwrap_or(""),
                "type": macro_clone.get("type").and_then(|v| v.as_str()).unwrap_or(""),
                "ok": !action_failed,
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
        "IntlBackslash" => "\\",
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
    let handle = thread::Builder::new()
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
                    error!("[Keyfire] Failed to install keyboard hook — GetLastError={}", err);
                    HOOKS_RUNNING.store(false, Ordering::SeqCst);
                    return;
                }
                info!("[Keyfire] LL hook registered: HHOOK=0x{:X}", kb as isize);
                KB_HOOK.store(kb as isize, Ordering::SeqCst);

                let ms = SetWindowsHookExW(
                    WH_MOUSE_LL,
                    Some(mouse_hook_proc),
                    std::ptr::null_mut(),
                    0,
                );
                if ms.is_null() {
                    let err = windows_sys::Win32::Foundation::GetLastError();
                    error!("[Keyfire] Failed to install mouse hook — GetLastError={}", err);
                    UnhookWindowsHookEx(kb);
                    KB_HOOK.store(0, Ordering::SeqCst);
                    HOOKS_RUNNING.store(false, Ordering::SeqCst);
                    return;
                }
                info!("[Keyfire] LL mouse hook registered: HHOOK=0x{:X}", ms as isize);
                MOUSE_HOOK.store(ms as isize, Ordering::SeqCst);
                HOOKS_RUNNING.store(true, Ordering::SeqCst);
                HOOK_HEARTBEAT.store(0, Ordering::SeqCst);

                // Reset shared atomics to safe defaults on reinstall — stale values
                // from a prior hook session can corrupt the new hook's behaviour.
                INJECTION_IN_PROGRESS.store(false, Ordering::SeqCst);
                SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                // Armed hold timers belong to the prior hook session — their
                // keyups may have been lost to the reinstall.
                clear_hold_timers();
                // Same reasoning for auto-repeat tracking: held-key state
                // is invalid across a hook reinstall (lost keyups).
                clear_held_keys();
                FILL_IN_ACTIVE.store(false, Ordering::SeqCst);
                FILLIN_HWND.store(0, Ordering::SeqCst);
                MOD_CTRL.store(false, Ordering::SeqCst);
                MOD_ALT.store(false, Ordering::SeqCst);
                MOD_SHIFT.store(false, Ordering::SeqCst);
                MOD_META.store(false, Ordering::SeqCst);
                MOUSE_DOWN_SUPPRESSED.store(0, Ordering::SeqCst);
                RADIAL_MENU_OPEN.store(false, Ordering::SeqCst);
                RADIAL_ACTION_VK.store(0, Ordering::SeqCst);
                info!("[Keyfire] Hook reinstall: shared atomics reset to safe defaults");

                log::info!("[HOOK] Input hooks installed (dedicated thread, high priority)");

                // PeekMessageW polling loop — actively pumps LL hook messages.
                // Unlike GetMessageW which blocks, this polls with a 1ms yield
                // to guarantee the thread is always responsive to hook dispatches.
                //
                // Custom messages handled here:
                //  - WM_KEYFIRE_MOUSE_HOOK_PAUSE: uninstall ONLY the mouse hook
                //  - WM_KEYFIRE_MOUSE_HOOK_RESUME: reinstall the mouse hook
                // Both posted from foreground.rs on fullscreen-state transition.
                // Win32 thread affinity is respected — install/uninstall here on
                // the same thread that originally installed the hooks.
                let mut msg: MSG = std::mem::zeroed();
                'pump: loop {
                    while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                        if msg.message == WM_QUIT {
                            break 'pump;
                        }
                        if msg.message == WM_KEYFIRE_MOUSE_HOOK_PAUSE {
                            let h = MOUSE_HOOK.load(Ordering::SeqCst);
                            if h != 0 {
                                UnhookWindowsHookEx(h as _);
                                MOUSE_HOOK.store(0, Ordering::SeqCst);
                                log::info!("[HOOK] Mouse hook paused (fullscreen detected)");
                            }
                            continue;
                        }
                        if msg.message == WM_KEYFIRE_MOUSE_HOOK_RESUME {
                            if MOUSE_HOOK.load(Ordering::SeqCst) == 0 {
                                let ms = SetWindowsHookExW(
                                    WH_MOUSE_LL,
                                    Some(mouse_hook_proc),
                                    std::ptr::null_mut(),
                                    0,
                                );
                                if !ms.is_null() {
                                    MOUSE_HOOK.store(ms as isize, Ordering::SeqCst);
                                    log::info!("[HOOK] Mouse hook resumed (foreground left fullscreen)");
                                } else {
                                    let err = windows_sys::Win32::Foundation::GetLastError();
                                    log::warn!("[HOOK] Mouse hook resume failed — GetLastError={}", err);
                                }
                            }
                            continue;
                        }
                    }
                    // Block until the next queued OR sent message. LL hook
                    // callbacks are delivered to this thread only while it is
                    // inside a message-retrieval call; the old Sleep(1) between
                    // PeekMessage drains rounded up to the 15.6 ms scheduler
                    // tick, adding up to ~15 ms of jitter to EVERY keystroke and
                    // mouse event on the machine while Keyfire ran. MsgWait
                    // returns the instant anything arrives (QS_ALLINPUT covers
                    // sent messages, so hook callbacks run immediately).
                    windows_sys::Win32::UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx(
                        0,
                        std::ptr::null(),
                        windows_sys::Win32::System::Threading::INFINITE,
                        windows_sys::Win32::UI::WindowsAndMessaging::QS_ALLINPUT,
                        windows_sys::Win32::UI::WindowsAndMessaging::MWMO_INPUTAVAILABLE,
                    );
                }

                // Cleanup. Gate on HOOK_THREAD_ID match so a stale exit path
                // can't clobber a newer thread's atomics — the reinstall
                // sequence joins the old thread before spawning the new, so
                // in practice the ids always match here, but the check is
                // cheap defence-in-depth against future refactors.
                let my_tid = windows_sys::Win32::System::Threading::GetCurrentThreadId();
                if HOOK_THREAD_ID.load(Ordering::SeqCst) == my_tid as isize {
                    UnhookWindowsHookEx(KB_HOOK.load(Ordering::SeqCst) as _);
                    UnhookWindowsHookEx(MOUSE_HOOK.load(Ordering::SeqCst) as _);
                    KB_HOOK.store(0, Ordering::SeqCst);
                    MOUSE_HOOK.store(0, Ordering::SeqCst);
                    HOOKS_RUNNING.store(false, Ordering::SeqCst);
                    HOOK_THREAD_ID.store(0, Ordering::SeqCst);
                }
            }
        })
        .expect("Failed to spawn hook thread");
    // Stash the handle so the reinstall path can join it deterministically.
    // Any prior handle should already have been taken + joined by the
    // reinstall code before this call — replacing a Some() here would be a
    // caller-side bug, but we still log rather than silently drop it.
    let mut slot = hook_thread_handle().lock().unwrap();
    if slot.is_some() {
        log::warn!("[HOOK] spawn_hook_thread: previous JoinHandle was not consumed — reinstall path likely skipped join");
    }
    *slot = Some(handle);
}

/// Tick-count timestamp of the last input event seen by the SYSTEM (any
/// process, injected included), via GetLastInputInfo. Used by the hook
/// health monitor to distinguish "user is AFK" (no input anywhere, heartbeat
/// legitimately silent) from "Windows silently removed our hook" (system saw
/// input our procs never reported). None if the API fails.
fn last_system_input_tick() -> Option<u32> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    unsafe {
        let mut lii = LASTINPUTINFO {
            cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        if GetLastInputInfo(&mut lii) != 0 {
            Some(lii.dwTime)
        } else {
            None
        }
    }
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

    // Hold trigger watcher (v0.5) — one thread for all hold timers.
    spawn_hold_watcher(app.clone());

    process_events(receiver, app.clone());

    // Health monitor — reinstalls hooks if heartbeat stalls for 30s
    thread::Builder::new()
        .name("trigr-hook-monitor".to_string())
        .spawn(move || {
            let mut last_heartbeat = HOOK_HEARTBEAT.load(Ordering::SeqCst);
            let mut last_input_tick = last_system_input_tick();
            let mut install_failure_notified = false;
            thread::sleep(Duration::from_secs(5));
            loop {
                thread::sleep(Duration::from_secs(15));
                // A failed SetWindowsHookExW at startup (AV/EDR, another hook
                // consumer) used to leave HOOKS_RUNNING=false forever: the
                // stale-heartbeat branch below only runs while hooks are up,
                // so nothing retried and nothing told the user. Retry each
                // tick and say so once.
                if !HOOKS_RUNNING.load(Ordering::SeqCst) && !MOUSE_HOOK_PAUSED.load(Ordering::SeqCst) {
                    log::warn!("[HOOK] Hooks not running — attempting (re)install");
                    spawn_hook_thread();
                    thread::sleep(Duration::from_millis(750));
                    let up = HOOKS_RUNNING.load(Ordering::SeqCst);
                    if up {
                        info!("[HOOK] Hooks installed on retry");
                        if install_failure_notified {
                            crate::emit_user_toast(&app, "success", "Keyfire's keyboard hooks are working again.");
                            install_failure_notified = false;
                        }
                        emit_engine_status(&app);
                    } else if !install_failure_notified {
                        crate::emit_user_toast(&app, "error", "Keyfire couldn't hook the keyboard, so hotkeys and expansions aren't running. It will keep retrying; if this persists, check antivirus settings or restart Keyfire.");
                        install_failure_notified = true;
                        emit_engine_status(&app);
                    }
                    last_heartbeat = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                    last_input_tick = last_system_input_tick();
                    continue;
                }
                // Skip the stale-check entirely while the mouse hook is intentionally
                // paused (foreground watcher detected a fullscreen game). Tearing
                // down the thread here would re-install the mouse hook and re-break
                // the game until the next foreground poll (~1.5s later). Re-baseline
                // the heartbeat counter so we don't immediately trip on resume.
                if MOUSE_HOOK_PAUSED.load(Ordering::SeqCst) {
                    last_heartbeat = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                    last_input_tick = last_system_input_tick();
                    continue;
                }
                let current = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                if current == last_heartbeat && HOOKS_RUNNING.load(Ordering::SeqCst) {
                    // Stale — wait another 15s to confirm (30s total)
                    thread::sleep(Duration::from_secs(15));
                    // Re-check pause state — user may have entered fullscreen during the wait.
                    if MOUSE_HOOK_PAUSED.load(Ordering::SeqCst) {
                        last_heartbeat = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                        last_input_tick = last_system_input_tick();
                        continue;
                    }
                    let recheck = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                    if recheck == last_heartbeat {
                        // The heartbeat only ticks on real input events, so a
                        // flat heartbeat can simply mean the user is AFK. Only
                        // reinstall if the SYSTEM saw input during the stale
                        // window that our hook procs never reported — that's
                        // the silently-removed-hook signature. If the system
                        // input tick hasn't moved either, the user is idle and
                        // the hooks are (presumably) fine. API failure falls
                        // through to reinstall, matching the old behaviour.
                        let input_recheck = last_system_input_tick();
                        let system_input_moved = match (last_input_tick, input_recheck) {
                            (Some(before), Some(after)) => after != before,
                            _ => true,
                        };
                        if !system_input_moved {
                            log::debug!("[HOOK] Heartbeat quiet but system idle too — AFK, no reinstall");
                            last_heartbeat = recheck;
                            last_input_tick = input_recheck;
                            continue;
                        }
                        log::warn!("[HOOK] Heartbeat stale for 30s with system input present — reinstalling hooks");
                        let tid = HOOK_THREAD_ID.load(Ordering::SeqCst);
                        if tid != 0 {
                            unsafe { PostThreadMessageW(tid as u32, WM_QUIT, 0, 0); }
                        }
                        // Join the old thread before spawning the new one.
                        // Skipping the join lets the old thread's cleanup
                        // block clobber the new thread's KB_HOOK/MOUSE_HOOK
                        // handles + HOOKS_RUNNING flag, and briefly leaves
                        // two LL hooks co-installed for duplicate-fire.
                        let old_handle = hook_thread_handle().lock().unwrap().take();
                        if let Some(h) = old_handle {
                            // Bound the wait: if the old thread is wedged
                            // (already stuck under load), don't stall the
                            // watchdog forever — fall back to the raw sleep
                            // and accept the race. In normal case join
                            // returns within a couple ms of WM_QUIT.
                            let joined = std::sync::mpsc::channel::<()>();
                            let (tx, rx) = joined;
                            thread::spawn(move || {
                                let _ = h.join();
                                let _ = tx.send(());
                            });
                            match rx.recv_timeout(Duration::from_millis(2000)) {
                                Ok(_) => log::info!("[HOOK] Old hook thread joined cleanly"),
                                Err(_) => {
                                    log::warn!("[HOOK] Old hook thread join timed out after 2s — unhooking its handles directly before reinstall");
                                    // The old thread's own cleanup is gated on
                                    // HOOK_THREAD_ID == its tid, which the new
                                    // thread is about to overwrite, so it would
                                    // never unhook. Do it here by handle.
                                    unsafe {
                                        let kb = KB_HOOK.swap(0, Ordering::SeqCst);
                                        if kb != 0 { UnhookWindowsHookEx(kb as _); }
                                        let ms = MOUSE_HOOK.swap(0, Ordering::SeqCst);
                                        if ms != 0 { UnhookWindowsHookEx(ms as _); }
                                    }
                                    HOOKS_RUNNING.store(false, Ordering::SeqCst);
                                }
                            }
                        } else {
                            thread::sleep(Duration::from_millis(500));
                        }
                        spawn_hook_thread();
                        // Rebuild suppress set so the new hook has correct state
                        {
                            let state = engine_state_lock();
                            rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
                            rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
                            add_overlay_to_suppress(state.overlay_hotkey);
                            add_pause_to_suppress(state.pause_hotkey);
                            add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
                            add_voice_to_suppress(state.voice_hotkey);
                            add_radial_menu_to_suppress(state.radial_menu_hotkey);
                        }
                        log::info!("[HOOK] Hooks reinstalled, suppress set rebuilt");
                        thread::sleep(Duration::from_secs(5));
                        last_heartbeat = HOOK_HEARTBEAT.load(Ordering::SeqCst);
                        last_input_tick = last_system_input_tick();
                        continue;
                    }
                }
                last_heartbeat = current;
                last_input_tick = last_system_input_tick();

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
                            error!("[Keyfire] INJECTION_IN_PROGRESS stuck for >5s — force-clearing to unfreeze keyboard");
                            INJECTION_IN_PROGRESS.store(false, Ordering::SeqCst);
                            INJECTION_STARTED_MS.store(0, Ordering::SeqCst);
                            SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                            // Keys typed during the stuck window were buffered
                            // for replay; replaying them into whatever is
                            // focused at the NEXT expansion fire (seconds or
                            // minutes later) typed them out of context. Drop.
                            if let Ok(mut buf) = injection_buffer().lock() {
                                if !buf.is_empty() {
                                    log::warn!("[Keyfire] Discarding {} keystrokes buffered during the stuck injection", buf.len());
                                    buf.clear();
                                }
                            }
                        }
                    }
                }
            }
        })
        .expect("Failed to spawn hook monitor thread");
}

/// Read the hook thread's Windows thread id. Returns 0 if hooks aren't running
/// or are mid-teardown. Used by the foreground watcher to post the
/// fullscreen-pause / resume custom messages via PostThreadMessageW.
pub fn hook_thread_id() -> isize {
    HOOK_THREAD_ID.load(Ordering::SeqCst)
}

// ── JS keydown forwarder (WebView2 capture path) ────────────────────────────

/// Handle a key event forwarded from the JS keydown listener in the webview.
/// This provides an alternative capture path when the LL hook can't see
/// keypresses directed at WebView2. Emits the same events as handle_keydown.
pub fn handle_js_key_event(code: &str, ctrl: bool, shift: bool, alt: bool, meta: bool, app: &AppHandle) {
    let key_id = code;

    // Check overlay hotkey (JS path — Keyfire has focus)
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
                        OVERLAY_JUST_OPENED.store(true, Ordering::SeqCst);
                        let _ = app.emit("toggle-overlay", Value::Null);
                        return;
                    }
                }
                // Radial menu hotkey (JS path)
                if let Some((mod_bits, vk)) = state.radial_menu_hotkey {
                    if js_bits == mod_bits && key_id_to_vk(key_id).or_else(|| parse_hotkey_combo(key_id).map(|(_, v)| v)) == Some(vk) {
                        drop(state);
                        MOD_CTRL.store(false, Ordering::SeqCst);
                        MOD_SHIFT.store(false, Ordering::SeqCst);
                        MOD_ALT.store(false, Ordering::SeqCst);
                        MOD_META.store(false, Ordering::SeqCst);
                        SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        crate::actions::release_held_modifiers();
                        SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                        let _ = app.emit("toggle-radial-menu", Value::Null);
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
        // Empty code = sole-modifier capture from the JS keyup listener.
        // Emit just the modifier name (e.g. "Ctrl") with no trailing key.
        if !key_id.is_empty() {
            parts.push(key_id_to_display(key_id).to_string());
        }

        let combo = parts.join("+");
        let _ = app.emit("key-captured", Value::String(combo));
    }
}

// ── Public API for Tauri commands ───────────────────────────────────────────

pub fn set_macros_enabled(enabled: bool) {
    MACROS_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn set_recording(recording: bool) {
    IS_RECORDING_HOTKEY.store(recording, Ordering::SeqCst);
}

pub fn set_capturing(capturing: bool) {
    IS_CAPTURING_KEY.store(capturing, Ordering::SeqCst);
    if capturing {
        let mut state = engine_state_lock();
        state.capture_sole_modifier = None;
    }
}

pub fn set_pixel_pick_active(active: bool) {
    PIXEL_PICK_ACTIVE.store(active, Ordering::SeqCst);
}

pub fn set_input_focused(focused: bool) {
    APP_INPUT_FOCUSED.store(focused, Ordering::SeqCst);
}

pub fn update_assignments(assignments: HashMap<String, Value>, profile: String) {
    log::info!("[Keyfire] update_assignments: {} entries for profile '{}'", assignments.len(), profile);
    // Armed hold timers reference clones of old macros — drop them.
    clear_hold_timers();
    // Auto-repeat tracking is keyed by raw VK and survives assignment
    // changes harmlessly, but clearing here keeps state minimal.
    clear_held_keys();
    let mut state = engine_state_lock();
    state.assignments = assignments;
    state.active_profile = profile;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    add_voice_to_suppress(state.voice_hotkey);
    add_radial_menu_to_suppress(state.radial_menu_hotkey);
    log::info!("[Keyfire] Assignments stored: {} entries", state.assignments.len());
}

pub fn set_active_profile(profile: String) {
    let mut state = engine_state_lock();
    state.active_profile = profile.clone();
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    add_voice_to_suppress(state.voice_hotkey);
    add_radial_menu_to_suppress(state.radial_menu_hotkey);
    // Clear down-suppressed flags so stale button-ups aren't eaten after switch
    MOUSE_DOWN_SUPPRESSED.store(0, Ordering::SeqCst);
    info!("[Keyfire] Active profile: {}", profile);
}

pub fn get_active_profile() -> String {
    engine_state_lock().active_profile.clone()
}

/// Clear the overlay-opened flag (called when overlay is hidden/toggled off).
pub fn clear_overlay_opened_flag() {
    OVERLAY_JUST_OPENED.store(false, Ordering::SeqCst);
}

/// Clear voice-active state (called when overlay is hidden).
/// Preserves VOICE_KEY_HELD and VOICE_ACTION_VK so the physical keyup
/// can still clear the held flag — prevents keyboard repeat from
/// reopening the overlay after a match fires and the overlay closes.
pub fn clear_voice_active() {
    VOICE_ACTIVE.store(false, Ordering::SeqCst);
}

/// Check whether voice mode is currently active (used to suppress blur auto-hide).
pub fn is_voice_active() -> bool {
    VOICE_ACTIVE.load(Ordering::SeqCst)
}

pub fn update_profile_settings(settings: HashMap<String, Value>) {
    let mut state = engine_state_lock();
    state.profile_settings = settings;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    add_voice_to_suppress(state.voice_hotkey);
    add_radial_menu_to_suppress(state.radial_menu_hotkey);
}

pub fn update_global_settings(settings: &Value) {
    let mut state = engine_state_lock();
    if let Some(dtw) = settings.get("doubleTapWindow").and_then(|v| v.as_u64()) {
        state.double_tap_window_ms = dtw;
    }
    if let Some(ht) = settings.get("holdThresholdMs").and_then(|v| v.as_u64()) {
        state.hold_threshold_ms = ht.clamp(200, 700);
    }
    if let Some(m) = settings.get("globalInputMethod").and_then(|v| v.as_str()) {
        state.global_input_method = m.to_string();
    }
    if let Some(s) = settings.get("macroSpeed").and_then(|v| v.as_str()) {
        state.macro_speed = s.to_string();
    }
    if let Some(v) = settings.get("fireOnPress").and_then(|v| v.as_bool()) {
        state.fire_on_press = v;
    }
    if let Some(v) = settings.get("keystrokeDelay").and_then(|v| v.as_u64()) {
        state.custom_keystroke_delay = v;
    }
    if let Some(v) = settings.get("macroTriggerDelay").and_then(|v| v.as_u64()) {
        state.custom_pre_execution_delay = v;
    }
    if let Some(s) = settings.get("defaultDateFormat").and_then(|v| v.as_str()) {
        // Whitelist accepted values; unknown strings fall through to existing default.
        if matches!(s, "DD/MM/YYYY" | "MM/DD/YYYY" | "YYYY-MM-DD") {
            state.default_date_format = s.to_string();
        }
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
        let mut state = engine_state_lock();
        state.overlay_hotkey = Some(parsed);
        rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
        add_overlay_to_suppress(Some(parsed));
        add_pause_to_suppress(state.pause_hotkey);
        add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
        add_voice_to_suppress(state.voice_hotkey);
        add_radial_menu_to_suppress(state.radial_menu_hotkey);
        log::info!("[HOOK] Overlay hotkey set: {} → bits={} vk=0x{:02X}", combo, parsed.0, parsed.1);
    }
}

/// Quick Search disabled (Settings toggle) or hotkey cleared. With
/// `overlay_hotkey = None` the LL hook never matches the combo, so it passes
/// through to the focused app untouched, and macro Press Key dispatch can't
/// open the overlay either. Mirrors clear_pause_hotkey / clear_voice_hotkey.
pub fn clear_overlay_hotkey() {
    let mut state = engine_state_lock();
    state.overlay_hotkey = None;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    add_voice_to_suppress(state.voice_hotkey);
    add_radial_menu_to_suppress(state.radial_menu_hotkey);
    log::info!("[HOOK] Overlay hotkey cleared (Quick Search disabled)");
}

pub fn set_pause_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        let mut state = engine_state_lock();
        state.pause_hotkey = Some(parsed);
        state.pause_hotkey_str = Some(combo.to_string());
        rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
        add_overlay_to_suppress(state.overlay_hotkey);
        add_pause_to_suppress(Some(parsed));
        add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
        add_voice_to_suppress(state.voice_hotkey);
        add_radial_menu_to_suppress(state.radial_menu_hotkey);
        log::info!("[HOOK] Pause hotkey set: {} → bits={} vk=0x{:02X}", combo, parsed.0, parsed.1);
    }
}

/// Quick Record (temp macro) record-toggle hotkey. Mirrors set_pause_hotkey:
/// parses the combo, updates engine state, and refreshes the hook-readable
/// atomics in recorder.rs so the LL hook can match without a lock. Pass an
/// empty combo to clear (record disabled).
pub fn set_temp_macro_record_hotkey(combo: &str) {
    let parsed = parse_hotkey_combo(combo);
    let mut state = engine_state_lock();
    match parsed {
        Some((bits, vk)) => {
            state.temp_macro_record_hotkey = Some((bits, vk));
            state.temp_macro_record_hotkey_str = Some(combo.to_string());
            crate::recorder::TEMP_MACRO_RECORD_BITS.store(bits, Ordering::SeqCst);
            crate::recorder::TEMP_MACRO_RECORD_VK.store(vk, Ordering::SeqCst);
            log::info!("[HOOK] Temp macro record hotkey set: {} → bits={} vk=0x{:02X}", combo, bits, vk);
        }
        None => {
            state.temp_macro_record_hotkey = None;
            state.temp_macro_record_hotkey_str = None;
            crate::recorder::TEMP_MACRO_RECORD_VK.store(0, Ordering::SeqCst);
            log::info!("[HOOK] Temp macro record hotkey cleared");
        }
    }
}

pub fn set_temp_macro_play_hotkey(combo: &str) {
    let parsed = parse_hotkey_combo(combo);
    let mut state = engine_state_lock();
    match parsed {
        Some((bits, vk)) => {
            state.temp_macro_play_hotkey = Some((bits, vk));
            state.temp_macro_play_hotkey_str = Some(combo.to_string());
            crate::recorder::TEMP_MACRO_PLAY_BITS.store(bits, Ordering::SeqCst);
            crate::recorder::TEMP_MACRO_PLAY_VK.store(vk, Ordering::SeqCst);
            log::info!("[HOOK] Temp macro play hotkey set: {} → bits={} vk=0x{:02X}", combo, bits, vk);
        }
        None => {
            state.temp_macro_play_hotkey = None;
            state.temp_macro_play_hotkey_str = None;
            crate::recorder::TEMP_MACRO_PLAY_VK.store(0, Ordering::SeqCst);
            log::info!("[HOOK] Temp macro play hotkey cleared");
        }
    }
}

/// Continuous-replay loop hotkey for the Quick Record temp macro. Pass an
/// empty combo to clear (loop disabled). Identical setter pattern to the
/// Record + Play hotkeys, with the dedicated TEMP_MACRO_LOOP_* atomics so
/// the LL hook can match without acquiring engine_state.
pub fn set_temp_macro_loop_hotkey(combo: &str) {
    let parsed = parse_hotkey_combo(combo);
    let mut state = engine_state_lock();
    match parsed {
        Some((bits, vk)) => {
            state.temp_macro_loop_hotkey = Some((bits, vk));
            state.temp_macro_loop_hotkey_str = Some(combo.to_string());
            crate::recorder::TEMP_MACRO_LOOP_BITS.store(bits, Ordering::SeqCst);
            crate::recorder::TEMP_MACRO_LOOP_VK.store(vk, Ordering::SeqCst);
            log::info!("[HOOK] Temp macro loop hotkey set: {} → bits={} vk=0x{:02X}", combo, bits, vk);
        }
        None => {
            state.temp_macro_loop_hotkey = None;
            state.temp_macro_loop_hotkey_str = None;
            crate::recorder::TEMP_MACRO_LOOP_VK.store(0, Ordering::SeqCst);
            log::info!("[HOOK] Temp macro loop hotkey cleared");
        }
    }
}

pub fn set_voice_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        let mut state = engine_state_lock();
        state.voice_hotkey = Some(parsed);
        rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
        rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
        add_overlay_to_suppress(state.overlay_hotkey);
        add_pause_to_suppress(state.pause_hotkey);
        add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
        add_voice_to_suppress(Some(parsed));
        add_radial_menu_to_suppress(state.radial_menu_hotkey);
        log::info!("[HOOK] Voice hotkey set: {} → bits={} vk=0x{:02X}", combo, parsed.0, parsed.1);
    }
}

pub fn clear_voice_hotkey() {
    let mut state = engine_state_lock();
    state.voice_hotkey = None;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    add_voice_to_suppress(state.voice_hotkey);
    add_radial_menu_to_suppress(state.radial_menu_hotkey);
    log::info!("[HOOK] Voice hotkey cleared");
}

pub fn clear_pause_hotkey() {
    let mut state = engine_state_lock();
    state.pause_hotkey = None;
    state.pause_hotkey_str = None;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    add_voice_to_suppress(state.voice_hotkey);
    add_radial_menu_to_suppress(state.radial_menu_hotkey);
    log::info!("[HOOK] Pause hotkey cleared");
}

pub fn set_clipboard_paste_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        let mut state = engine_state_lock();
        state.clipboard_paste_hotkey = Some(parsed);
        rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
        rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
        add_overlay_to_suppress(state.overlay_hotkey);
        add_pause_to_suppress(state.pause_hotkey);
        add_clipboard_paste_to_suppress(Some(parsed));
        add_voice_to_suppress(state.voice_hotkey);
        add_radial_menu_to_suppress(state.radial_menu_hotkey);
        log::info!("[HOOK] Clipboard paste hotkey set: {} → bits={} vk=0x{:02X}", combo, parsed.0, parsed.1);
    }
}

pub fn clear_clipboard_paste_hotkey() {
    let mut state = engine_state_lock();
    state.clipboard_paste_hotkey = None;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(None);
    add_voice_to_suppress(state.voice_hotkey);
    add_radial_menu_to_suppress(state.radial_menu_hotkey);
    log::info!("[HOOK] Clipboard paste hotkey cleared");
}

pub fn set_radial_menu_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        let mut state = engine_state_lock();
        state.radial_menu_hotkey = Some(parsed);
        rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
        rebuild_all_linked_mouse(&state.assignments, &state.profile_settings);
        add_overlay_to_suppress(state.overlay_hotkey);
        add_pause_to_suppress(state.pause_hotkey);
        add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
        add_voice_to_suppress(state.voice_hotkey);
        add_radial_menu_to_suppress(Some(parsed));
        log::info!("[HOOK] Radial menu hotkey set: {} → bits={} vk=0x{:02X}", combo, parsed.0, parsed.1);
    }
}

pub fn clear_radial_menu_hotkey() {
    let mut state = engine_state_lock();
    state.radial_menu_hotkey = None;
    rebuild_suppress_keys(&state.assignments, &state.active_profile, &state.profile_settings);
    add_overlay_to_suppress(state.overlay_hotkey);
    add_pause_to_suppress(state.pause_hotkey);
    add_clipboard_paste_to_suppress(state.clipboard_paste_hotkey);
    add_voice_to_suppress(state.voice_hotkey);
    log::info!("[HOOK] Radial menu hotkey cleared");
}

/// Returns true while the radial menu hotkey is physically held (keydown fired, keyup hasn't).
pub fn is_radial_menu_held() -> bool {
    RADIAL_MENU_OPEN.load(Ordering::SeqCst)
}

/// Clear the radial menu open state (called when overlay hides).
pub fn clear_radial_menu_open() {
    RADIAL_MENU_OPEN.store(false, Ordering::SeqCst);
    RADIAL_KEY_HELD.store(false, Ordering::SeqCst);
}

pub fn get_engine_status() -> Value {
    let state = engine_state_lock();
    serde_json::json!({
        "uiohookAvailable": HOOKS_RUNNING.load(Ordering::SeqCst),
        "nutjsAvailable": false,
        "macrosEnabled": MACROS_ENABLED.load(Ordering::SeqCst),
        "activeProfile": state.active_profile,
        "globalPauseToggleKey": state.pause_hotkey_str,
        "isDemoMode": false,
    })
}
