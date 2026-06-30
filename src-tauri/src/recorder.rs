// Macro recorder (Phase 1 — literal-replay mode).
//
// Pipeline:
//   user clicks Record  → recorder::start()  → IS_RECORDING_MACRO = true
//   LL hooks observe input events and call recorder::push_*  (purely additive —
//     does NOT alter normal processing; events still flow through suppress /
//     modifier tracking / dispatch the same as when not recording)
//   user presses Ctrl+Shift+R → recorder::stop() returns Vec<RecordedEvent>
//   frontend serialises the events Vec into the value of a "Replay Recording"
//     macro step inside a normal macro assignment
//   when that assignment fires, actions.rs replays each event with the
//     original inter-event gaps via SendInput
//
// All timestamps in events are RELATIVE to the recording start (ms), so a
// recording captured today plays back identically next week.

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// True while a recording is in progress. Hook procs check this on every
/// event with a single SeqCst load — when false the cost is one atomic read.
pub static IS_RECORDING_MACRO: AtomicBool = AtomicBool::new(false);

/// True across the ENTIRE recorder flow — set from the moment the user
/// clicks Record (when main hides), cleared only when the flow ends
/// (recording stopped + main restored, or cancellation). Broader scope than
/// IS_RECORDING_MACRO, which is only true between countdown-complete and
/// stop. The foreground watcher checks this so a profile-switch can't fire
/// during the 3-second countdown window — that switch was unmounting
/// ReplayRecordingValue mid-flow, triggering its cleanup which discarded
/// the recording and restored main.
pub static RECORDER_FLOW_ACTIVE: AtomicBool = AtomicBool::new(false);

/// True when the current recording was initiated via the global Quick Record
/// hotkey (Settings → Quick Record), false when initiated via the macro
/// editor's Record button. Set by the global-hotkey start path BEFORE
/// recorder::start(); cleared on stop in either flow. The stop handler reads
/// this to decide where the captured events go — temp-macro slot vs editor.
pub static TEMP_RECORDING_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set true to abort the countdown timer thread mid-tick. Polled on each
/// 1-second iteration. Cleared at the start of every show_recorder_countdown
/// call so a fresh flow always starts from a clean state.
pub static COUNTDOWN_CANCEL: AtomicBool = AtomicBool::new(false);

/// Anchor for relative timestamps. Captured at start(), used by relative_t().
static RECORDING_START_MS: AtomicI64 = AtomicI64::new(0);

/// Grace window after start() during which hooks observe input but do NOT
/// push to the buffer. Lets the user's click on the Record button (and any
/// mouseup that propagates after IS_RECORDING_MACRO flips true) miss the
/// recording. Set to RECORDING_START_MS + GRACE_MS at start().
static RECORDING_GRACE_UNTIL_MS: AtomicI64 = AtomicI64::new(0);
const GRACE_MS: i64 = 200;

/// Last accepted mousemove timestamp. Mouse moves are throttled to ~60fps to
/// keep storage bounded (LL hook fires per-pixel which can hit 1000 events/s
/// on fast mice). Buttons + wheel are not throttled.
static LAST_MOUSE_MOVE_MS: AtomicI64 = AtomicI64::new(0);
const MOUSE_MOVE_THROTTLE_MS: i64 = 16;

static EVENTS: OnceLock<Mutex<Vec<RecordedEvent>>> = OnceLock::new();
fn events() -> &'static Mutex<Vec<RecordedEvent>> {
    EVENTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// A single recorded input event. `t` is milliseconds since recording started.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum RecordedEvent {
    KeyDown { vk: u32, sc: u32, t: u64 },
    KeyUp { vk: u32, sc: u32, t: u64 },
    MouseDown { button: String, x: i32, y: i32, t: u64 },
    MouseUp { button: String, x: i32, y: i32, t: u64 },
    MouseMove { x: i32, y: i32, t: u64 },
    Wheel { delta: i32, x: i32, y: i32, t: u64 },
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn relative_t() -> u64 {
    let start = RECORDING_START_MS.load(Ordering::SeqCst);
    let now = now_ms();
    if start == 0 || now <= start {
        0
    } else {
        (now - start) as u64
    }
}

/// True if we're still in the post-start grace window (don't push events).
fn in_grace_window() -> bool {
    let until = RECORDING_GRACE_UNTIL_MS.load(Ordering::SeqCst);
    until > 0 && now_ms() < until
}

/// Begin a recording session. Clears any prior buffer. Idempotent if already
/// recording (no-op).
pub fn start() {
    // ALWAYS reset state — never early-return on "already recording". In dev,
    // HMR can reload the React side mid-recording while Rust keeps the flag
    // set; on the next Record click we'd otherwise adopt minutes of stale
    // buffer instead of starting fresh. In production this can also happen
    // if a previous flow's cleanup path silently failed.
    let was_active = IS_RECORDING_MACRO.swap(false, Ordering::SeqCst);
    if was_active {
        log::warn!("[RECORDER] start() called while already recording — resetting stale state");
    }
    if let Ok(mut vec) = events().lock() {
        vec.clear();
    }
    let now = now_ms();
    // Anchor relative-timestamps to the END of the grace window so the first
    // captured event has t ≈ 0 (rather than t = 200ms).
    RECORDING_START_MS.store(now + GRACE_MS, Ordering::SeqCst);
    RECORDING_GRACE_UNTIL_MS.store(now + GRACE_MS, Ordering::SeqCst);
    LAST_MOUSE_MOVE_MS.store(0, Ordering::SeqCst);
    IS_RECORDING_MACRO.store(true, Ordering::SeqCst);
    log::info!("[RECORDER] Recording started ({}ms grace window)", GRACE_MS);
}

/// Stop the recording and return the captured events. Idempotent — if the
/// hook already flipped the flag false on stop-hotkey detection, this just
/// returns the buffer that was being filled at that moment.
///
/// Trims trailing modifier KEYDOWNS without matching KEYUPS. These are
/// almost always the Ctrl+Shift from the user pressing Ctrl+Shift+R to
/// stop the recording — the R itself is suppressed by the hook, but the
/// preceding modifier keydowns are captured and the matching keyups never
/// arrive (recording is over by then). Replaying without trimming leaves
/// the OS with modifiers stuck held down, garbling all subsequent input
/// and making the macro hotkey unable to fire (Ctrl+Shift+<hotkey> ≠
/// <hotkey>).
pub fn stop() -> Vec<RecordedEvent> {
    IS_RECORDING_MACRO.store(false, Ordering::SeqCst);
    let mut captured = events().lock().map(|v| v.clone()).unwrap_or_default();
    let raw_len = captured.len();

    while let Some(last) = captured.last() {
        match last {
            RecordedEvent::KeyDown { vk, .. } if is_modifier_vk(*vk) => {
                captured.pop();
            }
            _ => break,
        }
    }
    let trimmed = raw_len - captured.len();

    log::info!(
        "[RECORDER] Recording retrieved — {} events ({}ms){}",
        captured.len(),
        captured.last().map(|e| event_t(e)).unwrap_or(0),
        if trimmed > 0 {
            format!(" — trimmed {} trailing modifier keydown(s)", trimmed)
        } else {
            String::new()
        }
    );
    captured
}

fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        0xA0 | 0xA1 |  // VK_LSHIFT, VK_RSHIFT
        0xA2 | 0xA3 |  // VK_LCONTROL, VK_RCONTROL
        0xA4 | 0xA5 |  // VK_LMENU, VK_RMENU (Alt)
        0x5B | 0x5C |  // VK_LWIN, VK_RWIN
        0x10 | 0x11 | 0x12  // VK_SHIFT, VK_CONTROL, VK_MENU (generic forms)
    )
}

/// Abandon a recording without returning events. Used when the user cancels
/// (Esc on the countdown window in Phase 2).
pub fn discard() {
    IS_RECORDING_MACRO.store(false, Ordering::SeqCst);
    if let Ok(mut vec) = events().lock() {
        vec.clear();
    }
    log::info!("[RECORDER] Recording discarded");
}

/// Current recording status — used by the frontend to render the recording
/// indicator + by hook procs to skip processing cheaply.
pub fn is_recording() -> bool {
    IS_RECORDING_MACRO.load(Ordering::SeqCst)
}

/// Event count + duration snapshot. Used by the toast / status pill.
pub fn status_snapshot() -> (usize, u64) {
    let count = events().lock().map(|v| v.len()).unwrap_or(0);
    let dur = if is_recording() { relative_t() } else { 0 };
    (count, dur)
}

fn event_t(e: &RecordedEvent) -> u64 {
    match e {
        RecordedEvent::KeyDown { t, .. }
        | RecordedEvent::KeyUp { t, .. }
        | RecordedEvent::MouseDown { t, .. }
        | RecordedEvent::MouseUp { t, .. }
        | RecordedEvent::MouseMove { t, .. }
        | RecordedEvent::Wheel { t, .. } => *t,
    }
}

// ── Hook ingestion ──────────────────────────────────────────────────────────
//
// These are called from inside the LL keyboard / mouse hook procs. They MUST
// stay cheap — hook callbacks have a Windows watchdog at ~300ms that uninstalls
// any hook that takes too long. Each push uses try_lock so a contended mutex
// drops the event (acceptable for mouse moves at 60Hz; rare for buttons/keys).
// We avoid mpsc::send to keep zero allocations on the hot path.

pub fn push_key(vk: u32, sc: u32, is_down: bool) {
    if !IS_RECORDING_MACRO.load(Ordering::SeqCst) || in_grace_window() {
        return;
    }
    let t = relative_t();
    let evt = if is_down {
        RecordedEvent::KeyDown { vk, sc, t }
    } else {
        RecordedEvent::KeyUp { vk, sc, t }
    };
    if let Ok(mut vec) = events().try_lock() {
        vec.push(evt);
    }
}

pub fn push_mouse_button(button: &'static str, x: i32, y: i32, is_down: bool) {
    if !IS_RECORDING_MACRO.load(Ordering::SeqCst) || in_grace_window() {
        return;
    }
    let t = relative_t();
    let evt = if is_down {
        RecordedEvent::MouseDown {
            button: button.to_string(),
            x,
            y,
            t,
        }
    } else {
        RecordedEvent::MouseUp {
            button: button.to_string(),
            x,
            y,
            t,
        }
    };
    if let Ok(mut vec) = events().try_lock() {
        vec.push(evt);
    }
}

pub fn push_mouse_move(x: i32, y: i32) {
    if !IS_RECORDING_MACRO.load(Ordering::SeqCst) || in_grace_window() {
        return;
    }
    let now = now_ms();
    let last = LAST_MOUSE_MOVE_MS.load(Ordering::SeqCst);
    if now - last < MOUSE_MOVE_THROTTLE_MS {
        return;
    }
    LAST_MOUSE_MOVE_MS.store(now, Ordering::SeqCst);
    let t = relative_t();
    if let Ok(mut vec) = events().try_lock() {
        vec.push(RecordedEvent::MouseMove { x, y, t });
    }
}

pub fn push_wheel(delta: i32, x: i32, y: i32) {
    if !IS_RECORDING_MACRO.load(Ordering::SeqCst) || in_grace_window() {
        return;
    }
    let t = relative_t();
    if let Ok(mut vec) = events().try_lock() {
        vec.push(RecordedEvent::Wheel { delta, x, y, t });
    }
}

// ── Quick Record hotkey cache (hook-readable atomics) ───────────────────────
//
// The hook callback can't lock engine_state on every keystroke (300ms
// watchdog), so we mirror the two configured hotkeys into atomics. Bits
// layout matches engine_state's (modifier_bits, vk) tuple. vk = 0 ⇒
// hotkey unset/disabled (suppressed entirely). Updated by lib.rs setters
// when the user changes them in Settings; defaults Ctrl+Alt+R / Ctrl+Alt+P.

// vk = 0 ⇒ hotkey unset / disabled. matches_*_hotkey returns false in that case.
// Users opt in via Settings → Quick Record; the lib.rs setter mirrors their
// choice into these atomics. See feedback_noactivate_overlay_pattern.md for
// the parent rationale on opt-in default vs default-on.
pub static TEMP_MACRO_RECORD_BITS: AtomicU8 = AtomicU8::new(0);
pub static TEMP_MACRO_RECORD_VK:   AtomicU32 = AtomicU32::new(0);
pub static TEMP_MACRO_PLAY_BITS:   AtomicU8 = AtomicU8::new(0);
pub static TEMP_MACRO_PLAY_VK:     AtomicU32 = AtomicU32::new(0);
pub static TEMP_MACRO_LOOP_BITS:   AtomicU8 = AtomicU8::new(0);
pub static TEMP_MACRO_LOOP_VK:     AtomicU32 = AtomicU32::new(0);

/// True while a continuous-replay loop is running for the temp macro.
/// Flipped by the processor on Loop-hotkey press (true → start thread, false
/// → in-flight thread observes and exits at the next checkpoint). Polled by
/// `actions::replay_recorded_events_loop` per iteration + per 100ms sleep
/// chunk so a Loop-hotkey press while the macro is mid-flight is honoured at
/// the next event gap.
pub static TEMP_MACRO_LOOP_ACTIVE: AtomicBool = AtomicBool::new(false);

/// True iff (mod_bits, vk) matches the configured record-toggle hotkey AND
/// it's actually configured (vk != 0). Caller holds modifier-state atomics.
pub fn matches_record_hotkey(vk: u32, mod_bits: u8) -> bool {
    let target_vk = TEMP_MACRO_RECORD_VK.load(Ordering::SeqCst);
    target_vk != 0 && vk == target_vk && mod_bits == TEMP_MACRO_RECORD_BITS.load(Ordering::SeqCst)
}

pub fn matches_play_hotkey(vk: u32, mod_bits: u8) -> bool {
    let target_vk = TEMP_MACRO_PLAY_VK.load(Ordering::SeqCst);
    target_vk != 0 && vk == target_vk && mod_bits == TEMP_MACRO_PLAY_BITS.load(Ordering::SeqCst)
}

pub fn matches_loop_hotkey(vk: u32, mod_bits: u8) -> bool {
    let target_vk = TEMP_MACRO_LOOP_VK.load(Ordering::SeqCst);
    target_vk != 0 && vk == target_vk && mod_bits == TEMP_MACRO_LOOP_BITS.load(Ordering::SeqCst)
}
