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
    pub(crate) pending_trigger_key: Option<String>,
    pub(crate) pending_is_bare: bool,
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
            pending_trigger_key: None,
            pending_is_bare: false,
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
            let mut created: Option<(CGEventTap, bool)> = None;
            for (options, can_suppress) in [
                (CGEventTapOptions::Default, true),
                (CGEventTapOptions::ListenOnly, false),
            ] {
                let cb_sender = sender.clone();
                match CGEventTap::new(
                    // Session tap sees events for the whole login session — the
                    // closest analogue to the Windows WH_KEYBOARD_LL global hook.
                    CGEventTapLocation::Session,
                    CGEventTapPlacement::HeadInsertEventTap,
                    options,
                    vec![
                        CGEventType::KeyDown,
                        CGEventType::KeyUp,
                        CGEventType::FlagsChanged,
                    ],
                    move |_proxy, etype, event| tap_callback(&cb_sender, etype, event),
                ) {
                    Ok(tap) => {
                        created = Some((tap, can_suppress));
                        break;
                    }
                    Err(()) => continue,
                }
            }

            let (tap, can_suppress) = match created {
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
                    "[HOOK] CGEventTap installed (session, ACTIVE — suppression enabled) \
                     after {} attempt(s) — pumping run loop",
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
            // discipline. Every CGEvent actions.rs posts is stamped with the
            // magic source-user-data tag (single fast field read; allowed in
            // the callback). Matched only for key/flag events: the
            // TapDisabled pseudo-events below are OS-generated and must
            // always be handled.
            CGEventType::KeyDown | CGEventType::KeyUp | CGEventType::FlagsChanged
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

    // ── Suppress-set rebuild ─────────────────────────────────────────────────

    /// Recompute the (bits, keycode) suppress set from the active profile's
    /// assignments plus the handled special hotkeys. Called with the
    /// engine_state lock HELD (mirror of the Windows contract — must not
    /// re-lock).
    ///
    /// Deliberately excluded until their milestones land (an unsuppressed key
    /// still reaches the target app; a suppressed-but-unfired key is dead):
    /// BARE keys, ::double, ::hold, voice, radial menu, Quick Record combos.
    pub(super) fn rebuild_suppress_keys(state: &EngineState) {
        let mut set: HashSet<(u8, u16)> = HashSet::new();
        let prefix = format!("{}::", state.active_profile);
        for key in state.assignments.keys() {
            if !key.starts_with(&prefix) {
                continue;
            }
            let parts: Vec<&str> = key.split("::").collect();
            if parts.len() < 3 {
                continue;
            }
            let combo_str = parts[1];
            if combo_str == "GLOBAL" || combo_str == "BARE" {
                continue;
            }
            // ::double / ::hold variants don't suppress on mac yet — the
            // matcher can't fire them, and the plain entry (if any) already
            // adds the key.
            if matches!(parts.last(), Some(&"double") | Some(&"hold")) {
                continue;
            }
            let bits = super::hotkeys_combo_bits(combo_str);
            if bits == 0 {
                continue;
            }
            if let Some(kc) = key_id_to_keycode(parts[2]) {
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
        info!("[HOOK] Rebuilt suppress set: {} combos", set.len());
        if let Ok(mut w) = suppress_keys().write() {
            *w = set;
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

    /// Runs on the processor thread. Owns modifier-state tracking, matching,
    /// dispatch and logging.
    fn process_events(receiver: mpsc::Receiver<TapEvent>, app: AppHandle) {
        info!("[HOOK] Event processor started");
        while let Ok(ev) = receiver.recv() {
            match ev {
                TapEvent::KeyDown {
                    keycode,
                    flags,
                    is_repeat,
                } => {
                    update_modifiers(flags);
                    handle_keydown(keycode as u16, flags, is_repeat, &app);
                }
                TapEvent::KeyUp { keycode, flags } => {
                    update_modifiers(flags);
                    maybe_fire_pending(&app);
                }
                TapEvent::FlagsChanged { keycode, flags } => {
                    handle_flags_changed(keycode as u16, flags, &app);
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
            // Bare-key matching is a later milestone — bare printables drive
            // the text-expansion buffer instead.
            process_expansion_keystroke(key_id, keycode, flags);
            return;
        }

        let combo = build_modifier_combo();
        let mut state = match engine_state().lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let storage_key = format!("{}::{}::{}", state.active_profile, combo, key_id);

        let mut hotkey_matched = false;
        if let Some(macro_val) = state.assignments.get(&storage_key).cloned() {
            hotkey_matched = true;
            crate::expansions::buffer_clear();
            let double_key = format!("{}::double", storage_key);
            let hold_key = format!("{}::hold", storage_key);
            if state.assignments.contains_key(&double_key)
                || state.assignments.contains_key(&hold_key)
            {
                info!(
                    "[Keyfire] {} has double/hold variants — not supported on macOS yet; \
                     firing the single mapping",
                    storage_key
                );
            }
            // Fire at keyup via the pending slot (injection wants clean
            // modifier state) — the Windows plain-single path.
            state.pending_macro = Some(macro_val);
            state.pending_trigger_key = Some(storage_key);
            state.pending_is_bare = false;
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
                    state.pending_trigger_key.take(),
                    std::mem::take(&mut state.pending_is_bare),
                )
            })
        });
        if let Some((macro_val, trigger_key, is_bare)) = taken {
            fire_macro(macro_val, is_bare, trigger_key, app);
        }
    }

    /// Twin of the Windows fire_macro: spawn a worker so the processor never
    /// blocks, execute, log analytics, notify the frontend. The Windows loop-
    /// cancel and per-trigger re-entrancy guards live in actions.rs statics
    /// that don't exist on mac yet — they arrive with the macro milestone.
    fn fire_macro(macro_val: Value, is_bare: bool, trigger_key: Option<String>, app: &AppHandle) {
        let app_clone = app.clone();
        thread::spawn(move || {
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
            // Other profile, double variant, bare and GLOBAL entries must all
            // be excluded.
            state.assignments.insert(
                "Home::Ctrl::KeyJ".into(),
                serde_json::json!({"type": "text"}),
            );
            state.assignments.insert(
                "Work::Ctrl+Shift::KeyK::double".into(),
                serde_json::json!({"type": "text"}),
            );
            state
                .assignments
                .insert("Work::BARE::F5".into(), serde_json::json!({"type": "text"}));
            state.assignments.insert(
                "GLOBAL::EXPANSION::foo".into(),
                serde_json::json!({"type": "text"}),
            );
            state.overlay_hotkey = Some((1, 49));
            state.pause_hotkey = Some((5, 35));
            state.clipboard_paste_hotkey = Some((3, 9));

            rebuild_suppress_keys(&state);
            let set = suppress_keys().read().unwrap();
            // KeyK keycode is 40; Ctrl+Shift = bits 3.
            assert!(set.contains(&(3, 40)));
            assert!(set.contains(&(1, 49)));
            assert!(set.contains(&(5, 35)));
            assert!(set.contains(&(3, 9)));
            assert_eq!(set.len(), 4);
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

// Voice / radial / Quick Record combos are parsed and stored so config
// round-trips, but their processor paths are later milestones — they are NOT
// added to the suppress set (a suppressed-but-unfired key would be dead).
pub fn set_radial_menu_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        state.radial_menu_hotkey = parse_hotkey_combo(combo);
    }
}

pub fn set_temp_macro_loop_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        state.temp_macro_loop_hotkey = parse_hotkey_combo(combo);
        state.temp_macro_loop_hotkey_str =
            (!combo.is_empty()).then(|| combo.to_string());
    }
}

pub fn set_temp_macro_play_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        state.temp_macro_play_hotkey = parse_hotkey_combo(combo);
        state.temp_macro_play_hotkey_str =
            (!combo.is_empty()).then(|| combo.to_string());
    }
}

pub fn set_temp_macro_record_hotkey(combo: &str) {
    if let Ok(mut state) = engine_state().lock() {
        state.temp_macro_record_hotkey = parse_hotkey_combo(combo);
        state.temp_macro_record_hotkey_str =
            (!combo.is_empty()).then(|| combo.to_string());
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
