//! Non-Windows hotkey engine. The real hotkeys.rs is built on Win32 low-level
//! hooks and only compiles on Windows. This twin exposes the exact surface
//! lib.rs and shared modules reference, so the app builds and boots on other
//! platforms.
//!
//! Mac port Phase 2, milestone 1 (`port/mac-hooks`): `start_hooks` now installs
//! a listen-only CGEventTap on macOS. A dedicated thread owns the tap and pumps
//! its CFRunLoop (the "hook thread"); a second thread drains an mpsc channel and
//! does all logging + modifier-state tracking (the "processor thread"). This
//! mirrors the Windows split — the tap callback only ingests, the processor
//! decides — and honours the hard rule that the tap callback never blocks, logs,
//! or does I/O (macOS disables taps that stall). Injection, suppression, and the
//! double-tap/hold state machine come in later milestones; for now the tap is
//! ListenOnly and every event passes through untouched.
//!
//! On non-macOS non-Windows targets (e.g. Linux CI) `start_hooks` stays a no-op.
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

/// Live modifier state, updated by the processor thread from CGEvent flags.
/// The processor decides; the tap callback only forwards raw flag bits. These
/// mirror the Windows `MOD_*` atomics and back the (future) hotkey matcher.
static MOD_CTRL: AtomicBool = AtomicBool::new(false);
static MOD_SHIFT: AtomicBool = AtomicBool::new(false);
static MOD_ALT: AtomicBool = AtomicBool::new(false);
static MOD_META: AtomicBool = AtomicBool::new(false);

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
        "uiohookAvailable": HOOKS_RUNNING.load(Ordering::SeqCst),
        "nutjsAvailable": false,
        "macrosEnabled": false,
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

// ── macOS CGEventTap engine (listen-only spike) ─────────────────────────────
#[cfg(target_os = "macos")]
mod macos {
    use super::{HOOKS_RUNNING, MOD_ALT, MOD_CTRL, MOD_META, MOD_SHIFT};
    use core_foundation::base::TCFType;
    use core_foundation::mach_port::CFMachPortRef;
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement,
        CGEventType, CallbackResult, EventField,
    };
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;
    use tauri::AppHandle;

    /// Raw CFMachPortRef of the live tap, stashed so the tap callback can
    /// re-enable itself after the OS disables it (see `TapDisabledByTimeout`).
    /// We can't borrow the `CGEventTap` from inside its own callback, so we go
    /// through the raw port + `CGEventTapEnable`. `isize` because pointers
    /// aren't `Sync`; 0 means "no tap yet".
    static TAP_PORT: AtomicIsize = AtomicIsize::new(0);

    // `CGEventTapEnable` is not re-exported by core-graphics. Bind it directly;
    // the CoreGraphics framework is already linked by the crate.
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    }

    /// Lightweight message from the tap callback (hook thread) to the processor
    /// thread. `Copy`, no allocation — cheap to send from the callback.
    #[derive(Clone, Copy)]
    enum TapEvent {
        KeyDown { keycode: i64, flags: u64 },
        KeyUp { keycode: i64, flags: u64 },
        FlagsChanged { flags: u64 },
        /// OS disabled the tap. The callback already re-enabled it; this is
        /// just so the processor logs. `by_user_input` distinguishes the two
        /// causes: `true` = kCGEventTapDisabledByUserInput (Secure Input mode,
        /// e.g. a terminal with Secure Keyboard Entry or a password field —
        /// key events are withheld from all taps), `false` =
        /// kCGEventTapDisabledByTimeout (our callback stalled).
        Disabled { by_user_input: bool },
    }

    pub fn start_hooks(app: AppHandle) {
        if HOOKS_RUNNING.load(Ordering::SeqCst) {
            return;
        }

        let (sender, receiver) = mpsc::channel::<TapEvent>();

        // Processor thread: drains the channel, tracks modifier state, logs.
        // All I/O lives here, never in the tap callback.
        thread::Builder::new()
            .name("keyfire-tap-processor".to_string())
            .spawn(move || process_events(receiver))
            .expect("failed to spawn tap processor thread");

        // Hook thread: owns the tap and pumps its CFRunLoop forever.
        thread::Builder::new()
            .name("keyfire-tap-hook".to_string())
            .spawn(move || run_tap(sender))
            .expect("failed to spawn tap hook thread");

        // `app` is unused in the listen-only spike — later milestones emit
        // Tauri events (overlay, toasts) back through it.
        let _ = app;
    }

    /// Runs on the hook thread. Creates the tap, installs it on this thread's
    /// run loop, and blocks in `CFRunLoop::run_current()`. If tap creation
    /// fails (Input Monitoring not yet granted), retries so the human can grant
    /// the TCC prompt without relaunching.
    fn run_tap(sender: mpsc::Sender<TapEvent>) {
        const RETRY_DELAY: Duration = Duration::from_secs(3);
        let mut attempt: u32 = 0;

        loop {
            attempt += 1;
            let cb_sender = sender.clone();
            let tap = CGEventTap::new(
                // Session tap sees events for the whole login session — the
                // closest analogue to the Windows WH_KEYBOARD_LL global hook.
                CGEventTapLocation::Session,
                CGEventTapPlacement::HeadInsertEventTap,
                // Listen-only for the spike: never suppresses, so a bug here
                // can't wedge the user's keyboard.
                CGEventTapOptions::ListenOnly,
                vec![
                    CGEventType::KeyDown,
                    CGEventType::KeyUp,
                    CGEventType::FlagsChanged,
                ],
                move |_proxy, etype, event| {
                    tap_callback(&cb_sender, etype, event);
                    // Ignored under ListenOnly, but must return something.
                    CallbackResult::Keep
                },
            );

            let tap = match tap {
                Ok(tap) => tap,
                Err(()) => {
                    if attempt == 1 {
                        log::warn!(
                            "[HOOK] CGEventTap creation failed — grant Keyfire (or your \
                             terminal, in `cargo tauri dev`) Input Monitoring under System \
                             Settings › Privacy & Security. Retrying every {}s…",
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

            let run_loop = CFRunLoop::get_current();
            unsafe {
                run_loop.add_source(&loop_source, kCFRunLoopCommonModes);
            }
            tap.enable();

            HOOKS_RUNNING.store(true, Ordering::SeqCst);
            log::info!(
                "[HOOK] CGEventTap installed (session, listen-only) after {} attempt(s) — \
                 pumping run loop",
                attempt
            );

            // Blocks forever pumping the tap. `tap` stays alive on this stack,
            // so it isn't dropped (and thus invalidated) while we run.
            CFRunLoop::run_current();

            // Only reached if the run loop is ever stopped — treat as teardown.
            HOOKS_RUNNING.store(false, Ordering::SeqCst);
            TAP_PORT.store(0, Ordering::SeqCst);
            log::warn!("[HOOK] event tap run loop exited");
            return;
        }
    }

    /// Runs on the hook thread (tap callback). MUST NOT block, log, or do I/O —
    /// macOS disables taps whose callback stalls. Extract the minimum and hand
    /// off to the processor thread.
    fn tap_callback(
        sender: &mpsc::Sender<TapEvent>,
        etype: CGEventType,
        event: &core_graphics::event::CGEvent,
    ) {
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
                return;
            }
            CGEventType::KeyDown => {
                let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                let flags = event.get_flags().bits();
                let _ = sender.send(TapEvent::KeyDown { keycode, flags });
            }
            CGEventType::KeyUp => {
                let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
                let flags = event.get_flags().bits();
                let _ = sender.send(TapEvent::KeyUp { keycode, flags });
            }
            CGEventType::FlagsChanged => {
                let flags = event.get_flags().bits();
                let _ = sender.send(TapEvent::FlagsChanged { flags });
            }
            CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
                // Re-enable immediately (a single fast syscall — allowed here).
                let port = TAP_PORT.load(Ordering::SeqCst);
                if port != 0 {
                    unsafe { CGEventTapEnable(port as CFMachPortRef, true) };
                }
                let by_user_input = matches!(etype, CGEventType::TapDisabledByUserInput);
                let _ = sender.send(TapEvent::Disabled { by_user_input });
            }
            _ => {}
        }
    }

    /// Runs on the processor thread. Owns modifier-state tracking and logging.
    fn process_events(receiver: mpsc::Receiver<TapEvent>) {
        while let Ok(ev) = receiver.recv() {
            match ev {
                TapEvent::KeyDown { keycode, flags } => {
                    update_modifiers(flags);
                    log::info!(
                        "[HOOK] keyDown keycode={} mods=[{}]",
                        keycode,
                        modifier_string(flags)
                    );
                }
                TapEvent::KeyUp { keycode, flags } => {
                    update_modifiers(flags);
                    log::info!(
                        "[HOOK] keyUp   keycode={} mods=[{}]",
                        keycode,
                        modifier_string(flags)
                    );
                }
                TapEvent::FlagsChanged { flags } => {
                    update_modifiers(flags);
                    log::info!("[HOOK] flagsChanged mods=[{}]", modifier_string(flags));
                }
                TapEvent::Disabled { by_user_input } => {
                    if by_user_input {
                        log::warn!(
                            "[HOOK] tap disabled by USER INPUT (Secure Input active — a \
                             frontmost app has Secure Keyboard Entry on, or focus is a \
                             password field; key events are withheld while it's active) — \
                             re-enabled"
                        );
                    } else {
                        log::warn!(
                            "[HOOK] tap disabled by TIMEOUT (callback stalled) — re-enabled"
                        );
                    }
                }
            }
        }
        // Sender dropped (hook thread gone) — nothing left to process.
        log::warn!("[HOOK] tap processor thread exiting (channel closed)");
    }

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
