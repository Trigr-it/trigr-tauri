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

pub fn release_held_key() -> Option<String> {
    // Hold/repeat key state machine arrives with the hotkey matcher milestone.
    None
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

pub fn set_foreground_robust(hwnd: isize) -> bool {
    // HWNDs don't exist on macOS; focus follows the frontmost app, which the
    // overlay windows don't steal (they are non-activating panels). The
    // NSWorkspace-based foreground module (milestone 6) may add a real
    // activate-by-pid here if a use case appears.
    false
}

pub fn stop_repeating_key() -> Option<String> {
    None
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
                    execute_send_hotkey(d);
                }
            }

            other => {
                warn!(
                    "[Keyfire] action type [{}] is not implemented on macOS yet — skipping \"{}\"",
                    other, label
                );
            }
        }

        // `app` is unused until actions emit UI events (toasts, loop badges).
        let _ = app;
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
    fn type_text_direct(text: &str) {
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
    /// Milestone-2 scope: the snapshot is text-only. The Windows original
    /// snapshots every clipboard format so images/RTF survive an expansion;
    /// the multi-format NSPasteboard snapshot lands with the clipboard-history
    /// milestone. A non-text clipboard (e.g. a copied image) is therefore not
    /// restored yet — the expansion text is left on the pasteboard instead.
    fn inject_via_clipboard(text: &str) {
        let prev = read_clipboard();
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
            match &prev {
                Some(p) => {
                    write_clipboard_impl(p, true);
                }
                // Nothing textual to restore (empty or non-text clipboard) —
                // leave our text in place until the multi-format snapshot lands.
                None => {}
            }
        }
        SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
    }

    /// Send Hotkey action — the plain path: press the modifier chord, tap the
    /// main key (15ms down→up so per-frame pollers see it), release in
    /// reverse. Bare-modifier chords (key="", modifiers only) press and
    /// release just the chord. holdMode / repeatMode / mouse buttons are
    /// later milestones. Modifier tokens use accelerator semantics, matching
    /// send_vk_key_pub: "ctrl" and "win" both mean ⌘ on macOS ("Ctrl+C"
    /// authored on Windows should copy on a Mac), "alt" is ⌥, "shift" ⇧.
    fn execute_send_hotkey(data: &Value) {
        let key_name = data.get("key").and_then(|v| v.as_str()).unwrap_or("");
        let hold_mode = data.get("holdMode").and_then(|v| v.as_bool()).unwrap_or(false);
        let repeat_mode = data.get("repeatMode").and_then(|v| v.as_bool()).unwrap_or(false);
        if hold_mode || repeat_mode {
            warn!("[Keyfire] Send Hotkey hold/repeat modes are not implemented on macOS yet");
            return;
        }
        if key_name.starts_with("MOUSE_") {
            warn!("[Keyfire] Send Hotkey mouse buttons are not implemented on macOS yet");
            return;
        }

        let mod_keycodes: Vec<u16> = data
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

        // Release the physically held trigger modifiers (e.g. ⌘ from a ⌘K
        // trigger) so the target app sees ONLY the chord we send.
        let held = release_held_modifiers();

        let mut flags = CGEventFlags::CGEventFlagNull;
        for &kc in &mod_keycodes {
            if let Some(f) = modifier_flag(kc) {
                flags.insert(f);
            }
            post_key(kc, false, flags);
        }
        if let Some(kc) = target {
            post_key(kc, false, flags);
            // Hold between down and up so per-frame key-state pollers (games)
            // observe the press — mirrors the Windows KEY_HOLD_MS invariant.
            thread::sleep(Duration::from_millis(15));
            post_key(kc, true, flags);
        }
        for &kc in mod_keycodes.iter().rev() {
            if let Some(f) = modifier_flag(kc) {
                flags.remove(f);
            }
            post_key(kc, true, flags);
        }

        restore_modifiers(&held);
    }

    /// Post the ⌘V chord (all four events tagged; ⌘ carried on the V events'
    /// flags as well, since synthetic modifiers don't latch).
    fn paste_cmd_v() {
        post_key(KC_LCMD, false, CGEventFlags::CGEventFlagCommand);
        post_key(KC_V, false, CGEventFlags::CGEventFlagCommand);
        post_key(KC_V, true, CGEventFlags::CGEventFlagCommand);
        post_key(KC_LCMD, true, CGEventFlags::CGEventFlagNull);
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
