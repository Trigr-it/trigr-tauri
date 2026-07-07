//! Non-Windows hotkey engine. The real hotkeys.rs is built on Win32 low-level
//! hooks and only compiles on Windows. This twin exposes the exact surface
//! lib.rs and shared modules reference, so the app builds and boots on other
//! platforms.
//!
//! Mac port Phase 2 (`port/mac-hooks`):
//!   * M1 — listen-only CGEventTap: hook thread owns the tap + CFRunLoop, a
//!     processor thread does all logging/state (tap callback never blocks,
//!     logs, or does I/O — macOS disables taps that stall).
//!   * M3 (this milestone) — the tap is now ACTIVE (suppressing) and the
//!     processor matches and fires hotkeys:
//!       - a precomputed suppress set (modifier-bits, mac keycode) is
//!         consulted in the tap callback via a non-blocking try_read — the
//!         same design as the Windows hook's suppress_keys();
//!       - the processor resolves keycodes to the cross-platform key_id
//!         strings ("KeyV", "Digit1", …) used by stored assignments, builds
//!         `profile::Combo::KeyId` storage keys, and fires via
//!         actions::execute_action on a spawned thread;
//!       - special hotkeys handled: overlay toggle, global pause toggle,
//!         clipboard quick-paste; plus the hotkey capture/recording flows
//!         the UI uses to assign keys;
//!       - non-modified ("bare") keys, ::double and ::hold variants, the
//!         expansion buffer, voice, radial menu and the macro recorder are
//!         LATER milestones — their keys are deliberately NOT suppressed so
//!         they keep working in the target app instead of dying on a
//!         matcher that can't fire them.
//!     Storage stays cross-platform (hard rule 6): the 'Win' modifier token
//!     means Meta, which is ⌘ Command on macOS; 'Ctrl' means macOS Control.
//!     If tap creation with suppression rights fails (Accessibility not yet
//!     granted), the engine falls back to a listen-only tap so key logging
//!     and the status dot still work; matching then observes but cannot
//!     suppress, so it stays disabled until rights arrive (clear log lines
//!     explain which mode is live).
//!
//! On non-macOS non-Windows targets (e.g. Linux CI) `start_hooks` stays a
//! no-op.
#![allow(dead_code, unused_variables, unused_imports)]

use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

pub static CLIPBOARD_OVERLAY_FOR_FILLIN: AtomicBool = AtomicBool::new(false);
pub static CLIPBOARD_OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);
pub static CLIPBOARD_OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);
pub static FILLIN_HWND: AtomicIsize = AtomicIsize::new(0);
pub static SEARCH_OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);

/// True once the CGEventTap is installed and its run loop is pumping. Read by
/// `get_engine_status` so the UI status dot goes live (mirror of the Windows
/// `HOOKS_RUNNING`). Cross-thread, so `Ordering::SeqCst` per the hard rules.
static HOOKS_RUNNING: AtomicBool = AtomicBool::new(false);

/// Engine pause state (mirror of the Windows MACROS_ENABLED — pub because
/// tray.rs reads and toggles it, same as on Windows). Toggled by the pause
/// hotkey and `set_macros_enabled`; gates suppression AND dispatch.
pub static MACROS_ENABLED: AtomicBool = AtomicBool::new(true);

/// True while a Keyfire UI input field has focus — normal hotkey dispatch is
/// suppressed so typing in the app doesn't fire macros (capture flows still
/// run; they sit above this gate, exactly like Windows).
static APP_INPUT_FOCUSED: AtomicBool = AtomicBool::new(false);

/// True while the expansions engine is injecting (backspaces + paste). Set by
/// expansions::InjectionGuard; fire paths serialise on it. Mirror of the
/// Windows static of the same name — but note there is no injection replay
/// buffer on macOS (tagged events replace SUPPRESS_SIMULATED, see
/// stubs/expansions.rs module docs).
pub static INJECTION_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// True while the fill-in / variant-picker window flow is active. Gates
/// re-entrant fill-in invocations (mirror of the Windows FILL_IN_ACTIVE).
pub static FILL_IN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Millisecond timestamp of the current injection's start (0 = none). The
/// Windows build runs a watchdog thread against this; on mac the bounded
/// wait in expansions::wait_for_injection_clear plays that role — these
/// fns keep the guard's call shape identical.
static INJECTION_STARTED_AT_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

pub fn mark_injection_start() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    INJECTION_STARTED_AT_MS.store(now, Ordering::SeqCst);
}

pub fn clear_injection_start() {
    INJECTION_STARTED_AT_MS.store(0, Ordering::SeqCst);
}

// ── Wait for Input infrastructure (twin of the Windows one) ────────────────

/// Events forwarded to a Wait for Input waiter. Mouse variants exist for
/// signature parity — the mac engine has no mouse tap yet, so only key
/// events are ever forwarded.
#[derive(Debug, Clone)]
pub enum WaitEvent {
    KeyDown { key_id: String },
    KeyUp { key_id: String },
    MouseDown { button_name: String },
    MouseUp { button_name: String },
}

/// One-shot channel for the Wait for Input step. Set by actions.rs, read by
/// the event processor.
static WAIT_FOR_INPUT_TX: OnceLock<Mutex<Option<std::sync::mpsc::Sender<WaitEvent>>>> =
    OnceLock::new();

fn wait_tx() -> &'static Mutex<Option<std::sync::mpsc::Sender<WaitEvent>>> {
    WAIT_FOR_INPUT_TX.get_or_init(|| Mutex::new(None))
}

/// Register a waiter channel. Returns the receiver. Called from actions.rs.
pub fn register_wait_for_input() -> std::sync::mpsc::Receiver<WaitEvent> {
    let (tx, rx) = std::sync::mpsc::channel();
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

/// Key capture mode (settings "press a key" fields). One-shot: cleared by the
/// processor when a combo is captured and emitted as `key-captured`.
static IS_CAPTURING_KEY: AtomicBool = AtomicBool::new(false);

/// Hotkey recording mode (assignment editor). One-shot: cleared when a combo
/// is recorded and emitted as `hotkey-recorded`.
static IS_RECORDING_HOTKEY: AtomicBool = AtomicBool::new(false);

/// Live modifier state, updated by the processor thread from CGEvent flags.
/// The processor decides; the tap callback only forwards raw flag bits. These
/// mirror the Windows `MOD_*` atomics and back the hotkey matcher.
static MOD_CTRL: AtomicBool = AtomicBool::new(false);
static MOD_SHIFT: AtomicBool = AtomicBool::new(false);
static MOD_ALT: AtomicBool = AtomicBool::new(false);
static MOD_META: AtomicBool = AtomicBool::new(false);

fn modifier_bits() -> u8 {
    let mut bits = 0u8;
    if MOD_CTRL.load(Ordering::SeqCst) {
        bits |= 1;
    }
    if MOD_SHIFT.load(Ordering::SeqCst) {
        bits |= 2;
    }
    if MOD_ALT.load(Ordering::SeqCst) {
        bits |= 4;
    }
    if MOD_META.load(Ordering::SeqCst) {
        bits |= 8;
    }
    bits
}

/// Mirror of the pub(crate) surface of the real EngineState. Private fields
/// of the Windows original are omitted -- lib.rs cannot touch them anyway.
/// The pending_* / capture fields back the mac matcher (same names and
/// semantics as the Windows original).
pub(crate) struct EngineState {
    pub(crate) active_profile: String,
    pub(crate) assignments: HashMap<String, Value>,
    pub(crate) profile_settings: HashMap<String, Value>,
    pub(crate) overlay_hotkey: Option<(u8, u32)>,
    pub(crate) pause_hotkey: Option<(u8, u32)>,
    pub(crate) pause_hotkey_str: Option<String>,
    pub(crate) global_input_method: String,
    pub(crate) macro_speed: String,
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
    // mac matcher state (stub-internal)
    pub(crate) double_tap_window_ms: u64,
    pub(crate) hold_threshold_ms: u64,
    pub(crate) capture_sole_modifier: Option<String>,
    pub(crate) pending_macro: Option<Value>,
    /// Set → keyup routes through dispatch_with_double_tap (double-tap
    /// resolution at keyup); None → fire directly (double already resolved
    /// at keydown, or no double variant).
    pub(crate) pending_storage_key: Option<String>,
    pub(crate) pending_trigger_key: Option<String>,
    pub(crate) pending_is_bare: bool,
    /// Double-tap tracking (twin of the Windows fields).
    pub(crate) last_hotkey_time: HashMap<String, std::time::Instant>,
    pub(crate) pending_single_cancel: HashMap<String, std::sync::Arc<AtomicBool>>,
}

impl Default for EngineState {
    fn default() -> Self {
        // Defaults mirror the Windows EngineState::default() values. The key
        // part of hotkey tuples is a mac keycode on macOS (Space=49, V=9) and
        // a Windows VK elsewhere — config-load re-parses real values at boot.
        Self {
            active_profile: "Default".to_string(),
            assignments: HashMap::new(),
            profile_settings: HashMap::new(),
            overlay_hotkey: Some((1, if cfg!(target_os = "macos") { 49 } else { 0x20 })),
            pause_hotkey: None,
            pause_hotkey_str: None,
            global_input_method: "direct".to_string(),
            macro_speed: "safe".to_string(),
            custom_keystroke_delay: 30,
            custom_pre_execution_delay: 150,
            clipboard_paste_hotkey: Some((3, if cfg!(target_os = "macos") { 9 } else { 0x56 })),
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
            double_tap_window_ms: 300,
            hold_threshold_ms: 350,
            capture_sole_modifier: None,
            pending_macro: None,
            pending_storage_key: None,
            pending_trigger_key: None,
            pending_is_bare: false,
            last_hotkey_time: HashMap::new(),
            pending_single_cancel: HashMap::new(),
        }
    }
}

/// Pauses hold-threshold detection (armed timers stop firing; new holds
/// don't arm). Internal to the matcher — same name as the Windows static.
pub static HOLD_DETECTION_PAUSED: AtomicBool = AtomicBool::new(false);

/// True while a linked profile's app is frontmost (written by the foreground
/// watcher, read by the tap callback to gate bare-mouse suppression — the
/// mac stand-in for the Windows is_cursor_over_linked_app hook check; on
/// macOS a click activates the app underneath, so frontmost ≈ under-cursor
/// within the watcher's poll window).
pub static LINKED_APP_FRONTMOST: AtomicBool = AtomicBool::new(false);

static ENGINE_STATE: OnceLock<Mutex<EngineState>> = OnceLock::new();

pub(crate) fn engine_state() -> &'static Mutex<EngineState> {
    ENGINE_STATE.get_or_init(|| Mutex::new(EngineState::default()))
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

pub fn start_hooks(app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        macos::start_hooks(app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        log::warn!("[stub] global input hooks are not available on this platform yet");
    }
}

/// After any change to assignments / profile / special hotkeys, recompute the
/// suppress set the tap callback consults. No-op off macOS.
fn rebuild_suppress(state: &EngineState) {
    #[cfg(target_os = "macos")]
    {
        macos::rebuild_suppress_keys(state);
    }
}

// ── macOS CGEventTap engine ──────────────────────────────────────────────────
#[cfg(target_os = "macos")]
mod macos {
    use super::{
        engine_state, modifier_bits, EngineState, APP_INPUT_FOCUSED, HOOKS_RUNNING,
        IS_CAPTURING_KEY, IS_RECORDING_HOTKEY, MACROS_ENABLED, MOD_ALT, MOD_CTRL, MOD_META,
        MOD_SHIFT,
    };
    use core_foundation::base::TCFType;
    use core_foundation::mach_port::CFMachPortRef;
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, CallbackResult, EventField,
    };
    use log::{info, warn};
    use serde_json::Value;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
    use std::sync::{mpsc, OnceLock, RwLock};
    use std::thread;
    use std::time::Duration;
    use tauri::{AppHandle, Emitter};

    /// Raw CFMachPortRef of the live tap, stashed so the tap callback can
    /// re-enable itself after the OS disables it (see `TapDisabledByTimeout`).
    /// We can't borrow the `CGEventTap` from inside its own callback, so we go
    /// through the raw port + `CGEventTapEnable`. `isize` because pointers
    /// aren't `Sync`; 0 means "no tap yet".
    static TAP_PORT: AtomicIsize = AtomicIsize::new(0);

    /// True when the live tap was created with suppression rights
    /// (CGEventTapOptions::Default). False = listen-only fallback: matching
    /// is disabled so half-working hotkeys can't eat keystrokes.
    static TAP_CAN_SUPPRESS: AtomicBool = AtomicBool::new(false);

    // `CGEventTapEnable` is not re-exported by core-graphics. Bind it directly;
    // the CoreGraphics framework is already linked by the crate.
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    }

    // Layout-aware key→character translation for the expansion buffer.
    // CGEventKeyboardGetUnicodeString looked like the natural source but
    // IGNORES the event's modifier flags (verified empirically: keycode 40
    // with maskShift still returns "k") — smart case and shift-triggers
    // (":KR", "?help") would silently break. UCKeyTranslate with an explicit
    // modifier state is deterministic for both real and synthetic events —
    // the mac twin of the Windows resolve_char_with_shift/ToUnicode path.
    //
    // THREADING: the TIS* calls dispatch-assert the MAIN queue on modern
    // macOS (verified: SIGTRAP in dispatch_assert_queue from the processor
    // thread, 2026-07-06). The 'uchr' layout bytes are therefore cached ONCE
    // on the main thread at start_hooks time; UCKeyTranslate itself is a
    // pure function over those bytes and safe anywhere. Caveat: switching
    // keyboard layouts needs an app restart to pick up — same fidelity
    // class as the US-ANSI keycode↔key_id table above, fixed together by a
    // future layout-aware pass.
    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn TISCopyCurrentKeyboardLayoutInputSource() -> *mut std::ffi::c_void;
        fn TISGetInputSourceProperty(
            source: *mut std::ffi::c_void,
            key: *const std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
        static kTISPropertyUnicodeKeyLayoutData: *const std::ffi::c_void;
        fn UCKeyTranslate(
            layout: *const u8,
            keycode: u16,
            action: u16,
            modifier_key_state: u32,
            keyboard_type: u32,
            options: u32,
            dead_key_state: *mut u32,
            max_len: usize,
            actual_len: *mut usize,
            output: *mut u16,
        ) -> i32;
        fn LMGetKbdType() -> u8;
        // CoreFoundation symbols resolve from the already-linked framework.
        fn CFDataGetBytePtr(data: *const std::ffi::c_void) -> *const u8;
        fn CFDataGetLength(data: *const std::ffi::c_void) -> isize;
        fn CFRelease(cf: *const std::ffi::c_void);
    }

    struct KeyboardLayout {
        uchr: Vec<u8>,
        kbd_type: u32,
    }

    fn keyboard_layout_cache() -> &'static RwLock<Option<KeyboardLayout>> {
        static CACHE: OnceLock<RwLock<Option<KeyboardLayout>>> = OnceLock::new();
        CACHE.get_or_init(|| RwLock::new(None))
    }

    /// Copy the current keyboard layout's 'uchr' data into the cache.
    /// MUST run on the main thread — see the threading note above.
    fn cache_keyboard_layout() {
        unsafe {
            let src = TISCopyCurrentKeyboardLayoutInputSource();
            if src.is_null() {
                warn!("[HOOK] TIS keyboard layout unavailable — expansion buffer disabled");
                return;
            }
            let data = TISGetInputSourceProperty(src, kTISPropertyUnicodeKeyLayoutData);
            if data.is_null() {
                // Non-'uchr' input source (rare) — no translation available.
                warn!("[HOOK] keyboard layout has no uchr data — expansion buffer disabled");
            } else {
                let len = CFDataGetLength(data) as usize;
                let bytes = std::slice::from_raw_parts(CFDataGetBytePtr(data), len).to_vec();
                let kbd_type = LMGetKbdType() as u32;
                if let Ok(mut w) = keyboard_layout_cache().write() {
                    *w = Some(KeyboardLayout { uchr: bytes, kbd_type });
                }
                info!("[HOOK] keyboard layout cached ({} bytes, kbdType {})", len, kbd_type);
            }
            CFRelease(src);
        }
    }

    /// Character this keycode would type under the cached keyboard layout
    /// with the given event flags (Shift/Option/CapsLock participate; Ctrl
    /// and Cmd chords never reach the buffer path). None for non-printing
    /// keys or multi-unit output (dead keys are bypassed via
    /// kUCKeyTranslateNoDeadKeysMask, same caveat class as Windows).
    fn resolve_typed_char(keycode: u16, flags: u64) -> Option<char> {
        const K_UC_KEY_ACTION_DISPLAY: u16 = 3;
        const K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK: u32 = 1;
        // Carbon event modifiers: shiftKey=0x0200, alphaLock=0x0400,
        // optionKey=0x0800. UCKeyTranslate wants (carbonMods >> 8) & 0xFF.
        let f = CGEventFlags::from_bits_truncate(flags);
        let mut carbon: u32 = 0;
        if f.contains(CGEventFlags::CGEventFlagShift) {
            carbon |= 0x0200;
        }
        if f.contains(CGEventFlags::CGEventFlagAlphaShift) {
            carbon |= 0x0400;
        }
        if f.contains(CGEventFlags::CGEventFlagAlternate) {
            carbon |= 0x0800;
        }
        let mod_state = (carbon >> 8) & 0xFF;

        let cache = keyboard_layout_cache().read().ok()?;
        let layout = cache.as_ref()?;
        let mut dead: u32 = 0;
        let mut buf = [0u16; 4];
        let mut len: usize = 0;
        let status = unsafe {
            UCKeyTranslate(
                layout.uchr.as_ptr(),
                keycode,
                K_UC_KEY_ACTION_DISPLAY,
                mod_state,
                layout.kbd_type,
                K_UC_KEY_TRANSLATE_NO_DEAD_KEYS_MASK,
                &mut dead,
                buf.len(),
                &mut len,
                buf.as_mut_ptr(),
            )
        };
        if status != 0 || len == 0 {
            return None;
        }
        let mut iter = char::decode_utf16(buf[..len].iter().copied());
        match (iter.next(), iter.next()) {
            (Some(Ok(c)), None) => Some(c),
            _ => None, // multi-char output — not buffer material
        }
    }

    /// Suppress set consulted by the tap callback: (modifier_bits, mac
    /// keycode) of every combo Keyfire will handle. Rebuilt on any
    /// assignment/profile/special-hotkey change. try_read in the callback —
    /// on write contention the event passes through (one leaked keystroke
    /// beats a stalled tap).
    fn suppress_keys() -> &'static RwLock<HashSet<(u8, u16)>> {
        static SET: OnceLock<RwLock<HashSet<(u8, u16)>>> = OnceLock::new();
        SET.get_or_init(|| RwLock::new(HashSet::new()))
    }

    // ── Mouse buttons ────────────────────────────────────────────────────────

    /// Buttons the engine handles. Side1/Side2 arrive as OtherMouse events
    /// with button numbers 3/4 (Middle is 2).
    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub(super) enum MacMouseButton {
        Left,
        Right,
        Middle,
        Side1,
        Side2,
    }

    pub(super) fn mouse_button_to_key_id(button: MacMouseButton) -> &'static str {
        match button {
            MacMouseButton::Left => "MOUSE_LEFT",
            MacMouseButton::Right => "MOUSE_RIGHT",
            MacMouseButton::Middle => "MOUSE_MIDDLE",
            MacMouseButton::Side1 => "MOUSE_SIDE1",
            MacMouseButton::Side2 => "MOUSE_SIDE2",
        }
    }

    fn mouse_key_id_to_button(key_id: &str) -> Option<MacMouseButton> {
        Some(match key_id {
            "MOUSE_LEFT" => MacMouseButton::Left,
            "MOUSE_RIGHT" => MacMouseButton::Right,
            "MOUSE_MIDDLE" => MacMouseButton::Middle,
            "MOUSE_SIDE1" => MacMouseButton::Side1,
            "MOUSE_SIDE2" => MacMouseButton::Side2,
            // Scroll triggers are deferred on macOS — trackpad momentum
            // generates continuous scroll streams a suppress set can't
            // sanely gate. Buttons only.
            _ => return None,
        })
    }

    /// Bit for the down/up pairing mask below.
    fn mouse_button_bit(button: MacMouseButton) -> u8 {
        match button {
            MacMouseButton::Left => 1,
            MacMouseButton::Right => 2,
            MacMouseButton::Middle => 4,
            MacMouseButton::Side1 => 8,
            MacMouseButton::Side2 => 16,
        }
    }

    /// Bare mouse buttons the active (linked) profile has assignments for.
    /// Consulted in the tap callback alongside LINKED_APP_FRONTMOST.
    fn suppress_bare_mouse() -> &'static RwLock<HashSet<MacMouseButton>> {
        static SET: OnceLock<RwLock<HashSet<MacMouseButton>>> = OnceLock::new();
        SET.get_or_init(|| RwLock::new(HashSet::new()))
    }

    /// Which buttons had their DOWN suppressed — only the matching UP is
    /// then suppressed, so a suppress-set change mid-click can't leave the
    /// OS with a mismatched down/up pair (same pairing rule as Windows).
    static MOUSE_DOWN_SUPPRESSED: std::sync::atomic::AtomicU8 =
        std::sync::atomic::AtomicU8::new(0);

    /// Lightweight message from the tap callback (hook thread) to the processor
    /// thread. `Copy`, no allocation — cheap to send from the callback.
    #[derive(Clone, Copy)]
    enum TapEvent {
        KeyDown {
            keycode: i64,
            flags: u64,
            is_repeat: bool,
        },
        KeyUp {
            keycode: i64,
            flags: u64,
        },
        FlagsChanged {
            keycode: i64,
            flags: u64,
        },
        MouseDown {
            button: MacMouseButton,
            flags: u64,
            x: f64,
            y: f64,
        },
        MouseUp {
            button: MacMouseButton,
            x: f64,
            y: f64,
        },
        /// Pointer motion / drag — sent ONLY while a macro recording is
        /// active (the callback fast-paths moves otherwise).
        MouseMoved {
            x: f64,
            y: f64,
        },
        /// Scroll — sent ONLY while a macro recording is active. `delta` is
        /// in Windows wheel units (±120/notch) so recorded streams stay
        /// cross-platform.
        Wheel {
            delta: i32,
            x: f64,
            y: f64,
        },
        /// The record hotkey fired while a recording was live — the callback
        /// already suppressed it and flipped IS_RECORDING_MACRO false.
        RecorderStop,
        /// Quick Record / Replay / Loop global hotkeys (suppressed in the
        /// callback so they never leak to the target app).
        TempRecord,
        TempPlay,
        TempLoop,
        /// OS disabled the tap. The callback already re-enabled it; this is
        /// just so the processor logs. `by_user_input` distinguishes the two
        /// causes: `true` = kCGEventTapDisabledByUserInput (Secure Input mode,
        /// e.g. a terminal with Secure Keyboard Entry or a password field —
        /// key events are withheld from all taps), `false` =
        /// kCGEventTapDisabledByTimeout (our callback stalled).
        Disabled {
            by_user_input: bool,
        },
    }

    pub fn start_hooks(app: AppHandle) {
        if HOOKS_RUNNING.load(Ordering::SeqCst) {
            return;
        }

        // Cache the keyboard layout while we're still on the main thread
        // (start_hooks is called from tauri's setup) — TIS asserts the main
        // queue, and the processor thread needs the bytes for char resolve.
        cache_keyboard_layout();

        // Hold-trigger watcher (16ms tick; fires ::hold macros at threshold).
        spawn_hold_watcher(app.clone());

        let (sender, receiver) = mpsc::channel::<TapEvent>();

        // Processor thread: drains the channel, tracks modifier state, matches
        // and fires hotkeys. All I/O lives here, never in the tap callback.
        let proc_app = app.clone();
        thread::Builder::new()
            .name("keyfire-tap-processor".to_string())
            .spawn(move || process_events(receiver, proc_app))
            .expect("failed to spawn tap processor thread");

        // Hook thread: owns the tap and pumps its CFRunLoop forever.
        thread::Builder::new()
            .name("keyfire-tap-hook".to_string())
            .spawn(move || run_tap(sender))
            .expect("failed to spawn tap hook thread");
    }

    /// Runs on the hook thread. Creates the tap, installs it on this thread's
    /// run loop, and blocks in `CFRunLoop::run_current()`.
    ///
    /// Tap options ladder: try Default (active — can suppress; requires
    /// Accessibility) first, then fall back to ListenOnly (requires only
    /// Input Monitoring) so the status dot and key logging still work while
    /// the user hasn't granted Accessibility yet. Retries forever so the
    /// human can grant TCC prompts without relaunching.
    fn run_tap(sender: mpsc::Sender<TapEvent>) {
        const RETRY_DELAY: Duration = Duration::from_secs(3);
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;
            let mut created: Option<(CGEventTap, bool, CGEventTapLocation)> = None;
            for (location, options, can_suppress) in [
                // HID first: a Session-level Drop stops the event reaching
                // APPS but not macOS's own function-key/media handling
                // (observed: a suppressed bare F1 trigger still launched
                // Apple Music). The HID insertion point is upstream of the
                // system handler, so suppression is total — the closest
                // analogue to the Windows WH_KEYBOARD_LL global hook.
                (CGEventTapLocation::HID, CGEventTapOptions::Default, true),
                (CGEventTapLocation::Session, CGEventTapOptions::Default, true),
                (CGEventTapLocation::Session, CGEventTapOptions::ListenOnly, false),
            ] {
                let cb_sender = sender.clone();
                match CGEventTap::new(
                    location,
                    CGEventTapPlacement::HeadInsertEventTap,
                    options,
                    vec![
                        CGEventType::KeyDown,
                        CGEventType::KeyUp,
                        CGEventType::FlagsChanged,
                        // Mouse buttons for mouse triggers / hold release /
                        // Wait-for-Input. Moves and scrolls are NOT tapped —
                        // moves are per-pixel noise and trackpad momentum
                        // makes scroll triggers ungateable (deferred).
                        CGEventType::LeftMouseDown,
                        CGEventType::LeftMouseUp,
                        CGEventType::RightMouseDown,
                        CGEventType::RightMouseUp,
                        CGEventType::OtherMouseDown,
                        CGEventType::OtherMouseUp,
                        // Moves/drags/scrolls are tapped ONLY for macro
                        // recording — the callback fast-paths them with a
                        // single atomic load when no recording is live.
                        // Scroll TRIGGERS stay deferred (trackpad momentum).
                        CGEventType::MouseMoved,
                        CGEventType::LeftMouseDragged,
                        CGEventType::RightMouseDragged,
                        CGEventType::OtherMouseDragged,
                        CGEventType::ScrollWheel,
                    ],
                    move |_proxy, etype, event| tap_callback(&cb_sender, etype, event),
                ) {
                    Ok(tap) => {
                        created = Some((tap, can_suppress, location));
                        break;
                    }
                    Err(()) => continue,
                }
            }

            let (tap, can_suppress, location) = match created {
                Some(t) => t,
                None => {
                    if attempt == 1 {
                        warn!(
                            "[HOOK] CGEventTap creation failed — grant Keyfire (or your \
                             terminal, in `cargo tauri dev`) Input Monitoring + Accessibility \
                             under System Settings › Privacy & Security. Retrying every {}s…",
                            RETRY_DELAY.as_secs()
                        );
                    }
                    thread::sleep(RETRY_DELAY);
                    continue;
                }
            };

            let loop_source = match tap.mach_port().create_runloop_source(0) {
                Ok(src) => src,
                Err(()) => {
                    log::error!("[HOOK] failed to create run loop source for event tap");
                    thread::sleep(RETRY_DELAY);
                    continue;
                }
            };

            // Stash the raw port so the callback can re-enable after a stall.
            TAP_PORT.store(
                tap.mach_port().as_concrete_TypeRef() as isize,
                Ordering::SeqCst,
            );
            TAP_CAN_SUPPRESS.store(can_suppress, Ordering::SeqCst);

            let run_loop = CFRunLoop::get_current();
            unsafe {
                run_loop.add_source(&loop_source, kCFRunLoopCommonModes);
            }
            tap.enable();

            HOOKS_RUNNING.store(true, Ordering::SeqCst);
            if can_suppress {
                info!(
                    "[HOOK] CGEventTap installed ({:?}, ACTIVE — suppression enabled) \
                     after {} attempt(s) — pumping run loop",
                    location,
                    attempt
                );
            } else {
                warn!(
                    "[HOOK] CGEventTap installed LISTEN-ONLY (Accessibility not granted?) — \
                     keys are logged but hotkey matching is DISABLED until Keyfire can \
                     suppress events. Grant Accessibility and restart the app."
                );
            }

            // Blocks forever pumping the tap. `tap` stays alive on this stack,
            // so it isn't dropped (and thus invalidated) while we run.
            CFRunLoop::run_current();

            // Only reached if the run loop is ever stopped — treat as teardown.
            HOOKS_RUNNING.store(false, Ordering::SeqCst);
            TAP_PORT.store(0, Ordering::SeqCst);
            warn!("[HOOK] event tap run loop exited");
            return;
        }
    }

    /// Modifier bits from raw CGEvent flags (authoritative per-event state —
    /// used in the callback where the atomics may lag by a channel hop).
    /// Storage semantics per hard rule 6: Ctrl = macOS Control, Win = ⌘.
    fn bits_from_flags(flags: u64) -> u8 {
        let f = CGEventFlags::from_bits_truncate(flags);
        let mut bits = 0u8;
        if f.contains(CGEventFlags::CGEventFlagControl) {
            bits |= 1;
        }
        if f.contains(CGEventFlags::CGEventFlagShift) {
            bits |= 2;
        }
        if f.contains(CGEventFlags::CGEventFlagAlternate) {
            bits |= 4;
        }
        if f.contains(CGEventFlags::CGEventFlagCommand) {
            bits |= 8;
        }
        bits
    }

    /// Resolve which button a mouse event is for. Left/Right are implied by
    /// the event type; OtherMouse carries the button number in the event
    /// (2 = middle, 3/4 = side buttons). Cheap field read — callback-safe.
    fn event_mouse_button(
        etype: CGEventType,
        event: &core_graphics::event::CGEvent,
    ) -> Option<MacMouseButton> {
        Some(match etype {
            CGEventType::LeftMouseDown | CGEventType::LeftMouseUp => MacMouseButton::Left,
            CGEventType::RightMouseDown | CGEventType::RightMouseUp => MacMouseButton::Right,
            CGEventType::OtherMouseDown | CGEventType::OtherMouseUp => {
                match event.get_integer_value_field(EventField::MOUSE_EVENT_BUTTON_NUMBER) {
                    2 => MacMouseButton::Middle,
                    3 => MacMouseButton::Side1,
                    4 => MacMouseButton::Side2,
                    _ => return None, // exotic buttons — pass through
                }
            }
            _ => return None,
        })
    }

    /// Runs on the hook thread (tap callback). MUST NOT block, log, or do I/O —
    /// macOS disables taps whose callback stalls. Extract the minimum, decide
    /// suppression from the precomputed set, and hand off to the processor.
    fn tap_callback(
        sender: &mpsc::Sender<TapEvent>,
        etype: CGEventType,
        event: &core_graphics::event::CGEvent,
    ) -> CallbackResult {
        match etype {
            // Drop Keyfire's own injected events before anything else — the
            // mac analogue of the Windows LLKHF_INJECTED / SUPPRESS_SIMULATED
            // discipline. Every CGEvent actions.rs posts (keys AND mouse) is
            // stamped with the magic source-user-data tag (single fast field
            // read; allowed in the callback). Matched only for key/flag/mouse
            // events: the TapDisabled pseudo-events below are OS-generated
            // and must always be handled.
            CGEventType::KeyDown
            | CGEventType::KeyUp
            | CGEventType::FlagsChanged
            | CGEventType::LeftMouseDown
            | CGEventType::LeftMouseUp
            | CGEventType::RightMouseDown
            | CGEventType::RightMouseUp
            | CGEventType::OtherMouseDown
            | CGEventType::OtherMouseUp
            | CGEventType::MouseMoved
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::OtherMouseDragged
            | CGEventType::ScrollWheel
                if event.get_integer_value_field(EventField::EVENT_SOURCE_USER_DATA)
                    == crate::actions::INJECTED_EVENT_MAGIC =>
            {
                CallbackResult::Keep
            }
            CGEventType::KeyDown => {
                let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                let flags = event.get_flags().bits();
                let is_repeat =
                    event.get_integer_value_field(EventField::KEYBOARD_EVENT_AUTOREPEAT) != 0;
                let bits = bits_from_flags(flags);

                // ── Space pre-swallow for text expansions ────────────────
                // CRITICAL ordering (same as the Windows hook): evaluate the
                // swallow decision and latch SPACE_PRE_SWALLOWED *before*
                // sending the KeyDown to the processor — otherwise the
                // processor can run check_space_trigger before the atomic is
                // stored and take the legacy +1-backspace path. All loads
                // here are atomics — allowed in the callback.
                let space_swallow = keycode == 49 /* Space */
                    && bits == 0
                    && crate::expansions::EXPANSION_PENDING_SPACE.load(Ordering::SeqCst)
                    && MACROS_ENABLED.load(Ordering::SeqCst)
                    && TAP_CAN_SUPPRESS.load(Ordering::SeqCst)
                    && !super::APP_INPUT_FOCUSED.load(Ordering::SeqCst)
                    && !IS_RECORDING_HOTKEY.load(Ordering::SeqCst)
                    && !IS_CAPTURING_KEY.load(Ordering::SeqCst)
                    && !super::CLIPBOARD_OVERLAY_VISIBLE.load(Ordering::SeqCst)
                    && super::FILLIN_HWND.load(Ordering::SeqCst) == 0;
                if space_swallow {
                    crate::expansions::SPACE_PRE_SWALLOWED.store(true, Ordering::SeqCst);
                }

                // ── Macro recorder hotkeys (atomic compares — callback-safe) ─
                // Stop combo while recording: suppress it entirely (must not
                // leak to the target app) and flip the flag IMMEDIATELY so
                // trailing modifier releases don't land in the buffer — the
                // same ordering rule as the Windows hook.
                if !is_modifier_keycode(keycode as u16) && bits != 0 && !is_repeat {
                    if crate::recorder::IS_RECORDING_MACRO.load(Ordering::SeqCst) {
                        if crate::recorder::matches_record_hotkey(keycode as u32, bits) {
                            crate::recorder::IS_RECORDING_MACRO.store(false, Ordering::SeqCst);
                            let _ = sender.send(TapEvent::RecorderStop);
                            return CallbackResult::Drop;
                        }
                    } else if MACROS_ENABLED.load(Ordering::SeqCst) {
                        if crate::recorder::matches_record_hotkey(keycode as u32, bits) {
                            let _ = sender.send(TapEvent::TempRecord);
                            return CallbackResult::Drop;
                        }
                        if crate::recorder::matches_play_hotkey(keycode as u32, bits) {
                            let _ = sender.send(TapEvent::TempPlay);
                            return CallbackResult::Drop;
                        }
                        if crate::recorder::matches_loop_hotkey(keycode as u32, bits) {
                            let _ = sender.send(TapEvent::TempLoop);
                            return CallbackResult::Drop;
                        }
                    }
                }

                let _ = sender.send(TapEvent::KeyDown {
                    keycode,
                    flags,
                    is_repeat,
                });
                // Suppression decision — synchronous, from the precomputed
                // set (a suppressed event can't be un-suppressed later).
                // Auto-repeats of a suppressed combo are also suppressed.
                if MACROS_ENABLED.load(Ordering::SeqCst)
                    && TAP_CAN_SUPPRESS.load(Ordering::SeqCst)
                {
                    if let Ok(set) = suppress_keys().try_read() {
                        if set.contains(&(bits, keycode as u16)) {
                            return CallbackResult::Drop;
                        }
                    }
                }
                if space_swallow {
                    return CallbackResult::Drop;
                }
                CallbackResult::Keep
            }
            CGEventType::KeyUp => {
                let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                let flags = event.get_flags().bits();
                let _ = sender.send(TapEvent::KeyUp { keycode, flags });
                // Key-ups pass through (mirror of the Windows hook, which
                // only ever swallows keydowns for combo triggers).
                CallbackResult::Keep
            }
            CGEventType::FlagsChanged => {
                let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                let flags = event.get_flags().bits();
                let _ = sender.send(TapEvent::FlagsChanged { keycode, flags });
                CallbackResult::Keep
            }
            CGEventType::MouseMoved
            | CGEventType::LeftMouseDragged
            | CGEventType::RightMouseDragged
            | CGEventType::OtherMouseDragged => {
                // Fast path: only a recording cares about motion.
                if crate::recorder::IS_RECORDING_MACRO.load(Ordering::SeqCst) {
                    let p = event.location();
                    let _ = sender.send(TapEvent::MouseMoved { x: p.x, y: p.y });
                }
                CallbackResult::Keep
            }
            CGEventType::ScrollWheel => {
                if crate::recorder::IS_RECORDING_MACRO.load(Ordering::SeqCst) {
                    let lines = event
                        .get_integer_value_field(EventField::SCROLL_WHEEL_EVENT_DELTA_AXIS_1);
                    if lines != 0 {
                        let p = event.location();
                        let _ = sender.send(TapEvent::Wheel {
                            delta: (lines as i32) * 120, // Windows wheel units
                            x: p.x,
                            y: p.y,
                        });
                    }
                }
                CallbackResult::Keep
            }
            CGEventType::LeftMouseDown
            | CGEventType::RightMouseDown
            | CGEventType::OtherMouseDown => {
                let Some(button) = event_mouse_button(etype, event) else {
                    return CallbackResult::Keep;
                };
                let flags = event.get_flags().bits();
                let p = event.location();
                let _ = sender.send(TapEvent::MouseDown {
                    button,
                    flags,
                    x: p.x,
                    y: p.y,
                });
                // Bare-mouse suppression: only when a linked profile's app is
                // frontmost AND the active profile has a bare assignment for
                // this button. DOWN/UP pairing via MOUSE_DOWN_SUPPRESSED so a
                // set change mid-click never orphans a down or up.
                let bit = mouse_button_bit(button);
                if MACROS_ENABLED.load(Ordering::SeqCst)
                    && TAP_CAN_SUPPRESS.load(Ordering::SeqCst)
                    && bits_from_flags(flags) == 0
                    && super::LINKED_APP_FRONTMOST.load(Ordering::SeqCst)
                {
                    if let Ok(set) = suppress_bare_mouse().try_read() {
                        if set.contains(&button) {
                            MOUSE_DOWN_SUPPRESSED.fetch_or(bit, Ordering::SeqCst);
                            return CallbackResult::Drop;
                        }
                    }
                }
                MOUSE_DOWN_SUPPRESSED.fetch_and(!bit, Ordering::SeqCst);
                CallbackResult::Keep
            }
            CGEventType::LeftMouseUp | CGEventType::RightMouseUp | CGEventType::OtherMouseUp => {
                let Some(button) = event_mouse_button(etype, event) else {
                    return CallbackResult::Keep;
                };
                let p = event.location();
                let _ = sender.send(TapEvent::MouseUp {
                    button,
                    x: p.x,
                    y: p.y,
                });
                // Only suppress the UP whose DOWN we suppressed.
                let bit = mouse_button_bit(button);
                if MOUSE_DOWN_SUPPRESSED.load(Ordering::SeqCst) & bit != 0 {
                    MOUSE_DOWN_SUPPRESSED.fetch_and(!bit, Ordering::SeqCst);
                    return CallbackResult::Drop;
                }
                CallbackResult::Keep
            }
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                // Re-enable immediately (a single fast syscall — allowed here).
                let port = TAP_PORT.load(Ordering::SeqCst);
                if port != 0 {
                    unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
                }
                let by_user_input = matches!(etype, CGEventType::TapDisabledByUserInput);
                let _ = sender.send(TapEvent::Disabled { by_user_input });
                CallbackResult::Keep
            }
            _ => CallbackResult::Keep,
        }
    }

    // ── keycode ↔ key_id tables (US ANSI layout) ─────────────────────────────
    // key_id strings are the cross-platform JS `event.code` names the config
    // stores ("KeyV", "Digit1", "ArrowUp", …). ISO/JIS variants differ on a
    // couple of keys (Backquote/IntlBackslash) — good enough until a layout-
    // aware pass (the Windows original has the same class of caveat, solved
    // there with scancode mapping).

    pub(super) fn keycode_to_key_id(keycode: u16) -> Option<&'static str> {
        Some(match keycode {
            0 => "KeyA",
            1 => "KeyS",
            2 => "KeyD",
            3 => "KeyF",
            4 => "KeyH",
            5 => "KeyG",
            6 => "KeyZ",
            7 => "KeyX",
            8 => "KeyC",
            9 => "KeyV",
            11 => "KeyB",
            12 => "KeyQ",
            13 => "KeyW",
            14 => "KeyE",
            15 => "KeyR",
            16 => "KeyY",
            17 => "KeyT",
            18 => "Digit1",
            19 => "Digit2",
            20 => "Digit3",
            21 => "Digit4",
            22 => "Digit6",
            23 => "Digit5",
            24 => "Equal",
            25 => "Digit9",
            26 => "Digit7",
            27 => "Minus",
            28 => "Digit8",
            29 => "Digit0",
            30 => "BracketRight",
            31 => "KeyO",
            32 => "KeyU",
            33 => "BracketLeft",
            34 => "KeyI",
            35 => "KeyP",
            36 => "Enter",
            37 => "KeyL",
            38 => "KeyJ",
            39 => "Quote",
            40 => "KeyK",
            41 => "Semicolon",
            42 => "Backslash",
            43 => "Comma",
            44 => "Slash",
            45 => "KeyN",
            46 => "KeyM",
            47 => "Period",
            48 => "Tab",
            49 => "Space",
            50 => "Backquote",
            51 => "Backspace",
            53 => "Escape",
            57 => "CapsLock",
            65 => "NumpadDecimal",
            67 => "NumpadMultiply",
            69 => "NumpadAdd",
            71 => "NumLock", // Clear key on mac numpads
            75 => "NumpadDivide",
            76 => "NumpadEnter",
            78 => "NumpadSubtract",
            82 => "Numpad0",
            83 => "Numpad1",
            84 => "Numpad2",
            85 => "Numpad3",
            86 => "Numpad4",
            87 => "Numpad5",
            88 => "Numpad6",
            89 => "Numpad7",
            91 => "Numpad8",
            92 => "Numpad9",
            96 => "F5",
            97 => "F6",
            98 => "F7",
            99 => "F3",
            100 => "F8",
            101 => "F9",
            103 => "F11",
            109 => "F10",
            111 => "F12",
            114 => "Insert", // Help key position on full-size mac keyboards
            115 => "Home",
            116 => "PageUp",
            117 => "Delete", // forward delete
            118 => "F4",
            119 => "End",
            120 => "F2",
            121 => "PageDown",
            122 => "F1",
            123 => "ArrowLeft",
            124 => "ArrowRight",
            125 => "ArrowDown",
            126 => "ArrowUp",
            // Modifiers (tracked separately but named for recording flows)
            54 => "MetaRight",
            55 => "MetaLeft",
            56 => "ShiftLeft",
            58 => "AltLeft",
            59 => "ControlLeft",
            60 => "ShiftRight",
            61 => "AltRight",
            62 => "ControlRight",
            _ => return None,
        })
    }

    pub(super) fn key_id_to_keycode(key_id: &str) -> Option<u16> {
        // Small reverse table built once — keeps the two directions in sync
        // by construction (derived from keycode_to_key_id).
        static REVERSE: OnceLock<HashMap<&'static str, u16>> = OnceLock::new();
        let map = REVERSE.get_or_init(|| {
            let mut m = HashMap::new();
            for kc in 0u16..=126 {
                if let Some(id) = keycode_to_key_id(kc) {
                    m.insert(id, kc);
                }
            }
            m
        });
        map.get(key_id).copied()
    }

    fn is_modifier_keycode(keycode: u16) -> bool {
        matches!(keycode, 54 | 55 | 56 | 58 | 59 | 60 | 61 | 62 | 63)
    }

    /// Display-name half of the capture flows ("hotkey-recorded" /
    /// "key-captured" payloads). Twin of the Windows key_id_to_display.
    pub(super) fn key_id_to_display(key_id: &str) -> &str {
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
            other => other
                .strip_prefix("Key")
                .or_else(|| other.strip_prefix("Digit"))
                .unwrap_or(other),
        }
    }

    /// Reverse of key_id_to_display for combo parsing ("Ctrl+`" → Backquote).
    fn display_to_key_id(name: &str) -> Option<&'static str> {
        Some(match name {
            "Up" => "ArrowUp",
            "Down" => "ArrowDown",
            "Left" => "ArrowLeft",
            "Right" => "ArrowRight",
            "`" => "Backquote",
            "'" => "Quote",
            ";" => "Semicolon",
            "[" => "BracketLeft",
            "]" => "BracketRight",
            "\\" => "Backslash",
            "," => "Comma",
            "." => "Period",
            "/" => "Slash",
            "-" => "Minus",
            "=" => "Equal",
            "Caps" => "CapsLock",
            "Menu" => "ContextMenu",
            "0" => "Digit0",
            "1" => "Digit1",
            "2" => "Digit2",
            "3" => "Digit3",
            "4" => "Digit4",
            "5" => "Digit5",
            "6" => "Digit6",
            "7" => "Digit7",
            "8" => "Digit8",
            "9" => "Digit9",
            _ => return None,
        })
    }

    /// Parse "Ctrl+Shift+V" → (modifier_bits, mac keycode). Mirror of the
    /// Windows parse_hotkey_combo (which returns a VK in the u32 slot).
    pub(super) fn parse_hotkey_combo(combo: &str) -> Option<(u8, u32)> {
        let parts: Vec<&str> = combo.split('+').map(|s| s.trim()).collect();
        let key_name = parts.last()?;
        if key_name.is_empty() {
            return None;
        }
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
        display_name_to_keycode(key_name).map(|kc| (bits, kc as u32))
    }

    /// Resolve a display-format key name ("K", "F5", "Up", ";", "Space") to a
    /// mac keycode. Shared by combo parsing and the Send Hotkey action.
    pub(crate) fn display_name_to_keycode(key_name: &str) -> Option<u16> {
        if key_name.eq_ignore_ascii_case("space") {
            return key_id_to_keycode("Space");
        }
        key_id_to_keycode(&format!("Key{}", key_name.to_uppercase()))
            .or_else(|| key_id_to_keycode(key_name))
            .or_else(|| display_to_key_id(key_name).and_then(key_id_to_keycode))
    }

    // ── Bare-key gating ──────────────────────────────────────────────────────

    /// Keys allowed for bare mapping in static (non-app-linked) profiles.
    /// Matches STATIC_BARE_ALLOWED in keyboardLayout.jsx (Windows-only keys
    /// like PrintScreen are harmless here — the mac keycode table never
    /// produces them).
    fn is_static_bare_allowed(key_id: &str) -> bool {
        matches!(key_id,
            "F1" | "F2" | "F3" | "F4" | "F5" | "F6" | "F7" | "F8" | "F9" | "F10" | "F11" | "F12"
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

    fn profile_is_linked(state: &EngineState) -> bool {
        state
            .profile_settings
            .get(&state.active_profile)
            .and_then(|s| s.get("linkedApp"))
            .and_then(|v| v.as_str())
            .is_some()
    }

    // ── Hold-trigger machinery (twin of the Windows HOLD_TIMERS watcher) ────
    // Map keyed by mac keycode (one physical key = one hold cycle). ONE
    // watcher thread total — never a thread per keypress.

    pub(super) struct HoldEntry {
        /// Base storage key, no suffix (e.g. "Default::Ctrl+Shift::F12").
        storage_key: String,
        fire_at: std::time::Instant,
        inserted_at: std::time::Instant,
        fired: bool,
        hold_macro: Value,
        /// The base (single-press) assignment, if one exists — re-injected
        /// at early release.
        single_macro: Option<Value>,
        has_double: bool,
        is_bare: bool,
    }

    pub(super) fn hold_timers() -> &'static std::sync::Mutex<HashMap<u16, HoldEntry>> {
        static TIMERS: OnceLock<std::sync::Mutex<HashMap<u16, HoldEntry>>> = OnceLock::new();
        TIMERS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
    }

    /// Arm a hold timer for a fresh keydown. Silently returns when the
    /// keycode already has a live entry (belt-and-braces — mac repeats are
    /// filtered upstream by the event's autorepeat field); a stale entry
    /// (>10s, keyup lost) is replaced.
    #[allow(clippy::too_many_arguments)]
    fn arm_hold_timer(
        keycode: u16,
        storage_key: String,
        hold_macro: Value,
        single_macro: Option<Value>,
        has_double: bool,
        is_bare: bool,
        threshold_ms: u64,
    ) {
        let mut timers = hold_timers().lock().unwrap();
        if let Some(existing) = timers.get(&keycode) {
            if existing.inserted_at.elapsed() < Duration::from_secs(10) {
                return;
            }
        }
        let now = std::time::Instant::now();
        timers.insert(
            keycode,
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

    /// Drop all armed hold timers (entries hold clones of old macros).
    /// Called on assignment updates.
    pub(super) fn clear_hold_timers() {
        if let Ok(mut timers) = hold_timers().lock() {
            timers.clear();
        }
    }

    static HOLD_WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);

    /// The single hold-watcher thread: 16ms tick, fires entries whose
    /// threshold has passed. Firing happens here (never in the tap callback)
    /// so the tap latency budget is untouched.
    pub(super) fn spawn_hold_watcher(app: AppHandle) {
        if HOLD_WATCHER_RUNNING.swap(true, Ordering::SeqCst) {
            return;
        }
        thread::Builder::new()
            .name("keyfire-hold-watcher".to_string())
            .spawn(move || {
                loop {
                    thread::sleep(Duration::from_millis(16));
                    if super::HOLD_DETECTION_PAUSED.load(Ordering::SeqCst) {
                        continue;
                    }
                    // Collect expired entries under the lock, fire after releasing it.
                    let mut to_fire: Vec<(String, Value, bool)> = Vec::new();
                    {
                        let mut timers = hold_timers().lock().unwrap();
                        if timers.is_empty() {
                            continue;
                        }
                        let now = std::time::Instant::now();
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
                        // Threshold reached → hold wins this press cycle:
                        // cancel the double window, any pending single timer,
                        // and any deferred pending macro for this key.
                        {
                            let mut state = engine_state().lock().unwrap();
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

    // ── Suppress-set rebuild ─────────────────────────────────────────────────

    /// Recompute the (bits, keycode) suppress set from the active profile's
    /// assignments plus the handled special hotkeys. Called with the
    /// engine_state lock HELD (mirror of the Windows contract — must not
    /// re-lock).
    ///
    /// Deliberately excluded until their milestones land (an unsuppressed key
    /// still reaches the target app; a suppressed-but-unfired key is dead):
    /// mouse triggers, voice, radial menu, Quick Record combos.
    pub(super) fn rebuild_suppress_keys(state: &EngineState) {
        let mut set: HashSet<(u8, u16)> = HashSet::new();
        let mut mouse_set: HashSet<MacMouseButton> = HashSet::new();
        let prefix = format!("{}::", state.active_profile);
        let is_linked = profile_is_linked(state);
        for key in state.assignments.keys() {
            if !key.starts_with(&prefix) {
                continue;
            }
            let parts: Vec<&str> = key.split("::").collect();
            if parts.len() < 3 {
                continue;
            }
            let combo_str = parts[1];
            if combo_str == "GLOBAL" {
                continue;
            }
            // ::double entries never suppress on their own — a double-only
            // key lets the single press pass through to the app; when both
            // single+double exist, the single entry already adds the key.
            if parts.last() == Some(&"double") {
                continue;
            }
            // ::hold entries DO suppress (a hold-armed key must not leak its
            // keystroke while the watcher waits) — but only for Pro; free
            // users' hold mappings are inert and suppressing would leave a
            // dead key.
            if parts.last() == Some(&"hold") && !crate::licence::is_pro() {
                continue;
            }
            let key_id = parts[2];
            if combo_str == "BARE" {
                // Bare mouse buttons: only in app-linked profiles (a global
                // left-click remap would make the machine unusable).
                if let Some(btn) = mouse_key_id_to_button(key_id) {
                    if is_linked {
                        mouse_set.insert(btn);
                    }
                    continue;
                }
                // App-linked profiles: all bare keys. Static profiles: only
                // non-character keys (same gate as the Windows original —
                // suppressing letters globally would eat normal typing).
                if is_linked || is_static_bare_allowed(key_id) {
                    if let Some(kc) = key_id_to_keycode(key_id) {
                        set.insert((0u8, kc));
                    }
                }
                continue;
            }
            let bits = super::hotkeys_combo_bits(combo_str);
            if bits == 0 {
                continue;
            }
            if let Some(kc) = key_id_to_keycode(key_id) {
                set.insert((bits, kc));
            }
        }
        // Clipboard-paste combo only suppresses while capture is enabled
        // (mirror of the Windows add/remove_clipboard_paste_from_suppress
        // pair) — a suppressed combo whose popup won't open is a dead key.
        let clipboard_special = if crate::clipboard::is_capture_enabled() {
            state.clipboard_paste_hotkey
        } else {
            None
        };
        for special in [state.overlay_hotkey, state.pause_hotkey, clipboard_special]
            .into_iter()
            .flatten()
        {
            set.insert((special.0, special.1 as u16));
        }
        info!(
            "[HOOK] Rebuilt suppress set: {} combos, {} bare mouse",
            set.len(),
            mouse_set.len()
        );
        if let Ok(mut w) = suppress_keys().write() {
            *w = set;
        }
        if let Ok(mut w) = suppress_bare_mouse().write() {
            *w = mouse_set;
        }
    }

    // ── Processor thread ─────────────────────────────────────────────────────

    /// Update the shared modifier atomics from a CGEvent flag mask. Maps to the
    /// engine's modifier convention: Cmd is stored as Meta ("Win"), per the
    /// cross-platform storage rule.
    fn update_modifiers(flags: u64) {
        let f = CGEventFlags::from_bits_truncate(flags);
        MOD_CTRL.store(f.contains(CGEventFlags::CGEventFlagControl), Ordering::SeqCst);
        MOD_SHIFT.store(f.contains(CGEventFlags::CGEventFlagShift), Ordering::SeqCst);
        MOD_ALT.store(f.contains(CGEventFlags::CGEventFlagAlternate), Ordering::SeqCst);
        MOD_META.store(f.contains(CGEventFlags::CGEventFlagCommand), Ordering::SeqCst);
    }

    /// Human-readable modifier list for log lines, e.g. "Ctrl+Shift".
    fn modifier_string(flags: u64) -> String {
        let f = CGEventFlags::from_bits_truncate(flags);
        let mut parts = Vec::new();
        if f.contains(CGEventFlags::CGEventFlagControl) {
            parts.push("Ctrl");
        }
        if f.contains(CGEventFlags::CGEventFlagShift) {
            parts.push("Shift");
        }
        if f.contains(CGEventFlags::CGEventFlagAlternate) {
            parts.push("Alt");
        }
        if f.contains(CGEventFlags::CGEventFlagCommand) {
            parts.push("Cmd");
        }
        parts.join("+")
    }

    /// Current combo string in storage order (Ctrl, Shift, Alt, Win) — twin
    /// of the Windows build_modifier_combo.
    fn build_modifier_combo() -> String {
        let mut parts = Vec::new();
        if MOD_CTRL.load(Ordering::SeqCst) {
            parts.push("Ctrl");
        }
        if MOD_SHIFT.load(Ordering::SeqCst) {
            parts.push("Shift");
        }
        if MOD_ALT.load(Ordering::SeqCst) {
            parts.push("Alt");
        }
        if MOD_META.load(Ordering::SeqCst) {
            parts.push("Win");
        }
        parts.join("+")
    }

    /// Recorded button names for crate::recorder (must be &'static).
    fn mouse_button_record_name(button: MacMouseButton) -> &'static str {
        match button {
            MacMouseButton::Left => "Left",
            MacMouseButton::Right => "Right",
            MacMouseButton::Middle => "Middle",
            MacMouseButton::Side1 => "Side1",
            MacMouseButton::Side2 => "Side2",
        }
    }

    /// mac keycode → Windows VK for the macro recorder. Recorded streams are
    /// stored in VK terms so they replay on BOTH platforms (replay routes
    /// through the VK→keycode translation in actions.rs). Derived from the
    /// keycode↔key_id table; keys with no VK equivalent record nothing.
    fn keycode_to_record_vk(keycode: u16) -> Option<u32> {
        // Modifiers first — they arrive via flagsChanged, not keyDown.
        let direct = match keycode {
            56 => Some(0xA0), // LShift
            60 => Some(0xA1), // RShift
            59 => Some(0xA2), // LCtrl
            62 => Some(0xA3), // RCtrl
            58 => Some(0xA4), // LAlt (⌥)
            61 => Some(0xA5), // RAlt
            55 => Some(0x5B), // LCmd → LWin (Meta stored as 'Win', hard rule 6)
            54 => Some(0x5C), // RCmd → RWin
            _ => None,
        };
        if direct.is_some() {
            return direct;
        }
        let key_id = keycode_to_key_id(keycode)?;
        Some(match key_id {
            id if id.len() == 4 && id.starts_with("Key") => id.as_bytes()[3] as u32, // KeyA..KeyZ → 0x41..
            id if id.len() == 6 && id.starts_with("Digit") => id.as_bytes()[5] as u32, // Digit0.. → 0x30..
            "Enter" => 0x0D,
            "Tab" => 0x09,
            "Space" => 0x20,
            "Backspace" => 0x08,
            "Escape" => 0x1B,
            "CapsLock" => 0x14,
            "Delete" => 0x2E,
            "Insert" => 0x2D,
            "Home" => 0x24,
            "End" => 0x23,
            "PageUp" => 0x21,
            "PageDown" => 0x22,
            "ArrowLeft" => 0x25,
            "ArrowUp" => 0x26,
            "ArrowRight" => 0x27,
            "ArrowDown" => 0x28,
            "F1" => 0x70, "F2" => 0x71, "F3" => 0x72, "F4" => 0x73,
            "F5" => 0x74, "F6" => 0x75, "F7" => 0x76, "F8" => 0x77,
            "F9" => 0x78, "F10" => 0x79, "F11" => 0x7A, "F12" => 0x7B,
            "Semicolon" => 0xBA, "Equal" => 0xBB, "Comma" => 0xBC,
            "Minus" => 0xBD, "Period" => 0xBE, "Slash" => 0xBF,
            "Backquote" => 0xC0, "BracketLeft" => 0xDB, "Backslash" => 0xDC,
            "BracketRight" => 0xDD, "Quote" => 0xDE,
            "Numpad0" => 0x60, "Numpad1" => 0x61, "Numpad2" => 0x62,
            "Numpad3" => 0x63, "Numpad4" => 0x64, "Numpad5" => 0x65,
            "Numpad6" => 0x66, "Numpad7" => 0x67, "Numpad8" => 0x68,
            "Numpad9" => 0x69, "NumpadMultiply" => 0x6A, "NumpadAdd" => 0x6B,
            "NumpadSubtract" => 0x6D, "NumpadDecimal" => 0x6E, "NumpadDivide" => 0x6F,
            "NumpadEnter" => 0x0D,
            "NumLock" => 0x90,
            _ => return None,
        })
    }

    /// Feed modifier transitions from a flagsChanged into the recorder as
    /// synthetic modifier key events (Windows captures these as plain
    /// keydowns; mac modifiers never arrive as keyDown/keyUp).
    fn record_modifier_transition(keycode: u16, before_bits: u8, after_bits: u8) {
        if before_bits == after_bits {
            return;
        }
        if let Some(vk) = keycode_to_record_vk(keycode) {
            let is_down = after_bits & !before_bits != 0;
            crate::recorder::push_key(vk, 0, is_down);
        }
    }

    /// Runs on the processor thread. Owns modifier-state tracking, matching,
    /// dispatch and logging.
    fn process_events(receiver: mpsc::Receiver<TapEvent>, app: AppHandle) {
        info!("[HOOK] Event processor started");
        // Mouse-move capture throttle (16ms) — same rate as the Windows hook.
        let mut last_move_push = std::time::Instant::now();
        while let Ok(ev) = receiver.recv() {
            match ev {
                TapEvent::KeyDown {
                    keycode,
                    flags,
                    is_repeat,
                } => {
                    update_modifiers(flags);
                    // Recorder capture — side observation; the keystroke
                    // still flows through normal handling below (Windows
                    // parity). push_key self-guards on IS_RECORDING_MACRO.
                    if let Some(vk) = keycode_to_record_vk(keycode as u16) {
                        crate::recorder::push_key(vk, 0, true);
                    }
                    // Esc sets the global macro-cancel flag on every real
                    // keydown (injected events never reach the processor, so
                    // "real" is guaranteed by the tag filter). Not
                    // suppressed — the target app should still see Esc.
                    if keycode == 53 && !is_repeat {
                        crate::actions::ESC_LOOP_BREAK.store(true, Ordering::SeqCst);
                    }
                    // Forward to a Wait for Input waiter before normal
                    // handling (the waiter sees events regardless of mode).
                    if !is_modifier_keycode(keycode as u16) {
                        if let Some(id) = keycode_to_key_id(keycode as u16) {
                            super::forward_to_waiter(&super::WaitEvent::KeyDown {
                                key_id: key_id_to_display(id).to_string(),
                            });
                        }
                    }
                    handle_keydown(keycode as u16, flags, is_repeat, &app);
                }
                TapEvent::KeyUp { keycode, flags } => {
                    update_modifiers(flags);
                    if let Some(vk) = keycode_to_record_vk(keycode as u16) {
                        crate::recorder::push_key(vk, 0, false);
                    }
                    if !is_modifier_keycode(keycode as u16) {
                        if let Some(id) = keycode_to_key_id(keycode as u16) {
                            super::forward_to_waiter(&super::WaitEvent::KeyUp {
                                key_id: key_id_to_display(id).to_string(),
                            });
                        }
                    }
                    handle_keyup(keycode as u16, &app);
                }
                TapEvent::FlagsChanged { keycode, flags } => {
                    // Capture modifier transitions before the atomics update.
                    if crate::recorder::IS_RECORDING_MACRO.load(Ordering::SeqCst) {
                        let before = modifier_bits();
                        let after = bits_from_flags(flags);
                        record_modifier_transition(keycode as u16, before, after);
                    }
                    handle_flags_changed(keycode as u16, flags, &app);
                }
                TapEvent::MouseDown { button, flags, x, y } => {
                    update_modifiers(flags);
                    crate::recorder::push_mouse_button(
                        mouse_button_record_name(button),
                        x as i32,
                        y as i32,
                        true,
                    );
                    super::forward_to_waiter(&super::WaitEvent::MouseDown {
                        button_name: mouse_button_to_key_id(button).to_string(),
                    });
                    handle_mouse_down(button, flags, &app);
                }
                TapEvent::MouseUp { button, x, y } => {
                    crate::recorder::push_mouse_button(
                        mouse_button_record_name(button),
                        x as i32,
                        y as i32,
                        false,
                    );
                    super::forward_to_waiter(&super::WaitEvent::MouseUp {
                        button_name: mouse_button_to_key_id(button).to_string(),
                    });
                    handle_mouse_up(button, &app);
                }
                TapEvent::MouseMoved { x, y } => {
                    // Throttled capture — only sent while recording.
                    if last_move_push.elapsed() >= Duration::from_millis(16) {
                        last_move_push = std::time::Instant::now();
                        crate::recorder::push_mouse_move(x as i32, y as i32);
                    }
                }
                TapEvent::Wheel { delta, x, y } => {
                    crate::recorder::push_wheel(delta, x as i32, y as i32);
                }
                TapEvent::RecorderStop => {
                    handle_recorder_stop(&app);
                }
                TapEvent::TempRecord => {
                    handle_temp_record(&app);
                }
                TapEvent::TempPlay => {
                    handle_temp_play(&app);
                }
                TapEvent::TempLoop => {
                    handle_temp_loop(&app);
                }
                TapEvent::Disabled { by_user_input } => {
                    if by_user_input {
                        warn!(
                            "[HOOK] tap disabled by USER INPUT (Secure Input active — a \
                             frontmost app has Secure Keyboard Entry on, or focus is a \
                             password field; key events are withheld while it's active) — \
                             re-enabled"
                        );
                    } else {
                        warn!(
                            "[HOOK] tap disabled by TIMEOUT (callback stalled) — re-enabled"
                        );
                    }
                }
            }
        }
        // Sender dropped (hook thread gone) — nothing left to process.
        warn!("[HOOK] tap processor thread exiting (channel closed)");
    }

    // ── Quick Record / macro recorder handlers (twins of the Windows
    // processor blocks; the callback already suppressed the hotkeys) ────────

    /// Stop-hotkey fired mid-recording. IS_RECORDING_MACRO is already false
    /// (flipped in the callback). Branch on TEMP_RECORDING_ACTIVE: the
    /// editor flow lets the frontend retrieve events via
    /// stop_macro_recording; the global flow finalises here.
    fn handle_recorder_stop(app: &AppHandle) {
        let (count, dur) = crate::recorder::status_snapshot();
        if crate::recorder::TEMP_RECORDING_ACTIVE.load(Ordering::SeqCst) {
            crate::recorder::TEMP_RECORDING_ACTIVE.store(false, Ordering::SeqCst);
            let events = crate::recorder::stop();
            let captured_at = chrono::Local::now().to_rfc3339();
            if let Ok(mut state) = engine_state().lock() {
                state.temp_macro_events = Some(events.clone());
                state.temp_macro_captured_at = Some(captured_at.clone());
            }
            crate::persist_temp_macro(&events, &captured_at);
            crate::hide_recorder_bar(app.clone());
            let _ = app.emit(
                "temp-macro-saved",
                serde_json::json!({
                    "count": events.len(),
                    "durationMs": dur,
                    "capturedAt": captured_at,
                }),
            );
            info!("[RECORDER] Temp macro saved ({} events, {}ms)", events.len(), dur);
        } else {
            let _ = app.emit(
                "recorder-stop-requested",
                serde_json::json!({ "count": count, "durationMs": dur }),
            );
            info!("[RECORDER] Stop hotkey relayed to frontend");
        }
    }

    fn handle_temp_record(app: &AppHandle) {
        // Ignore while the Loop is running — mixing user input into a
        // replaying stream would corrupt the new recording.
        if crate::recorder::TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst) {
            info!("[RECORDER] Quick Record press ignored — Quick Loop is running");
            return;
        }
        crate::recorder::TEMP_RECORDING_ACTIVE.store(true, Ordering::SeqCst);
        // show_recorder_bar shows the bottom-centre recording bar AND calls
        // recorder::start internally — same pill as the editor flow.
        crate::show_recorder_bar(app.clone());
        let _ = app.emit("temp-macro-recording-started", serde_json::json!({}));
        info!("[RECORDER] Quick Record: recording started via global hotkey");
    }

    fn temp_macro_snapshot() -> Option<(Vec<crate::recorder::RecordedEvent>, String)> {
        engine_state().lock().ok().and_then(|s| {
            match (&s.temp_macro_events, &s.temp_macro_captured_at) {
                (Some(ev), Some(ts)) if !ev.is_empty() => Some((ev.clone(), ts.clone())),
                _ => None,
            }
        })
    }

    fn handle_temp_play(app: &AppHandle) {
        if crate::recorder::TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst) {
            info!("[RECORDER] Quick Replay press ignored — Quick Loop is running");
            return;
        }
        match temp_macro_snapshot() {
            Some((events, captured_at)) => {
                // Clear any stale Esc from before this fire — Quick Replay
                // bypasses the MacroRunningGuard reset infrastructure.
                crate::actions::ESC_LOOP_BREAK.store(false, Ordering::SeqCst);
                let _ = app.emit(
                    "temp-macro-replay-started",
                    serde_json::json!({ "count": events.len(), "capturedAt": captured_at }),
                );
                thread::spawn(move || {
                    crate::actions::replay_recorded_events(&events, "Quick Replay");
                });
            }
            None => {
                let _ = app.emit("temp-macro-replay-empty", serde_json::json!({}));
                info!("[RECORDER] Quick Replay: no temp macro saved");
            }
        }
    }

    fn handle_temp_loop(app: &AppHandle) {
        // Toggle: if the loop is already running, this press is a stop.
        if crate::recorder::TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst) {
            crate::recorder::TEMP_MACRO_LOOP_ACTIVE.store(false, Ordering::SeqCst);
            let _ = app.emit("temp-macro-loop-stopped", serde_json::json!({}));
            info!("[RECORDER] Quick Loop: stop requested via hotkey");
            return;
        }
        match temp_macro_snapshot() {
            Some((events, captured_at)) => {
                crate::actions::ESC_LOOP_BREAK.store(false, Ordering::SeqCst);
                let _ = app.emit(
                    "temp-macro-loop-started",
                    serde_json::json!({ "count": events.len(), "capturedAt": captured_at }),
                );
                thread::spawn(move || {
                    crate::actions::replay_recorded_events_loop(&events, "Quick Loop");
                });
            }
            None => {
                let _ = app.emit("temp-macro-replay-empty", serde_json::json!({}));
                info!("[RECORDER] Quick Loop: no temp macro saved");
            }
        }
    }

    /// Modifier press/release bookkeeping (modifiers arrive as flagsChanged
    /// on macOS, not keyDown/keyUp). Also drives the sole-modifier capture
    /// path and pending-macro firing on final modifier release.
    fn handle_flags_changed(keycode: u16, flags: u64, app: &AppHandle) {
        let before_bits = modifier_bits();
        update_modifiers(flags);
        let after_bits = modifier_bits();
        log::debug!("[HOOK] flagsChanged mods=[{}]", modifier_string(flags));

        // Sole-modifier capture (settings fields accept bare "Ctrl" etc.):
        // on press with no other modifiers → remember; on full release while
        // still capturing → emit it. Mirror of the Windows flow.
        if IS_CAPTURING_KEY.load(Ordering::SeqCst) {
            let pressed = after_bits & !before_bits;
            if pressed != 0 {
                let sole = match pressed {
                    1 => Some("Ctrl"),
                    2 => Some("Shift"),
                    4 => Some("Alt"),
                    8 => Some("Win"),
                    _ => None,
                };
                if let Ok(mut state) = engine_state().lock() {
                    state.capture_sole_modifier = if before_bits == 0 {
                        sole.map(String::from)
                    } else {
                        None
                    };
                }
            } else if after_bits == 0 && before_bits != 0 {
                let sole = engine_state()
                    .lock()
                    .ok()
                    .and_then(|mut s| s.capture_sole_modifier.take());
                if let Some(sole) = sole {
                    IS_CAPTURING_KEY.store(false, Ordering::SeqCst);
                    let _ = app.emit("key-captured", Value::String(sole));
                }
            }
        }

        // Any NEW modifier press clears the expansion buffer (mirror of the
        // Windows hook's "clear on any modifier press" — a chord is starting,
        // the typed word is no longer a live trigger candidate).
        if after_bits & !before_bits != 0 {
            crate::expansions::buffer_clear();
        }

        // Final modifier release completes a pending combo fire (the trigger
        // key's own keyup may have arrived while modifiers were still held).
        if after_bits == 0 {
            maybe_fire_pending(app);
        }
    }

    /// Drive the text-expansion buffer for a bare or Shift-only printable
    /// keystroke. Called once any hotkey-matching is known to have NOT
    /// matched (twin of the Windows process_expansion_keystroke). Skips work
    /// while the fill-in window is up — those keystrokes belong to it.
    fn process_expansion_keystroke(key_id: &str, keycode: u16, flags: u64) {
        if super::FILLIN_HWND.load(Ordering::SeqCst) != 0 {
            return;
        }
        match key_id {
            "Backspace" => crate::expansions::buffer_pop(),
            "Space" => {
                crate::expansions::check_space_trigger();
                crate::expansions::buffer_clear();
            }
            "Enter" | "NumpadEnter" | "Escape" | "Tab" => crate::expansions::buffer_clear(),
            _ => {
                if let Some(ch) = resolve_typed_char(keycode, flags).filter(|c| !c.is_control()) {
                    crate::expansions::buffer_push(ch);
                    crate::expansions::check_immediate_triggers();
                }
            }
        }
    }

    fn handle_keydown(keycode: u16, flags: u64, is_repeat: bool, app: &AppHandle) {
        if is_modifier_keycode(keycode) {
            return; // arrives via flagsChanged; belt-and-braces
        }
        let key_id = match keycode_to_key_id(keycode) {
            Some(id) => id,
            None => {
                log::debug!("[HOOK] keyDown unmapped keycode={}", keycode);
                return;
            }
        };
        let bits = bits_from_flags(flags);
        log::debug!(
            "[HOOK] keyDown {} mods=[{}] repeat={}",
            key_id,
            modifier_string(flags),
            is_repeat
        );

        // ── Release any held key (Send Hotkey hold mode) on physical press ──
        if crate::actions::is_key_held() {
            crate::actions::release_held_key();
            crate::tray::update_tray_icon_normal(app);
        }

        // ── Recording mode: capture combo and send to frontend ──────────────
        // Must run BEFORE the APP_INPUT_FOCUSED check — recording works while
        // the Keyfire UI is focused (mirror of Windows ordering).
        if IS_RECORDING_HOTKEY.load(Ordering::SeqCst) {
            IS_RECORDING_HOTKEY.store(false, Ordering::SeqCst);
            let mut mods = Vec::new();
            if bits & 1 != 0 {
                mods.push("Ctrl");
            }
            if bits & 2 != 0 {
                mods.push("Shift");
            }
            if bits & 4 != 0 {
                mods.push("Alt");
            }
            if bits & 8 != 0 {
                mods.push("Win");
            }
            let _ = app.emit(
                "hotkey-recorded",
                serde_json::json!({ "modifiers": mods, "keyId": key_id }),
            );
            return;
        }

        // ── Key capture mode: capture combo string for settings ─────────────
        if IS_CAPTURING_KEY.load(Ordering::SeqCst) {
            IS_CAPTURING_KEY.store(false, Ordering::SeqCst);
            let mut parts = Vec::new();
            if bits & 1 != 0 {
                parts.push("Ctrl".to_string());
            }
            if bits & 2 != 0 {
                parts.push("Shift".to_string());
            }
            if bits & 4 != 0 {
                parts.push("Alt".to_string());
            }
            if bits & 8 != 0 {
                parts.push("Win".to_string());
            }
            parts.push(key_id_to_display(key_id).to_string());
            let _ = app.emit("key-captured", Value::String(parts.join("+")));
            return;
        }

        // ── Global pause hotkey (works even when paused) ─────────────────────
        if bits != 0 {
            let pause = engine_state().lock().ok().and_then(|s| s.pause_hotkey);
            if let Some((mod_bits, kc)) = pause {
                if bits == mod_bits && keycode as u32 == kc {
                    let was_enabled = MACROS_ENABLED.load(Ordering::SeqCst);
                    MACROS_ENABLED.store(!was_enabled, Ordering::SeqCst);
                    let now_enabled = !was_enabled;
                    info!("[PAUSE] Global pause toggled: macros={}", now_enabled);
                    crate::tray::rebuild_tray_menu(app);
                    crate::tray::update_tray_icon(app, now_enabled);
                    let _ = app.emit("engine-status", get_status_json());
                    return;
                }
            }
        }

        if !MACROS_ENABLED.load(Ordering::SeqCst) {
            return;
        }
        let shift_only = bits & !2 == 0; // no modifiers, or Shift alone
        if is_repeat {
            // Auto-repeats never dispatch (the original press already
            // decided); the tap already suppressed repeats of matched combos.
            // They DO feed the expansion buffer — held character keys drive
            // triggers like ":kr" (same fall-through as Windows).
            if shift_only
                && !APP_INPUT_FOCUSED.load(Ordering::SeqCst)
                && !super::CLIPBOARD_OVERLAY_VISIBLE.load(Ordering::SeqCst)
            {
                process_expansion_keystroke(key_id, keycode, flags);
            }
            return;
        }

        // ── Overlay toggle hotkey ────────────────────────────────────────────
        if bits != 0 {
            let overlay = engine_state().lock().ok().and_then(|s| s.overlay_hotkey);
            if let Some((mod_bits, kc)) = overlay {
                if bits == mod_bits && keycode as u32 == kc {
                    let _ = app.emit("toggle-overlay", Value::Null);
                    return;
                }
            }
        }

        // ── Clipboard quick-paste hotkey ─────────────────────────────────────
        if bits != 0 {
            let clip = engine_state()
                .lock()
                .ok()
                .and_then(|s| s.clipboard_paste_hotkey);
            if let Some((mod_bits, kc)) = clip {
                if bits == mod_bits && keycode as u32 == kc {
                    let _ = app.emit("toggle-clipboard-overlay", Value::Null);
                    return;
                }
            }
        }

        // ── Normal hotkey matching (modified combos only in this milestone) ──
        if APP_INPUT_FOCUSED.load(Ordering::SeqCst) {
            return;
        }
        // While the clipboard popup is up its DOM handlers own the keys
        // (mirror of the Windows early-return; the mac popup has real focus).
        if super::CLIPBOARD_OVERLAY_VISIBLE.load(Ordering::SeqCst) {
            return;
        }
        if bits == 0 {
            // ── Bare-key matching ────────────────────────────────────────
            // App-linked profiles: all bare keys fire when the linked app is
            // focused. Static profiles: only non-character keys fire
            // globally (letters would eat normal typing).
            let mut state = match engine_state().lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            let bare_allowed = profile_is_linked(&state) || is_static_bare_allowed(key_id);

            if bare_allowed {
                let bare_key = format!("{}::BARE::{}", state.active_profile, key_id);

                // Repeat-mode stop on the repeat's own bare trigger.
                if crate::actions::is_repeating() {
                    if let Some(trigger) = crate::actions::get_repeating_trigger() {
                        if trigger == bare_key {
                            drop(state);
                            crate::actions::stop_repeating_key();
                            crate::tray::update_tray_icon_normal(app);
                            return;
                        }
                    }
                }

                // ── Hold trigger (Pro) on bare keys ──────────────────────
                let bare_hold_key = format!("{}::hold", bare_key);
                if crate::licence::is_pro()
                    && state.assignments.contains_key(&bare_hold_key)
                    && !super::HOLD_DETECTION_PAUSED.load(Ordering::SeqCst)
                {
                    crate::expansions::buffer_clear();
                    let hold_macro = state
                        .assignments
                        .get(&bare_hold_key)
                        .cloned()
                        .unwrap_or(Value::Null);
                    let single_macro = state.assignments.get(&bare_key).cloned();
                    let double_key = format!("{}::double", bare_key);
                    let has_double = state.assignments.contains_key(&double_key);
                    let threshold = state.hold_threshold_ms;

                    // Keydown-time double-tap detection — deferring it to
                    // keyup breaks the tap-to-tap timing (same rule as the
                    // Windows hold branch).
                    if has_double {
                        let now = std::time::Instant::now();
                        let dtw = state.double_tap_window_ms;
                        if let Some(last) = state.last_hotkey_time.get(&bare_key) {
                            if now.duration_since(*last).as_millis() < dtw as u128 {
                                if let Some(cancel) =
                                    state.pending_single_cancel.remove(&bare_key)
                                {
                                    cancel.store(true, Ordering::SeqCst);
                                }
                                state.last_hotkey_time.remove(&bare_key);
                                info!(
                                    "[Keyfire] x2 Keydown double-tap (hold-armed bare key): {}",
                                    bare_key
                                );
                                // fired=true sentinel keeps the watcher and
                                // the keyup re-injection inert for this press.
                                hold_timers().lock().unwrap().insert(keycode, HoldEntry {
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
                    arm_hold_timer(keycode, bare_key, hold_macro, single_macro, has_double, true, threshold);
                    return;
                }

                if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                    crate::expansions::buffer_clear();
                    let action_type = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let double_key_str = format!("{}::double", bare_key);
                    // Pro gate: Free users ignore double mappings so single fires normally.
                    let has_double = crate::licence::is_pro()
                        && state.assignments.contains_key(&double_key_str);

                    // Hotkey actions on bare keys: AHK-style direct
                    // passthrough — keydown posts the target chord's downs,
                    // keyup (handle_keyup → remap_key_release) the ups, so
                    // hold and tap feel identical to the target key.
                    if action_type == "hotkey" && !has_double {
                        if let Some(data) = macro_val.get("data") {
                            if crate::actions::remap_key_press(keycode, data) {
                                drop(state);
                                return;
                            }
                        }
                        let trigger = bare_key.clone();
                        drop(state);
                        fire_macro(macro_val, false, Some(trigger), app);
                        return;
                    }

                    state.pending_macro = Some(macro_val);
                    state.pending_storage_key = Some(bare_key.clone());
                    state.pending_trigger_key = Some(bare_key);
                    state.pending_is_bare = true;
                    return;
                }

                // No single-press — double-only bare key (Pro).
                let double_key = format!("{}::double", bare_key);
                if crate::licence::is_pro() && state.assignments.contains_key(&double_key) {
                    crate::expansions::buffer_clear();
                    let now = std::time::Instant::now();
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
                    // First tap — passes through to the app (double-only
                    // keys are not in the suppress set); no buffer feed.
                    state.last_hotkey_time.insert(bare_key, now);
                    return;
                }
            }

            drop(state);
            // No bare match — bare printables drive the expansion buffer.
            process_expansion_keystroke(key_id, keycode, flags);
            return;
        }

        let combo = build_modifier_combo();
        let mut state = match engine_state().lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let storage_key = format!("{}::{}::{}", state.active_profile, combo, key_id);

        // ── Repeat-mode stop: pressing the repeat's own trigger stops it ────
        if crate::actions::is_repeating() {
            if let Some(trigger) = crate::actions::get_repeating_trigger() {
                if trigger == storage_key {
                    drop(state);
                    crate::actions::stop_repeating_key();
                    crate::tray::update_tray_icon_normal(app);
                    return;
                }
            }
        }

        // ── Hold trigger (Pro) on modified combos ────────────────────────────
        let hold_key = format!("{}::hold", storage_key);
        if crate::licence::is_pro()
            && state.assignments.contains_key(&hold_key)
            && !super::HOLD_DETECTION_PAUSED.load(Ordering::SeqCst)
        {
            crate::expansions::buffer_clear();
            let hold_macro = state.assignments.get(&hold_key).cloned().unwrap_or(Value::Null);
            let single_macro = state.assignments.get(&storage_key).cloned();
            let double_key = format!("{}::double", storage_key);
            let has_double = state.assignments.contains_key(&double_key);
            let threshold = state.hold_threshold_ms;

            if has_double {
                let now = std::time::Instant::now();
                let dtw = state.double_tap_window_ms;
                if let Some(last) = state.last_hotkey_time.get(&storage_key) {
                    if now.duration_since(*last).as_millis() < dtw as u128 {
                        if let Some(cancel) = state.pending_single_cancel.remove(&storage_key) {
                            cancel.store(true, Ordering::SeqCst);
                        }
                        state.last_hotkey_time.remove(&storage_key);
                        info!("[Keyfire] x2 Keydown double-tap (hold-armed key): {}", storage_key);
                        hold_timers().lock().unwrap().insert(keycode, HoldEntry {
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
                state.last_hotkey_time.insert(storage_key.clone(), now);
            }

            drop(state);
            arm_hold_timer(keycode, storage_key, hold_macro, single_macro, has_double, false, threshold);
            return;
        }

        let mut hotkey_matched = false;
        if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
            hotkey_matched = true;
            crate::expansions::buffer_clear();
            // Pro gate: Free users ignore double-tap mappings.
            let double_key = format!("{}::double", storage_key);
            let has_double =
                crate::licence::is_pro() && state.assignments.contains_key(&double_key);

            if has_double {
                let double_macro = state.assignments.get(&double_key).cloned();
                let now = std::time::Instant::now();
                let dtw = state.double_tap_window_ms;

                if let Some(last) = state.last_hotkey_time.get(&storage_key) {
                    if now.duration_since(*last).as_millis() < dtw as u128 {
                        // Second tap within window — fire double at keyup.
                        if let Some(cancel) = state.pending_single_cancel.remove(&storage_key) {
                            cancel.store(true, Ordering::SeqCst);
                        }
                        state.last_hotkey_time.remove(&storage_key);
                        info!("[Keyfire] x2 Keydown double-tap: {}", storage_key);
                        state.pending_macro = double_macro;
                        state.pending_storage_key = None; // fire directly at keyup
                        state.pending_trigger_key = Some(storage_key);
                        state.pending_is_bare = false;
                        return;
                    }
                }
                // First tap — record time and start the single-press timer.
                state.last_hotkey_time.insert(storage_key.clone(), now);
                if let Some(old_cancel) = state.pending_single_cancel.remove(&storage_key) {
                    old_cancel.store(true, Ordering::SeqCst);
                }
                let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
                state
                    .pending_single_cancel
                    .insert(storage_key.clone(), cancel_flag.clone());
                info!("[Keyfire] x1 First tap: {} — waiting {}ms", storage_key, dtw);

                let sk = storage_key.clone();
                let app_clone = app.clone();
                let macro_clone = macro_val.clone();
                drop(state);
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(dtw));
                    if cancel_flag.load(Ordering::SeqCst) {
                        return; // second tap came in — cancelled
                    }
                    {
                        let mut state = engine_state().lock().unwrap();
                        state.pending_single_cancel.remove(&sk);
                        state.last_hotkey_time.remove(&sk);
                    }
                    info!("[Keyfire] x1 Single confirmed: {}", sk);
                    fire_macro(macro_clone, false, Some(sk), &app_clone);
                });
                return;
            } else {
                // No double variant. Hotkey actions fire inline at keydown
                // (no deferred wait); everything else fires at keyup via the
                // pending slot (injection wants clean modifier state).
                let action_type = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if action_type == "hotkey" {
                    if let Some(data) = macro_val.get("data") {
                        if crate::actions::execute_hotkey_inline(data, app) {
                            drop(state);
                            return;
                        }
                    }
                }
                state.pending_macro = Some(macro_val);
                state.pending_storage_key = None;
                state.pending_trigger_key = Some(storage_key);
                state.pending_is_bare = false;
            }
        } else {
            // No single-press — double-only (Pro).
            let double_key = format!("{}::double", storage_key);
            if crate::licence::is_pro() && state.assignments.contains_key(&double_key) {
                hotkey_matched = true;
                crate::expansions::buffer_clear();
                let now = std::time::Instant::now();
                let dtw = state.double_tap_window_ms;
                if let Some(last) = state.last_hotkey_time.get(&storage_key) {
                    if now.duration_since(*last).as_millis() < dtw as u128 {
                        state.last_hotkey_time.remove(&storage_key);
                        info!("[Keyfire] x2 Double-only: {}", storage_key);
                        state.pending_macro = state.assignments.get(&double_key).cloned();
                        state.pending_storage_key = None;
                        state.pending_trigger_key = Some(storage_key);
                        state.pending_is_bare = false;
                        drop(state);
                        return;
                    }
                }
                state.last_hotkey_time.insert(storage_key, now);
            }
        }
        drop(state);

        // Shift-only fallthrough: if no modified hotkey matched and Shift is
        // the only modifier held, route the keystroke through the expansion
        // buffer so triggers requiring Shift (":kr", "?help", uppercase
        // letters) work. Ctrl/Alt/Win combos do NOT fall through.
        if !hotkey_matched && bits == 2 {
            process_expansion_keystroke(key_id, keycode, flags);
        }
    }

    /// Mouse-button dispatch (twin of the Windows handle_mouse_down, minus
    /// the cursor-over / click-to-refocus refinements — on macOS a click
    /// activates the app underneath, and the synchronous frontmost check
    /// below closes most of the watcher's 1.5s poll window).
    fn handle_mouse_down(button: MacMouseButton, flags: u64, app: &AppHandle) {
        if APP_INPUT_FOCUSED.load(Ordering::SeqCst) {
            return;
        }
        if !MACROS_ENABLED.load(Ordering::SeqCst) {
            return;
        }

        let mouse_id = mouse_button_to_key_id(button);

        // Clear any stale pending-release from a previous click cycle so it
        // can't be falsely consumed by a new hold action for this button.
        crate::actions::clear_pending_mouse_release(mouse_id);

        let bits = bits_from_flags(flags);
        let state = engine_state().lock().unwrap();
        let profile = state.active_profile.clone();
        let linked_app = state
            .profile_settings
            .get(&profile)
            .and_then(|s| s.get("linkedApp"))
            .and_then(|v| v.as_str())
            .map(String::from);

        // Fresh frontmost check — bare mouse remaps must not fire when the
        // user clicked into a different app before the watcher's next poll.
        let over_linked = linked_app
            .as_deref()
            .map(crate::foreground::frontmost_app_matches)
            .unwrap_or(false);

        if bits == 0 {
            // Bare mouse — all buttons allowed in app-linked profiles.
            if over_linked {
                let bare_key = format!("{}::BARE::{}", profile, mouse_id);
                if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                    drop(state);
                    dispatch_with_double_tap(&bare_key, macro_val, Some(bare_key.clone()), app);
                } else {
                    // No single — check for double-only bare mouse.
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

        // Modified mouse button — explicit modifier assignment first.
        let combo = build_modifier_combo();
        let storage_key = format!("{}::{}::{}", profile, combo, mouse_id);

        if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
            drop(state);
            // Mouse buttons fire immediately (no deferred-to-keyup).
            dispatch_with_double_tap(&storage_key, macro_val, Some(storage_key.clone()), app);
            return;
        }
        let double_key = format!("{}::double", storage_key);
        if state.assignments.contains_key(&double_key) {
            let dm = state.assignments.get(&double_key).cloned();
            drop(state);
            dispatch_double_only(&storage_key, dm, app);
            return;
        }

        // Fall through to the bare assignment in app-linked profiles: bare
        // mouse remaps act as full button replacements, modifiers pass
        // through naturally since they're physically held.
        if over_linked {
            let bare_key = format!("{}::BARE::{}", profile, mouse_id);
            if let Some(macro_val) = state.assignments.get(&bare_key).cloned() {
                drop(state);
                dispatch_with_double_tap(&bare_key, macro_val, Some(bare_key.clone()), app);
                return;
            }
            let double_key = format!("{}::double", bare_key);
            if state.assignments.contains_key(&double_key) {
                let dm = state.assignments.get(&double_key).cloned();
                drop(state);
                dispatch_double_only(&bare_key, dm, app);
            }
        }
    }

    fn handle_mouse_up(button: MacMouseButton, app: &AppHandle) {
        // Release a held chord if this button was the hold's trigger
        // (press-hold mirroring). The pending-release fallback is only
        // allowed for buttons that actually carry a hold assignment —
        // otherwise every ordinary click would clobber the slot.
        let mouse_id = mouse_button_to_key_id(button);
        let allow_pending = button_has_hold_assignment(mouse_id);
        if let Some(label) =
            crate::actions::release_held_if_mouse_trigger(mouse_id, allow_pending)
        {
            crate::tray::update_tray_icon_normal(app);
            info!("[Keyfire] Mouse-up released hold: {}", label);
        }
    }

    /// True if any assignment (any profile, any combo, incl. ::double) is
    /// triggered by this mouse button with holdMode enabled. Cheap map scan
    /// on the processor thread — never in the tap callback.
    fn button_has_hold_assignment(mouse_id: &str) -> bool {
        let single_suffix = format!("::{}", mouse_id);
        let double_suffix = format!("::{}::double", mouse_id);
        let state = engine_state().lock().unwrap();
        state.assignments.iter().any(|(k, v)| {
            (k.ends_with(&single_suffix) || k.ends_with(&double_suffix))
                && v.get("data")
                    .and_then(|d| d.get("holdMode"))
                    .and_then(|h| h.as_bool())
                    .unwrap_or(false)
        })
    }

    /// Double-only dispatch for mouse: no single-press action exists. First
    /// click records time, second click within the window fires.
    fn dispatch_double_only(storage_key: &str, double_macro: Option<Value>, app: &AppHandle) {
        // Pro gate: Free users never fire double-only assignments.
        if !crate::licence::is_pro() {
            return;
        }
        let mut state = engine_state().lock().unwrap();
        let now = std::time::Instant::now();
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

    /// Post a synthetic tap of `keycode` carrying the CURRENT modifier state
    /// so hold-passthrough taps compose with modifiers the user still holds.
    /// Tagged — it reaches the app but never re-enters the matcher.
    fn synthetic_tap_with_mods(keycode: u16) {
        let f = CGEventFlags::from_bits_truncate({
            let mut bits: u64 = 0;
            if MOD_CTRL.load(Ordering::SeqCst) {
                bits |= CGEventFlags::CGEventFlagControl.bits();
            }
            if MOD_SHIFT.load(Ordering::SeqCst) {
                bits |= CGEventFlags::CGEventFlagShift.bits();
            }
            if MOD_ALT.load(Ordering::SeqCst) {
                bits |= CGEventFlags::CGEventFlagAlternate.bits();
            }
            if MOD_META.load(Ordering::SeqCst) {
                bits |= CGEventFlags::CGEventFlagCommand.bits();
            }
            bits
        });
        crate::actions::post_tap_keycode(keycode, f.bits());
    }

    /// Keyup bookkeeping: bare-remap release, hold-cycle resolution, then the
    /// pending fire. Twin of the Windows handle_keyup (minus voice/radial,
    /// which are later milestones).
    fn handle_keyup(keycode: u16, app: &AppHandle) {
        // Release phase of an AHK-style bare-key remap.
        if crate::actions::remap_key_release(keycode) {
            return;
        }

        // ── Hold trigger: trigger-key release ends the hold cycle ──────────
        // fired == true → the watcher already fired the hold; suppress all.
        // fired == false → released before threshold; re-inject the dispatch
        // that keydown deferred.
        let removed = {
            let mut timers = hold_timers().lock().unwrap();
            timers.remove(&keycode)
        };
        if let Some(entry) = removed {
            if !entry.fired {
                if let Some(single) = entry.single_macro {
                    if entry.has_double {
                        // Single + double + hold, released early: the single
                        // waits out the double window on a cancelable timer —
                        // a second tap cancels it via pending_single_cancel.
                        let sk = entry.storage_key.clone();
                        let is_bare = entry.is_bare;
                        let mut state = engine_state().lock().unwrap();
                        let dtw = state.double_tap_window_ms;
                        if let Some(old_cancel) = state.pending_single_cancel.remove(&sk) {
                            old_cancel.store(true, Ordering::SeqCst);
                        }
                        let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
                        state.pending_single_cancel.insert(sk.clone(), cancel_flag.clone());
                        drop(state);
                        let app_clone = app.clone();
                        thread::spawn(move || {
                            thread::sleep(Duration::from_millis(dtw));
                            if cancel_flag.load(Ordering::SeqCst) {
                                return; // second tap arrived — double fired instead
                            }
                            {
                                let mut state = engine_state().lock().unwrap();
                                state.pending_single_cancel.remove(&sk);
                                state.last_hotkey_time.remove(&sk);
                            }
                            info!("[Keyfire] x1 Single confirmed (hold-deferred): {}", sk);
                            fire_macro(single, is_bare, Some(sk), &app_clone);
                        });
                    } else {
                        // Single + hold only: fire through the pending route
                        // so injection waits for clean modifier state.
                        if let Ok(mut state) = engine_state().lock() {
                            state.pending_macro = Some(single);
                            state.pending_storage_key = None;
                            state.pending_trigger_key = Some(entry.storage_key.clone());
                            state.pending_is_bare = entry.is_bare;
                        }
                    }
                } else if entry.has_double {
                    // Hold + double, NO single — defer the passthrough tap
                    // through the dtw window; a second tap fires the double
                    // and cancels this.
                    let sk = entry.storage_key.clone();
                    let kc = keycode;
                    let mut state = engine_state().lock().unwrap();
                    let dtw = state.double_tap_window_ms;
                    if let Some(old_cancel) = state.pending_single_cancel.remove(&sk) {
                        old_cancel.store(true, Ordering::SeqCst);
                    }
                    let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
                    state.pending_single_cancel.insert(sk.clone(), cancel_flag.clone());
                    drop(state);
                    thread::spawn(move || {
                        thread::sleep(Duration::from_millis(dtw));
                        if cancel_flag.load(Ordering::SeqCst) {
                            return;
                        }
                        {
                            let mut state = engine_state().lock().unwrap();
                            state.pending_single_cancel.remove(&sk);
                            state.last_hotkey_time.remove(&sk);
                        }
                        info!("[Keyfire] [HOLD] tap passthrough (hold+double, no single): {}", sk);
                        synthetic_tap_with_mods(kc);
                    });
                } else {
                    // Hold-only — immediate passthrough tap so the app's
                    // native key behaviour fires (the tap suppressed the
                    // user's physical keydown).
                    info!("[Keyfire] [HOLD] tap passthrough (hold-only): {}", entry.storage_key);
                    synthetic_tap_with_mods(keycode);
                }
            }
        }

        maybe_fire_pending(app);
    }

    /// Fire the pending macro once all modifiers are released. Called from
    /// both KeyUp and the final FlagsChanged release (on Windows both arrive
    /// as keyups; on mac they're two event types).
    fn maybe_fire_pending(app: &AppHandle) {
        if modifier_bits() != 0 {
            return;
        }
        let taken = engine_state().lock().ok().and_then(|mut state| {
            state.pending_macro.take().map(|m| {
                (
                    m,
                    state.pending_storage_key.take(),
                    state.pending_trigger_key.take(),
                    std::mem::take(&mut state.pending_is_bare),
                )
            })
        });
        if let Some((macro_val, storage_key, trigger_key, is_bare)) = taken {
            if let Some(sk) = storage_key {
                // Has a storage key → resolve double-tap at keyup.
                dispatch_with_double_tap(&sk, macro_val, trigger_key, app);
            } else {
                // Double already resolved at keydown, or no double variant.
                fire_macro(macro_val, is_bare, trigger_key, app);
            }
        }
    }

    /// Keyup-time double-tap resolution for pendings that carry a storage
    /// key (bare keys). Twin of the Windows dispatch_with_double_tap.
    fn dispatch_with_double_tap(
        storage_key: &str,
        macro_val: Value,
        trigger_key: Option<String>,
        app: &AppHandle,
    ) {
        let mut state = engine_state().lock().unwrap();
        let double_key = format!("{}::double", storage_key);
        // Pro gate: Free users get single-press only.
        let double_macro = if crate::licence::is_pro() {
            state.assignments.get(&double_key).cloned()
        } else {
            None
        };

        if double_macro.is_none() {
            drop(state);
            fire_macro(macro_val, false, trigger_key, app);
            return;
        }

        let dtw = state.double_tap_window_ms;
        let now = std::time::Instant::now();

        if let Some(last) = state.last_hotkey_time.get(storage_key) {
            if now.duration_since(*last).as_millis() < dtw as u128 {
                // Second tap within window → fire double.
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

        // First tap — schedule the single after the double-tap window.
        state.last_hotkey_time.insert(storage_key.to_string(), now);
        if let Some(old_cancel) = state.pending_single_cancel.remove(storage_key) {
            old_cancel.store(true, Ordering::SeqCst);
        }
        let cancel_flag = std::sync::Arc::new(AtomicBool::new(false));
        state
            .pending_single_cancel
            .insert(storage_key.to_string(), cancel_flag.clone());
        info!("[Keyfire] x1 First tap: {} — waiting {}ms", storage_key, dtw);

        let sk = storage_key.to_string();
        let app_clone = app.clone();
        drop(state);

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(dtw));
            if cancel_flag.load(Ordering::SeqCst) {
                return; // second tap came in — cancelled
            }
            {
                let mut state = engine_state().lock().unwrap();
                state.pending_single_cancel.remove(&sk);
                state.last_hotkey_time.remove(&sk);
            }
            info!("[Keyfire] x1 Single confirmed: {}", sk);
            fire_macro(macro_val, false, Some(sk), &app_clone);
        });
    }

    /// Twin of the Windows fire_macro: spawn a worker so the processor never
    /// blocks, execute, log analytics, notify the frontend.
    fn fire_macro(macro_val: Value, is_bare: bool, trigger_key: Option<String>, app: &AppHandle) {
        // Re-press cancel — if a loop is already running for this trigger,
        // pressing it again is the canonical stop gesture. Set the cancel
        // flag and bail before any thread spawn happens.
        if let Some(ref key) = trigger_key {
            if crate::actions::cancel_loop_if_running(key) {
                info!("[Keyfire] Loop cancel signal: {}", key);
                return;
            }
        }

        // H1 re-entrancy guard — if the same trigger is mid-flight, drop the
        // new fire (per-storage-key; different macros still run concurrently).
        // The guard travels into the spawned thread; Drop releases it.
        let macro_guard = if let Some(ref key) = trigger_key {
            match crate::actions::MacroRunningGuard::try_acquire(key) {
                Some(g) => Some(g),
                None => {
                    warn!(
                        "[Keyfire] Dropped re-fire: {} already running (H1 re-entrancy guard)",
                        key
                    );
                    return;
                }
            }
        } else {
            None
        };

        let app_clone = app.clone();
        thread::spawn(move || {
            let _macro_guard = macro_guard;
            crate::actions::execute_action(
                &macro_val,
                is_bare,
                0, // no HWNDs on macOS — frontmost app keeps focus
                false,
                trigger_key.as_deref(),
                &app_clone,
            );

            let action_type = macro_val.get("type").and_then(|v| v.as_str()).unwrap_or("hotkey");
            let label = macro_val.get("label").and_then(|v| v.as_str()).unwrap_or("");
            let trigger = trigger_key.as_deref().unwrap_or("");
            let macro_steps = if action_type == "macro" {
                macro_val
                    .get("data")
                    .and_then(|d| d.get("steps"))
                    .and_then(|s| s.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|s| s.get("type").and_then(|v| v.as_str()).map(String::from))
                            .collect()
                    })
            } else {
                None
            };
            crate::analytics::log_action_ext(action_type, 0, trigger, label, macro_steps);

            let _ = app_clone.emit(
                "macro-fired",
                serde_json::json!({
                    "label": label,
                    "type": macro_val.get("type").and_then(|v| v.as_str()).unwrap_or(""),
                }),
            );
        });
    }

    fn get_status_json() -> Value {
        super::get_engine_status()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn keycode_table_round_trips() {
            for kc in 0u16..=126 {
                if let Some(id) = keycode_to_key_id(kc) {
                    assert_eq!(
                        key_id_to_keycode(id),
                        Some(kc),
                        "round-trip failed for {} (keycode {})",
                        id,
                        kc
                    );
                }
            }
        }

        #[test]
        fn parse_combo_matches_storage_semantics() {
            // ⌘ is stored as Win (hard rule 6); V is keycode 9.
            assert_eq!(parse_hotkey_combo("Ctrl+Shift+V"), Some((3, 9)));
            assert_eq!(parse_hotkey_combo("Win+Space"), Some((8, 49)));
            assert_eq!(parse_hotkey_combo("Ctrl+Space"), Some((1, 49)));
            assert_eq!(parse_hotkey_combo("Ctrl+`"), Some((1, 50)));
            assert_eq!(parse_hotkey_combo("Alt+Up"), Some((4, 126)));
            assert_eq!(parse_hotkey_combo("Ctrl+1"), Some((1, 18)));
            assert_eq!(parse_hotkey_combo(""), None);
        }

        #[test]
        fn suppress_rebuild_uses_active_profile_and_specials() {
            let mut state = EngineState::default();
            state.active_profile = "Work".into();
            state.assignments.insert(
                "Work::Ctrl+Shift::KeyK".into(),
                serde_json::json!({"type": "text"}),
            );
            // Other profile, double variant and GLOBAL entries must all be
            // excluded.
            state.assignments.insert(
                "Home::Ctrl::KeyJ".into(),
                serde_json::json!({"type": "text"}),
            );
            state.assignments.insert(
                "Work::Ctrl+Shift::KeyK::double".into(),
                serde_json::json!({"type": "text"}),
            );
            state.assignments.insert(
                "GLOBAL::EXPANSION::foo".into(),
                serde_json::json!({"type": "text"}),
            );
            // Bare keys: static profile (no linkedApp) suppresses F5 (in the
            // static-allowed set) but NOT KeyZ (character key — suppressing
            // it would eat normal typing).
            state
                .assignments
                .insert("Work::BARE::F5".into(), serde_json::json!({"type": "text"}));
            state
                .assignments
                .insert("Work::BARE::KeyZ".into(), serde_json::json!({"type": "text"}));
            state.overlay_hotkey = Some((1, 49));
            state.pause_hotkey = Some((5, 35));
            state.clipboard_paste_hotkey = Some((3, 9));

            rebuild_suppress_keys(&state);
            let set = suppress_keys().read().unwrap();
            // KeyK keycode is 40; Ctrl+Shift = bits 3. F5 keycode is 96.
            assert!(set.contains(&(3, 40)));
            assert!(set.contains(&(0, 96)));
            assert!(!set.contains(&(0, 6))); // KeyZ not suppressed (static profile)
            assert!(set.contains(&(1, 49)));
            assert!(set.contains(&(5, 35)));
            assert!(set.contains(&(3, 9)));
            assert_eq!(set.len(), 5);
        }

        #[test]
        fn suppress_rebuild_linked_profile_allows_character_bare_keys() {
            let mut state = EngineState::default();
            state.active_profile = "Game".into();
            state.profile_settings.insert(
                "Game".into(),
                serde_json::json!({"linkedApp": "some-game"}),
            );
            state
                .assignments
                .insert("Game::BARE::KeyZ".into(), serde_json::json!({"type": "text"}));
            // Double-only bare keys let the first press through.
            state.assignments.insert(
                "Game::BARE::KeyX::double".into(),
                serde_json::json!({"type": "text"}),
            );
            state.overlay_hotkey = None;
            state.pause_hotkey = None;
            state.clipboard_paste_hotkey = None;

            rebuild_suppress_keys(&state);
            let set = suppress_keys().read().unwrap();
            assert!(set.contains(&(0, 6))); // KeyZ suppressed (linked profile)
            assert!(!set.contains(&(0, 7))); // KeyX double-only — not suppressed
        }
    }
}

/// Display-name → mac keycode bridge for other stub modules (actions.rs
/// Send Hotkey uses the same key-name universe as combo strings).
#[cfg(target_os = "macos")]
pub(crate) fn display_name_to_keycode(name: &str) -> Option<u16> {
    macos::display_name_to_keycode(name)
}

/// Combo string → modifier bits (shared with the macos module; token names
/// are the cross-platform storage tokens, hard rule 6).
fn hotkeys_combo_bits(combo: &str) -> u8 {
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

pub fn handle_js_key_event(code: &str, ctrl: bool, shift: bool, alt: bool, meta: bool, app: &AppHandle) {}

pub fn is_radial_menu_held() -> bool {
    false
}

pub fn is_voice_active() -> bool {
    false
}

/// Parse a combo string like "Ctrl+Space" into (modifier_bits, key code).
/// On macOS the key code slot carries a mac virtual keycode; the Windows
/// original carries a VK. Callers only round-trip the tuple into engine
/// state, so the currency difference stays internal.
pub fn parse_hotkey_combo(combo: &str) -> Option<(u8, u32)> {
    #[cfg(target_os = "macos")]
    {
        macos::parse_hotkey_combo(combo)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

pub fn set_active_profile(profile: String) {
    if let Ok(mut s) = engine_state().lock() {
        s.active_profile = profile;
        rebuild_suppress(&s);
    }
}

pub fn update_assignments(assignments: HashMap<String, Value>, profile: String) {
    if let Ok(mut s) = engine_state().lock() {
        s.assignments = assignments;
        s.active_profile = profile;
        rebuild_suppress(&s);
    }
    // Armed hold timers hold clones of the OLD macros — drop them.
    #[cfg(target_os = "macos")]
    macos::clear_hold_timers();
}

pub fn update_profile_settings(settings: HashMap<String, Value>) {
    if let Ok(mut s) = engine_state().lock() {
        s.profile_settings = settings;
        rebuild_suppress(&s);
    }
}

pub fn update_global_settings(settings: &Value) {
    if let Ok(mut state) = engine_state().lock() {
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
        if let Some(v) = settings.get("keystrokeDelay").and_then(|v| v.as_u64()) {
            state.custom_keystroke_delay = v;
        }
        if let Some(v) = settings.get("macroTriggerDelay").and_then(|v| v.as_u64()) {
            state.custom_pre_execution_delay = v;
        }
        if let Some(s) = settings.get("defaultDateFormat").and_then(|v| v.as_str()) {
            if matches!(s, "DD/MM/YYYY" | "MM/DD/YYYY" | "YYYY-MM-DD") {
                state.default_date_format = s.to_string();
            }
        }
    }
}

pub fn set_capturing(capturing: bool) {
    IS_CAPTURING_KEY.store(capturing, Ordering::SeqCst);
    if capturing {
        if let Ok(mut s) = engine_state().lock() {
            s.capture_sole_modifier = None;
        }
    }
}

pub fn set_input_focused(focused: bool) {
    APP_INPUT_FOCUSED.store(focused, Ordering::SeqCst);
}

pub fn set_macros_enabled(enabled: bool) {
    MACROS_ENABLED.store(enabled, Ordering::SeqCst);
}

pub fn set_recording(recording: bool) {
    IS_RECORDING_HOTKEY.store(recording, Ordering::SeqCst);
}

pub fn set_clipboard_paste_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        if let Ok(mut state) = engine_state().lock() {
            state.clipboard_paste_hotkey = Some(parsed);
            rebuild_suppress(&state);
            log::info!(
                "[HOOK] Clipboard paste hotkey set: {} → bits={} key={}",
                combo, parsed.0, parsed.1
            );
        }
    }
}

pub fn set_overlay_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        if let Ok(mut state) = engine_state().lock() {
            state.overlay_hotkey = Some(parsed);
            rebuild_suppress(&state);
            log::info!(
                "[HOOK] Overlay hotkey set: {} → bits={} key={}",
                combo, parsed.0, parsed.1
            );
        }
    }
}

pub fn set_pause_hotkey(combo: &str) {
    if let Some(parsed) = parse_hotkey_combo(combo) {
        if let Ok(mut state) = engine_state().lock() {
            state.pause_hotkey = Some(parsed);
            state.pause_hotkey_str = Some(combo.to_string());
            rebuild_suppress(&state);
            log::info!(
                "[HOOK] Pause hotkey set: {} → bits={} key={}",
                combo, parsed.0, parsed.1
            );
        }
    }
}

// Voice / radial combos are parsed and stored so config round-trips, but
// their processor paths are later milestones — they are NOT added to the
// suppress set (a suppressed-but-unfired key would be dead).
pub fn set_radial_menu_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        state.radial_menu_hotkey = parse_hotkey_combo(combo);
    }
}

// Quick Record hotkeys: the tuple's key slot carries a mac keycode; the
// recorder statics get the same value (the tap callback matches keycodes
// against them — consistent currency within the platform, and recorded
// STREAMS are stored in VK terms regardless).
pub fn set_temp_macro_loop_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        let parsed = parse_hotkey_combo(combo);
        state.temp_macro_loop_hotkey = parsed;
        state.temp_macro_loop_hotkey_str = (!combo.is_empty()).then(|| combo.to_string());
        let (bits, key) = parsed.unwrap_or((0, 0));
        crate::recorder::TEMP_MACRO_LOOP_BITS.store(bits, Ordering::SeqCst);
        crate::recorder::TEMP_MACRO_LOOP_VK.store(key, Ordering::SeqCst);
    }
}

pub fn set_temp_macro_play_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        let parsed = parse_hotkey_combo(combo);
        state.temp_macro_play_hotkey = parsed;
        state.temp_macro_play_hotkey_str = (!combo.is_empty()).then(|| combo.to_string());
        let (bits, key) = parsed.unwrap_or((0, 0));
        crate::recorder::TEMP_MACRO_PLAY_BITS.store(bits, Ordering::SeqCst);
        crate::recorder::TEMP_MACRO_PLAY_VK.store(key, Ordering::SeqCst);
    }
}

pub fn set_temp_macro_record_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        let parsed = parse_hotkey_combo(combo);
        state.temp_macro_record_hotkey = parsed;
        state.temp_macro_record_hotkey_str = (!combo.is_empty()).then(|| combo.to_string());
        let (bits, key) = parsed.unwrap_or((0, 0));
        crate::recorder::TEMP_MACRO_RECORD_BITS.store(bits, Ordering::SeqCst);
        crate::recorder::TEMP_MACRO_RECORD_VK.store(key, Ordering::SeqCst);
    }
}

pub fn set_voice_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        state.voice_hotkey = parse_hotkey_combo(combo);
    }
}

/// Re-derive the suppress set after the clipboard capture toggle flips —
/// the clipboard-paste combo must stop being suppressed while capture is
/// off (twin of the Windows refresh_clipboard_paste_suppress).
pub fn refresh_clipboard_paste_suppress() {
    if let Ok(state) = engine_state().lock() {
        rebuild_suppress(&state);
    }
}

pub fn clear_clipboard_paste_hotkey() {
    if let Ok(mut state) = engine_state().lock() {
        state.clipboard_paste_hotkey = None;
        rebuild_suppress(&state);
    }
}

pub fn clear_overlay_opened_flag() {}

pub fn clear_pause_hotkey() {
    if let Ok(mut state) = engine_state().lock() {
        state.pause_hotkey = None;
        state.pause_hotkey_str = None;
        rebuild_suppress(&state);
    }
}

pub fn clear_radial_menu_hotkey() {
    if let Ok(mut state) = engine_state().lock() {
        state.radial_menu_hotkey = None;
    }
}

pub fn clear_radial_menu_open() {}

pub fn clear_voice_active() {}

pub fn clear_voice_hotkey() {
    if let Ok(mut state) = engine_state().lock() {
        state.voice_hotkey = None;
    }
}
