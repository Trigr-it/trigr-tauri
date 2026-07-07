//! Non-Windows twin of actions.rs. The real actions.rs drives
//! SendInput/clipboard/AHK on Win32.
//!
//! Mac port Phase 2, milestone 2 (`port/mac-hooks`): on macOS this file is no
//! longer a no-op — it is the native injection engine. CGEventPost posts
//! synthetic keys, NSPasteboard backs the clipboard fns, and `execute_action`
//! dispatches the first real action types ("text" via clipboard paste or
//! direct typing, "url", "expansion" pass-through). Public signatures are
//! identical to the Windows original; lib.rs and the frontend are unchanged.
//!
//! Injection discipline (mirror of the Windows SUPPRESS_SIMULATED /
//! LLKHF_INJECTED rules, see MAC-PORT.md hard rule 4):
//!   * EVERY CGEvent Keyfire posts is stamped with `INJECTED_EVENT_MAGIC` in
//!     the event's source-user-data field, and the event tap in
//!     stubs/hotkeys.rs drops stamped events before they reach the processor.
//!     All posting therefore funnels through `macos::post_key` /
//!     `macos::post_unicode`.
//!   * Physically held modifiers are released before any synthetic paste and
//!     re-pressed after (hard rule 5). `release_held_modifiers` returns NATIVE
//!     mac keycodes in its Vec<u16> — the vec is opaque to callers, who only
//!     ever round-trip it into `restore_modifiers`.
//!   * `send_vk_key_pub` accepts WINDOWS virtual-key codes (shared lib.rs code
//!     is written in VK terms) and translates to mac keycodes. Ctrl VKs map to
//!     Command deliberately: every shared caller uses Ctrl as the accelerator
//!     modifier (Ctrl+V paste sequences), and the mac accelerator is ⌘.
//!     Synthetic events do not latch modifiers on macOS the way SendInput
//!     does, so injected-modifier state is tracked in a mask and stamped onto
//!     every subsequent key event.
//!
//! AHK fns stay permanent no-ops (AHK is Windows-only forever). On non-mac
//! non-Windows targets (e.g. Linux) everything remains a no-op.
#![allow(dead_code, unused_variables)]

use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Future clipboard manager checks this flag and skips logging if set.
/// (Level flag, same contract as Windows; the changeCount queue below is the
/// precise mechanism, this covers the synchronous window.)
pub static SUPPRESS_NEXT_CLIPBOARD_WRITE: AtomicBool = AtomicBool::new(false);

/// Magic value stamped into `EventField::EVENT_SOURCE_USER_DATA` of every
/// CGEvent Keyfire posts. The event tap in stubs/hotkeys.rs filters on it —
/// the mac analogue of the Windows LLKHF_INJECTED bit. "KFYR".
#[cfg(target_os = "macos")]
pub(crate) const INJECTED_EVENT_MAGIC: i64 = 0x4B46_5952;

/// Serialises tests that mutate the real NSPasteboard (it is not safe for
/// concurrent mutation from two threads — parallel test runners SIGTRAP on
/// an ObjC exception). Lock it in every test that writes the pasteboard.
#[cfg(all(test, target_os = "macos"))]
pub(crate) static PASTEBOARD_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn cleanup_stale_ahk_scripts(app_data_dir: PathBuf) {}

pub fn execute_action(
    macro_val: &Value,
    is_bare: bool,
    target_hwnd: isize,
    is_altgr: bool,
    trigger_key: Option<&str>,
    app: &tauri::AppHandle,
) {
    #[cfg(target_os = "macos")]
    {
        macos::execute_action(macro_val, is_bare, target_hwnd, is_altgr, trigger_key, app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("[stub] execute_action: action engine is not available on this platform yet");
    }
}

pub fn kill_all_ahk_processes() {}

// ── Paste-op re-entrancy guard ──────────────────────────────────────────────
// Same contract as the Windows original: `paste_clipboard_item`, `paste_text`
// and `copy_clipboard_item` in lib.rs each do a read-prev / write-text /
// paste / restore-prev dance; concurrent invocations interleave and flood the
// clipboard. One AtomicBool gates them all — first caller acquires, everyone
// else drops out instantly. Released on Drop (including panic).
pub static PASTE_OP_ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) struct PasteOpGuard;

impl PasteOpGuard {
    /// Returns Some(guard) if no other paste/copy op is running. Returns None
    /// if one is already in flight — the caller MUST return without touching
    /// the clipboard.
    pub(crate) fn try_acquire() -> Option<Self> {
        match PASTE_OP_ACTIVE.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => Some(PasteOpGuard),
            Err(_) => None,
        }
    }
}

impl Drop for PasteOpGuard {
    fn drop(&mut self) {
        PASTE_OP_ACTIVE.store(false, Ordering::SeqCst);
    }
}

pub fn read_clipboard_pub() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos::read_clipboard()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Record the current pasteboard changeCount as a Keyfire-internal write so
/// the (future, milestone 5) clipboard listener won't log it. Mac analogue of
/// the Windows clipboard-sequence-number queue.
pub(crate) fn record_self_clipboard_write() {
    #[cfg(target_os = "macos")]
    {
        macos::record_self_change_count();
    }
}

/// True if pasteboard change `count` was produced by a Keyfire-internal write.
/// Consumes the match so a single internal write is only ever skipped once.
/// Consumed by the milestone-5 clipboard listener.
#[cfg(target_os = "macos")]
pub(crate) fn is_self_clipboard_change(count: i64) -> bool {
    macos::is_self_change_count(count)
}

/// Current NSPasteboard changeCount — the mac analogue of the Windows
/// clipboard sequence number (expansions::clipboard_sequence_number).
#[cfg(target_os = "macos")]
pub(crate) fn clipboard_change_count() -> i64 {
    macos::change_count()
}

// ── Macro re-entrancy + loop-cancel machinery (twin of the Windows statics) ─

use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, LazyLock, Mutex};

static ACTIVE_MACRO_KEYS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

static LOOPING_MACROS: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub static LOOPING_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static ESC_LOOP_BREAK: AtomicBool = AtomicBool::new(false);

/// H1 re-entrancy guard — one in-flight fire per storage key. Dropped (incl.
/// on panic) releases the key.
pub(crate) struct MacroRunningGuard {
    key: String,
}

impl MacroRunningGuard {
    pub(crate) fn try_acquire(key: &str) -> Option<Self> {
        let mut set = ACTIVE_MACRO_KEYS.lock().ok()?;
        if set.contains(key) {
            return None;
        }
        set.insert(key.to_string());
        Some(Self { key: key.to_string() })
    }
}

impl Drop for MacroRunningGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = ACTIVE_MACRO_KEYS.lock() {
            set.remove(&self.key);
        }
    }
}

pub(crate) struct LoopHandle {
    key: String,
    cancel_flag: Arc<AtomicBool>,
}

impl LoopHandle {
    pub(crate) fn register(key: &str) -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        if let Ok(mut map) = LOOPING_MACROS.lock() {
            map.insert(key.to_string(), flag.clone());
        }
        LOOPING_COUNT.fetch_add(1, Ordering::SeqCst);
        Self {
            key: key.to_string(),
            cancel_flag: flag,
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst) || ESC_LOOP_BREAK.load(Ordering::SeqCst)
    }
}

impl Drop for LoopHandle {
    fn drop(&mut self) {
        if let Ok(mut map) = LOOPING_MACROS.lock() {
            map.remove(&self.key);
        }
        let prev = LOOPING_COUNT.fetch_sub(1, Ordering::SeqCst);
        if prev <= 1 {
            ESC_LOOP_BREAK.store(false, Ordering::SeqCst);
        }
    }
}

/// Hook-side re-press handler. If `trigger_key` has a loop in flight, set its
/// cancel flag and return true so the caller can drop the new fire. The
/// running iteration will observe the flag at its next per-iter check or
/// between-step poll and exit cleanly.
pub fn cancel_loop_if_running(trigger_key: &str) -> bool {
    if let Ok(map) = LOOPING_MACROS.lock() {
        if let Some(flag) = map.get(trigger_key) {
            flag.store(true, Ordering::SeqCst);
            return true;
        }
    }
    false
}

/// Release the currently held key (if any). Safe to call from any thread.
/// Returns the label of the released key (for logging) or None.
pub fn release_held_key() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos::release_held_key()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Check if a key is currently being held (Send Hotkey hold mode).
pub fn is_key_held() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::is_key_held()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Release the held key only if it was triggered by the given mouse button.
/// `allow_pending` records the up for the fast-click race — see the manager.
pub fn release_held_if_mouse_trigger(mouse_id: &str, allow_pending: bool) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos::release_held_if_mouse_trigger(mouse_id, allow_pending)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (mouse_id, allow_pending);
        None
    }
}

/// Clear a stale pending-release from a previous click cycle.
pub fn clear_pending_mouse_release(mouse_id: &str) {
    #[cfg(target_os = "macos")]
    {
        macos::clear_pending_mouse_release(mouse_id);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = mouse_id;
    }
}

pub fn is_repeating() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::is_repeating()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn get_repeating_trigger() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos::get_repeating_trigger()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Read which modifiers are physically held, post tagged key-ups for them,
/// and return the list that was held for later re-press.
///
/// NOTE: on macOS the returned codes are NATIVE mac keycodes, not Windows
/// VKs. The vec is opaque to callers — every caller only round-trips it into
/// `restore_modifiers`, which expects the same currency.
pub fn release_held_modifiers() -> Vec<u16> {
    #[cfg(target_os = "macos")]
    {
        macos::release_held_modifiers()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Re-press modifiers that were held before injection (mac keycodes, from
/// `release_held_modifiers`).
pub fn restore_modifiers(held: &[u16]) {
    #[cfg(target_os = "macos")]
    {
        macos::restore_modifiers(held);
    }
}

/// Post a single key event for a WINDOWS virtual-key code (shared lib.rs code
/// speaks VK). Translated to a mac keycode; Ctrl maps to ⌘ (see module docs).
pub fn send_vk_key_pub(vk: u16, key_up: bool) {
    #[cfg(target_os = "macos")]
    {
        macos::send_vk_key(vk, key_up);
    }
}

/// Post a modifier chord + main key (NATIVE mac keycodes). Modifiers press in
/// order, the main key taps with the accumulated flags (15ms down→up so
/// per-frame pollers see it), modifiers release in reverse. Used by the
/// expansions engine's `{key:...}` tokens; `None` main = bare-modifier chord.
pub(crate) fn post_chord_keycodes(mod_keycodes: &[u16], main: Option<u16>) {
    #[cfg(target_os = "macos")]
    {
        macos::post_chord(mod_keycodes, main);
    }
}

/// Character-by-character direct typing (unicode key events). Exposed for the
/// expansions engine; the "text" action's direct path uses the same impl.
pub(crate) fn type_text_direct_pub(text: &str) {
    #[cfg(target_os = "macos")]
    {
        macos::type_text_direct(text);
    }
}

/// Post a tagged down+15ms+up tap of a NATIVE mac keycode with an explicit
/// CGEventFlags bit mask. Used by the matcher's hold-passthrough taps so the
/// app receives the suppressed key with the user's live modifier state.
pub(crate) fn post_tap_keycode(keycode: u16, flags_bits: u64) {
    #[cfg(target_os = "macos")]
    {
        macos::post_tap_with_flags(keycode, flags_bits);
    }
}

/// Replay a captured RecordedEvent stream (keys in Windows-VK terms, mouse
/// with absolute coordinates). Shared by the "Record Macro" macro step and
/// the Quick Replay global hotkey — same pub surface as the Windows original.
pub fn replay_recorded_events(events: &[crate::recorder::RecordedEvent], label: &str) {
    #[cfg(target_os = "macos")]
    {
        macos::replay_recorded_events(events, label);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (events, label);
        log::warn!("[stub] recorded-event replay is not available on this platform yet");
    }
}

/// Continuous-replay wrapper for the Quick Loop hotkey: repeats until the
/// Loop hotkey fires again, Esc, or macros are paused. Inter-iteration pause
/// polled in 100ms chunks so a stop signal is honoured promptly.
pub fn replay_recorded_events_loop(events: &[crate::recorder::RecordedEvent], label: &str) {
    use crate::recorder::TEMP_MACRO_LOOP_ACTIVE;
    TEMP_MACRO_LOOP_ACTIVE.store(true, Ordering::SeqCst);
    log::info!("[Keyfire] {}: loop started", label);
    let mut iter: u64 = 0;
    while TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst)
        && !ESC_LOOP_BREAK.load(Ordering::SeqCst)
        && crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst)
    {
        iter += 1;
        let iter_label = format!("{} (loop iter {})", label, iter);
        replay_recorded_events(events, &iter_label);
        // 500ms breathing room between iterations, polled cancellable.
        for _ in 0..5 {
            if !TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst)
                || ESC_LOOP_BREAK.load(Ordering::SeqCst)
                || !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst)
            {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
    TEMP_MACRO_LOOP_ACTIVE.store(false, Ordering::SeqCst);
    log::info!("[Keyfire] {}: loop stopped after {} iter(s)", label, iter);
}

/// AHK-style bare-key remap: keydown posts the target chord's downs (no up);
/// `remap_key_release` posts the ups on the trigger's keyup. `trigger_key`
/// carries a NATIVE mac keycode on macOS (a VK on Windows) — opaque to the
/// caller, which round-trips the same value into the release. Returns false
/// for mouse/hold/repeat/unknown targets so the caller falls back to
/// fire_macro.
pub fn remap_key_press(trigger_key: u16, data: &Value) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::remap_key_press(trigger_key, data)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Release phase of a bare-key remap (called on keyup). Returns true if a
/// remap was active for this trigger (caller should early-return).
pub fn remap_key_release(trigger_key: u16) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::remap_key_release(trigger_key)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Execute a Send Hotkey combo inline on the calling thread — no thread
/// spawn, no pending deferral, fires on keydown. Returns false → fall
/// through to pending/fire_macro for mouse buttons, hold mode, repeat mode,
/// or unknown key names.
pub fn execute_hotkey_inline(data: &Value, _app: &tauri::AppHandle) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::execute_hotkey_inline(data)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub fn set_foreground_robust(hwnd: isize) -> bool {
    // HWNDs don't exist on macOS; focus follows the frontmost app, which the
    // overlay windows don't steal (they are non-activating panels). The
    // NSWorkspace-based foreground module (milestone 6) may add a real
    // activate-by-pid here if a use case appears.
    false
}

pub fn stop_repeating_key() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        macos::stop_repeating_key()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// On Windows this guard sets hotkeys::SUPPRESS_SIMULATED so the LL hook
/// ignores injected events. On macOS suppression is per-event — every posted
/// CGEvent carries INJECTED_EVENT_MAGIC and the tap drops it — so the guard
/// is a structural no-op kept for call-site compatibility.
pub(crate) struct SuppressionGuard;

impl SuppressionGuard {
    pub(crate) fn new() -> Self {
        SuppressionGuard
    }
}

pub fn write_clipboard_pub(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::write_clipboard_impl(text, true)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Write to the clipboard but let the clipboard listener record this write as
/// a new history entry (used by Save as New / transform copies where the new
/// text is a genuinely novel variant the user wants in their history).
pub fn write_clipboard_recordable_pub(text: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::write_clipboard_impl(text, false)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

// ── macOS injection engine ───────────────────────────────────────────────────
#[cfg(target_os = "macos")]
mod macos {
    use super::{INJECTED_EVENT_MAGIC, SUPPRESS_NEXT_CLIPBOARD_WRITE};
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTapLocation, CGEventType, EventField,
    };
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
    use log::{info, warn};
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;
    use serde_json::Value;
    use std::cell::Cell;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{LazyLock, Mutex};
    use std::thread;
    use std::time::Duration;

    // CGEventSourceKeyState is not bound by core-graphics 0.25. Same direct-
    // bind pattern as CGEventTapEnable in stubs/hotkeys.rs.
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventSourceKeyState(state: CGEventSourceStateID, key: u16) -> bool;
    }

    const KEYSTROKE_DELAY_MS: u64 = 10;

    // ── mac virtual keycodes (kVK_*) used by the engine ─────────────────────
    const KC_V: u16 = 9;
    const KC_RETURN: u16 = 36;
    const KC_LCMD: u16 = 55;
    const KC_RCMD: u16 = 54;
    const KC_LSHIFT: u16 = 56;
    const KC_RSHIFT: u16 = 60;
    const KC_LOPTION: u16 = 58;
    const KC_ROPTION: u16 = 61;
    const KC_LCTRL: u16 = 59;
    const KC_RCTRL: u16 = 62;

    /// All modifier keycodes we track for release/restore (left + right).
    const MAC_MODIFIER_KEYCODES: &[u16] = &[
        KC_LCMD, KC_RCMD, KC_LSHIFT, KC_RSHIFT, KC_LOPTION, KC_ROPTION, KC_LCTRL, KC_RCTRL,
    ];

    /// CGEventFlags bit for a modifier keycode.
    fn modifier_flag(keycode: u16) -> Option<CGEventFlags> {
        match keycode {
            KC_LCMD | KC_RCMD => Some(CGEventFlags::CGEventFlagCommand),
            KC_LSHIFT | KC_RSHIFT => Some(CGEventFlags::CGEventFlagShift),
            KC_LOPTION | KC_ROPTION => Some(CGEventFlags::CGEventFlagAlternate),
            KC_LCTRL | KC_RCTRL => Some(CGEventFlags::CGEventFlagControl),
            _ => None,
        }
    }

    /// Injected-modifier mask. macOS does not latch synthetic modifier state
    /// the way SendInput does — each posted event carries its own flags — so
    /// when shared code sends "Ctrl down, V down, V up, Ctrl up" as four
    /// separate calls, the V events must be stamped with the accumulated mask
    /// or the target app sees a bare V. SeqCst per the hard rules.
    static INJECTED_MODS: AtomicU64 = AtomicU64::new(0);

    // ── Event posting (ALL synthetic events go through these two fns) ───────

    fn new_source() -> Option<CGEventSource> {
        match CGEventSource::new(CGEventSourceStateID::CombinedSessionState) {
            Ok(src) => Some(src),
            Err(()) => {
                warn!("[INJECT] CGEventSource creation failed — cannot post events");
                None
            }
        }
    }

    /// Post one keyboard event, stamped with the injected-event tag so our own
    /// tap drops it. `flags` is stamped verbatim (empty = no modifiers).
    fn post_key(keycode: u16, key_up: bool, flags: CGEventFlags) {
        let Some(src) = new_source() else { return };
        let Ok(ev) = CGEvent::new_keyboard_event(src, keycode, !key_up) else {
            warn!("[INJECT] CGEvent creation failed (keycode {})", keycode);
            return;
        };
        ev.set_flags(flags);
        ev.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INJECTED_EVENT_MAGIC);
        ev.post(CGEventTapLocation::HID);
    }

    /// Post a down+up pair carrying a unicode string payload (the mac analogue
    /// of KEYEVENTF_UNICODE). The keycode is a dummy carrier; apps read the
    /// attached string.
    fn post_unicode(units: &[u16]) {
        for key_up in [false, true] {
            let Some(src) = new_source() else { return };
            let Ok(ev) = CGEvent::new_keyboard_event(src, 0, !key_up) else { return };
            if !key_up {
                ev.set_string_from_utf16_unchecked(units);
            }
            ev.set_flags(CGEventFlags::CGEventFlagNull);
            ev.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INJECTED_EVENT_MAGIC);
            ev.post(CGEventTapLocation::HID);
        }
    }

    // ── Windows-VK → mac keycode translation (see module docs re Ctrl→⌘) ────

    fn vk_to_mac_keycode(vk: u16) -> Option<u16> {
        Some(match vk {
            // Letters A–Z
            0x41 => 0,   // A
            0x42 => 11,  // B
            0x43 => 8,   // C
            0x44 => 2,   // D
            0x45 => 14,  // E
            0x46 => 3,   // F
            0x47 => 5,   // G
            0x48 => 4,   // H
            0x49 => 34,  // I
            0x4A => 38,  // J
            0x4B => 40,  // K
            0x4C => 37,  // L
            0x4D => 46,  // M
            0x4E => 45,  // N
            0x4F => 31,  // O
            0x50 => 35,  // P
            0x51 => 12,  // Q
            0x52 => 15,  // R
            0x53 => 1,   // S
            0x54 => 17,  // T
            0x55 => 32,  // U
            0x56 => KC_V,
            0x57 => 13,  // W
            0x58 => 7,   // X
            0x59 => 16,  // Y
            0x5A => 6,   // Z
            // Digit row 0–9
            0x30 => 29,
            0x31 => 18,
            0x32 => 19,
            0x33 => 20,
            0x34 => 21,
            0x35 => 23,
            0x36 => 22,
            0x37 => 26,
            0x38 => 28,
            0x39 => 25,
            // Control & navigation
            0x08 => 51,        // Backspace → Delete
            0x09 => 48,        // Tab
            0x0D => KC_RETURN, // Enter → Return
            0x1B => 53,        // Esc
            0x20 => 49,        // Space
            0x21 => 116,       // PgUp
            0x22 => 121,       // PgDn
            0x23 => 119,       // End
            0x24 => 115,       // Home
            0x25 => 123,       // Left
            0x26 => 126,       // Up
            0x27 => 124,       // Right
            0x28 => 125,       // Down
            0x2E => 117,       // Delete → Forward Delete
            // F1–F12
            0x70 => 122,
            0x71 => 120,
            0x72 => 99,
            0x73 => 118,
            0x74 => 96,
            0x75 => 97,
            0x76 => 98,
            0x77 => 100,
            0x78 => 101,
            0x79 => 109,
            0x7A => 103,
            0x7B => 111,
            // Modifiers. Ctrl VKs map to COMMAND on purpose: shared callers
            // use Ctrl as the accelerator modifier (Ctrl+V paste sequences in
            // lib.rs), and the mac accelerator is ⌘. Native mac Control is
            // only reachable internally (release/restore use raw keycodes).
            0x10 | 0xA0 => KC_LSHIFT,
            0xA1 => KC_RSHIFT,
            0x11 | 0xA2 => KC_LCMD,
            0xA3 => KC_RCMD,
            0x12 | 0xA4 => KC_LOPTION,
            0xA5 => KC_ROPTION,
            0x5B => KC_LCMD, // LWin → LCmd (Meta stored as 'Win', hard rule 6)
            0x5C => KC_RCMD,
            _ => return None,
        })
    }

    pub(super) fn send_vk_key(vk: u16, key_up: bool) {
        let Some(keycode) = vk_to_mac_keycode(vk) else {
            warn!("[INJECT] no mac keycode mapping for VK 0x{:02X} — dropped", vk);
            return;
        };
        if let Some(flag) = modifier_flag(keycode) {
            // Modifier: update the injected mask, stamp the event with the
            // resulting state so apps tracking flagsChanged stay coherent.
            let mut mask = INJECTED_MODS.load(Ordering::SeqCst);
            if key_up {
                mask &= !flag.bits();
            } else {
                mask |= flag.bits();
            }
            INJECTED_MODS.store(mask, Ordering::SeqCst);
            post_key(keycode, key_up, CGEventFlags::from_bits_truncate(mask));
        } else {
            let mask = INJECTED_MODS.load(Ordering::SeqCst);
            post_key(keycode, key_up, CGEventFlags::from_bits_truncate(mask));
        }
    }

    // ── Physical modifier release/restore (hard rule 5) ─────────────────────

    fn physical_key_down(keycode: u16) -> bool {
        unsafe { CGEventSourceKeyState(CGEventSourceStateID::CombinedSessionState, keycode) }
    }

    pub(super) fn release_held_modifiers() -> Vec<u16> {
        let held: Vec<u16> = MAC_MODIFIER_KEYCODES
            .iter()
            .copied()
            .filter(|&kc| physical_key_down(kc))
            .collect();
        // Post key-ups with a decreasing flag mask so each event reflects the
        // modifier state after it lands (mirrors real key-up flagsChanged).
        let mut mask = held
            .iter()
            .filter_map(|&kc| modifier_flag(kc))
            .fold(CGEventFlags::CGEventFlagNull, |acc, f| acc | f);
        for &kc in &held {
            if let Some(f) = modifier_flag(kc) {
                mask.remove(f);
            }
            post_key(kc, true, mask);
        }
        held
    }

    pub(super) fn restore_modifiers(held: &[u16]) {
        let mut mask = CGEventFlags::CGEventFlagNull;
        for &kc in held {
            if let Some(f) = modifier_flag(kc) {
                mask.insert(f);
            }
            post_key(kc, false, mask);
        }
    }

    // ── NSPasteboard clipboard ───────────────────────────────────────────────
    // NSPasteboard is safe to use from any thread for these plain string ops
    // (arboard and every mac clipboard manager do the same); all callers are
    // background threads (processor thread, lib.rs paste threads).

    /// Pasteboard changeCounts produced by Keyfire's own writes; the
    /// milestone-5 clipboard listener skips these. Mirror of the Windows
    /// SELF_CLIPBOARD_SEQNUMS queue.
    static SELF_CHANGE_COUNTS: LazyLock<Mutex<VecDeque<i64>>> =
        LazyLock::new(|| Mutex::new(VecDeque::new()));

    pub(super) fn record_self_change_count() {
        let count = change_count();
        if let Ok(mut q) = SELF_CHANGE_COUNTS.lock() {
            if !q.contains(&count) {
                q.push_back(count);
            }
            // Cap — changeCounts are monotonic so stale ones never match.
            while q.len() > 64 {
                q.pop_front();
            }
        }
    }

    pub(super) fn is_self_change_count(count: i64) -> bool {
        if let Ok(mut q) = SELF_CHANGE_COUNTS.lock() {
            if let Some(pos) = q.iter().position(|&c| c == count) {
                q.remove(pos);
                return true;
            }
        }
        false
    }

    pub(super) fn change_count() -> i64 {
        let pb = NSPasteboard::generalPasteboard();
        pb.changeCount() as i64
    }

    pub(super) fn read_clipboard() -> Option<String> {
        let pb = NSPasteboard::generalPasteboard();
        let s = pb.stringForType(unsafe { NSPasteboardTypeString })?;
        Some(s.to_string())
    }

    pub(super) fn write_clipboard_impl(text: &str, suppress_listener: bool) -> bool {
        // Set suppress BEFORE touching the pasteboard so any listener that
        // fires mid-write skips it (same ordering as Windows).
        if suppress_listener {
            SUPPRESS_NEXT_CLIPBOARD_WRITE.store(true, Ordering::SeqCst);
        }
        let pb = NSPasteboard::generalPasteboard();
        pb.clearContents();
        let ok = pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        if !ok {
            warn!("[CLIP] NSPasteboard setString failed");
            return false;
        }
        if suppress_listener {
            super::record_self_clipboard_write();
        }
        true
    }

    // ── Speed presets (mirror of Windows speed_delays) ───────────────────────

    /// (initial_delay, step_settle, paste_settle, clipboard_restore) in ms.
    fn speed_delays() -> (u64, u64, u64, u64) {
        let state = crate::hotkeys::engine_state().lock().unwrap();
        match state.macro_speed.as_str() {
            "fast" => (5, 5, 5, 25),
            "instant" => (0, 0, 5, 25),
            "custom" => {
                let pre = state.custom_pre_execution_delay;
                let fg = if pre == 0 { 5 } else { (pre / 10).max(5) };
                let clip = if pre == 0 { 25 } else { (pre / 3).max(25) };
                (pre.min(10), pre.min(10), fg, clip)
            }
            _ => (10, 10, 10, 50), // "safe" (default)
        }
    }

    // ── Recursion guard (mirror of Windows FIRE_DEPTH) ───────────────────────

    const MAX_FIRE_DEPTH: u32 = 5;

    thread_local! {
        static FIRE_DEPTH: Cell<u32> = const { Cell::new(0) };
    }

    struct DepthGuard;

    impl Drop for DepthGuard {
        fn drop(&mut self) {
            FIRE_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }

    // ── Action executor ──────────────────────────────────────────────────────

    /// First ~40 chars of injected text for log lines (the Windows original
    /// uses expansions::log_preview; the mac expansions module isn't built
    /// yet, so keep a local twin).
    fn log_preview(text: &str) -> String {
        let mut s: String = text.chars().take(40).collect();
        if text.chars().count() > 40 {
            s.push('…');
        }
        s.replace('\n', "⏎")
    }

    /// Prepend https:// to a URL if no recognised scheme is present (twin of
    /// the Windows normalise_url — bare "google.com" fails in `open` too).
    fn normalise_url(raw: &str) -> String {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return String::new();
        }
        let lower = trimmed.to_lowercase();
        let schemes = [
            "http://", "https://", "ftp://", "file://",
            "mailto:", "tel:", "sms:", "javascript:", "about:",
        ];
        if schemes.iter().any(|s| lower.starts_with(s)) {
            return trimmed.to_string();
        }
        format!("https://{}", trimmed)
    }

    fn resolve_input_method(data: Option<&Value>) -> String {
        if let Some(d) = data {
            let method = d
                .get("inputMethod")
                .or_else(|| d.get("pasteMethod")) // legacy field name
                .and_then(|v| v.as_str());
            if let Some(m) = method {
                if m != "global" {
                    return m.to_string();
                }
            }
        }
        let state = crate::hotkeys::engine_state().lock().unwrap();
        state.global_input_method.clone()
    }

    pub(super) fn execute_action(
        macro_val: &Value,
        is_bare: bool,
        target_hwnd: isize,
        is_altgr: bool,
        trigger_key: Option<&str>,
        app: &tauri::AppHandle,
    ) {
        let depth = FIRE_DEPTH.with(|d| {
            let next = d.get() + 1;
            d.set(next);
            next
        });
        let _depth_guard = DepthGuard;
        if depth > MAX_FIRE_DEPTH {
            warn!(
                "[Keyfire] Fire recursion limit hit (depth {}, max {}) — aborting. A trigger or text expansion is calling itself directly or via a chain.",
                depth, MAX_FIRE_DEPTH
            );
            return;
        }

        let macro_type = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let label = macro_val
            .get("label")
            .and_then(|v| v.as_str())
            .unwrap_or("(unlabelled)");
        let data = macro_val.get("data");

        info!("[ACTION] Firing: [{}] {} altgr={}", macro_type, label, is_altgr);
        info!("[Keyfire] Firing: [{}] {} (depth {})", macro_type, label, depth);

        let (initial_ms, step_settle_ms, _, _) = speed_delays();
        if initial_ms > 0 && macro_type != "hotkey" {
            thread::sleep(Duration::from_millis(initial_ms));
        }

        // Bare-key / AltGr leaked-character erase is handled by the hotkey
        // matcher milestone (the tap doesn't suppress triggers yet, so there
        // is no leaked char to erase and no bare triggers can fire).
        // `target_hwnd` is Windows-only; on macOS the frontmost app keeps
        // focus (our overlays are non-activating), so injection needs no
        // focus juggling.

        match macro_type {
            "text" => {
                if let Some(text) = data.and_then(|d| d.get("text")).and_then(|v| v.as_str()) {
                    if step_settle_ms > 0 {
                        thread::sleep(Duration::from_millis(step_settle_ms));
                    }
                    let method = resolve_input_method(data);
                    // Token resolution routes through expansions::resolve_tokens
                    // (currently pass-through on mac; real tokens land with the
                    // expansions milestone — this call keeps the seam identical).
                    let global_vars = crate::expansions::get_global_variables();
                    let empty_fillin: std::collections::HashMap<String, String> =
                        std::collections::HashMap::new();
                    let (resolved, _cursor_back) =
                        crate::expansions::resolve_tokens(text, &global_vars, &empty_fillin);
                    output_text(&resolved, &method);
                }
            }

            "expansion" => {
                let trigger = data
                    .and_then(|d| d.get("trigger"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if trigger.is_empty() {
                    warn!("[Keyfire] expansion action: empty trigger, skipping");
                } else {
                    if step_settle_ms > 0 {
                        thread::sleep(Duration::from_millis(step_settle_ms));
                    }
                    crate::expansions::fire_expansion_by_trigger(trigger);
                }
            }

            "url" => {
                if let Some(url) = data.and_then(|d| d.get("url")).and_then(|v| v.as_str()) {
                    let normalised = normalise_url(url);
                    if !normalised.is_empty() {
                        let _ = opener::open(&normalised);
                    }
                }
            }

            "hotkey" => {
                if let Some(d) = data {
                    execute_send_hotkey(d, trigger_key, app);
                }
            }

            // App / folder launches. macOS `open` handles .app bundles,
            // plain binaries, files and directories uniformly via the opener
            // crate. The app picker stores the bundle PATH in appId; browsed
            // files store `path`. Windows AUMIDs (no leading '/') are
            // meaningless here and skip with a warning. Monitor targeting is
            // a later milestone — the target monitor field is ignored.
            "app" => {
                let path = data.and_then(|d| d.get("path")).and_then(|v| v.as_str()).unwrap_or("");
                let app_id = data.and_then(|d| d.get("appId")).and_then(|v| v.as_str()).unwrap_or("");
                let target = if !path.is_empty() { path } else { app_id };
                if target.is_empty() {
                    warn!("[Keyfire] app action: empty path, skipping");
                } else if !target.starts_with('/') {
                    warn!("[Keyfire] app action: \"{}\" is a Windows app id — re-pick the app on this Mac", target);
                } else {
                    let _ = opener::open(target);
                }
            }

            "folder" => {
                if let Some(path) = data.and_then(|d| d.get("path")).and_then(|v| v.as_str()) {
                    if !path.is_empty() {
                        let _ = opener::open(path);
                    }
                }
            }

            "macro" => {
                if let Some(steps) = data.and_then(|d| d.get("steps")).and_then(|v| v.as_array()) {
                    execute_macro_sequence(data, steps, label, trigger_key, app);
                }
            }

            // AHK is Windows-only forever (closed decision) — hidden in the
            // mac UI; a config authored on Windows just skips these.
            "ahk" => {
                warn!("[Keyfire] AHK actions are Windows-only — skipping \"{}\"", label);
            }

            other => {
                warn!(
                    "[Keyfire] action type [{}] is not implemented on macOS yet — skipping \"{}\"",
                    other, label
                );
            }
        }
    }

    // ── Macro sequence runner (twin of the Windows "macro" branch) ──────────

    fn execute_macro_sequence(
        data: Option<&Value>,
        steps: &[Value],
        label: &str,
        trigger_key: Option<&str>,
        app: &tauri::AppHandle,
    ) {
        use tauri::Emitter;
        let method = resolve_input_method(data);
        let uses_clipboard = method != "send-input" && method != "direct";
        let (_, settle_ms, _, clip_restore_ms) = speed_delays();

        // Clear any stale Esc-cancel flag so a pre-press doesn't immediately
        // abort the macro we're about to fire. The flag is set globally on
        // every real Esc keydown — once we're running, any subsequent Esc
        // press will set it again and the per-step check below catches it.
        super::ESC_LOOP_BREAK.store(false, Ordering::SeqCst);

        // Loop config — backward compatible: missing `loop` = single fire.
        let loop_cfg = data.and_then(|d| d.get("loop"));
        let loop_enabled = loop_cfg
            .and_then(|l| l.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let loop_mode = loop_cfg
            .and_then(|l| l.get("mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("count");
        let loop_count = loop_cfg
            .and_then(|l| l.get("count"))
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .max(1);
        let loop_delay_ms = loop_cfg
            .and_then(|l| l.get("delayMs"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let max_iters: u64 = if loop_enabled {
            if loop_mode == "forever" { u64::MAX } else { loop_count }
        } else {
            1
        };

        // Register loop handle only when looping AND we have a trigger key to
        // cancel against (re-press cancel can't reach an anonymous chain).
        let loop_handle = if loop_enabled && max_iters > 1 {
            trigger_key.map(super::LoopHandle::register)
        } else {
            None
        };

        if loop_enabled && max_iters > 1 {
            let _ = app.emit(
                "loop-fire-started",
                serde_json::json!({
                    "label": label,
                    "trigger": trigger_key.unwrap_or(""),
                    "mode": loop_mode,
                    "count": if loop_mode == "forever" { 0 } else { max_iters },
                }),
            );
            info!(
                "[Keyfire] Macro loop: {} step(s) × {}, method={}",
                steps.len(),
                if loop_mode == "forever" { "forever".to_string() } else { max_iters.to_string() },
                method
            );
        } else {
            info!("[Keyfire] Macro sequence: {} step(s), method={}", steps.len(), method);
        }

        // Clipboard batching: snapshot every pasteboard flavor once, batch
        // pastes, restore once — images/RTF survive the whole macro (and the
        // whole loop), same guarantee as Windows.
        let saved_snapshot = if uses_clipboard {
            crate::expansions::snapshot_clipboard()
        } else {
            Vec::new()
        };
        let mut clipboard_dirty = false;
        let mut cancelled = false;
        let mut iter_index: u64 = 0;

        'outer: while iter_index < max_iters {
            // Per-iteration cancel checks: re-press flag (loops only) +
            // global Esc break (loops AND one-shots).
            if let Some(ref lh) = loop_handle {
                if lh.is_cancelled() {
                    info!("[Keyfire] Macro loop cancelled at iter {}", iter_index);
                    cancelled = true;
                    break;
                }
            }
            if super::ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                info!("[Keyfire] Macro cancelled (Esc) at iter {}", iter_index);
                cancelled = true;
                break;
            }

            if iter_index > 0 && loop_delay_ms > 0 {
                // Polled sleep — 100ms chunks so Esc/re-press/pause are
                // honoured promptly even on very long inter-iteration waits.
                let sleep_chunk = Duration::from_millis(100);
                let total = Duration::from_millis(loop_delay_ms);
                let start = std::time::Instant::now();
                while start.elapsed() < total {
                    if let Some(ref lh) = loop_handle {
                        if lh.is_cancelled() {
                            info!("[Keyfire] Macro loop cancelled during inter-iter delay");
                            cancelled = true;
                            break 'outer;
                        }
                    }
                    if super::ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                        info!("[Keyfire] Macro cancelled (Esc) during inter-iter delay");
                        cancelled = true;
                        break 'outer;
                    }
                    if !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
                        info!("[Keyfire] Macro aborted (paused) during inter-iter delay");
                        cancelled = true;
                        break 'outer;
                    }
                    let remaining = total.saturating_sub(start.elapsed());
                    thread::sleep(sleep_chunk.min(remaining));
                }
            }

            for (i, step) in steps.iter().enumerate() {
                // Inter-step cancel poll — bounds Esc/re-press response time
                // by step duration even inside long macros.
                if let Some(ref lh) = loop_handle {
                    if lh.is_cancelled() {
                        info!("[Keyfire] Macro loop cancelled mid-iter at step {}/{}", i + 1, steps.len());
                        cancelled = true;
                        break 'outer;
                    }
                }
                if super::ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                    info!("[Keyfire] Macro cancelled (Esc) at step {}/{}", i + 1, steps.len());
                    cancelled = true;
                    break 'outer;
                }
                let step_type = step.get("type").and_then(|v| v.as_str()).unwrap_or("");
                let step_value = step.get("value").and_then(|v| v.as_str()).unwrap_or("");
                info!("[Keyfire]   Step {}/{}: [{}] \"{}\"", i + 1, steps.len(), step_type, step_value);

                if matches!(step_type, "Type Text" | "Dynamic Text") && uses_clipboard && !step_value.is_empty() {
                    if settle_ms > 0 {
                        thread::sleep(Duration::from_millis(settle_ms));
                    }
                    let resolved = resolve_type_text_tokens(step_value);
                    clipboard_paste_core(&resolved);
                    clipboard_dirty = true;
                } else {
                    // Restore clipboard before non-Type-Text steps if we
                    // dirtied it. No changeCount guard mid-macro — this is a
                    // controlled sequential flow.
                    if clipboard_dirty {
                        thread::sleep(Duration::from_millis(clip_restore_ms));
                        crate::expansions::restore_clipboard_snapshot(&saved_snapshot);
                        SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
                        clipboard_dirty = false;
                    }
                    let cont = execute_macro_step(step, &method, app);
                    if !cont {
                        info!("[Keyfire] Macro aborted at step {}/{} ({})", i + 1, steps.len(), step_type);
                        cancelled = true;
                        break 'outer;
                    }
                }
                // No foreground-HWND recapture on macOS — focus follows the
                // frontmost app naturally; injection targets whatever is
                // frontmost when the events land.
            }

            iter_index += 1;
        }

        // Final restore after all iterations. changeCount guard: if the user
        // copied something during the final paste window, leave their content.
        if clipboard_dirty {
            let post = change_count();
            thread::sleep(Duration::from_millis(clip_restore_ms));
            if change_count() == post {
                crate::expansions::restore_clipboard_snapshot(&saved_snapshot);
            }
            SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
        }

        if loop_handle.is_some() {
            let _ = app.emit(
                "loop-fire-ended",
                serde_json::json!({
                    "trigger": trigger_key.unwrap_or(""),
                    "iterations": iter_index,
                    "cancelled": cancelled,
                }),
            );
            info!("[Keyfire] Macro loop ended: {} iter(s), cancelled={}", iter_index, cancelled);
        }
        // loop_handle drop removes the LOOPING_MACROS entry, decrements
        // LOOPING_COUNT and resets ESC_LOOP_BREAK if this was the last loop.
    }

    /// Token resolution for macro Type Text — same resolver the expansion
    /// fire path uses, so tokens behave identically across both.
    fn resolve_type_text_tokens(text: &str) -> String {
        let global_vars = crate::expansions::get_global_variables();
        let empty_fillin: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let (resolved, _cursor_back) =
            crate::expansions::resolve_tokens(text, &global_vars, &empty_fillin);
        resolved
    }

    /// Core clipboard paste: write text + ⌘V. Does NOT save/restore the
    /// pasteboard — the macro runner batches that around the whole sequence.
    fn clipboard_paste_core(text: &str) {
        if !write_clipboard_impl(text, true) {
            warn!("[Keyfire] Skipping paste — clipboard write failed, would paste wrong content");
            return;
        }
        info!("[Keyfire] Clipboard write (macro): \"{}\"", log_preview(text));
        let (_, _, paste_settle_ms, _) = speed_delays();
        let held = release_held_modifiers();
        if paste_settle_ms > 0 {
            thread::sleep(Duration::from_millis(paste_settle_ms));
        }
        paste_cmd_v();
        restore_modifiers(&held);
    }

    /// One macro step. Returns false to abort the whole macro.
    fn execute_macro_step(step: &Value, method: &str, app: &tauri::AppHandle) -> bool {
        let step_type = step.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let step_value = step.get("value").and_then(|v| v.as_str()).unwrap_or("");
        let repeat_count = step
            .get("repeat")
            .and_then(|v| v.as_u64())
            .unwrap_or(1)
            .clamp(1, 99) as u32;
        let (_, settle_ms, _, _) = speed_delays();

        match step_type {
            "Type Text" | "Dynamic Text" => {
                if !step_value.is_empty() {
                    if settle_ms > 0 {
                        thread::sleep(Duration::from_millis(settle_ms));
                    }
                    let resolved = resolve_type_text_tokens(step_value);
                    output_text(&resolved, method);
                }
            }

            "Click Mouse" => {
                let btn = if step_value.is_empty() { "LButton" } else { step_value };
                if is_mouse_button(btn) {
                    for i in 0..repeat_count {
                        send_mouse_click(btn);
                        if i + 1 < repeat_count && settle_ms > 0 {
                            thread::sleep(Duration::from_millis(settle_ms));
                        }
                    }
                }
            }

            "Press Key" => {
                if !step_value.is_empty() {
                    // Legacy: mouse buttons stored under Press Key — still supported
                    if is_mouse_button(step_value) {
                        for i in 0..repeat_count {
                            send_mouse_click(step_value);
                            if i + 1 < repeat_count && settle_ms > 0 {
                                thread::sleep(Duration::from_millis(settle_ms));
                            }
                        }
                        return true;
                    }
                    // Parse "Ctrl+Shift+N" style strings — accelerator
                    // semantics (Ctrl/Win → ⌘), same as Send Hotkey.
                    let parts: Vec<&str> = step_value.split('+').map(|s| s.trim()).collect();
                    if let Some((&key_name, mod_parts)) = parts.split_last() {
                        let Some(target_kc) = crate::hotkeys::display_name_to_keycode(key_name) else {
                            warn!("[Keyfire] Unknown macro step key: {}", key_name);
                            return true;
                        };
                        let mod_kcs: Vec<u16> = mod_parts
                            .iter()
                            .filter_map(|m| match m.to_lowercase().as_str() {
                                "ctrl" | "win" => Some(KC_LCMD),
                                "alt" => Some(KC_LOPTION),
                                "shift" => Some(KC_LSHIFT),
                                _ => None,
                            })
                            .collect();
                        for i in 0..repeat_count {
                            post_chord(&mod_kcs, Some(target_kc));
                            if i + 1 < repeat_count && settle_ms > 0 {
                                thread::sleep(Duration::from_millis(settle_ms));
                            }
                        }
                    }
                }
            }

            "Wait (ms)" => {
                let ms: u64 = step_value.parse().unwrap_or(500).min(30000);
                // Polled sleep — 100ms chunks so Esc / pause reach the user
                // promptly even on long waits.
                let total = Duration::from_millis(ms);
                let start = std::time::Instant::now();
                while start.elapsed() < total {
                    if super::ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                        info!("[Keyfire] Wait (ms) cancelled (Esc)");
                        return false;
                    }
                    if !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
                        info!("[Keyfire] Wait (ms) aborted (macros disabled)");
                        return false;
                    }
                    let remaining = total.saturating_sub(start.elapsed());
                    thread::sleep(Duration::from_millis(100).min(remaining));
                }
            }

            // ⌘C / ⌘V / ⌘A as first-class macro steps (the Windows Ctrl
            // accelerator maps to ⌘). The OS handles copy/paste semantics —
            // this doesn't touch Keyfire's own clipboard write path.
            "Copy to Clipboard" | "Paste Clipboard" | "Select All" => {
                let target_kc: u16 = match step_type {
                    "Copy to Clipboard" => 8, // C
                    "Paste Clipboard" => KC_V,
                    _ => 0, // A
                };
                for i in 0..repeat_count {
                    post_chord(&[KC_LCMD], Some(target_kc));
                    if i + 1 < repeat_count && settle_ms > 0 {
                        thread::sleep(Duration::from_millis(settle_ms));
                    }
                }
                if matches!(step_type, "Copy to Clipboard" | "Paste Clipboard") {
                    thread::sleep(Duration::from_millis(50));
                }
            }

            // Passive wait until an app whose process name matches is
            // frontmost. Window-title matching needs the Screen Recording
            // permission on macOS, so title-only criteria are skipped with a
            // warning instead of hanging the macro for the full timeout —
            // same policy as the foreground watcher's linkedWindowTitle.
            "Wait for Window" => {
                if step_value.is_empty() {
                    warn!("[Keyfire] Wait for Window step: empty value");
                    return true;
                }
                let parsed: Value = match serde_json::from_str(step_value) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[Keyfire] Wait for Window step: invalid JSON: {}", e);
                        return true;
                    }
                };
                let target_proc = parsed
                    .get("process")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .trim_end_matches(".exe")
                    .to_lowercase();
                let target_title = parsed.get("title").and_then(|v| v.as_str()).unwrap_or("").trim();
                if target_proc.is_empty() {
                    warn!(
                        "[Keyfire] Wait for Window: title-only criteria not supported on macOS (needs Screen Recording) — skipping"
                    );
                    return true;
                }
                if !target_title.is_empty() {
                    warn!("[Keyfire] Wait for Window: title filter ignored on macOS — matching app name only");
                }
                const WAIT_FOR_WINDOW_TIMEOUT_MS: u64 = 30_000;
                let start = std::time::Instant::now();
                loop {
                    if super::ESC_LOOP_BREAK.load(Ordering::SeqCst)
                        || !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst)
                    {
                        return false;
                    }
                    let fg = crate::foreground::get_current_fg_proc().to_lowercase();
                    if !fg.is_empty() && (fg == target_proc || fg.trim_end_matches(".app") == target_proc) {
                        info!(
                            "[Keyfire] Wait for Window: matched (app='{}') after {:?}",
                            target_proc,
                            start.elapsed()
                        );
                        break;
                    }
                    if start.elapsed() >= Duration::from_millis(WAIT_FOR_WINDOW_TIMEOUT_MS) {
                        warn!(
                            "[Keyfire] Wait for Window: timeout waiting for app='{}' — aborting macro",
                            target_proc
                        );
                        return false;
                    }
                    thread::sleep(Duration::from_millis(150));
                }
            }

            "Open URL" => {
                let normalised = normalise_url(step_value);
                if !normalised.is_empty() {
                    let _ = opener::open(&normalised);
                }
            }

            "Open Folder" => {
                if step_value.is_empty() {
                    return true;
                }
                // Legacy macros stored a plain path; new writes emit JSON
                // {path, monitor}. Monitor targeting is ignored on macOS.
                let trimmed = step_value.trim_start();
                let path_owned = if trimmed.starts_with('{') {
                    serde_json::from_str::<Value>(step_value)
                        .ok()
                        .and_then(|p| p.get("path").and_then(|v| v.as_str()).map(String::from))
                        .unwrap_or_else(|| step_value.to_string())
                } else {
                    step_value.to_string()
                };
                if !path_owned.is_empty() {
                    let _ = opener::open(&path_owned);
                }
            }

            "Open App" => {
                if step_value.is_empty() {
                    warn!("[Keyfire] Open App step: empty value");
                    return true;
                }
                let parsed: Value = match serde_json::from_str(step_value) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[Keyfire] Open App step: invalid JSON: {}", e);
                        return true;
                    }
                };
                let path = parsed.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let app_id = parsed.get("appId").and_then(|v| v.as_str()).unwrap_or("");
                let target = if !path.is_empty() { path } else { app_id };
                if target.is_empty() {
                    warn!("[Keyfire] Open App step: empty path");
                } else if !target.starts_with('/') {
                    warn!("[Keyfire] Open App step: \"{}\" is a Windows app id — re-pick the app on this Mac", target);
                } else {
                    let _ = opener::open(target);
                }
            }

            "Focus Window" => {
                if step_value.is_empty() {
                    warn!("[Keyfire] Focus Window step: empty value");
                    return true;
                }
                let parsed: Value = match serde_json::from_str(step_value) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[Keyfire] Focus Window step: invalid JSON: {}", e);
                        return true;
                    }
                };
                let process = parsed.get("process").and_then(|v| v.as_str()).unwrap_or("");
                let title = parsed.get("title").and_then(|v| v.as_str()).unwrap_or("");
                if process.is_empty() {
                    warn!("[Keyfire] Focus Window: title-only criteria not supported on macOS — skipping");
                    return true;
                }
                if !title.is_empty() {
                    warn!("[Keyfire] Focus Window: title filter ignored on macOS — activating by app name");
                }
                if crate::foreground::activate_app_by_name(process) {
                    let (_, _, fg_settle_ms, _) = speed_delays();
                    thread::sleep(Duration::from_millis(fg_settle_ms.max(10) * 2));
                    info!("[Keyfire] Focus Window: activated app '{}'", process);
                } else {
                    warn!("[Keyfire] Focus Window: no running app matches '{}'", process);
                }
            }

            "Wait for Input" => {
                wait_for_input(step_value);
            }

            "Run AHK Script" => {
                warn!("[Keyfire] Run AHK Script step is Windows-only — skipping");
            }

            "Click at Position" => {
                if !step_value.is_empty() {
                    if let Ok(parsed) = serde_json::from_str::<Value>(step_value) {
                        let x = parsed.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
                        let y = parsed.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as f64;
                        let button = parsed.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                        let mode = parsed.get("mode").and_then(|v| v.as_str()).unwrap_or("absolute");
                        if mode == "relative" {
                            // Window-relative coordinates need the target
                            // window's frame — Screen Recording territory.
                            warn!("[Keyfire] Click at Position: relative mode not supported on macOS yet — skipping");
                            return true;
                        }
                        let click_button = match button {
                            "right" => "RButton",
                            "middle" => "MButton",
                            _ => "LButton",
                        };
                        info!("[Keyfire] Click at Position: ({}, {}) button={}", x, y, click_button);
                        let original = cursor_position();
                        send_mouse_move(x, y);
                        thread::sleep(Duration::from_millis(20));
                        let point = core_graphics::geometry::CGPoint::new(x, y);
                        send_mouse_event_at(click_button, false, Some(point));
                        thread::sleep(Duration::from_millis(15));
                        send_mouse_event_at(click_button, true, Some(point));
                        thread::sleep(Duration::from_millis(20));
                        if let Some(orig) = original {
                            send_mouse_move(orig.x, orig.y);
                        }
                    } else {
                        warn!("[Keyfire] Click at Position: invalid JSON");
                    }
                }
            }

            // Fire an existing hotkey assignment by its storage key. The
            // FIRE_DEPTH guard in execute_action bounds recursion.
            "Fire Trigger" => {
                if step_value.is_empty() {
                    warn!("[Keyfire] Fire Trigger: empty step value, skipping");
                    return true;
                }
                let lookup = {
                    let state = crate::hotkeys::engine_state().lock().unwrap();
                    state.assignments.get(step_value).cloned()
                };
                match lookup {
                    Some(target_macro) => {
                        info!("[Keyfire] Fire Trigger: invoking \"{}\"", step_value);
                        if settle_ms > 0 {
                            thread::sleep(Duration::from_millis(settle_ms));
                        }
                        execute_action(&target_macro, false, 0, false, None, app);
                    }
                    None => {
                        warn!("[Keyfire] Fire Trigger: assignment \"{}\" not found, skipping", step_value);
                    }
                }
            }

            "Fire Text Expansion" => {
                if step_value.is_empty() {
                    warn!("[Keyfire] Fire Text Expansion: empty step value, skipping");
                    return true;
                }
                if settle_ms > 0 {
                    thread::sleep(Duration::from_millis(settle_ms));
                }
                crate::expansions::fire_expansion_by_trigger(step_value);
            }

            // Literal replay of a captured event stream. Capture (Quick
            // Record) is a later milestone on mac, but streams recorded on
            // Windows replay through the VK→keycode translation.
            "Record Macro" => {
                if step_value.is_empty() {
                    warn!("[Keyfire] Record Macro: empty step value, skipping");
                    return true;
                }
                let events: Vec<crate::recorder::RecordedEvent> =
                    match serde_json::from_str(step_value) {
                        Ok(v) => v,
                        Err(e) => {
                            warn!("[Keyfire] Record Macro: invalid JSON ({})", e);
                            return true;
                        }
                    };
                replay_recorded_events(&events, "Record Macro");
            }

            _ => {
                warn!("[Keyfire] Unknown macro step type: {}", step_type);
            }
        }
        true
    }

    // ── Recorded-event replay ────────────────────────────────────────────────

    /// Replay a captured RecordedEvent stream, preserving inter-event gaps
    /// (capped so absurd waits can't freeze the macro). Events are tagged, so
    /// unlike Windows (which deliberately lets replayed events re-enter the
    /// hook) replayed events do NOT re-trigger Keyfire's own assignments —
    /// acceptable divergence; revisit if a real nesting use case appears.
    /// Always finishes with a defensive modifier release.
    pub(super) fn replay_recorded_events(events: &[crate::recorder::RecordedEvent], label: &str) {
        use crate::recorder::RecordedEvent;
        info!("[Keyfire] {}: replaying {} events", label, events.len());

        let mut prev_t: u64 = 0;
        const MAX_GAP_MS: u64 = 5000;

        for evt in events.iter() {
            if !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
                info!("[Keyfire] {}: aborted (macros disabled)", label);
                break;
            }
            if super::ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                info!("[Keyfire] {}: aborted (Esc)", label);
                break;
            }
            let evt_t = match evt {
                RecordedEvent::KeyDown { t, .. }
                | RecordedEvent::KeyUp { t, .. }
                | RecordedEvent::MouseDown { t, .. }
                | RecordedEvent::MouseUp { t, .. }
                | RecordedEvent::MouseMove { t, .. }
                | RecordedEvent::Wheel { t, .. } => *t,
            };
            let gap = evt_t.saturating_sub(prev_t).min(MAX_GAP_MS);
            if gap > 0 {
                thread::sleep(Duration::from_millis(gap));
            }
            prev_t = evt_t;

            match evt {
                RecordedEvent::KeyDown { vk, .. } => send_vk_key(*vk as u16, false),
                RecordedEvent::KeyUp { vk, .. } => send_vk_key(*vk as u16, true),
                RecordedEvent::MouseDown { button, x, y, .. } => {
                    send_mouse_move(*x as f64, *y as f64);
                    replay_mouse_button(button, false);
                }
                RecordedEvent::MouseUp { button, x, y, .. } => {
                    send_mouse_move(*x as f64, *y as f64);
                    replay_mouse_button(button, true);
                }
                RecordedEvent::MouseMove { x, y, .. } => {
                    send_mouse_move(*x as f64, *y as f64);
                }
                RecordedEvent::Wheel { delta, x, y, .. } => {
                    send_mouse_move(*x as f64, *y as f64);
                    send_scroll(*delta);
                }
            }
        }
        // Defensive cleanup: a stream that ended mid-modifier-press must not
        // leave the OS with stuck modifiers.
        for &kc in MAC_MODIFIER_KEYCODES {
            post_key(kc, true, CGEventFlags::CGEventFlagNull);
        }
        info!("[Keyfire] {}: complete", label);
    }

    /// Recorded button names ("Left"/"Right"/"Middle") → mouse event.
    fn replay_mouse_button(button: &str, is_up: bool) {
        let name = match button {
            "Right" => "RButton",
            "Middle" => "MButton",
            _ => "LButton",
        };
        send_mouse_event(name, is_up);
    }

    // ── Wait for Input step (keyboard; mouse needs the mouse-tap milestone) ─

    fn wait_for_input(config_json: &str) {
        use crate::hotkeys::{self, WaitEvent};
        use std::sync::mpsc::RecvTimeoutError;

        let config: serde_json::Value = serde_json::from_str(config_json).unwrap_or_default();
        let input_type = config.get("inputType").and_then(|v| v.as_str()).unwrap_or("LButton");
        let trigger = config.get("trigger").and_then(|v| v.as_str()).unwrap_or("press");
        let specific_key = config.get("specificKey").and_then(|v| v.as_str()).unwrap_or("");
        let wanted_key = specific_key.split('+').last().unwrap_or("").to_string();

        let is_mouse = matches!(input_type, "LButton" | "RButton" | "MButton");
        let mouse_name = match input_type {
            "LButton" => "MOUSE_LEFT",
            "RButton" => "MOUSE_RIGHT",
            "MButton" => "MOUSE_MIDDLE",
            _ => "",
        };

        info!("[WAIT] Wait for Input: type={} trigger={} key={}", input_type, trigger, wanted_key);

        const TIMEOUT: Duration = Duration::from_secs(30);
        const POLL_INTERVAL: Duration = Duration::from_millis(100);

        let rx = hotkeys::register_wait_for_input();
        let mut phase = "down";
        let deadline = std::time::Instant::now() + TIMEOUT;

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                warn!("[WAIT] Timed out after 30s");
                break;
            }
            if !hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
                info!("[WAIT] Cancelled — macros disabled");
                break;
            }
            if super::ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                info!("[WAIT] Cancelled — Esc");
                break;
            }

            let timeout = remaining.min(POLL_INTERVAL);
            match rx.recv_timeout(timeout) {
                Ok(event) => {
                    let matched = match (&event, is_mouse) {
                        (WaitEvent::MouseDown { button_name }, true) => {
                            button_name == mouse_name && matches!(trigger, "press" | "pressRelease")
                        }
                        (WaitEvent::MouseUp { button_name }, true) => {
                            button_name == mouse_name
                                && matches!(trigger, "release" | "pressRelease")
                        }
                        (WaitEvent::KeyDown { key_id }, false) => {
                            let key_matches = input_type == "AnyKey"
                                || (input_type == "SpecificKey" && *key_id == wanted_key);
                            key_matches && matches!(trigger, "press" | "pressRelease")
                        }
                        (WaitEvent::KeyUp { key_id }, false) => {
                            let key_matches = input_type == "AnyKey"
                                || (input_type == "SpecificKey" && *key_id == wanted_key);
                            key_matches && matches!(trigger, "release" | "pressRelease")
                        }
                        _ => false,
                    };
                    if !matched {
                        continue;
                    }
                    if trigger == "pressRelease" {
                        let is_down =
                            matches!(event, WaitEvent::KeyDown { .. } | WaitEvent::MouseDown { .. });
                        if phase == "down" && is_down {
                            phase = "up";
                            continue;
                        } else if phase == "up" && !is_down {
                            break;
                        }
                        continue;
                    }
                    break;
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    warn!("[WAIT] Channel disconnected");
                    break;
                }
            }
        }

        hotkeys::clear_wait_for_input();
        info!("[WAIT] Wait for Input complete");
    }

    // ── Text output ──────────────────────────────────────────────────────────

    fn output_text(text: &str, method: &str) {
        match method {
            "send-input" | "direct" => {
                info!("[Keyfire] Output text (direct): \"{}\"", log_preview(text));
                type_text_direct(text);
            }
            _ => {
                info!("[Keyfire] Output text (clipboard): \"{}\"", log_preview(text));
                inject_via_clipboard(text);
            }
        }
    }

    /// Character-by-character typing via unicode-string key events (mac
    /// analogue of the Windows KEYEVENTF_UNICODE path). Newlines are posted
    /// as real Return taps — apps ignore a bare U+000A payload.
    pub(super) fn type_text_direct(text: &str) {
        let _guard = super::SuppressionGuard::new();
        let held = release_held_modifiers();
        let mut buf = [0u16; 2];
        for ch in text.chars() {
            match ch {
                '\r' => continue,
                '\n' => {
                    post_key(KC_RETURN, false, CGEventFlags::CGEventFlagNull);
                    post_key(KC_RETURN, true, CGEventFlags::CGEventFlagNull);
                }
                _ => post_unicode(ch.encode_utf16(&mut buf)),
            }
            if KEYSTROKE_DELAY_MS > 0 {
                thread::sleep(Duration::from_millis(KEYSTROKE_DELAY_MS));
            }
        }
        restore_modifiers(&held);
    }

    /// Clipboard paste injection: snapshot → write → ⌘V → restore.
    ///
    /// The snapshot round-trips EVERY pasteboard flavor (expansions module's
    /// multi-flavor snapshot), so a copied image or rich-text clipboard
    /// survives a text action fire — same guarantee as the Windows original.
    fn inject_via_clipboard(text: &str) {
        let snapshot = crate::expansions::snapshot_clipboard();
        let (_, _, paste_settle_ms, clip_restore_ms) = speed_delays();

        if !write_clipboard_impl(text, true) {
            warn!("[Keyfire] Skipping paste — clipboard write failed, would paste wrong content");
            SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
            return;
        }
        info!("[Keyfire] Clipboard write (actions): \"{}\"", log_preview(text));
        // Capture changeCount AFTER our write so a third-party (or user)
        // clipboard change during the paste window blocks the restore.
        let post_write_count = change_count();

        let _guard = super::SuppressionGuard::new();
        let held = release_held_modifiers();
        if paste_settle_ms > 0 {
            thread::sleep(Duration::from_millis(paste_settle_ms));
        }
        paste_cmd_v();
        restore_modifiers(&held);

        thread::sleep(Duration::from_millis(clip_restore_ms));
        if change_count() == post_write_count {
            crate::expansions::restore_clipboard_snapshot(&snapshot);
        }
        SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
    }

    /// Send Hotkey action. Modifier tokens use accelerator semantics,
    /// matching send_vk_key_pub: "ctrl" and "win" both mean ⌘ on macOS
    /// ("Ctrl+C" authored on Windows should copy on a Mac), "alt" is ⌥,
    /// "shift" ⇧.
    ///
    /// Modes (twin of the Windows execute_send_hotkey):
    ///   * normal — press the chord, tap the main key (15ms down→up so
    ///     per-frame pollers see it), release in reverse. Bare-modifier
    ///     chords (key="", modifiers only) press and release just the chord.
    ///   * hold — press and LEAVE held; re-fire of the same chord releases
    ///     (toggle), a different chord switches. Any physical keypress also
    ///     releases (the matcher calls release_held_key first, same as the
    ///     Windows hook). Mouse-trigger release-on-up is a later milestone
    ///     (no mouse tap yet) — mouse holds release by toggle only.
    ///   * repeat — tap the chord every `repeatInterval` ms until the same
    ///     trigger re-fires, Esc, or pause. Runs on its own thread.
    fn execute_send_hotkey(data: &Value, trigger_key: Option<&str>, app: &tauri::AppHandle) {
        let key_name = data.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let is_mouse = is_mouse_button(key_name);
        let hold_mode = data.get("holdMode").and_then(|v| v.as_bool()).unwrap_or(false);
        let repeat_mode = data.get("repeatMode").and_then(|v| v.as_bool()).unwrap_or(false);
        let repeat_interval = data
            .get("repeatInterval")
            .and_then(|v| v.as_u64())
            .unwrap_or(100)
            .max(50);

        let modifiers: Vec<String> = if is_mouse {
            vec![]
        } else {
            data.get("modifiers")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default()
        };
        let mod_keycodes: Vec<u16> = modifiers
            .iter()
            .filter_map(|m| match m.to_lowercase().as_str() {
                "ctrl" | "win" => Some(KC_LCMD),
                "alt" => Some(KC_LOPTION),
                "shift" => Some(KC_LSHIFT),
                _ => None,
            })
            .collect();

        // Bare-modifier mode: key="" with non-empty modifiers → chord only.
        let target: Option<u16> = if is_mouse {
            None
        } else if key_name.is_empty() {
            if mod_keycodes.is_empty() {
                warn!("[Keyfire] Send Hotkey has no key or modifiers — nothing to send");
                return;
            }
            None
        } else {
            match crate::hotkeys::display_name_to_keycode(key_name) {
                Some(kc) => Some(kc),
                None => {
                    warn!("[Keyfire] Unknown Send Hotkey key: {}", key_name);
                    return;
                }
            }
        };

        let combo_label = if key_name.is_empty() {
            modifiers.join("+")
        } else if modifiers.is_empty() {
            key_name.to_string()
        } else {
            format!("{}+{}", modifiers.join("+"), key_name)
        };

        // ── Repeat mode ──
        if repeat_mode {
            let trigger_storage_key = trigger_key.unwrap_or("").to_string();

            {
                let mut rep = REPEATING_KEY.lock().unwrap();
                if let Some(ref state) = *rep {
                    if state.trigger_storage_key == trigger_storage_key {
                        // Same trigger — stop (toggle off)
                        state.stop.store(true, Ordering::SeqCst);
                        info!("[Keyfire] Repeat stopped (toggle): {}", combo_label);
                        *rep = None;
                        drop(rep);
                        crate::tray::update_tray_icon_normal(app);
                        return;
                    } else {
                        // Different trigger — stop old, start new
                        state.stop.store(true, Ordering::SeqCst);
                        info!("[Keyfire] Repeat stopped (switching): {}", state.label);
                        *rep = None;
                    }
                }
            }

            let stop = std::sync::Arc::new(super::AtomicBool::new(false));
            let stop_clone = stop.clone();
            let app_clone = app.clone();
            let key_name_owned = key_name.to_string();
            let mods_clone = mod_keycodes.clone();
            let target_copy = target;
            let is_mouse_copy = is_mouse;

            {
                let mut rep = REPEATING_KEY.lock().unwrap();
                *rep = Some(RepeatingKeyState {
                    trigger_storage_key,
                    label: combo_label.clone(),
                    stop: stop.clone(),
                });
            }

            crate::tray::update_tray_icon_repeating(app, &combo_label, repeat_interval);
            info!("[Keyfire] Repeat started: {} ({}ms)", combo_label, repeat_interval);

            thread::spawn(move || {
                // post_chord's 15ms hold window counts toward the interval so
                // the configured rate holds (interval floor is 50ms).
                const KEY_HOLD_MS: u64 = 15;
                loop {
                    if stop_clone.load(Ordering::SeqCst) {
                        break;
                    }
                    if !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
                        break;
                    }
                    if is_mouse_copy {
                        send_mouse_click(&key_name_owned);
                    } else {
                        post_chord(&mods_clone, target_copy);
                    }
                    thread::sleep(Duration::from_millis(
                        repeat_interval.saturating_sub(KEY_HOLD_MS),
                    ));
                }
                // Cleanup: clear state if this thread's stop flag is still the active one
                {
                    let mut rep = REPEATING_KEY.lock().unwrap();
                    if let Some(ref state) = *rep {
                        if std::sync::Arc::ptr_eq(&state.stop, &stop_clone) {
                            *rep = None;
                        }
                    }
                }
                crate::tray::update_tray_icon_normal(&app_clone);
            });
            return;
        }

        // ── Hold mode ──
        if hold_mode {
            let mut mgr = HELD_KEY.lock().unwrap();

            // Same chord already held — toggle release
            let same_held = if let Some(ref state) = mgr.key {
                if is_mouse {
                    state.mouse_button.as_deref() == Some(key_name)
                } else {
                    state.target_kc == target.unwrap_or(0)
                        && state.mod_kcs == mod_keycodes
                        && state.mouse_button.is_none()
                }
            } else {
                false
            };

            if same_held {
                let state = mgr.key.take().unwrap();
                mgr.pending_mouse_release = None;
                post_hold_release(&state);
                info!("[Keyfire] Hold released: {}", combo_label);
                drop(mgr);
                crate::tray::update_tray_icon_normal(app);
                return;
            }

            // Different key held — release previous first
            if let Some(ref state) = mgr.key {
                post_hold_release(state);
                info!("[Keyfire] Hold released (switching): {}", state.label);
            }

            info!("[ACTION] Send Hotkey HOLD: {}", combo_label);
            if is_mouse {
                send_mouse_event(key_name, false); // mousedown only
            } else {
                let physically_held = release_held_modifiers();
                let mut flags = CGEventFlags::CGEventFlagNull;
                for &kc in &mod_keycodes {
                    if let Some(f) = modifier_flag(kc) {
                        flags.insert(f);
                    }
                    post_key(kc, false, flags);
                }
                if let Some(kc) = target {
                    post_key(kc, false, flags);
                }
                // No keyup — key/modifiers stay held
                restore_modifiers(&physically_held);
            }

            // Detect a mouse-button trigger from the storage key so mouse-up
            // releases the hold (press-hold mirroring).
            let trigger_mouse = trigger_key
                .and_then(|tk| tk.split("::").last())
                .filter(|last| last.starts_with("MOUSE_"))
                .map(|s| s.to_string());

            mgr.key = Some(HeldKeyState {
                target_kc: target.unwrap_or(0),
                mod_kcs: mod_keycodes.clone(),
                mouse_button: if is_mouse { Some(key_name.to_string()) } else { None },
                label: combo_label.clone(),
                trigger_mouse_id: trigger_mouse.clone(),
            });

            // Fast-click race: the trigger button's UP may have arrived
            // before this thread stored the hold. If so, release immediately
            // so the simulated key/button never stays stuck down.
            let already_released = trigger_mouse
                .as_deref()
                .and_then(|tm| mgr.pending_mouse_release.as_deref().filter(|&p| p == tm))
                .is_some();
            if already_released {
                let state = mgr.key.take().unwrap();
                mgr.pending_mouse_release = None;
                post_hold_release(&state);
                info!("[Keyfire] Hold immediately released — mouse was already up: {}", combo_label);
                drop(mgr);
                crate::tray::update_tray_icon_normal(app);
                return;
            }

            drop(mgr);
            crate::tray::update_tray_icon_held(app, &combo_label);
            return;
        }

        // ── Normal mode ──
        if is_mouse {
            info!("[Keyfire] Send Hotkey → mouse click: {}", key_name);
            send_mouse_click(key_name);
        } else {
            // Release the physically held trigger modifiers (e.g. ⌘ from a
            // ⌘K trigger) so the target app sees ONLY the chord we send.
            let held = release_held_modifiers();
            post_chord(&mod_keycodes, target);
            restore_modifiers(&held);
        }
    }

    /// Tap a keycode with explicit flags (see post_tap_keycode).
    pub(super) fn post_tap_with_flags(keycode: u16, flags_bits: u64) {
        let flags = CGEventFlags::from_bits_truncate(flags_bits);
        post_key(keycode, false, flags);
        thread::sleep(Duration::from_millis(15));
        post_key(keycode, true, flags);
    }

    /// Press a modifier chord, tap the main key (15ms down→up so per-frame
    /// key-state pollers observe the press — the Windows KEY_HOLD_MS
    /// invariant), release the chord in reverse. All keycodes are native mac.
    pub(super) fn post_chord(mod_keycodes: &[u16], main: Option<u16>) {
        let mut flags = CGEventFlags::CGEventFlagNull;
        for &kc in mod_keycodes {
            if let Some(f) = modifier_flag(kc) {
                flags.insert(f);
            }
            post_key(kc, false, flags);
        }
        if let Some(kc) = main {
            post_key(kc, false, flags);
            thread::sleep(Duration::from_millis(15));
            post_key(kc, true, flags);
        }
        for &kc in mod_keycodes.iter().rev() {
            if let Some(f) = modifier_flag(kc) {
                flags.remove(f);
            }
            post_key(kc, true, flags);
        }
    }

    /// Post the ⌘V chord (all four events tagged; ⌘ carried on the V events'
    /// flags as well, since synthetic modifiers don't latch).
    fn paste_cmd_v() {
        post_key(KC_LCMD, false, CGEventFlags::CGEventFlagCommand);
        post_key(KC_V, false, CGEventFlags::CGEventFlagCommand);
        post_key(KC_V, true, CGEventFlags::CGEventFlagCommand);
        post_key(KC_LCMD, true, CGEventFlags::CGEventFlagNull);
    }

    // ── Mouse synthesis ──────────────────────────────────────────────────────

    /// Current cursor position (a fresh null CGEvent carries the pointer
    /// location — the standard CGEventGetLocation trick).
    fn cursor_position() -> Option<core_graphics::geometry::CGPoint> {
        let src = new_source()?;
        CGEvent::new(src).ok().map(|e| e.location())
    }

    /// (down event type, up event type, CGMouseButton) for a Windows-style
    /// button name (LButton/RButton/MButton).
    fn mouse_button_types(
        button: &str,
    ) -> Option<(CGEventType, CGEventType, core_graphics::event::CGMouseButton)> {
        use core_graphics::event::CGMouseButton;
        Some(match button {
            "LButton" => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, CGMouseButton::Left),
            "RButton" => (CGEventType::RightMouseDown, CGEventType::RightMouseUp, CGMouseButton::Right),
            "MButton" => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, CGMouseButton::Center),
            _ => return None,
        })
    }

    /// Returns true if the value is a mouse button name (LButton, RButton, MButton).
    fn is_mouse_button(name: &str) -> bool {
        matches!(name, "LButton" | "RButton" | "MButton")
    }

    /// Post a single tagged mouse event (down or up) at `point` (or the
    /// current cursor position when None).
    fn send_mouse_event_at(
        button: &str,
        is_up: bool,
        point: Option<core_graphics::geometry::CGPoint>,
    ) {
        let Some((down_t, up_t, cg_button)) = mouse_button_types(button) else {
            warn!("[INJECT] unknown mouse button: {}", button);
            return;
        };
        let Some(pos) = point.or_else(cursor_position) else { return };
        let Some(src) = new_source() else { return };
        let etype = if is_up { up_t } else { down_t };
        let Ok(ev) = CGEvent::new_mouse_event(src, etype, pos, cg_button) else {
            warn!("[INJECT] mouse event creation failed ({})", button);
            return;
        };
        ev.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INJECTED_EVENT_MAGIC);
        ev.post(CGEventTapLocation::HID);
    }

    fn send_mouse_event(button: &str, is_up: bool) {
        send_mouse_event_at(button, is_up, None);
    }

    /// Full click (down + 15ms hold + up) at the current cursor position —
    /// same hold window as key taps so per-frame pollers see the press.
    fn send_mouse_click(button: &str) {
        let pos = cursor_position();
        send_mouse_event_at(button, false, pos);
        thread::sleep(Duration::from_millis(15));
        send_mouse_event_at(button, true, pos);
    }

    /// Move the pointer with a real (tagged) mouse-moved event so apps under
    /// the cursor receive the move, not just a silent warp.
    fn send_mouse_move(x: f64, y: f64) {
        use core_graphics::event::CGMouseButton;
        let Some(src) = new_source() else { return };
        let point = core_graphics::geometry::CGPoint::new(x, y);
        if let Ok(ev) =
            CGEvent::new_mouse_event(src, CGEventType::MouseMoved, point, CGMouseButton::Left)
        {
            ev.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INJECTED_EVENT_MAGIC);
            ev.post(CGEventTapLocation::HID);
        }
    }

    /// Vertical scroll — Windows wheel deltas are ±120 per notch; mac line
    /// units are small integers, so translate notches → ±3 lines.
    fn send_scroll(delta: i32) {
        use core_graphics::event::ScrollEventUnit;
        let Some(src) = new_source() else { return };
        let lines = (delta / 120).clamp(-10, 10) * 3;
        if let Ok(ev) = CGEvent::new_scroll_event(src, ScrollEventUnit::LINE, 1, lines, 0, 0) {
            ev.set_integer_value_field(EventField::EVENT_SOURCE_USER_DATA, INJECTED_EVENT_MAGIC);
            ev.post(CGEventTapLocation::HID);
        }
    }

    // ── Bare-key remap (AHK-style passthrough) ──────────────────────────────

    /// trigger keycode → (target keycode, modifier keycodes) for remaps whose
    /// down phase has fired and whose up phase waits on the trigger's keyup.
    static ACTIVE_BARE_REMAPS: LazyLock<Mutex<std::collections::HashMap<u16, (u16, Vec<u16>)>>> =
        LazyLock::new(|| Mutex::new(std::collections::HashMap::new()));

    pub(super) fn remap_key_press(trigger_kc: u16, data: &Value) -> bool {
        let key_name = match data.get("key").and_then(|v| v.as_str()) {
            Some(k) if !k.is_empty() => k,
            _ => return false,
        };
        if is_mouse_button(key_name) {
            return false;
        }
        if data.get("holdMode").and_then(|v| v.as_bool()).unwrap_or(false) {
            return false;
        }
        if data.get("repeatMode").and_then(|v| v.as_bool()).unwrap_or(false) {
            return false;
        }
        let Some(target_kc) = crate::hotkeys::display_name_to_keycode(key_name) else {
            return false;
        };
        let mod_kcs: Vec<u16> = data
            .get("modifiers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| match v.as_str()?.to_lowercase().as_str() {
                        "ctrl" | "win" => Some(KC_LCMD),
                        "alt" => Some(KC_LOPTION),
                        "shift" => Some(KC_LSHIFT),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Record the active remap so keyup knows what to release.
        // Overwriting on a repeat keydown is intentional and idempotent.
        ACTIVE_BARE_REMAPS
            .lock()
            .unwrap()
            .insert(trigger_kc, (target_kc, mod_kcs.clone()));

        // Mods down + target down — keyup comes from remap_key_release.
        let mut flags = CGEventFlags::CGEventFlagNull;
        for &kc in &mod_kcs {
            if let Some(f) = modifier_flag(kc) {
                flags.insert(f);
            }
            post_key(kc, false, flags);
        }
        post_key(target_kc, false, flags);
        true
    }

    pub(super) fn remap_key_release(trigger_kc: u16) -> bool {
        let entry = ACTIVE_BARE_REMAPS.lock().unwrap().remove(&trigger_kc);
        if let Some((target_kc, mod_kcs)) = entry {
            let mut flags = mod_kcs
                .iter()
                .filter_map(|&kc| modifier_flag(kc))
                .fold(CGEventFlags::CGEventFlagNull, |acc, f| acc | f);
            post_key(target_kc, true, flags);
            for &kc in mod_kcs.iter().rev() {
                if let Some(f) = modifier_flag(kc) {
                    flags.remove(f);
                }
                post_key(kc, true, flags);
            }
            true
        } else {
            false
        }
    }

    /// Inline Send Hotkey fire (keydown path). Normal mode only — mouse,
    /// hold, repeat and unknown keys return false for the deferred route.
    pub(super) fn execute_hotkey_inline(data: &Value) -> bool {
        let key_name = data.get("key").and_then(|v| v.as_str()).unwrap_or("");
        if is_mouse_button(key_name) {
            return false;
        }
        if data.get("holdMode").and_then(|v| v.as_bool()).unwrap_or(false) {
            return false;
        }
        if data.get("repeatMode").and_then(|v| v.as_bool()).unwrap_or(false) {
            return false;
        }
        let mod_kcs: Vec<u16> = data
            .get("modifiers")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| match v.as_str()?.to_lowercase().as_str() {
                        "ctrl" | "win" => Some(KC_LCMD),
                        "alt" => Some(KC_LOPTION),
                        "shift" => Some(KC_LSHIFT),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let target = if key_name.is_empty() {
            if mod_kcs.is_empty() {
                return false;
            }
            None
        } else {
            match crate::hotkeys::display_name_to_keycode(key_name) {
                Some(kc) => Some(kc),
                None => return false,
            }
        };
        let held = release_held_modifiers();
        post_chord(&mod_kcs, target);
        restore_modifiers(&held);
        true
    }

    // ── Send Hotkey hold / repeat state (twin of the Windows managers) ──────

    struct HeldKeyState {
        target_kc: u16,
        mod_kcs: Vec<u16>,
        mouse_button: Option<String>,
        label: String,
        /// e.g. "MOUSE_RIGHT" — when set, the hold releases on that button's
        /// mouse-up (press-hold mirroring) instead of by re-fire toggle.
        trigger_mouse_id: Option<String>,
    }

    /// `pending_mouse_release` handles the race where handle_mouse_up fires
    /// before the hold thread has stored the held state: the mouse-up records
    /// the button here; the hold setup checks it under the same lock and
    /// immediately releases — no timing assumptions (same design as Windows).
    struct HeldKeyManager {
        key: Option<HeldKeyState>,
        pending_mouse_release: Option<String>,
    }

    static HELD_KEY: Mutex<HeldKeyManager> = Mutex::new(HeldKeyManager {
        key: None,
        pending_mouse_release: None,
    });

    struct RepeatingKeyState {
        trigger_storage_key: String,
        label: String,
        stop: std::sync::Arc<super::AtomicBool>,
    }

    static REPEATING_KEY: Mutex<Option<RepeatingKeyState>> = Mutex::new(None);

    /// Release a held chord: main key up, then modifiers in reverse. The
    /// events carry the chord's flags so the release reads coherently.
    fn post_hold_release(state: &HeldKeyState) {
        if let Some(ref button) = state.mouse_button {
            send_mouse_event(button, true);
            return;
        }
        let mut flags = state
            .mod_kcs
            .iter()
            .filter_map(|&kc| modifier_flag(kc))
            .fold(CGEventFlags::CGEventFlagNull, |acc, f| acc | f);
        if state.target_kc != 0 {
            post_key(state.target_kc, true, flags);
        }
        for &kc in state.mod_kcs.iter().rev() {
            if let Some(f) = modifier_flag(kc) {
                flags.remove(f);
            }
            post_key(kc, true, flags);
        }
    }

    pub(super) fn release_held_key() -> Option<String> {
        let mut mgr = HELD_KEY.lock().unwrap();
        if let Some(state) = mgr.key.take() {
            mgr.pending_mouse_release = None; // no longer relevant
            post_hold_release(&state);
            info!("[Keyfire] Released held key: {}", state.label);
            Some(state.label)
        } else {
            None
        }
    }

    pub(super) fn is_key_held() -> bool {
        HELD_KEY.lock().unwrap().key.is_some()
    }

    /// Release the held key only if it was triggered by the given mouse
    /// button (press-hold mirroring: hold while the button is down, release
    /// on its up). `allow_pending` records the up for the fast-click race —
    /// only for buttons that actually carry a hold assignment, so ordinary
    /// clicks don't clobber the slot.
    pub(super) fn release_held_if_mouse_trigger(
        mouse_id: &str,
        allow_pending: bool,
    ) -> Option<String> {
        let mut mgr = HELD_KEY.lock().unwrap();
        let matches = mgr
            .key
            .as_ref()
            .and_then(|s| s.trigger_mouse_id.as_deref())
            .is_some_and(|t| t == mouse_id);
        if matches {
            let state = mgr.key.take().unwrap();
            mgr.pending_mouse_release = None;
            post_hold_release(&state);
            Some(state.label)
        } else {
            if allow_pending {
                mgr.pending_mouse_release = Some(mouse_id.to_string());
            }
            None
        }
    }

    pub(super) fn clear_pending_mouse_release(mouse_id: &str) {
        let mut mgr = HELD_KEY.lock().unwrap();
        if mgr.pending_mouse_release.as_deref() == Some(mouse_id) {
            mgr.pending_mouse_release = None;
        }
    }

    pub(super) fn stop_repeating_key() -> Option<String> {
        let mut rep = REPEATING_KEY.lock().unwrap();
        if let Some(state) = rep.take() {
            state.stop.store(true, Ordering::SeqCst);
            info!("[Keyfire] Repeat stopped: {}", state.label);
            Some(state.label)
        } else {
            None
        }
    }

    pub(super) fn is_repeating() -> bool {
        REPEATING_KEY.lock().unwrap().is_some()
    }

    pub(super) fn get_repeating_trigger() -> Option<String> {
        REPEATING_KEY
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.trigger_storage_key.clone())
    }

    // Silence unused warnings for items reserved for later milestones.
    #[allow(unused)]
    fn _reserved() {
        let _ = CGEventType::Null;
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn vk_translation_covers_shared_callers() {
            // The exact VKs lib.rs sends today (paste + cursor-back paths).
            assert_eq!(vk_to_mac_keycode(0x56), Some(KC_V)); // V
            assert_eq!(vk_to_mac_keycode(0xA2), Some(KC_LCMD)); // LCtrl → ⌘ (accelerator)
            assert_eq!(vk_to_mac_keycode(0x25), Some(123)); // Left arrow
            assert_eq!(vk_to_mac_keycode(0x5B), Some(KC_LCMD)); // LWin → ⌘
            assert_eq!(vk_to_mac_keycode(0xFF), None); // unmapped drops, no panic
        }

        #[test]
        fn modifier_flags_map_all_tracked_keycodes() {
            for &kc in MAC_MODIFIER_KEYCODES {
                assert!(modifier_flag(kc).is_some(), "keycode {} missing flag", kc);
            }
            assert!(modifier_flag(KC_V).is_none());
        }

        #[test]
        fn self_change_count_consumed_once() {
            if let Ok(mut q) = SELF_CHANGE_COUNTS.lock() {
                q.clear();
            }
            record_self_change_count();
            let count = change_count();
            assert!(is_self_change_count(count), "recorded count should match");
            assert!(
                !is_self_change_count(count),
                "match must be consumed after first hit"
            );
        }

        /// Round-trips the real general pasteboard. Saves and restores the
        /// user's clipboard text; text-only, same limitation as the M2 engine.
        #[test]
        fn clipboard_write_read_roundtrip() {
            let _pb = super::super::PASTEBOARD_TEST_LOCK.lock().unwrap();
            let prev = read_clipboard();
            let probe = "keyfire-m2-clipboard-probe";

            assert!(write_clipboard_impl(probe, true));
            assert_eq!(read_clipboard().as_deref(), Some(probe));
            // The suppress path must have recorded our own changeCount.
            assert!(is_self_change_count(change_count()));
            assert!(SUPPRESS_NEXT_CLIPBOARD_WRITE.load(Ordering::SeqCst));
            SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);

            if let Some(p) = prev {
                write_clipboard_impl(&p, true);
                assert_eq!(read_clipboard(), Some(p));
            }
        }
    }
}
