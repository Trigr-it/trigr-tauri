// Distillation engine — Phase 2 of the macro recorder.
//
// Walks a raw RecordedEvent stream and emits a list of DistilledStep values
// the user can read as semantic actions: Type Text, Press Key, Click at
// Position, Focus Window, Wait, Scroll, Drag.
//
// Design principle (Rory-confirmed 2026-06-24): distillation is purely
// additive — raw events are NEVER discarded. Distilled steps live alongside
// the raw event array on the Record Macro step. The user can flip playback
// mode between the two.
//
// The state machine is a single-pass walk with an accumulator for Type Text
// runs. Wait steps are injected when the gap between events exceeds
// WAIT_THRESHOLD_MS. Clicks capture BOTH absolute AND client-relative coords
// plus a TargetWindow identity, so replay can pick the right coord system
// per-step (window-relative survives moves and monitor swaps).

use crate::recorder::RecordedEvent;
use serde::{Deserialize, Serialize};
use windows_sys::Win32::Foundation::{HWND, LPARAM, POINT};
use windows_sys::Win32::Graphics::Gdi::ClientToScreen;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, ToUnicodeEx};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
};
// `windows` (not `windows-sys`) exposes IVirtualDesktopManager. Used to keep
// window lookups on the CURRENT virtual desktop so a distilled macro replayed
// on Desktop 2 doesn't SetForegroundWindow a candidate on Desktop 1 (which
// would yank the user to that desktop). See is_hwnd_on_current_desktop below.
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};

// ── Tunable thresholds ──────────────────────────────────────────────────────
//
// Two Wait thresholds: a HIGH one used while accumulating a Type Text run
// (so natural pauses between keystrokes don't fragment typing into many
// Wait+TypeText+Wait+TypeText fragments), and a LOW one used everywhere else
// (so UI navigation gaps, "open folder → wait for render → click something",
// are preserved and replay doesn't outrun the target app). 100ms picks up the
// gaps a UI needs to settle without capturing every micro-jitter between clicks.

const CLICK_TOLERANCE_PX: i32 = 4;
const WAIT_THRESHOLD_TEXT_MS: u64 = 500;
const WAIT_THRESHOLD_UI_MS: u64 = 100;
const WAIT_PRECISION_MS: u64 = 50;
const MAX_TYPE_TEXT_CHARS: usize = 500;

// ── Record Macro step value ─────────────────────────────────────────────────
//
// The step's `data` JSON was originally a bare `Vec<RecordedEvent>` (Phase 1).
// Phase 2 wraps that in an object with distilled steps + a playback-mode
// selector alongside the raw events. `parse_record_macro_value` accepts both
// shapes so pre-Phase-2 recordings load unchanged.

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetApp {
    pub exe: String,
    #[serde(default)]
    pub window_title_hint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordMacroValue {
    pub events: Vec<RecordedEvent>,
    /// Distilled steps in the SAME shape as manual macro steps
    /// (`{type: "Type Text", value: "..."}` etc). Replay walks them via
    /// `execute_macro_step` recursively. Option C architecture, no separate
    /// distilled executor path. Stored as raw JSON so the frontend can pass
    /// them straight into MacroSequenceForm without conversion.
    #[serde(default)]
    pub distilled: Option<Vec<serde_json::Value>>,
    #[serde(default = "default_playback_mode")]
    pub playback_mode: String,
    #[serde(default)]
    pub target_app: Option<TargetApp>,
    /// User-explicit "no binding" flag. Set to true when the user clicks the
    /// Clear button on the target-app chip in the macro editor. Defaults to
    /// false so pre-existing wrappers keep the auto-detect-from-events fallback.
    /// When true, the fire path skips all binding logic and runs the distilled
    /// steps against whatever window is currently focused. Re-distil resets it
    /// to false since a fresh distillation provides a fresh binding.
    #[serde(default)]
    pub disable_target_binding: bool,
}

fn default_playback_mode() -> String {
    "raw".into()
}

/// Parse the Record Macro step's `data` JSON. Backwards-compat: recordings
/// saved before Phase 2 are a bare `Vec<RecordedEvent>` — those still load
/// with distilled=None + playback_mode="raw".
pub fn parse_record_macro_value(json: &str) -> Option<RecordMacroValue> {
    let trimmed = json.trim_start();
    if trimmed.starts_with('{') {
        serde_json::from_str::<RecordMacroValue>(json).ok()
    } else if trimmed.starts_with('[') {
        let events: Vec<RecordedEvent> = serde_json::from_str(json).ok()?;
        Some(RecordMacroValue {
            events,
            distilled: None,
            playback_mode: "raw".into(),
            target_app: None,
            disable_target_binding: false,
        })
    } else {
        None
    }
}

// ── Types ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetWindow {
    pub title: String,
    pub exe: String,
    pub class: String,
    pub client_w: i32,
    pub client_h: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DistilledStep {
    TypeText {
        text: String,
    },
    PressKey {
        vks: Vec<u32>,
    },
    #[serde(rename_all = "camelCase")]
    ClickAtPosition {
        x_abs: i32,
        y_abs: i32,
        x_rel: Option<i32>,
        y_rel: Option<i32>,
        button: String,
        target_window: Option<TargetWindow>,
        /// How long the button was held between down and up. Preserved so
        /// click-and-hold interactions (long-press UI, games) replay
        /// faithfully — the executor holds the button for this duration when
        /// it exceeds a normal-click threshold.
        #[serde(default)]
        hold_ms: u64,
        /// Modifier VKs held during the click (generic codes: 0x11 Ctrl,
        /// 0x12 Alt, 0x5B Win, 0x10 Shift). The executor presses these
        /// before the button-down and releases after the button-up, so
        /// Shift+click / Ctrl+click / Shift+drag replay correctly.
        #[serde(default)]
        modifiers: Vec<u32>,
    },
    #[serde(rename_all = "camelCase")]
    Drag {
        from_x: i32,
        from_y: i32,
        to_x: i32,
        to_y: i32,
        button: String,
        hold_ms: u64,
        #[serde(default)]
        modifiers: Vec<u32>,
    },
    Scroll {
        delta: i32,
    },
    FocusWindow {
        title: String,
        exe: String,
    },
    Wait {
        ms: u64,
    },
}

// ── Public entrypoint ───────────────────────────────────────────────────────

/// Convert a raw event stream into a list of macro steps shaped IDENTICALLY
/// to hand-built macro steps (`{type: "Type Text", value: "..."}` etc). This
/// unifies replay: the Record Macro arm in actions.rs calls execute_macro_step
/// recursively on each distilled step, so window-mode Click, Focus Window,
/// Type Text etc. all run through the same executors as manual macros.
/// Deterministic — same input always produces the same output.
pub fn distill(events: &[RecordedEvent]) -> Vec<serde_json::Value> {
    distill_internal(events)
        .into_iter()
        .map(distilled_to_macro_step)
        .collect()
}

/// Extract the recording's target app from the first ForegroundChanged event.
/// Used at Distil time to auto-bind the macro to the app the user recorded
/// against — so replay can precheck the app is running and abort with a modal
/// if it isn't (rather than firing steps into whatever's foreground).
pub fn extract_target_app(events: &[RecordedEvent]) -> Option<TargetApp> {
    for evt in events {
        if let RecordedEvent::ForegroundChanged { exe, title, .. } = evt {
            if !exe.is_empty() {
                return Some(TargetApp {
                    exe: exe.clone(),
                    window_title_hint: if title.is_empty() { None } else { Some(title.clone()) },
                });
            }
        }
    }
    None
}

/// Internal state-machine output — used only as an intermediate representation
/// on the path to the manual macro-step JSON.
fn distill_internal(events: &[RecordedEvent]) -> Vec<DistilledStep> {
    let mut out: Vec<DistilledStep> = Vec::new();
    let mut modifiers: Vec<u32> = Vec::new();
    let mut text_buf = String::new();
    let mut current_fg: Option<CurrentFg> = None;
    let mut mouse_down: Option<PendingMouse> = None;
    let mut last_t: u64 = 0;
    let mut first_event_seen = false;

    for evt in events {
        // MouseMove events fire every ~20ms while the cursor is moving, so if
        // we let them advance `last_t` the gap between consecutive events is
        // always tiny and no Wait step ever gets emitted between real actions.
        // Drag detection doesn't need mousemoves either — it uses the mousedown
        // and mouseup coords directly (line 282+). Skip them entirely.
        if matches!(evt, RecordedEvent::MouseMove { .. }) {
            continue;
        }
        let evt_t = event_t(evt);
        // Wait injection — only after we've seen at least one event (avoid a
        // leading Wait for the natural gap between recorder start and first
        // real input), and NEVER while a mouse button is held: the down→up
        // span becomes the click's hold_ms, and emitting a Wait for the same
        // span would double-count the hold on replay (Wait 2s + hold 2s = 4s
        // for a 2s press).
        if first_event_seen && mouse_down.is_none() {
            let gap = evt_t.saturating_sub(last_t);
            // Threshold depends on whether we're mid-typing. Inside a typing
            // run (text_buf non-empty), only large gaps break the run. Outside
            // one (UI navigation), preserve smaller gaps too so the target has
            // time to respond before the next click fires.
            let threshold = if text_buf.is_empty() {
                WAIT_THRESHOLD_UI_MS
            } else {
                WAIT_THRESHOLD_TEXT_MS
            };
            if gap >= threshold {
                flush_text_buffer(&mut text_buf, &mut out);
                let rounded = round_ms(gap);
                if rounded > 0 {
                    out.push(DistilledStep::Wait { ms: rounded });
                }
            }
        }
        first_event_seen = true;
        last_t = evt_t;

        match evt {
            RecordedEvent::KeyDown { vk, sc, .. } => {
                handle_keydown(*vk, *sc, &mut modifiers, &mut text_buf, &mut out);
            }
            RecordedEvent::KeyUp { vk, .. } => {
                if is_modifier_vk(*vk) {
                    modifiers.retain(|m| *m != *vk);
                }
            }
            RecordedEvent::MouseDown { button, x, y, t } => {
                flush_text_buffer(&mut text_buf, &mut out);
                let target_at_down = current_fg
                    .as_ref()
                    .and_then(|fg| resolve_rel(*x, *y, fg));
                mouse_down = Some(PendingMouse {
                    button: button.clone(),
                    x: *x,
                    y: *y,
                    t: *t,
                    target_at_down,
                    // Snapshot held modifiers so Shift+click / Shift+drag
                    // replay with the modifier held around the button action.
                    modifiers: canonicalise_modifiers(&modifiers),
                });
            }
            RecordedEvent::MouseUp { x, y, t, .. } => {
                if let Some(down) = mouse_down.take() {
                    let dx = (*x - down.x).abs();
                    let dy = (*y - down.y).abs();
                    // TEMP diagnostic (strip after drag-detection wild-verifies):
                    // shows exactly what the classifier saw for every pair.
                    log::info!(
                        "[DISTILL] mouse pair: down=({},{}) up=({},{}) delta=({},{}) hold={}ms → {}",
                        down.x, down.y, x, y, dx, dy,
                        t.saturating_sub(down.t),
                        if dx <= CLICK_TOLERANCE_PX && dy <= CLICK_TOLERANCE_PX { "click" } else { "drag" }
                    );
                    if dx <= CLICK_TOLERANCE_PX && dy <= CLICK_TOLERANCE_PX {
                        let (x_rel, y_rel, target_window) = match down.target_at_down {
                            Some(t) => (Some(t.rel_x), Some(t.rel_y), Some(t.target)),
                            None => (None, None, None),
                        };
                        out.push(DistilledStep::ClickAtPosition {
                            x_abs: *x,
                            y_abs: *y,
                            x_rel,
                            y_rel,
                            button: down.button,
                            target_window,
                            hold_ms: t.saturating_sub(down.t),
                            modifiers: down.modifiers,
                        });
                    } else {
                        out.push(DistilledStep::Drag {
                            from_x: down.x,
                            from_y: down.y,
                            to_x: *x,
                            to_y: *y,
                            button: down.button,
                            hold_ms: t.saturating_sub(down.t),
                            modifiers: down.modifiers,
                        });
                    }
                }
            }
            RecordedEvent::MouseMove { .. } => unreachable!("filtered above"),
            RecordedEvent::Wheel { delta, .. } => {
                flush_text_buffer(&mut text_buf, &mut out);
                out.push(DistilledStep::Scroll { delta: *delta });
            }
            RecordedEvent::ForegroundChanged {
                title,
                exe,
                class,
                client_x,
                client_y,
                client_w,
                client_h,
                ..
            } => {
                flush_text_buffer(&mut text_buf, &mut out);
                let new_fg = CurrentFg {
                    title: title.clone(),
                    exe: exe.clone(),
                    class: class.clone(),
                    client_x: *client_x,
                    client_y: *client_y,
                    client_w: *client_w,
                    client_h: *client_h,
                };
                let should_emit = match &current_fg {
                    None => true,
                    Some(cur) => cur.title != new_fg.title || cur.exe != new_fg.exe,
                };
                if should_emit {
                    out.push(DistilledStep::FocusWindow {
                        title: new_fg.title.clone(),
                        exe: new_fg.exe.clone(),
                    });
                }
                current_fg = Some(new_fg);
            }
        }
    }
    flush_text_buffer(&mut text_buf, &mut out);
    out
}

// ── Convert internal DistilledStep → manual macro step JSON ─────────────────
//
// Output shape MUST match the arms in actions.rs::execute_macro_step:
//   "Type Text"          → { type, value: string }
//   "Press Key"          → { type, value: "Ctrl+Shift+S" }
//   "Wait (ms)"          → { type, value: ms.to_string() }
//   "Focus Window"       → { type, value: JSON.stringify({ process, title }) }
//   "Mouse Scroll"       → { type, value: JSON.stringify({ direction, amount }) }
//   "Click at Position"  → { type, value: JSON.stringify({ x, y, button, mode, targetWindow? }) }
//
// New "windowClient" mode on Click at Position carries `targetWindow` so the
// executor can resolve the live HWND and translate rel→screen. Pro-gated at
// execute time.

fn distilled_to_macro_step(step: DistilledStep) -> serde_json::Value {
    use serde_json::json;
    match step {
        DistilledStep::TypeText { text } => json!({
            "type": "Type Text",
            "value": text,
        }),
        DistilledStep::PressKey { vks } => json!({
            "type": "Press Key",
            "value": vks_to_chord(&vks),
        }),
        DistilledStep::ClickAtPosition {
            x_abs,
            y_abs,
            x_rel,
            y_rel,
            button,
            target_window,
            hold_ms,
            modifiers,
        } => {
            let button_short = button_short_name(&button);
            let mod_names = modifier_vk_names(&modifiers);
            let click_value = match (x_rel, y_rel, target_window) {
                (Some(rx), Some(ry), Some(tw)) => json!({
                    "x": rx,
                    "y": ry,
                    "button": button_short,
                    "mode": "windowClient",
                    "targetWindow": {
                        "title": tw.title,
                        "exe": tw.exe,
                        "class": tw.class,
                        "clientW": tw.client_w,
                        "clientH": tw.client_h,
                    },
                    // Default ON: modern responsive apps benefit, and when the
                    // window hasn't been resized the scale ratio is 1.0 (identical
                    // to static mode). Users can flip to "static" per-click via
                    // the distilled step editor for fixed-anchor dialog apps.
                    "resizeBehavior": "proportional",
                    "holdMs": hold_ms,
                    "modifiers": mod_names,
                    "fallbackX": x_abs,
                    "fallbackY": y_abs,
                }),
                _ => json!({
                    "x": x_abs,
                    "y": y_abs,
                    "button": button_short,
                    "mode": "absolute",
                    "holdMs": hold_ms,
                    "modifiers": mod_names,
                }),
            };
            json!({
                "type": "Click at Position",
                "value": click_value.to_string(),
            })
        }
        // Drag emits as a Click at Position carrying dragToX/dragToY — the
        // executor replays down-at-origin → interpolated real WM_MOUSEMOVE
        // steps → up-at-end, spread across holdMs, so drag-detecting apps
        // (sliders, drag-drop, canvas tools) see a genuine drag. The exact
        // recorded cursor path is not preserved — start/end/duration are.
        DistilledStep::Drag { from_x, from_y, to_x, to_y, button, hold_ms, modifiers } => {
            let click_value = json!({
                "x": from_x,
                "y": from_y,
                "button": button_short_name(&button),
                "mode": "absolute",
                "holdMs": hold_ms,
                "modifiers": modifier_vk_names(&modifiers),
                "dragToX": to_x,
                "dragToY": to_y,
            });
            json!({
                "type": "Click at Position",
                "value": click_value.to_string(),
            })
        }
        DistilledStep::Scroll { delta } => {
            const WHEEL_NOTCH: i32 = 120;
            let notches = ((delta.abs() as f32) / (WHEEL_NOTCH as f32)).round().max(1.0) as i32;
            let direction = if delta > 0 { "up" } else { "down" };
            let scroll_value = json!({
                "direction": direction,
                "amount": notches,
            });
            json!({
                "type": "Mouse Scroll",
                "value": scroll_value.to_string(),
            })
        }
        DistilledStep::FocusWindow { title, exe } => {
            let fw_value = json!({
                "process": exe,
                "title": title,
            });
            json!({
                "type": "Focus Window",
                "value": fw_value.to_string(),
            })
        }
        DistilledStep::Wait { ms } => json!({
            "type": "Wait (ms)",
            "value": ms.to_string(),
        }),
    }
}

/// Generic modifier VKs → display names for the Click at Position JSON.
/// Mirrors the name→VK map in the executor (actions.rs Click at Position arm).
fn modifier_vk_names(vks: &[u32]) -> Vec<&'static str> {
    vks.iter()
        .filter_map(|vk| match vk {
            0x11 => Some("Ctrl"),
            0x12 => Some("Alt"),
            0x5B => Some("Win"),
            0x10 => Some("Shift"),
            _ => None,
        })
        .collect()
}

fn button_short_name(button: &str) -> &'static str {
    match button {
        "left" | "LButton" => "left",
        "right" | "RButton" => "right",
        "middle" | "MButton" => "middle",
        _ => "left",
    }
}

/// Turn a set of held modifiers + a final VK into the chord string the
/// "Press Key" macro step arm parses (`"Ctrl+Shift+N"`). Mirror of the parse
/// side in actions.rs::execute_macro_step.
fn vks_to_chord(vks: &[u32]) -> String {
    if vks.is_empty() {
        return String::new();
    }
    let (main_vk, mods) = vks.split_last().unwrap();
    let mut parts: Vec<&'static str> = Vec::new();
    for m in mods {
        match m {
            0x11 | 0xA2 | 0xA3 => parts.push("Ctrl"),
            0x12 | 0xA4 | 0xA5 => parts.push("Alt"),
            0x10 | 0xA0 | 0xA1 => parts.push("Shift"),
            0x5B | 0x5C => parts.push("Win"),
            _ => {}
        }
    }
    // Canonical modifier order: Ctrl, Alt, Shift, Win (matches how the app renders chords elsewhere)
    let mut ordered = Vec::new();
    for m in ["Ctrl", "Alt", "Shift", "Win"] {
        if parts.contains(&m) && !ordered.contains(&m) {
            ordered.push(m);
        }
    }
    let main = vk_display_name(*main_vk);
    if ordered.is_empty() {
        main
    } else {
        format!("{}+{}", ordered.join("+"), main)
    }
}

fn vk_display_name(vk: u32) -> String {
    match vk {
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x1B => "Escape".into(),
        0x20 => "Space".into(),
        0x14 => "CapsLock".into(),
        0x90 => "NumLock".into(),
        0x91 => "ScrollLock".into(),
        0x2C => "PrintScreen".into(),
        0x13 => "Pause".into(),
        0x21 => "PageUp".into(),
        0x22 => "PageDown".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x30..=0x39 => (vk as u8 as char).to_string(),        // 0-9
        0x41..=0x5A => (vk as u8 as char).to_string(),        // A-Z
        0x70..=0x87 => format!("F{}", vk - 0x6F),             // F1-F24
        _ => format!("0x{:X}", vk),
    }
}

// ── Internals ───────────────────────────────────────────────────────────────

struct CurrentFg {
    title: String,
    exe: String,
    class: String,
    client_x: i32,
    client_y: i32,
    client_w: i32,
    client_h: i32,
}

struct PendingMouse {
    button: String,
    x: i32,
    y: i32,
    t: u64,
    target_at_down: Option<TargetAtDown>,
    /// Canonicalised modifier VKs held at button-down (Shift+drag etc).
    modifiers: Vec<u32>,
}

struct TargetAtDown {
    rel_x: i32,
    rel_y: i32,
    target: TargetWindow,
}

fn resolve_rel(x: i32, y: i32, fg: &CurrentFg) -> Option<TargetAtDown> {
    // Only bind to the window if the click falls INSIDE its client area.
    // Clicks on window frames / title bars / desktop stay absolute-only.
    let rel_x = x - fg.client_x;
    let rel_y = y - fg.client_y;
    if rel_x < 0 || rel_y < 0 || rel_x >= fg.client_w || rel_y >= fg.client_h {
        return None;
    }
    Some(TargetAtDown {
        rel_x,
        rel_y,
        target: TargetWindow {
            title: fg.title.clone(),
            exe: fg.exe.clone(),
            class: fg.class.clone(),
            client_w: fg.client_w,
            client_h: fg.client_h,
        },
    })
}

fn event_t(e: &RecordedEvent) -> u64 {
    match e {
        RecordedEvent::KeyDown { t, .. }
        | RecordedEvent::KeyUp { t, .. }
        | RecordedEvent::MouseDown { t, .. }
        | RecordedEvent::MouseUp { t, .. }
        | RecordedEvent::MouseMove { t, .. }
        | RecordedEvent::Wheel { t, .. }
        | RecordedEvent::ForegroundChanged { t, .. } => *t,
    }
}

fn round_ms(ms: u64) -> u64 {
    let step = WAIT_PRECISION_MS;
    ((ms + step / 2) / step) * step
}

fn flush_text_buffer(buf: &mut String, out: &mut Vec<DistilledStep>) {
    if buf.is_empty() {
        return;
    }
    out.push(DistilledStep::TypeText { text: buf.clone() });
    buf.clear();
}

fn handle_keydown(
    vk: u32,
    sc: u32,
    modifiers: &mut Vec<u32>,
    text_buf: &mut String,
    out: &mut Vec<DistilledStep>,
) {
    if is_modifier_vk(vk) {
        if !modifiers.contains(&vk) {
            modifiers.push(vk);
        }
        // Modifier press ends a Type Text run. The next non-modifier keydown
        // decides whether we resume typing (Shift-only + letter → typing) or
        // emit a chord (Ctrl/Alt/Win + key → PressKey).
        flush_text_buffer(text_buf, out);
        return;
    }

    // Non-shift modifier held → chord
    if has_non_shift_mod(modifiers) {
        flush_text_buffer(text_buf, out);
        let mut vks = canonicalise_modifiers(modifiers);
        vks.push(vk);
        out.push(DistilledStep::PressKey { vks });
        return;
    }

    // Navigation / edit / function keys → PressKey (with shift if held)
    if is_navigation_or_edit(vk) || is_function_key(vk) {
        flush_text_buffer(text_buf, out);
        let mut vks = canonicalise_modifiers(modifiers);
        vks.push(vk);
        out.push(DistilledStep::PressKey { vks });
        return;
    }

    // Character-producing
    if let Some(s) = vk_to_char(vk, sc, modifiers) {
        text_buf.push_str(&s);
        if text_buf.chars().count() >= MAX_TYPE_TEXT_CHARS {
            flush_text_buffer(text_buf, out);
        }
    }
    // else: unmapped VK, silently skip (e.g. dead-key state, non-char VKs)
}

fn is_modifier_vk(vk: u32) -> bool {
    matches!(
        vk,
        0xA0 | 0xA1 |  // L/R Shift
        0xA2 | 0xA3 |  // L/R Ctrl
        0xA4 | 0xA5 |  // L/R Alt
        0x5B | 0x5C |  // L/R Win
        0x10 | 0x11 | 0x12  // Generic Shift/Ctrl/Alt
    )
}

fn has_non_shift_mod(mods: &[u32]) -> bool {
    mods.iter().any(|m| {
        matches!(
            m,
            0xA2 | 0xA3 | 0x11 | 0xA4 | 0xA5 | 0x12 | 0x5B | 0x5C
        )
    })
}

/// Canonicalise the currently-held modifier set into a stable, deduped order:
/// Ctrl, Alt, Win, Shift. Uses generic VK codes (0x11/0x12/0x5B/0x10) so a
/// chord recorded via LCtrl round-trips the same as one recorded via RCtrl —
/// which is what the user reads and what the Press Key executor accepts.
fn canonicalise_modifiers(mods: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    if mods.iter().any(|m| matches!(m, 0xA2 | 0xA3 | 0x11)) {
        out.push(0x11);
    }
    if mods.iter().any(|m| matches!(m, 0xA4 | 0xA5 | 0x12)) {
        out.push(0x12);
    }
    if mods.iter().any(|m| matches!(m, 0x5B | 0x5C)) {
        out.push(0x5B);
    }
    if mods.iter().any(|m| matches!(m, 0xA0 | 0xA1 | 0x10)) {
        out.push(0x10);
    }
    out
}

fn is_navigation_or_edit(vk: u32) -> bool {
    matches!(
        vk,
        0x08 | 0x09 | 0x0D | 0x1B |             // Backspace, Tab, Enter, Esc
        0x21 | 0x22 | 0x23 | 0x24 |             // PgUp, PgDn, End, Home
        0x25 | 0x26 | 0x27 | 0x28 |             // Left, Up, Right, Down
        0x2D | 0x2E |                            // Insert, Delete
        0x14 | 0x90 | 0x91                       // CapsLock, NumLock, ScrollLock
    )
}

fn is_function_key(vk: u32) -> bool {
    (0x70..=0x87).contains(&vk) // F1..F24
}

/// Convert a KeyDown (vk + scan code + held modifiers) into the character(s)
/// it would produce on the CURRENT keyboard layout. Uses ToUnicodeEx so
/// non-US layouts (UK, DE, FR, Cyrillic, …) work correctly.
///
/// Limitations for the Phase 2 prototype:
/// - Uses GetKeyboardLayout(0), which returns the layout of the calling thread
///   at distill time. Almost always matches the recording's layout for
///   single-user workflows.
/// - Caps Lock state is NOT tracked across the recording. If the user hit
///   Caps Lock mid-recording, subsequent letters may be wrong-cased in the
///   distilled Type Text. Fixable in Phase 2.5 by tracking VK_CAPITAL toggle
///   through the event stream.
/// - ToUnicodeEx has internal state for dead keys; calling it repeatedly can
///   affect subsequent calls. Fine for a distillation pass over a bounded
///   event list; may need a scratch-buffer reset if it causes issues in the
///   wild.
fn vk_to_char(vk: u32, sc: u32, modifiers: &[u32]) -> Option<String> {
    unsafe {
        let mut key_state = [0u8; 256];
        // Set the specific L/R modifier bits AND the generic ones so
        // ToUnicodeEx sees a consistent state regardless of which side was
        // pressed.
        for m in modifiers {
            key_state[*m as usize] = 0x80;
        }
        if modifiers.iter().any(|m| matches!(m, 0xA0 | 0xA1 | 0x10)) {
            key_state[0x10] = 0x80;
        }
        if modifiers.iter().any(|m| matches!(m, 0xA2 | 0xA3 | 0x11)) {
            key_state[0x11] = 0x80;
        }
        if modifiers.iter().any(|m| matches!(m, 0xA4 | 0xA5 | 0x12)) {
            key_state[0x12] = 0x80;
        }

        let layout = GetKeyboardLayout(0);
        let mut buf = [0u16; 8];
        let n = ToUnicodeEx(
            vk,
            sc,
            key_state.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as i32,
            0,
            layout,
        );
        if n <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..n as usize]))
    }
}

// ── Replay helpers (window resolution + client-to-screen) ───────────────────
//
// Called by actions.rs during distilled replay of ClickAtPosition steps in
// window mode. Resolution walks all top-level visible windows and matches by
// exe + title (strict) → exe + class (title drift tolerant) → exe-only (last
// resort). Returns None when no candidate matches — replay treats that as
// "skip this step" (toast + continue).

/// Resolve a stored TargetWindow to a live HWND, or None if no top-level
/// visible window matches on the CURRENT virtual desktop. Never falls back to
/// windows on other desktops. That would otherwise yank the user's foreground
/// across desktops when a distilled macro re-focuses its target.
pub fn resolve_target_window(target: &TargetWindow) -> Option<isize> {
    let mut ctx = ResolveCtx {
        wanted_exe: target.exe.to_lowercase(),
        wanted_title: target.title.clone(),
        wanted_class: target.class.clone(),
        matches: Vec::new(),
    };
    unsafe {
        EnumWindows(
            Some(enum_windows_cb),
            &mut ctx as *mut _ as LPARAM,
        );
    }
    // Filter to windows on the current virtual desktop only. Permissive fallback
    // on any COM failure (older Windows, denied access) keeps behaviour unchanged
    // for users whose VDM interface doesn't work.
    let vdm = make_vdm();
    let on_current: Vec<&WindowCandidate> = ctx.matches.iter()
        .filter(|c| is_hwnd_on_current_desktop(vdm.as_ref(), c.hwnd))
        .collect();
    // Strict: exe + exact title
    if let Some(c) = on_current.iter().find(|c| c.exe == ctx.wanted_exe && c.title == ctx.wanted_title) {
        return Some(c.hwnd);
    }
    // Loose: exe + class (handles title drift, e.g. "Doc1 - Word" → "Report.docx - Word")
    if let Some(c) = on_current.iter().find(|c| c.exe == ctx.wanted_exe && c.class == ctx.wanted_class) {
        return Some(c.hwnd);
    }
    // Last resort: exe only (first match wins on current desktop)
    on_current.iter().find(|c| c.exe == ctx.wanted_exe).map(|c| c.hwnd)
}

/// Create an IVirtualDesktopManager COM instance, or None on any failure.
/// Callers treat None as "assume the window IS on the current desktop"
/// (permissive) so behaviour degrades gracefully on Windows versions or
/// user configurations where the VDM interface is unavailable.
pub(crate) fn make_vdm() -> Option<IVirtualDesktopManager> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&VirtualDesktopManager, None, CLSCTX_INPROC_SERVER).ok()
    }
}

/// True if `hwnd` lives on the current virtual desktop. Also true on any COM
/// failure (missing VDM) so we never over-filter and miss legitimate matches.
pub(crate) fn is_hwnd_on_current_desktop(vdm: Option<&IVirtualDesktopManager>, hwnd: isize) -> bool {
    let Some(vdm) = vdm else { return true; };
    unsafe {
        match vdm.IsWindowOnCurrentVirtualDesktop(windows::Win32::Foundation::HWND(hwnd as _)) {
            Ok(b) => b.as_bool(),
            Err(_) => true,
        }
    }
}

/// Translate client-relative coords back to absolute screen coords using the
/// live window's client origin. Returns None if ClientToScreen fails.
pub fn client_to_screen(hwnd: isize, rel_x: i32, rel_y: i32) -> Option<(i32, i32)> {
    unsafe {
        let mut pt = POINT { x: rel_x, y: rel_y };
        if ClientToScreen(hwnd as HWND, &mut pt) == 0 {
            return None;
        }
        Some((pt.x, pt.y))
    }
}

struct ResolveCtx {
    wanted_exe: String,
    wanted_title: String,
    wanted_class: String,
    matches: Vec<WindowCandidate>,
}

struct WindowCandidate {
    hwnd: isize,
    title: String,
    exe: String,
    class: String,
}

unsafe extern "system" fn enum_windows_cb(hwnd: HWND, lparam: LPARAM) -> i32 {
    if IsWindowVisible(hwnd) == 0 {
        return 1; // keep enumerating
    }
    let ctx = &mut *(lparam as *mut ResolveCtx);
    let title = get_window_title(hwnd as isize);
    if title.is_empty() {
        return 1;
    }
    let exe = match get_process_exe_name(hwnd as isize) {
        Some(e) => e,
        None => return 1,
    };
    // Fast-fail on exe mismatch — most candidates
    if exe != ctx.wanted_exe {
        return 1;
    }
    let class = get_window_class(hwnd as isize);
    ctx.matches.push(WindowCandidate {
        hwnd: hwnd as isize,
        title,
        exe,
        class,
    });
    1
}

fn get_window_title(hwnd: isize) -> String {
    unsafe {
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd as HWND, buf.as_mut_ptr(), 512);
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

fn get_window_class(hwnd: isize) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let len = GetClassNameW(hwnd as HWND, buf.as_mut_ptr(), 256);
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize])
    }
}

fn get_process_exe_name(hwnd: isize) -> Option<String> {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd as HWND, &mut pid);
        if pid == 0 {
            return None;
        }
        let h_proc = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h_proc.is_null() {
            return None;
        }
        let mut buf = [0u16; 260];
        let mut size: u32 = 260;
        let ok = QueryFullProcessImageNameW(h_proc, 0, buf.as_mut_ptr(), &mut size);
        windows_sys::Win32::Foundation::CloseHandle(h_proc);
        if ok == 0 || size == 0 {
            return None;
        }
        let full_path = String::from_utf16_lossy(&buf[..size as usize]);
        std::path::Path::new(&full_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn k_down(vk: u32, t: u64) -> RecordedEvent {
        RecordedEvent::KeyDown { vk, sc: 0, t }
    }
    fn k_up(vk: u32, t: u64) -> RecordedEvent {
        RecordedEvent::KeyUp { vk, sc: 0, t }
    }
    fn m_down(x: i32, y: i32, t: u64) -> RecordedEvent {
        RecordedEvent::MouseDown {
            button: "left".into(),
            x,
            y,
            t,
        }
    }
    fn m_up(x: i32, y: i32, t: u64) -> RecordedEvent {
        RecordedEvent::MouseUp {
            button: "left".into(),
            x,
            y,
            t,
        }
    }

    #[test]
    fn wait_injected_for_long_gap() {
        let events = vec![k_down(0x41, 0), k_up(0x41, 100), k_down(0x42, 800), k_up(0x42, 900)];
        let steps = distill_internal(&events);
        // Expect: TypeText, Wait, TypeText — with the exact chars depending
        // on the system keyboard layout at test time.
        assert!(steps.iter().any(|s| matches!(s, DistilledStep::Wait { .. })));
    }

    #[test]
    fn chord_emits_press_key_not_type_text() {
        // Ctrl + S — should NOT type "s", should emit PressKey [Ctrl, S]
        let events = vec![
            k_down(0xA2, 0),                // LCtrl down
            k_down(0x53, 10),               // S down
            k_up(0x53, 20),                 // S up
            k_up(0xA2, 30),                 // LCtrl up
        ];
        let steps = distill_internal(&events);
        let is_chord = steps.iter().any(|s| matches!(s, DistilledStep::PressKey { vks } if vks.contains(&0x11) && vks.contains(&0x53)));
        assert!(is_chord, "expected PressKey Ctrl+S, got {:?}", steps);
    }

    #[test]
    fn click_vs_drag_classified_by_movement() {
        let click_events = vec![m_down(100, 100, 0), m_up(102, 101, 50)];
        let drag_events = vec![m_down(100, 100, 0), m_up(300, 200, 500)];
        assert!(matches!(distill_internal(&click_events)[0], DistilledStep::ClickAtPosition { .. }));
        assert!(matches!(distill_internal(&drag_events)[0], DistilledStep::Drag { .. }));
    }

    #[test]
    fn foreground_change_emits_focus_window_step() {
        let events = vec![
            RecordedEvent::ForegroundChanged {
                hwnd: 1,
                title: "Slack".into(),
                exe: "slack".into(),
                class: "Chrome_WidgetWin_1".into(),
                x: 0,
                y: 0,
                client_x: 0,
                client_y: 0,
                client_w: 800,
                client_h: 600,
                t: 0,
            },
            k_down(0x41, 100),
            k_up(0x41, 110),
        ];
        let steps = distill_internal(&events);
        assert!(matches!(steps.first(), Some(DistilledStep::FocusWindow { .. })));
    }
}
