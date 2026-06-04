use log::{info, warn};
use serde_json::Value;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use tauri::Manager;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT,
    KEYEVENTF_KEYUP, KEYEVENTF_UNICODE, MOUSEINPUT, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    VIRTUAL_KEY,
};
use windows_sys::Win32::Foundation::CloseHandle as CloseHandleWin;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    SetForegroundWindow, SW_SHOW,
};

/// Future clipboard manager checks this flag and skips logging if set.
pub static SUPPRESS_NEXT_CLIPBOARD_WRITE: AtomicBool = AtomicBool::new(false);

// ── Self-write clipboard suppression (robust to async WM_CLIPBOARDUPDATE) ────
// SUPPRESS_NEXT_CLIPBOARD_WRITE is a level flag — it's cleared synchronously
// after a write/restore, but Windows delivers WM_CLIPBOARDUPDATE asynchronously,
// so the listener can process the event AFTER the flag is cleared and record
// Trigr's own injected text into history (the H3 leak). To fix this precisely,
// every internal write records the resulting clipboard sequence number here; the
// listener skips any update whose seqnum we produced. This is exact: a real user
// copy (or a `Copy to Clipboard` macro step, which the target app performs) gets
// a seqnum we never recorded, so it is always still captured.
static SELF_CLIPBOARD_SEQNUMS: LazyLock<Mutex<VecDeque<u32>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

/// Record the current clipboard sequence number as a Trigr-internal write so the
/// listener won't log it. Call immediately after CloseClipboard on any internal
/// write/restore. Best-effort — on lock failure the level flag still covers the
/// synchronous window.
pub(crate) fn record_self_clipboard_write() {
    let seq = crate::expansions::clipboard_sequence_number();
    if let Ok(mut q) = SELF_CLIPBOARD_SEQNUMS.lock() {
        if !q.contains(&seq) {
            q.push_back(seq);
        }
        // Cap — seqnums are monotonic so stale ones never match a future event.
        while q.len() > 64 {
            q.pop_front();
        }
    }
}

/// True if `seq` was produced by a Trigr-internal write. Consumes the match so a
/// single internal write is only ever skipped once.
pub(crate) fn is_self_clipboard_seq(seq: u32) -> bool {
    if let Ok(mut q) = SELF_CLIPBOARD_SEQNUMS.lock() {
        if let Some(pos) = q.iter().position(|&s| s == seq) {
            q.remove(pos);
            return true;
        }
    }
    false
}

/// Number of action/macro executions currently in flight (inc/dec in fire_macro's
/// thread). >1 means overlapping fires (re-entrancy) — fire_macro logs a warn when
/// that happens, a lightweight guard-rail for the rare macro-freeze report whose
/// re-entrancy path (H1) we could not reproduce in dev.
pub static ACTIVE_FIRE_COUNT: AtomicU32 = AtomicU32::new(0);

// ── Recursion guard for Fire Trigger / Fire Text Expansion macro steps ────
// Thread-local depth counter — execute_action increments on entry, the Drop
// guard decrements on exit (including panic). Beyond MAX_FIRE_DEPTH we abort
// the call so a macro that fires a trigger whose action fires the original
// (directly or via a chain) can't lock the processor thread.
//
// Allowed chain length = MAX_FIRE_DEPTH (initial fire = depth 1, fifth nested
// fire would set depth=6 which trips the guard). 5 is generous for legitimate
// chaining but recovers fast from accidental loops.

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

// ── Suppression guard — ensures SUPPRESS_SIMULATED is always cleared ──────
// Without this guard, a panic between store(true) and store(false) would
// leave SUPPRESS_SIMULATED stuck true, silently disabling all Trigr hotkeys.

pub(crate) struct SuppressionGuard;

impl SuppressionGuard {
    pub(crate) fn new() -> Self {
        crate::hotkeys::SUPPRESS_SIMULATED
            .store(true, Ordering::SeqCst);
        Self
    }
}

impl Drop for SuppressionGuard {
    fn drop(&mut self) {
        crate::hotkeys::SUPPRESS_SIMULATED
            .store(false, Ordering::SeqCst);
    }
}

// ── Unified app launcher (path or AppsFolder AUMID) ───────────────────────
//
// Single helper for both single-action `app` assignments and macro `Open App`
// steps. When `kind == "aumid"`, prefixes the AppID with `shell:AppsFolder\`
// so `ShellExecuteW` resolves it through the Windows Apps namespace — this is
// portable across devices because the AUMID is the same wherever the app is
// installed. When `kind == "path"` (legacy default), launches the absolute
// path directly.

fn shell_launch_app(kind: &str, path: &str, app_id: &str, args: &str) {
    let target = if kind == "aumid" && !app_id.is_empty() {
        format!("shell:AppsFolder\\{}", app_id)
    } else {
        path.to_string()
    };

    if target.is_empty() {
        warn!("[Trigr] Open App: empty target (kind={})", kind);
        return;
    }

    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let params_wide: Vec<u16> = if !args.is_empty() {
        args.encode_utf16().chain(std::iter::once(0)).collect()
    } else {
        Vec::new()
    };
    let params_ptr = if !args.is_empty() { params_wide.as_ptr() } else { std::ptr::null() };

    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params_ptr,
            std::ptr::null(),
            SW_SHOW,
        )
    };
    if (result as usize) > 32 {
        info!("[Trigr] Open App: launched {}", target);
    } else {
        warn!("[Trigr] Open App: ShellExecuteW failed for {} (code {})", target, result as usize);
    }
}

// ── AHK Script Runner process tracking ─────────────────────────────────────

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Mutex, LazyLock};

struct AhkProcess {
    child: Child,
    script_path: PathBuf,
}

static AHK_PROCESSES: LazyLock<Mutex<HashMap<String, AhkProcess>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Held key state for Send Hotkey hold mode.
/// Stores (target_vk, Vec<modifier_vks>) so we can send the correct keyup later.
///
/// `pending_mouse_release` handles the race where handle_mouse_up fires before
/// fire_macro's spawned thread has stored the held state.  Instead of retrying
/// in a sleep loop, we record that the mouse button was already released.  When
/// the hold action finally stores its state it checks the pending flag under the
/// same lock and immediately releases — no timing assumptions needed.
static HELD_KEY: Mutex<HeldKeyManager> = Mutex::new(HeldKeyManager {
    key: None,
    pending_mouse_release: None,
});

struct HeldKeyManager {
    key: Option<HeldKeyState>,
    /// Mouse ID (e.g. "MOUSE_RIGHT") that was released before the hold was stored.
    /// Consumed by the hold setup code — if it matches `trigger_mouse_id`, the hold
    /// is immediately released so the simulated button never stays stuck DOWN.
    pending_mouse_release: Option<String>,
}

struct HeldKeyState {
    target_vk: u16,
    mod_vks: Vec<u16>,
    mouse_button: Option<String>, // Some("LButton") for mouse hold, None for keyboard
    label: String, // e.g. "Ctrl+W" for tray tooltip
    trigger_mouse_id: Option<String>, // e.g. "MOUSE_RIGHT" — when set, release on mouse-up instead of toggle
}

const CF_UNICODETEXT: u32 = 13;

/// Release the currently held key (if any). Safe to call from any thread.
/// Returns the label of the released key (for logging) or None.
pub fn release_held_key() -> Option<String> {
    let mut mgr = HELD_KEY.lock().unwrap();
    if let Some(state) = mgr.key.take() {
        mgr.pending_mouse_release = None; // no longer relevant
        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
        if let Some(ref button) = state.mouse_button {
            // Mouse button release — send the corresponding UP event
            send_mouse_event(button, true);
        } else {
            // Keyboard release
            send_vk_key(state.target_vk, true);
            for &vk in state.mod_vks.iter().rev() {
                send_vk_key(vk, true);
            }
        }
        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
        info!("[Trigr] Released held key: {}", state.label);
        Some(state.label)
    } else {
        None
    }
}

/// Check if a key is currently being held.
pub fn is_key_held() -> bool {
    HELD_KEY.lock().unwrap().key.is_some()
}

/// Release the held key only if it was triggered by the given mouse button (e.g. "MOUSE_RIGHT").
/// Used by handle_mouse_up for press-hold mouse remapping (hold while button is down, release on up).
///
/// If the hold action hasn't stored its state yet (race with fire_macro's spawned
/// thread), we set `pending_mouse_release` so the hold action can release
/// immediately when it finishes — no sleep/retry needed.
///
/// `allow_pending` must be true ONLY when the released button actually has a
/// hold-mode assignment (caller checks the engine state). Without that gate,
/// every ordinary click recorded a pending release — spamming the log AND
/// clobbering the single pending slot, which could leave a genuinely
/// hold-mapped button's synthetic key stuck down (its setup thread would find
/// the wrong button in the slot and never release). The gate deliberately does
/// NOT apply to the release path above it: a matching held key must always be
/// releasable, even if its assignment was deleted mid-hold.
pub fn release_held_if_mouse_trigger(mouse_id: &str, allow_pending: bool) -> Option<String> {
    let mut mgr = HELD_KEY.lock().unwrap();
    let matches = mgr.key.as_ref()
        .and_then(|s| s.trigger_mouse_id.as_deref())
        .map_or(false, |id| id == mouse_id);
    if matches {
        let state = mgr.key.take().unwrap();
        mgr.pending_mouse_release = None;
        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
        if let Some(ref button) = state.mouse_button {
            send_mouse_event(button, true);
        } else {
            send_vk_key(state.target_vk, true);
            for &vk in state.mod_vks.iter().rev() {
                send_vk_key(vk, true);
            }
        }
        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
        info!("[Trigr] Released held key on mouse-up: {}", state.label);
        Some(state.label)
    } else {
        if allow_pending {
            // Hold not stored yet — record that the button was released so the
            // hold action can release immediately when it finishes setting up.
            info!("[Trigr] Mouse-up for {} but no held key yet — setting pending release", mouse_id);
            mgr.pending_mouse_release = Some(mouse_id.to_string());
        }
        None
    }
}

/// Clear any pending mouse release for the given button.
/// Called from handle_mouse_down to prevent stale pending flags from a previous
/// click cycle being consumed by a new hold action.
pub fn clear_pending_mouse_release(mouse_id: &str) {
    let mut mgr = HELD_KEY.lock().unwrap();
    if mgr.pending_mouse_release.as_deref() == Some(mouse_id) {
        mgr.pending_mouse_release = None;
    }
}

// ── Repeat mode state ──────────────────────────────────────────────────────

struct RepeatingKeyState {
    trigger_storage_key: String,
    label: String,
    #[allow(dead_code)]
    interval_ms: u64,
    stop: Arc<AtomicBool>,
}

static REPEATING_KEY: Mutex<Option<RepeatingKeyState>> = Mutex::new(None);

/// Stop the currently repeating key (if any). Safe to call from any thread.
pub fn stop_repeating_key() -> Option<String> {
    let mut rep = REPEATING_KEY.lock().unwrap();
    if let Some(state) = rep.take() {
        state.stop.store(true, Ordering::SeqCst);
        info!("[Trigr] Stopped repeating: {}", state.label);
        Some(state.label)
    } else {
        None
    }
}

/// Check if a key is currently repeating.
pub fn is_repeating() -> bool {
    REPEATING_KEY.lock().unwrap().is_some()
}

/// Get the trigger storage key of the currently repeating key.
pub fn get_repeating_trigger() -> Option<String> {
    REPEATING_KEY.lock().unwrap().as_ref().map(|s| s.trigger_storage_key.clone())
}

// ── Timing constants ────────────────────────────────────────────────────────

const MODIFIER_SETTLE_MS: u64 = 30;
const KEYSTROKE_DELAY_MS: u64 = 10;

// Open URL launches the default browser via ShellExecute, which is async.
// Without a settle pause the next macro step targets Trigr's HWND instead of
// the new browser window. 250ms is enough for warm-cache launches; cold-start
// browsers may still miss, in which case the user can add an explicit Wait step.
const OPEN_URL_FOCUS_SETTLE_MS: u64 = 250;

/// Prepend https:// to a URL if no recognised scheme is present.
/// Windows ShellExecute requires a scheme — bare "google.com" silently no-ops,
/// while "www.google.com" only works because Windows guesses for that prefix.
fn normalise_url(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Already has a scheme — pass through unchanged.
    let lower = trimmed.to_lowercase();
    let schemes = [
        "http://", "https://", "ftp://", "file://",
        "mailto:", "tel:", "sms:", "javascript:", "about:",
    ];
    if schemes.iter().any(|s| lower.starts_with(s)) {
        return trimmed.to_string();
    }
    // No scheme — default to https.
    format!("https://{}", trimmed)
}

/// Speed presets: (initial_delay, step_settle, foreground_settle, clipboard_restore)
fn speed_delays() -> (u64, u64, u64, u64) {
    let state = crate::hotkeys::engine_state().lock().unwrap();
    match state.macro_speed.as_str() {
        "fast"    => (5,  5,  5, 25),
        "instant" => (0,  0,  5, 25),
        "custom"  => {
            let pre = state.custom_pre_execution_delay;
            // Scale foreground settle and clipboard restore proportionally to pre-execution
            let fg = if pre == 0 { 5 } else { (pre / 10).max(5) };
            let clip = if pre == 0 { 25 } else { (pre / 3).max(25) };
            (pre.min(10), pre.min(10), fg, clip)
        }
        _         => (10, 10, 10, 50), // "safe" (default)
    }
}

// ── Modifier VK codes ───────────────────────────────────────────────────────

const VK_LCONTROL: u16 = 0xA2;
const VK_LALT: u16 = 0xA4;
const VK_LSHIFT: u16 = 0xA0;
const VK_LWIN: u16 = 0x5B;
const VK_BACKSPACE: u16 = 0x08;
const VK_INSERT: u16 = 0x2D;

// ── Public action executor ──────────────────────────────────────────────────

/// Execute a macro action. Called from the hotkey processor thread.
/// `target_hwnd` = the foreground window HWND captured at hotkey fire time.
/// `is_altgr` = true if Ctrl+Alt (AltGr) was held — dead character may have leaked.
pub fn execute_action(macro_val: &Value, is_bare: bool, target_hwnd: isize, is_altgr: bool, trigger_key: Option<&str>, app: &tauri::AppHandle) {
    // Recursion guard — increment depth, register Drop to decrement on any exit.
    // The +1 is captured before the early-return so the guard always pairs with
    // the increment (the Drop runs regardless of where we return below).
    let depth = FIRE_DEPTH.with(|d| {
        let next = d.get() + 1;
        d.set(next);
        next
    });
    let _depth_guard = DepthGuard;
    if depth > MAX_FIRE_DEPTH {
        warn!(
            "[Trigr] Fire recursion limit hit (depth {}, max {}) — aborting. A trigger or text expansion is calling itself directly or via a chain.",
            depth, MAX_FIRE_DEPTH
        );
        return;
    }

    let macro_type = macro_val
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let label = macro_val
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("(unlabelled)");
    let data = macro_val.get("data");

    log::info!("[ACTION] Firing: [{}] {} altgr={}", macro_type, label, is_altgr);
    info!("[Trigr] Firing: [{}] {} (depth {})", macro_type, label, depth);

    let (initial_ms, step_settle_ms, _fg_settle_ms, _clip_restore_ms) = speed_delays();

    // Initial delay — lets Windows finish delivering the trigger keydown.
    // Skip for hotkey actions: trigger was already suppressed by the hook, nothing to wait for.
    if initial_ms > 0 && macro_type != "hotkey" { thread::sleep(Duration::from_millis(initial_ms)); }

    // Erase leaked character for bare keys or AltGr dead characters
    if is_bare || is_altgr {
        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
        if is_altgr {
            thread::sleep(Duration::from_millis(10));
        }
        send_vk_tap(VK_BACKSPACE);
        thread::sleep(Duration::from_millis(5));
        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
    }

    // NOTE: modifier release is handled by each action handler (inject_via_clipboard,
    // send_unicode_text, execute_send_hotkey) using release_held_modifiers() which
    // reads physical state via GetAsyncKeyState. Do NOT release here — it would
    // fool GetAsyncKeyState into thinking modifiers are already up.

    match macro_type {
        "text" => {
            if let Some(text) = data.and_then(|d| d.get("text")).and_then(|v| v.as_str()) {
                if step_settle_ms > 0 { thread::sleep(Duration::from_millis(step_settle_ms)); }
                let method = resolve_input_method(data);
                let resolved = resolve_type_text_tokens(text);
                output_text(&resolved, &method, target_hwnd);
            }
        }

        "hotkey" => {
            if let Some(d) = data {
                // Skip step delay for plain hotkey — SendInput doesn't need settle time
                execute_send_hotkey(d, trigger_key, app);
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

        "app" => {
            let kind = data.and_then(|d| d.get("kind")).and_then(|v| v.as_str()).unwrap_or("path");
            let app_id = data.and_then(|d| d.get("appId")).and_then(|v| v.as_str()).unwrap_or("");
            let path = data.and_then(|d| d.get("path")).and_then(|v| v.as_str()).unwrap_or("");
            shell_launch_app(kind, path, app_id, "");
        }

        "folder" => {
            if let Some(path) = data.and_then(|d| d.get("path")).and_then(|v| v.as_str()) {
                let _ = opener::open(path);
            }
        }

        "macro" => {
            if let Some(steps) = data.and_then(|d| d.get("steps")).and_then(|v| v.as_array()) {
                let method = resolve_input_method(data);
                let uses_clipboard = method != "send-input" && method != "direct";
                let mut current_hwnd = target_hwnd;
                let (_, settle_ms, _, clip_restore_ms) = speed_delays();
                info!("[Trigr] Macro sequence: {} step(s), method={}", steps.len(), method);

                // For clipboard method: snapshot once, batch pastes, restore once.
                // Snapshot captures EVERY format (CF_DIB, RTF, CF_HDROP, registered
                // formats) so non-text clipboard content (e.g. an image from Snagit)
                // is preserved across the macro — text-only save would silently drop
                // the image and leak the expansion text into the Windows clipboard.
                let saved_snapshot = if uses_clipboard {
                    crate::expansions::snapshot_clipboard()
                } else {
                    Vec::new()
                };
                let mut clipboard_dirty = false;

                for (i, step) in steps.iter().enumerate() {
                    let step_type = step.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let step_value = step.get("value").and_then(|v| v.as_str()).unwrap_or("");
                    info!("[Trigr]   Step {}/{}: [{}] \"{}\"", i + 1, steps.len(), step_type, step_value);

                    if matches!(step_type, "Type Text" | "Dynamic Text") && uses_clipboard && !step_value.is_empty() {
                        if settle_ms > 0 { thread::sleep(Duration::from_millis(settle_ms)); }
                        let resolved = resolve_type_text_tokens(step_value);
                        clipboard_paste_core(&resolved, current_hwnd);
                        clipboard_dirty = true;
                    } else {
                        // Restore clipboard before non-Type-Text steps if we dirtied it.
                        // Inter-step restore — no seqnum guard because the macro is a
                        // controlled sequential flow; the user isn't expected to copy
                        // something mid-macro.
                        if clipboard_dirty {
                            thread::sleep(Duration::from_millis(clip_restore_ms));
                            crate::expansions::restore_clipboard_snapshot(&saved_snapshot);
                            SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
                            clipboard_dirty = false;
                        }
                        let cont = execute_macro_step(step, &mut current_hwnd, &method, app);
                        if !cont {
                            info!("[Trigr] Macro aborted at step {}/{} ({})", i + 1, steps.len(), step_type);
                            // Break out of the steps loop. The post-loop clipboard
                            // restore (just below this for-block) still runs, so any
                            // mid-paste state is cleaned up correctly.
                            break;
                        }
                    }

                    // After steps that may change focus, re-capture foreground HWND
                    // so subsequent Type Text / Press Key targets the correct window.
                    // Open URL is async (ShellExecute → browser launch) so we sleep a
                    // short stabilisation window before reading the foreground.
                    // Fire Trigger is included because the nested action it invokes
                    // may itself contain Focus Window / Open App / Open URL — without
                    // a re-capture, subsequent parent-macro steps would target the
                    // pre-fire window.
                    if matches!(step_type, "Wait (ms)" | "Wait for Input" | "Open App" | "Focus Window" | "Wait for Window" | "Click at Position" | "Open URL" | "Fire Trigger" | "Fire Text Expansion") {
                        if step_type == "Open URL" {
                            thread::sleep(Duration::from_millis(OPEN_URL_FOCUS_SETTLE_MS));
                        }
                        let new_hwnd = unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
                        };
                        if new_hwnd != 0 && new_hwnd != current_hwnd {
                            current_hwnd = new_hwnd;
                        } else if step_type == "Open URL" {
                            warn!("[Trigr] Open URL: foreground HWND unchanged after {}ms settle. Subsequent steps will target the pre-launch window. Add a Wait step if the browser is slow to focus.", OPEN_URL_FOCUS_SETTLE_MS);
                        }
                    }
                }

                // Final restore after all steps. Seqnum guard: if the user copied
                // something during the final paste window, leave their content.
                if clipboard_dirty {
                    let post_seq = crate::expansions::clipboard_sequence_number();
                    thread::sleep(Duration::from_millis(clip_restore_ms));
                    if crate::expansions::clipboard_sequence_number() == post_seq {
                        crate::expansions::restore_clipboard_snapshot(&saved_snapshot);
                    }
                    SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
                }
            }
        }

        "ahk" => {
            if let Some(script) = data.and_then(|d| d.get("script")).and_then(|v| v.as_str()) {
                if !script.trim().is_empty() {
                    let version = data.and_then(|d| d.get("ahkVersion")).and_then(|v| v.as_str()).unwrap_or("v1");
                    execute_ahk_script(script, version, trigger_key, app);
                }
            }
        }

        _ => {
            warn!("[Trigr] Unknown macro type: {}", macro_type);
        }
    }

    // Re-press modifiers that were held (user may still be holding them physically)
    // Skip this — the user's physical key state will naturally reassert via the hook
}

// ── Input method resolution ─────────────────────────────────────────────────

/// Resolve the effective input method: macro override → global default (shift-insert).
fn resolve_input_method(data: Option<&Value>) -> String {
    if let Some(d) = data {
        // Check macro-level override
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
    // Fall through to global default from settings
    let state = crate::hotkeys::engine_state().lock().unwrap();
    state.global_input_method.clone()
}

// ── Text output dispatcher ──────────────────────────────────────────────────

/// Resolve dynamic tokens ({date}, {time}, {clipboard}, {{var}}, ...) in
/// macro Type Text strings. Mirrors the resolution that fire_expansion does
/// for text expansions, so users get consistent token behaviour across
/// expansions and macros. {cursor} is resolved (token removed) but cursor
/// positioning isn't honoured here — output_text has no caret-back hook.
fn resolve_type_text_tokens(text: &str) -> String {
    let global_vars = crate::expansions::get_global_variables();
    let (resolved, _cursor_back) = crate::expansions::resolve_tokens(text, &global_vars);
    resolved
}

fn output_text(text: &str, method: &str, target_hwnd: isize) {
    match method {
        "send-input" | "direct" => {
            // Character-by-character fallback for apps that don't support paste
            info!("[Trigr] Output text (sendinput): \"{}\"", crate::expansions::log_preview(text));
            send_unicode_text(text, target_hwnd);
        }
        _ => {
            // Default: clipboard paste (instant)
            info!("[Trigr] Output text (clipboard): \"{}\"", crate::expansions::log_preview(text));
            inject_via_clipboard(text, target_hwnd);
        }
    }
}

// ── Clipboard paste injection ───────────────────────────────────────────────
// CRITICAL: SUPPRESS_SIMULATED must be set true before any SendInput call.
// SUPPRESS_NEXT_CLIPBOARD_WRITE must be set before any internal clipboard write.
// New injection paths must follow this pattern or Trigr will intercept its own
// simulated keystrokes and/or log its own clipboard writes.

fn inject_via_clipboard(text: &str, target_hwnd: isize) {
    // Snapshot every clipboard format (CF_DIB, RTF, CF_HDROP, registered formats)
    // so non-text content (e.g. an image from Snagit) is preserved across the
    // expansion. Text-only save silently drops the image and leaks the expansion
    // text into the Windows clipboard for the user's next Ctrl+V.
    let snapshot = crate::expansions::snapshot_clipboard();
    let (_, _, _, clip_restore_ms) = speed_delays();
    clipboard_paste_core(text, target_hwnd);
    // Capture sequence number AFTER our write so we can detect a third-party
    // (or user) clipboard change during the paste window.
    let post_write_seq = crate::expansions::clipboard_sequence_number();
    thread::sleep(Duration::from_millis(clip_restore_ms));
    if crate::expansions::clipboard_sequence_number() == post_write_seq {
        crate::expansions::restore_clipboard_snapshot(&snapshot);
    }
    SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
}

/// Core clipboard paste: write text to clipboard + send paste keystroke.
/// Does NOT save/restore the clipboard — caller is responsible for that.
fn clipboard_paste_core(text: &str, target_hwnd: isize) {
    let write_ok = write_clipboard(text);
    info!("[Trigr] Clipboard write (actions, ok={}): \"{}\"", write_ok, crate::expansions::log_preview(text));
    if !write_ok {
        warn!("[Trigr] Skipping paste — clipboard write failed, would paste wrong content");
        return;
    }

    let (_, _, fg_settle_ms, _) = speed_delays();

    let _suppress = SuppressionGuard::new();
    let held = release_held_modifiers();

    if target_hwnd != 0 {
        unsafe {
            SetForegroundWindow(target_hwnd as _);
        }
        if fg_settle_ms > 0 { thread::sleep(Duration::from_millis(fg_settle_ms)); }
    }

    // Per-app override: VS Code WSL terminal (xterm.js) needs Shift+Insert with
    // KEYEVENTF_EXTENDEDKEY because bash readline treats raw Ctrl+V as
    // quoted-insert, and xterm.js's paste keybinding only resolves DOM event.code
    // to "Insert" when the extended-key flag is set. Without this, Send Text
    // hotkey actions silently fail in VS Code's WSL terminal. See feedback memory
    // chromium_terminal_paste for the full diagnosis.
    let needs_shift_insert = crate::expansions::target_needs_shift_insert(target_hwnd);
    let use_ctrl_v = !needs_shift_insert && !is_ctrl_v_mapped();
    if use_ctrl_v {
        send_vk_key(VK_LCONTROL, false);
        send_vk_key(0x56, false); // V
        send_vk_key(0x56, true);
        send_vk_key(VK_LCONTROL, true);
    } else {
        send_vk_key(VK_LSHIFT, false);
        if needs_shift_insert {
            crate::expansions::send_vk_key_extended(VK_INSERT, false);
            crate::expansions::send_vk_key_extended(VK_INSERT, true);
        } else {
            send_vk_key(VK_INSERT, false);
            send_vk_key(VK_INSERT, true);
        }
        send_vk_key(VK_LSHIFT, true);
    }

    restore_modifiers(&held);
    // _suppress drops here → SUPPRESS_SIMULATED = false (even on panic)
}

/// Check if Ctrl+V is mapped as a hotkey in the current assignments.
fn is_ctrl_v_mapped() -> bool {
    let state = crate::hotkeys::engine_state().lock().unwrap();
    let profile = &state.active_profile;
    let key = format!("{}::Ctrl::KeyV", profile);
    state.assignments.contains_key(&key)
}

// ── Win32 clipboard operations ──────────────────────────────────────────────

fn read_clipboard() -> Option<String> {
    // Retry up to 5 times — clipboard may be briefly held by another process
    for attempt in 0..5 {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                if attempt < 4 { thread::sleep(Duration::from_millis(3)); continue; }
                return None;
            }
            let handle = GetClipboardData(CF_UNICODETEXT);
            if handle.is_null() {
                CloseClipboard();
                return None;
            }
            let ptr = GlobalLock(handle) as *const u16;
            if ptr.is_null() {
                CloseClipboard();
                return None;
            }
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            GlobalUnlock(handle);
            CloseClipboard();
            return Some(text);
        }
    }
    None
}

fn write_clipboard(text: &str) -> bool {
    write_clipboard_impl(text, true)
}

/// Write to the clipboard but let the clipboard listener record this write as a
/// new history entry (used by Save as New / transform copies where the new text
/// is a genuinely novel variant the user wants in their history).
fn write_clipboard_recordable(text: &str) -> bool {
    write_clipboard_impl(text, false)
}

fn write_clipboard_impl(text: &str, suppress_listener: bool) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let byte_len = wide.len() * 2;
    // Set suppress BEFORE touching the clipboard so any clipboard listener skips this write
    if suppress_listener {
        SUPPRESS_NEXT_CLIPBOARD_WRITE.store(true, Ordering::SeqCst);
    }
    // Retry up to 10 times — clipboard may be briefly held by the clipboard listener
    for attempt in 0..10 {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                if attempt < 9 { thread::sleep(Duration::from_millis(3)); continue; }
                let err = windows_sys::Win32::Foundation::GetLastError();
                log::warn!("[CLIP] OpenClipboard failed after retries, GetLastError={}", err);
                return false;
            }
            if EmptyClipboard() == 0 {
                let err = windows_sys::Win32::Foundation::GetLastError();
                log::warn!("[CLIP] EmptyClipboard failed, GetLastError={}", err);
                CloseClipboard();
                return false;
            }
            let h_mem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
            if h_mem.is_null() {
                log::warn!("[CLIP] GlobalAlloc failed for {} bytes", byte_len);
                CloseClipboard();
                return false;
            }
            let ptr = GlobalLock(h_mem) as *mut u16;
            if ptr.is_null() {
                log::warn!("[CLIP] GlobalLock failed");
                CloseClipboard();
                return false;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            GlobalUnlock(h_mem);
            let result = SetClipboardData(CF_UNICODETEXT, h_mem);
            if result.is_null() {
                let err = windows_sys::Win32::Foundation::GetLastError();
                log::warn!("[CLIP] SetClipboardData failed, GetLastError={}", err);
                CloseClipboard();
                return false;
            }
            // For Trigr's own paste writes (suppress_listener = true), keep the
            // injected text out of Windows Clipboard History (Win+V) and Cloud
            // Clipboard. Recordable writes (Save as New etc.) pass false and are
            // deliberately left visible. Target apps read CF_UNICODETEXT either
            // way — paste is unaffected.
            if suppress_listener {
                crate::expansions::mark_clipboard_excluded();
            }
            CloseClipboard();
            // Record the seqnum so the listener skips our own write even if the
            // WM_CLIPBOARDUPDATE arrives after SUPPRESS_NEXT is cleared.
            if suppress_listener {
                record_self_clipboard_write();
            }
            return true;
        }
    }
    false
}

// ── Type Text: character-by-character fallback ──────────────────────────────

fn send_unicode_text(text: &str, target_hwnd: isize) {
    let (_, _, fg_settle_ms, _) = speed_delays();
    let _suppress = SuppressionGuard::new();
    let held = release_held_modifiers();

    // Restore focus to target window
    if target_hwnd != 0 {
        unsafe {
            SetForegroundWindow(target_hwnd as _);
        }
        if fg_settle_ms > 0 { thread::sleep(Duration::from_millis(fg_settle_ms)); }
    }

    for ch in text.chars() {
        let code = ch as u32;
        if code > 0xFFFF {
            let adjusted = code - 0x10000;
            let hi = (0xD800 + (adjusted >> 10)) as u16;
            let lo = (0xDC00 + (adjusted & 0x3FF)) as u16;
            send_unicode_key(hi, false);
            send_unicode_key(hi, true);
            send_unicode_key(lo, false);
            send_unicode_key(lo, true);
        } else {
            send_unicode_key(code as u16, false);
            send_unicode_key(code as u16, true);
        }
        if KEYSTROKE_DELAY_MS > 0 {
            thread::sleep(Duration::from_millis(KEYSTROKE_DELAY_MS));
        }
    }

    restore_modifiers(&held);
}

fn send_unicode_key(scan: u16, key_up: bool) {
    let mut flags = KEYEVENTF_UNICODE;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

// ── Direct inline key remap (AHK-style T::W passthrough) ────────────────────

/// Build a single VK keyboard INPUT struct (key-down or key-up).
fn make_vk_input(vk: u16, key_up: bool) -> INPUT {
    let flags = if key_up { KEYEVENTF_KEYUP } else { 0 };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk as VIRTUAL_KEY,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// trigger_vk → (target_vk, mod_vks)
/// Populated on keydown, removed on keyup so hold and OS key-repeat work correctly.
static ACTIVE_BARE_REMAPS: LazyLock<Mutex<HashMap<u16, (u16, Vec<u16>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Press phase of a bare-key remap (called on keydown / OS key-repeat events).
///
/// Sends mod_downs + target_keydown only — NO keyup.  The matching
/// `remap_key_release` sends the keyup when the trigger is released.
/// This gives true AHK-style hold behaviour: hold I → Tab is held in the game.
///
/// Returns `false` → fall through to `fire_macro` for:
///   - mouse buttons, hold mode, repeat mode, unknown key name.
pub fn remap_key_press(trigger_vk: u16, data: &Value) -> bool {
    let key_name = match data.get("key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return false,
    };
    if is_mouse_button(key_name) { return false; }
    if data.get("holdMode").and_then(|v| v.as_bool()).unwrap_or(false) { return false; }
    if data.get("repeatMode").and_then(|v| v.as_bool()).unwrap_or(false) { return false; }

    let target_vk = match display_name_to_vk(key_name) {
        Some(vk) => vk,
        None => return false,
    };

    let mod_vks: Vec<u16> = data
        .get("modifiers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| match v.as_str()?.to_lowercase().as_str() {
                    "ctrl" => Some(VK_LCONTROL),
                    "alt" => Some(VK_LALT),
                    "shift" => Some(VK_LSHIFT),
                    "win" => Some(VK_LWIN),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();

    // Record active remap so keyup knows which target VK to release.
    // Overwriting on OS key-repeat is intentional and idempotent.
    ACTIVE_BARE_REMAPS.lock().unwrap().insert(trigger_vk, (target_vk, mod_vks.clone()));

    // Send mod_downs + target_down (keyup is sent by remap_key_release).
    let mut inputs: Vec<INPUT> = Vec::with_capacity(mod_vks.len() + 1);
    for &vk in &mod_vks {
        inputs.push(make_vk_input(vk, false));
    }
    inputs.push(make_vk_input(target_vk, false));

    let _guard = SuppressionGuard::new();
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
    true
}

/// Release phase of a bare-key remap (called on keyup).
///
/// Sends target_keyup + mod_ups for the remap that was started by `remap_key_press`.
/// Returns `true` if a remap was active for this trigger VK (caller should early-return).
pub fn remap_key_release(trigger_vk: u16) -> bool {
    let entry = ACTIVE_BARE_REMAPS.lock().unwrap().remove(&trigger_vk);
    if let Some((target_vk, mod_vks)) = entry {
        let mut inputs: Vec<INPUT> = Vec::with_capacity(mod_vks.len() + 1);
        inputs.push(make_vk_input(target_vk, true));
        for &vk in mod_vks.iter().rev() {
            inputs.push(make_vk_input(vk, true));
        }
        let _guard = SuppressionGuard::new();
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
        true
    } else {
        false
    }
}

/// Execute a modified hotkey combo (e.g. Ctrl+K → Ctrl+C) inline on the calling
/// thread — no thread spawn, no `pending_macro` deferral, fires on keydown.
///
/// Returns `false` → fall through to `pending_macro` / `fire_macro` for:
///   - mouse buttons, hold mode, repeat mode, unknown key name.
pub fn execute_hotkey_inline(data: &Value, _app: &tauri::AppHandle) -> bool {
    let key_name = match data.get("key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return false,
    };
    if is_mouse_button(key_name) { return false; }
    if data.get("holdMode").and_then(|v| v.as_bool()).unwrap_or(false) { return false; }
    if data.get("repeatMode").and_then(|v| v.as_bool()).unwrap_or(false) { return false; }

    let target_vk = match display_name_to_vk(key_name) {
        Some(vk) => vk,
        None => return false,
    };

    let mod_vks: Vec<u16> = data
        .get("modifiers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| match v.as_str()?.to_lowercase().as_str() {
                    "ctrl" => Some(VK_LCONTROL),
                    "alt" => Some(VK_LALT),
                    "shift" => Some(VK_LSHIFT),
                    "win" => Some(VK_LWIN),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();

    // Release the physically held trigger modifiers (e.g. Ctrl from Ctrl+K).
    // Done outside SuppressionGuard so the key-ups update Trigr's modifier tracking.
    let held = release_held_modifiers();

    // Batched SendInput: target mod_downs, target_down, target_up, target mod_ups.
    let mut inputs: Vec<INPUT> = Vec::with_capacity(mod_vks.len() * 2 + 2);
    for &vk in &mod_vks { inputs.push(make_vk_input(vk, false)); }
    inputs.push(make_vk_input(target_vk, false));
    inputs.push(make_vk_input(target_vk, true));
    for &vk in mod_vks.iter().rev() { inputs.push(make_vk_input(vk, true)); }
    {
        let _guard = SuppressionGuard::new();
        unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32); }
    }

    // Restore trigger modifiers so the user's held keys remain active.
    restore_modifiers(&held);
    true
}

// ── Send Hotkey: VK-based key simulation ────────────────────────────────────

fn execute_send_hotkey(data: &Value, trigger_key: Option<&str>, app: &tauri::AppHandle) {
    let key_name = match data.get("key").and_then(|v| v.as_str()) {
        Some(k) => k,
        None => return,
    };

    let is_mouse = is_mouse_button(key_name);

    // Parse modifiers and VK (keyboard only)
    let modifiers: Vec<String> = if is_mouse {
        vec![]
    } else {
        data.get("modifiers")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default()
    };

    // Bare-modifier mode: when the user captured a sole modifier (Ctrl / Shift
    // / Alt / Win alone), the frontend sends key="" with non-empty modifiers.
    // target_vk stays 0 to signal "no main key" — keystroke chains below skip
    // the main-key tap and only press/release the modifier chord.
    let target_vk: u16 = if is_mouse {
        0
    } else if key_name.is_empty() {
        if modifiers.is_empty() {
            warn!("[Trigr] Send Hotkey has no key or modifiers — nothing to send");
            return;
        }
        0
    } else {
        match display_name_to_vk(key_name) {
            Some(vk) => vk,
            None => {
                warn!("[Trigr] Unknown Send Hotkey key: {}", key_name);
                return;
            }
        }
    };
    let has_main_key = !is_mouse && target_vk != 0;

    let hold_mode = data.get("holdMode").and_then(|v| v.as_bool()).unwrap_or(false);
    let repeat_mode = data.get("repeatMode").and_then(|v| v.as_bool()).unwrap_or(false);
    let repeat_interval = data.get("repeatInterval").and_then(|v| v.as_u64()).unwrap_or(100).max(50);

    let mod_vks: Vec<u16> = modifiers
        .iter()
        .filter_map(|m| match m.to_lowercase().as_str() {
            "ctrl" => Some(VK_LCONTROL),
            "alt" => Some(VK_LALT),
            "shift" => Some(VK_LSHIFT),
            "win" => Some(VK_LWIN),
            _ => None,
        })
        .collect();

    let combo_label = if key_name.is_empty() {
        modifiers.join("+")
    } else if modifiers.is_empty() {
        key_name.to_string()
    } else {
        format!("{}+{}", modifiers.join("+"), key_name)
    };

    // ── Repeat mode ──
    // Brief sleep inside the SuppressionGuard scope after SendInput. SendInput
    // returns when events are inserted into the system queue, not when the LL
    // hook processes them — without this drain window, the guard drops before
    // the hook finishes, and our synthetic events get treated as real input
    // (buffer push, modifier atomic churn, and suppress_keys swallow for any
    // synthetic that matches a Trigr binding). 5ms covers typical dispatch.
    const SUPPRESS_DRAIN_MS: u64 = 5;
    // Hold duration between synthetic KEYDOWN and KEYUP. Games poll key state
    // per-frame (60fps = ~16.67ms, 144fps = ~6.94ms). A back-to-back keydown
    // and keyup sent in a single SendInput batch finishes in microseconds —
    // the game's polling loop never sees the key as "down" on any frame, so
    // the press doesn't register. 15ms covers 1+ frame at 60fps and 2+ at
    // 144fps. Matches the ballpark of AHK's SetKeyDelay default (~10-20ms).
    const KEY_HOLD_MS: u64 = 15;
    if repeat_mode {
        let trigger_storage_key = trigger_key.unwrap_or("").to_string();

        // Check if already repeating
        {
            let mut rep = REPEATING_KEY.lock().unwrap();
            if let Some(ref state) = *rep {
                if state.trigger_storage_key == trigger_storage_key {
                    // Same trigger — stop (toggle off)
                    state.stop.store(true, Ordering::SeqCst);
                    info!("[Trigr] Repeat stopped (toggle): {}", combo_label);
                    *rep = None;
                    drop(rep);
                    crate::tray::update_tray_icon_normal(app);
                    return;
                } else {
                    // Different trigger — stop old, start new
                    state.stop.store(true, Ordering::SeqCst);
                    info!("[Trigr] Repeat stopped (switching): {}", state.label);
                    *rep = None;
                }
            }
        }

        // Start repeating
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = stop.clone();
        let app_clone = app.clone();
        let key_name_owned = key_name.to_string();
        let mod_vks_clone = mod_vks.clone();
        let is_mouse_copy = is_mouse;
        let target_vk_copy = target_vk;

        {
            let mut rep = REPEATING_KEY.lock().unwrap();
            *rep = Some(RepeatingKeyState {
                trigger_storage_key,
                label: combo_label.clone(),
                interval_ms: repeat_interval,
                stop: stop.clone(),
            });
        }

        crate::tray::update_tray_icon_repeating(app, &combo_label, repeat_interval);
        info!("[Trigr] Repeat started: {} ({}ms)", combo_label, repeat_interval);

        thread::spawn(move || {
            // Request 1ms timer resolution for the lifetime of this thread.
            // Without it, thread::sleep on Windows runs at the default scheduler
            // quantum (~15.625ms), so sleep(100) actually waits 109-125ms and
            // the configured rate drifts low. timeBeginPeriod is per-process on
            // Windows 8.1+, so this doesn't impact other apps.
            unsafe {
                windows_sys::Win32::Media::timeBeginPeriod(1);
            }

            loop {
                if stop_clone.load(Ordering::SeqCst) { break; }
                if !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) { break; }

                if is_mouse_copy {
                    send_mouse_click(&key_name_owned);
                } else {
                    // Split into separate KEYDOWN and KEYUP batches with a hold
                    // window between them. Games / DirectInput-style apps poll
                    // key state per-frame; a fused down+up in one SendInput
                    // batch finishes in microseconds and the poll never sees
                    // the key as down. Modifiers wrap around the main key:
                    // mods-down + key-down → hold → key-up + mods-up-reversed.
                    let mut down_inputs: Vec<INPUT> = Vec::with_capacity(mod_vks_clone.len() + 1);
                    for &vk in &mod_vks_clone { down_inputs.push(make_vk_input(vk, false)); }
                    if target_vk_copy != 0 {
                        down_inputs.push(make_vk_input(target_vk_copy, false));
                    }
                    let mut up_inputs: Vec<INPUT> = Vec::with_capacity(mod_vks_clone.len() + 1);
                    if target_vk_copy != 0 {
                        up_inputs.push(make_vk_input(target_vk_copy, true));
                    }
                    for &vk in mod_vks_clone.iter().rev() { up_inputs.push(make_vk_input(vk, true)); }

                    // Hold the SuppressionGuard across both phases plus the
                    // drain window. SendInput returns when events are queued,
                    // not when the LL hook has processed them — without the
                    // drain, the guard drops microseconds after the final
                    // SendInput and our synthetic events leak into the
                    // expansion buffer + modifier atomics + (if the synthetic
                    // key is bound elsewhere) the suppress_keys swallow path.
                    let _guard = SuppressionGuard::new();
                    unsafe { SendInput(down_inputs.len() as u32, down_inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32); }
                    thread::sleep(Duration::from_millis(KEY_HOLD_MS));
                    unsafe { SendInput(up_inputs.len() as u32, up_inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32); }
                    thread::sleep(Duration::from_millis(SUPPRESS_DRAIN_MS));
                }

                // Subtract in-guard sleeps from the outer sleep so the total
                // iteration matches the configured interval. saturating_sub
                // covers any case where the in-guard window exceeds the
                // configured interval (impossible at the .max(50) floor with
                // current constants: 15 + 5 = 20 < 50).
                thread::sleep(Duration::from_millis(
                    repeat_interval
                        .saturating_sub(KEY_HOLD_MS)
                        .saturating_sub(SUPPRESS_DRAIN_MS),
                ));
            }
            // Cleanup: clear state if this thread's stop flag is still the active one
            {
                let mut rep = REPEATING_KEY.lock().unwrap();
                if let Some(ref state) = *rep {
                    if Arc::ptr_eq(&state.stop, &stop_clone) {
                        *rep = None;
                    }
                }
            }
            unsafe {
                windows_sys::Win32::Media::timeEndPeriod(1);
            }
            crate::tray::update_tray_icon_normal(&app_clone);
        });
        return;
    }

    // ── Hold mode ──
    if hold_mode {
        let mut mgr = HELD_KEY.lock().unwrap();

        // Check if same key already held — toggle release
        let same_held = if let Some(ref state) = mgr.key {
            if is_mouse {
                state.mouse_button.as_deref() == Some(key_name)
            } else {
                state.target_vk == target_vk && state.mod_vks == mod_vks && state.mouse_button.is_none()
            }
        } else {
            false
        };

        if same_held {
            // Release it
            let state = mgr.key.take().unwrap();
            mgr.pending_mouse_release = None;
            {
                let _guard = SuppressionGuard::new();
                if let Some(ref button) = state.mouse_button {
                    send_mouse_event(button, true);
                } else {
                    // Skip main-key release for bare-modifier holds (target_vk == 0).
                    if state.target_vk != 0 {
                        send_vk_key(state.target_vk, true);
                    }
                    for &vk in state.mod_vks.iter().rev() {
                        send_vk_key(vk, true);
                    }
                }
            }
            info!("[Trigr] Hold released: {}", combo_label);
            drop(mgr);
            crate::tray::update_tray_icon_normal(app);
            return;
        }

        // Different key held — release previous first
        if let Some(ref state) = mgr.key {
            {
                let _guard = SuppressionGuard::new();
                if let Some(ref button) = state.mouse_button {
                    send_mouse_event(button, true);
                } else {
                    if state.target_vk != 0 {
                        send_vk_key(state.target_vk, true);
                    }
                    for &vk in state.mod_vks.iter().rev() {
                        send_vk_key(vk, true);
                    }
                }
            }
            info!("[Trigr] Hold released (switching): {}", state.label);
        }

        // Hold the new key/button
        log::info!("[ACTION] Send Hotkey HOLD: {}", combo_label);
        {
            let _guard = SuppressionGuard::new();
            if is_mouse {
                send_mouse_event(key_name, false); // mousedown only
            } else {
                let physically_held = release_held_modifiers();
                for &vk in &mod_vks {
                    send_vk_key(vk, false);
                }
                if has_main_key {
                    send_vk_key(target_vk, false);
                }
                // Do NOT send keyup — key/modifiers stay held
                restore_modifiers(&physically_held);
            }
        }

        // Detect if the trigger was a mouse button (from the storage key)
        let trigger_mouse = trigger_key
            .and_then(|tk| tk.split("::").last())
            .filter(|last| last.starts_with("MOUSE_"))
            .map(|s| s.to_string());

        mgr.key = Some(HeldKeyState {
            target_vk,
            mod_vks: mod_vks.clone(),
            mouse_button: if is_mouse { Some(key_name.to_string()) } else { None },
            label: combo_label.clone(),
            trigger_mouse_id: trigger_mouse.clone(),
        });

        // Check if the mouse button was already released before we stored the hold.
        // This happens when the user clicks and releases quickly — handle_mouse_up
        // ran before this thread and set pending_mouse_release.
        let already_released = trigger_mouse.as_deref()
            .and_then(|tm| mgr.pending_mouse_release.as_deref().filter(|&p| p == tm))
            .is_some();

        if already_released {
            // Immediately release — the button UP event already fired
            let state = mgr.key.take().unwrap();
            mgr.pending_mouse_release = None;
            {
                let _guard = SuppressionGuard::new();
                if let Some(ref button) = state.mouse_button {
                    send_mouse_event(button, true);
                } else {
                    send_vk_key(state.target_vk, true);
                    for &vk in state.mod_vks.iter().rev() {
                        send_vk_key(vk, true);
                    }
                }
            }
            info!("[Trigr] Hold immediately released — mouse was already up: {}", combo_label);
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
        info!("[Trigr] Send Hotkey → mouse click: {}", key_name);
        send_mouse_click(key_name);
    } else {
        let held = release_held_modifiers();
        let mut inputs: Vec<INPUT> = Vec::with_capacity(mod_vks.len() * 2 + 2);
        for &vk in &mod_vks { inputs.push(make_vk_input(vk, false)); }
        // Skip main-key tap when in bare-modifier mode — the modifier chord
        // itself is the full hotkey (down all, up all in reverse).
        if has_main_key {
            inputs.push(make_vk_input(target_vk, false));
            inputs.push(make_vk_input(target_vk, true));
        }
        for &vk in mod_vks.iter().rev() { inputs.push(make_vk_input(vk, true)); }
        {
            let _guard = SuppressionGuard::new();
            unsafe { SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32); }
        }
        restore_modifiers(&held);
    }
}

// ── Focus Window — find a window by process name and/or title ──────────────

struct FindWindowState {
    target_process_lower: String,
    target_title_lower: String,
    found_hwnd: isize,
}

unsafe extern "system" fn find_window_cb(
    hwnd: windows_sys::Win32::Foundation::HWND,
    lparam: isize,
) -> i32 {
    let state = &mut *(lparam as *mut FindWindowState);

    if IsWindowVisible(hwnd) == 0 {
        return 1;
    }

    // Title check
    if !state.target_title_lower.is_empty() {
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if len <= 0 {
            return 1;
        }
        let title = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
        if !title.contains(&state.target_title_lower) {
            return 1;
        }
    }

    // Process name check
    if !state.target_process_lower.is_empty() {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 {
            return 1;
        }
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return 1;
        }
        let mut buf = [0u16; 260];
        let mut size: u32 = 260;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandleWin(handle);
        if ok == 0 || size == 0 {
            return 1;
        }
        let full_path = String::from_utf16_lossy(&buf[..size as usize]);
        let basename = std::path::Path::new(&full_path)
            .file_name()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if basename != state.target_process_lower
            && basename.trim_end_matches(".exe") != state.target_process_lower.trim_end_matches(".exe")
        {
            return 1;
        }
    }

    // All criteria matched
    state.found_hwnd = hwnd as isize;
    0 // stop enumeration
}

fn find_window_by_criteria(process_name: &str, title: &str) -> Option<isize> {
    let mut state = FindWindowState {
        target_process_lower: process_name.to_lowercase(),
        target_title_lower: title.to_lowercase(),
        found_hwnd: 0,
    };
    unsafe {
        EnumWindows(
            Some(find_window_cb),
            &mut state as *mut FindWindowState as isize,
        );
    }
    if state.found_hwnd != 0 {
        Some(state.found_hwnd)
    } else {
        None
    }
}

// ── Mouse click simulation ─────────────────────────────────────────────────

/// Returns true if the value is a mouse button name (LButton, RButton, MButton).
fn is_mouse_button(name: &str) -> bool {
    matches!(name, "LButton" | "RButton" | "MButton")
}

/// Send a single mouse event (down or up) at the current cursor position.
fn send_mouse_event(button: &str, is_up: bool) {
    let flag = match (button, is_up) {
        ("LButton", false) => MOUSEEVENTF_LEFTDOWN,
        ("LButton", true) => MOUSEEVENTF_LEFTUP,
        ("RButton", false) => MOUSEEVENTF_RIGHTDOWN,
        ("RButton", true) => MOUSEEVENTF_RIGHTUP,
        ("MButton", false) => MOUSEEVENTF_MIDDLEDOWN,
        ("MButton", true) => MOUSEEVENTF_MIDDLEUP,
        _ => return,
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Send a mouse click (down + up) at the current cursor position.
/// `button` must be "LButton", "RButton", or "MButton".
fn send_mouse_click(button: &str) {
    let (down_flag, up_flag) = match button {
        "LButton" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        "RButton" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        "MButton" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        _ => {
            warn!("[Trigr] Unknown mouse button: {}", button);
            return;
        }
    };

    crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);

    let input_down = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: down_flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let input_up = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: up_flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    unsafe {
        SendInput(1, &input_down, std::mem::size_of::<INPUT>() as i32);
        SendInput(1, &input_up, std::mem::size_of::<INPUT>() as i32);
    }

    crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
    info!("[Trigr] Mouse click: {}", button);
}

// ── Macro sequence step executor ────────────────────────────────────────────

/// Returns true to continue the macro, false to abort it (caller breaks out of
/// the steps loop). Most arms continue unconditionally; only Wait for Window
/// can request abort, when its target window doesn't appear before the 30s
/// timeout — letting subsequent steps fire into whatever happens to be focused
/// is worse than just stopping the macro there.
fn execute_macro_step(step: &Value, target_hwnd: &mut isize, method: &str, app: &tauri::AppHandle) -> bool {
    let step_type = step.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let step_value = step.get("value").and_then(|v| v.as_str()).unwrap_or("");
    let repeat_count = step.get("repeat").and_then(|v| v.as_u64()).unwrap_or(1).max(1).min(99) as u32;
    let (_, settle_ms, _, _) = speed_delays();

    match step_type {
        "Type Text" | "Dynamic Text" => {
            if !step_value.is_empty() {
                if settle_ms > 0 { thread::sleep(Duration::from_millis(settle_ms)); }
                let resolved = resolve_type_text_tokens(step_value);
                output_text(&resolved, method, *target_hwnd);
            }
        }

        "Click Mouse" => {
            if !step_value.is_empty() && is_mouse_button(step_value) {
                for i in 0..repeat_count {
                    send_mouse_click(step_value);
                    if i + 1 < repeat_count && settle_ms > 0 {
                        thread::sleep(Duration::from_millis(settle_ms));
                    }
                }
            }
        }

        "Press Key" => {
            if !step_value.is_empty() {
                // Legacy: mouse click buttons stored under Press Key — still supported
                if is_mouse_button(step_value) {
                    for i in 0..repeat_count {
                        send_mouse_click(step_value);
                        if i + 1 < repeat_count && settle_ms > 0 {
                            thread::sleep(Duration::from_millis(settle_ms));
                        }
                    }
                    return true;
                }
                // Parse "Ctrl+Shift+N" style strings
                let parts: Vec<&str> = step_value.split('+').map(|s| s.trim()).collect();
                if let Some((&key_name, mod_parts)) = parts.split_last() {
                    let target_vk = match display_name_to_vk(key_name) {
                        Some(vk) => vk,
                        None => {
                            warn!("[Trigr] Unknown macro step key: {}", key_name);
                            return true;
                        }
                    };

                    let mod_vks: Vec<u16> = mod_parts
                        .iter()
                        .filter_map(|m| match m.to_lowercase().as_str() {
                            "ctrl" => Some(VK_LCONTROL),
                            "alt" => Some(VK_LALT),
                            "shift" => Some(VK_LSHIFT),
                            "win" => Some(VK_LWIN),
                            _ => None,
                        })
                        .collect();

                    for i in 0..repeat_count {
                        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        for &vk in &mod_vks {
                            send_vk_key(vk, false);
                        }
                        send_vk_key(target_vk, false);
                        send_vk_key(target_vk, true);
                        for &vk in mod_vks.iter().rev() {
                            send_vk_key(vk, true);
                        }
                        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                        if i + 1 < repeat_count && settle_ms > 0 {
                            thread::sleep(Duration::from_millis(settle_ms));
                        }
                    }
                }
            }
        }

        "Wait (ms)" => {
            let ms: u64 = step_value.parse().unwrap_or(500).min(30000);
            thread::sleep(Duration::from_millis(ms));
        }

        // Ctrl+C / Ctrl+V / Ctrl+A as first-class macro steps. Implemented as
        // a synthetic LCTRL + letter pulse — same path Press Key takes for the
        // equivalent chord, but exposed with a clearer label in the editor.
        // Doesn't touch Trigr's own clipboard write path; the OS handles paste
        // semantics for whatever was last copied (per feedback_paste_architecture
        // memory — that rule is about Trigr-injected content, not raw Ctrl+V).
        "Copy to Clipboard" | "Paste Clipboard" | "Select All" => {
            const VK_C: u16 = 0x43;
            const VK_V: u16 = 0x56;
            const VK_A: u16 = 0x41;
            let target_vk = match step_type {
                "Copy to Clipboard" => VK_C,
                "Paste Clipboard"   => VK_V,
                _                   => VK_A,
            };
            for i in 0..repeat_count {
                crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                send_vk_key(VK_LCONTROL, false);
                send_vk_key(target_vk, false);
                send_vk_key(target_vk, true);
                send_vk_key(VK_LCONTROL, true);
                crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                if i + 1 < repeat_count && settle_ms > 0 {
                    thread::sleep(Duration::from_millis(settle_ms));
                }
            }
            // Copy/Paste need a brief settle so the system clipboard state
            // stabilises before the next macro step runs (the next step often
            // reads or modifies the same clipboard).
            if matches!(step_type, "Copy to Clipboard" | "Paste Clipboard") {
                thread::sleep(Duration::from_millis(50));
            }
        }

        // Passive wait until a window matching the given criteria is foreground.
        // Used for macros that need the user to switch to a particular window
        // (a specific app, a particular browser tab, a specific document) before
        // continuing. Match semantics: process basename must match if set, AND
        // title substring must match if set. At least one of process/title is
        // required. Returns when match found or timeout expires.
        "Wait for Window" => {
            if step_value.is_empty() {
                warn!("[Trigr] Wait for Window step: empty value");
                return true;
            }
            let parsed: Value = match serde_json::from_str(step_value) {
                Ok(v) => v,
                Err(e) => {
                    warn!("[Trigr] Wait for Window step: invalid JSON: {}", e);
                    return true;
                }
            };
            let process_raw = parsed.get("process").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
            let target_proc = process_raw.trim_end_matches(".exe").to_string();
            let target_title = parsed.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
            if target_proc.is_empty() && target_title.is_empty() {
                warn!("[Trigr] Wait for Window step: both process and title are empty");
                return true;
            }
            // 30s hardcoded — kept off the UI per design. If a typo or stale
            // criterion never matches, the macro continues to the next step
            // instead of hanging Trigr indefinitely.
            const WAIT_FOR_WINDOW_TIMEOUT_MS: u64 = 30_000;
            let timeout_ms = WAIT_FOR_WINDOW_TIMEOUT_MS;

            let start = std::time::Instant::now();
            let poll_interval = Duration::from_millis(150);
            loop {
                let hwnd = unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
                };
                if hwnd != 0 {
                    let proc_ok = if target_proc.is_empty() {
                        true
                    } else {
                        crate::foreground::proc_name_for_hwnd(hwnd)
                            .map(|n| n.trim_end_matches(".exe").eq_ignore_ascii_case(&target_proc))
                            .unwrap_or(false)
                    };
                    let title_ok = if target_title.is_empty() {
                        true
                    } else {
                        // GetWindowTextW into a stack buffer, decode, lowercase,
                        // substring-match. Mirrors the title check inside the
                        // EnumWindows callback used by find_window_by_criteria.
                        let mut buf = [0u16; 512];
                        let len = unsafe { GetWindowTextW(hwnd as _, buf.as_mut_ptr(), buf.len() as i32) };
                        if len <= 0 {
                            false
                        } else {
                            let title = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
                            title.contains(&target_title)
                        }
                    };
                    if proc_ok && title_ok {
                        *target_hwnd = hwnd;
                        info!(
                            "[Trigr] Wait for Window: matched (process='{}' title~='{}') after {:?}",
                            target_proc, target_title, start.elapsed()
                        );
                        break;
                    }
                }
                if start.elapsed() >= Duration::from_millis(timeout_ms) {
                    warn!(
                        "[Trigr] Wait for Window: timeout ({} ms) waiting for process='{}' title~='{}' — aborting macro",
                        timeout_ms, target_proc, target_title
                    );
                    return false;
                }
                thread::sleep(poll_interval);
            }
        }

        "Open URL" => {
            let normalised = normalise_url(step_value);
            if !normalised.is_empty() {
                let _ = opener::open(&normalised);
            }
        }

        "Open Folder" => {
            if !step_value.is_empty() {
                let _ = opener::open(step_value);
            }
        }

        "Open App" => {
            if step_value.is_empty() {
                warn!("[Trigr] Open App step: empty value");
                return true;
            }
            let parsed: Value = match serde_json::from_str(step_value) {
                Ok(v) => v,
                Err(e) => {
                    warn!("[Trigr] Open App step: invalid JSON: {}", e);
                    return true;
                }
            };
            let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("path");
            let app_id = parsed.get("appId").and_then(|v| v.as_str()).unwrap_or("");
            let path = parsed.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let args = parsed.get("args").and_then(|v| v.as_str()).unwrap_or("");
            shell_launch_app(kind, path, app_id, args);
        }

        "Focus Window" => {
            if step_value.is_empty() {
                warn!("[Trigr] Focus Window step: empty value");
                return true;
            }
            let parsed: Value = match serde_json::from_str(step_value) {
                Ok(v) => v,
                Err(e) => {
                    warn!("[Trigr] Focus Window step: invalid JSON: {}", e);
                    return true;
                }
            };
            let process = parsed.get("process").and_then(|v| v.as_str()).unwrap_or("");
            let title = parsed.get("title").and_then(|v| v.as_str()).unwrap_or("");
            if process.is_empty() && title.is_empty() {
                warn!("[Trigr] Focus Window step: both process and title are empty");
                return true;
            }
            match find_window_by_criteria(process, title) {
                Some(hwnd) => {
                    let (_, _, fg_settle_ms, _) = speed_delays();
                    unsafe { SetForegroundWindow(hwnd as _); }
                    // Focus Window needs longer settle than normal foreground restore
                    thread::sleep(Duration::from_millis(fg_settle_ms.max(10) * 2));
                    *target_hwnd = hwnd;
                    info!("[Trigr] Focus Window: found and focused HWND {} (process='{}' title='{}')", hwnd, process, title);
                }
                None => {
                    warn!("[Trigr] Focus Window: no matching window found for process='{}' title='{}'", process, title);
                }
            }
        }

        "Wait for Input" => {
            wait_for_input(step_value);
        }

        "Run AHK Script" => {
            if !step_value.is_empty() {
                if let Ok(parsed) = serde_json::from_str::<Value>(step_value) {
                    let script = parsed.get("script").and_then(|v| v.as_str()).unwrap_or("");
                    let version = parsed.get("ahkVersion").and_then(|v| v.as_str()).unwrap_or("v1");
                    if !script.is_empty() {
                        // Macro step variant: wait for completion (synchronous)
                        execute_ahk_script_sync(script, version, app);
                    }
                } else {
                    warn!("[Trigr] Run AHK Script step: invalid JSON");
                }
            }
        }

        "Click at Position" => {
            if !step_value.is_empty() {
                if let Ok(parsed) = serde_json::from_str::<Value>(step_value) {
                    let x = parsed.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let y = parsed.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let button = parsed.get("button").and_then(|v| v.as_str()).unwrap_or("left");
                    let mode = parsed.get("mode").and_then(|v| v.as_str()).unwrap_or("absolute");

                    let (abs_x, abs_y) = if mode == "relative" {
                        // Relative to target window
                        let mut rect = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
                        unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(*target_hwnd as _, &mut rect);
                        }
                        (rect.left + x, rect.top + y)
                    } else {
                        (x, y)
                    };

                    info!("[Trigr] Click at Position: ({}, {}) mode={} button={}", abs_x, abs_y, mode, button);

                    // Save original cursor position
                    let mut original_pos = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut original_pos);
                    }

                    // Move cursor to target
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(abs_x, abs_y);
                    }
                    thread::sleep(Duration::from_millis(20));

                    // Map button name to SendInput format
                    let click_button = match button {
                        "right" => "RButton",
                        "middle" => "MButton",
                        _ => "LButton",
                    };

                    // Click
                    crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                    send_mouse_click(click_button);
                    crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);

                    // Restore cursor to original position
                    thread::sleep(Duration::from_millis(20));
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(original_pos.x, original_pos.y);
                    }
                } else {
                    warn!("[Trigr] Click at Position: invalid JSON");
                }
            }
        }

        // Fire an existing hotkey assignment by its storage key. Looks up the
        // assignment in engine_state and recursively invokes execute_action with
        // the macro's current target HWND. Depth guard in execute_action prevents
        // infinite recursion if a trigger fires itself (directly or via a chain).
        // Missing target = silent no-op + warn (the user may have deleted/renamed
        // the referenced trigger; the macro continues to the next step).
        "Fire Trigger" => {
            if step_value.is_empty() {
                warn!("[Trigr] Fire Trigger: empty step value, skipping");
                return true;
            }
            let lookup = {
                let state = crate::hotkeys::engine_state().lock().unwrap();
                state.assignments.get(step_value).cloned()
            };
            match lookup {
                Some(target_macro) => {
                    info!("[Trigr] Fire Trigger: invoking \"{}\"", step_value);
                    if settle_ms > 0 { thread::sleep(Duration::from_millis(settle_ms)); }
                    // is_bare=false, is_altgr=false — we're firing programmatically,
                    // not from a real keypress, so no dead character to erase and no
                    // bare-key handling needed. trigger_key=None for the same reason.
                    execute_action(&target_macro, false, *target_hwnd, false, None, app);
                }
                None => {
                    warn!("[Trigr] Fire Trigger: assignment \"{}\" not found, skipping", step_value);
                }
            }
        }

        // Fire an existing text expansion by trigger word. Routes through the
        // shared dispatch in expansions.rs which honours variants / fill-in /
        // image / tokens / case patterns — same fire paths the space-trigger
        // and immediate-trigger entry points use, so parity is automatic.
        "Fire Text Expansion" => {
            if step_value.is_empty() {
                warn!("[Trigr] Fire Text Expansion: empty step value, skipping");
                return true;
            }
            if settle_ms > 0 { thread::sleep(Duration::from_millis(settle_ms)); }
            crate::expansions::fire_expansion_by_trigger(step_value);
        }

        _ => {
            warn!("[Trigr] Unknown macro step type: {}", step_type);
        }
    }
    true
}

// ── Wait for Input step ─────────────────────────────────────────────────────

fn wait_for_input(config_json: &str) {
    use crate::hotkeys::{self, WaitEvent};
    use std::sync::mpsc::RecvTimeoutError;

    // Parse config from JSON stored in step.value
    let config: serde_json::Value = serde_json::from_str(config_json).unwrap_or_default();
    let input_type = config.get("inputType").and_then(|v| v.as_str()).unwrap_or("LButton");
    let trigger = config.get("trigger").and_then(|v| v.as_str()).unwrap_or("press");
    let specific_key = config.get("specificKey").and_then(|v| v.as_str()).unwrap_or("");

    // Extract just the key name from "Ctrl+Enter" style strings
    let wanted_key = specific_key.split('+').last().unwrap_or("").to_string();

    let is_mouse = matches!(input_type, "LButton" | "RButton" | "MButton");
    let mouse_name = match input_type {
        "LButton" => "MOUSE_LEFT",
        "RButton" => "MOUSE_RIGHT",
        "MButton" => "MOUSE_MIDDLE",
        _ => "",
    };

    log::info!(
        "[WAIT] Wait for Input: type={} trigger={} key={}",
        input_type, trigger, wanted_key
    );

    const TIMEOUT: Duration = Duration::from_secs(30);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    // Register the waiter channel
    let rx = hotkeys::register_wait_for_input();

    // Two-phase state for pressRelease trigger (per-waiter, not global)
    let mut phase = "down"; // "down" = waiting for press, "up" = waiting for release

    let deadline = std::time::Instant::now() + TIMEOUT;

    loop {
        // Check timeout
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            log::warn!("[WAIT] Timed out after 30s");
            break;
        }

        // Check if macros were disabled
        if !hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
            log::info!("[WAIT] Cancelled — macros disabled");
            break;
        }

        // Wait for next event with short timeout for polling cancellation
        let timeout = remaining.min(POLL_INTERVAL);
        match rx.recv_timeout(timeout) {
            Ok(event) => {
                let matched = match (&event, is_mouse) {
                    // Mouse events
                    (WaitEvent::MouseDown { button_name }, true) => {
                        button_name == mouse_name && matches!(trigger, "press" | "pressRelease")
                    }
                    (WaitEvent::MouseUp { button_name }, true) => {
                        button_name == mouse_name && matches!(trigger, "release" | "pressRelease")
                    }
                    // Keyboard events
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

                // Handle pressRelease two-phase state machine
                if trigger == "pressRelease" {
                    let is_down = matches!(event, WaitEvent::KeyDown { .. } | WaitEvent::MouseDown { .. });
                    if phase == "down" && is_down {
                        phase = "up"; // Got the press, now wait for release
                        log::debug!("[WAIT] pressRelease phase 1: press detected, waiting for release");
                        continue;
                    } else if phase == "up" && !is_down {
                        log::debug!("[WAIT] pressRelease phase 2: release detected, done");
                        break; // Got the release, done
                    }
                    continue; // Not the right phase
                }

                // Simple press or release trigger
                log::debug!("[WAIT] Input detected: {:?}", event);
                break;
            }
            Err(RecvTimeoutError::Timeout) => continue, // Poll loop
            Err(RecvTimeoutError::Disconnected) => {
                log::warn!("[WAIT] Channel disconnected");
                break;
            }
        }
    }

    // Always clear the waiter on exit
    hotkeys::clear_wait_for_input();
    log::info!("[WAIT] Wait for Input complete");
}

// ── Low-level VK key simulation ─────────────────────────────────────────────

fn send_vk_key(vk: u16, key_up: bool) {
    let flags = if key_up { KEYEVENTF_KEYUP } else { 0 };

    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk as VIRTUAL_KEY,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };

    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_vk_key_checked(vk: u16, key_up: bool) -> u32 {
    let flags = if key_up { KEYEVENTF_KEYUP } else { 0 };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk as VIRTUAL_KEY,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32) }
}

fn send_vk_tap(vk: u16) {
    send_vk_key(vk, false);
    send_vk_key(vk, true);
}

// ── Release/restore modifiers ───────────────────────────────────────────────

/// All modifier VK codes we track (left + right variants).
const ALL_MODIFIER_VKS: &[(u16, &str)] = &[
    (0xA2, "LCtrl"),
    (0xA3, "RCtrl"),
    (0xA0, "LShift"),
    (0xA1, "RShift"),
    (0xA4, "LAlt"),
    (0xA5, "RAlt"),
    (0x5B, "LWin"),
    (0x5C, "RWin"),
];

/// Check if a key is physically held using GetAsyncKeyState.
fn is_key_down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) < 0 }
}

/// Read which modifiers are physically held, release them via SendInput,
/// and return the list of VKs that were held (for later re-press).
///
/// The old code had a fallback that released all 8 modifier VKs when
/// GetAsyncKeyState detected none — causing spurious key-ups in the target app.
/// Removed: trust GetAsyncKeyState. If it says nothing is held, nothing is sent.
pub fn release_held_modifiers() -> Vec<u16> {
    let mut held = Vec::new();
    for &(vk, _name) in ALL_MODIFIER_VKS {
        if unsafe { GetAsyncKeyState(vk as i32) } < 0 {
            held.push(vk);
            send_vk_key(vk, true);
        }
    }
    held
}

/// Re-press modifiers that were held before injection.
pub fn restore_modifiers(held: &[u16]) {
    for &vk in held {
        send_vk_key(vk, false);
    }
}

/// Release ALL modifier keys unconditionally (legacy — used in preamble).
fn release_all_modifiers() {
    for &(vk, _) in ALL_MODIFIER_VKS {
        send_vk_key(vk, true);
    }
}

// ── Display name → VK code mapping ─────────────────────────────────────────
// Maps the display names used in the UI / macro.data.key to Windows VK codes.

fn display_name_to_vk(name: &str) -> Option<u16> {
    match name.to_uppercase().as_str() {
        // Letters
        "A" => Some(0x41),
        "B" => Some(0x42),
        "C" => Some(0x43),
        "D" => Some(0x44),
        "E" => Some(0x45),
        "F" => Some(0x46),
        "G" => Some(0x47),
        "H" => Some(0x48),
        "I" => Some(0x49),
        "J" => Some(0x4A),
        "K" => Some(0x4B),
        "L" => Some(0x4C),
        "M" => Some(0x4D),
        "N" => Some(0x4E),
        "O" => Some(0x4F),
        "P" => Some(0x50),
        "Q" => Some(0x51),
        "R" => Some(0x52),
        "S" => Some(0x53),
        "T" => Some(0x54),
        "U" => Some(0x55),
        "V" => Some(0x56),
        "W" => Some(0x57),
        "X" => Some(0x58),
        "Y" => Some(0x59),
        "Z" => Some(0x5A),
        // Digits
        "0" => Some(0x30),
        "1" => Some(0x31),
        "2" => Some(0x32),
        "3" => Some(0x33),
        "4" => Some(0x34),
        "5" => Some(0x35),
        "6" => Some(0x36),
        "7" => Some(0x37),
        "8" => Some(0x38),
        "9" => Some(0x39),
        // Function keys
        "F1" => Some(0x70),
        "F2" => Some(0x71),
        "F3" => Some(0x72),
        "F4" => Some(0x73),
        "F5" => Some(0x74),
        "F6" => Some(0x75),
        "F7" => Some(0x76),
        "F8" => Some(0x77),
        "F9" => Some(0x78),
        "F10" => Some(0x79),
        "F11" => Some(0x7A),
        "F12" => Some(0x7B),
        // Navigation
        "UP" | "ARROWUP" => Some(0x26),
        "DOWN" | "ARROWDOWN" => Some(0x28),
        "LEFT" | "ARROWLEFT" => Some(0x25),
        "RIGHT" | "ARROWRIGHT" => Some(0x27),
        "HOME" => Some(0x24),
        "END" => Some(0x23),
        "PAGEUP" => Some(0x21),
        "PAGEDOWN" => Some(0x22),
        "INSERT" => Some(0x2D),
        "DELETE" => Some(0x2E),
        // Special
        "SPACE" => Some(0x20),
        "TAB" => Some(0x09),
        "ENTER" | "RETURN" => Some(0x0D),
        "ESCAPE" | "ESC" => Some(0x1B),
        "BACKSPACE" => Some(0x08),
        "CAPSLOCK" => Some(0x14),
        "NUMLOCK" => Some(0x90),
        "SCROLLLOCK" => Some(0x91),
        "PRINTSCREEN" => Some(0x2C),
        "PAUSE" => Some(0x13),
        // Symbols
        "MINUS" | "-" => Some(0xBD),
        "EQUAL" | "=" => Some(0xBB),
        "BRACKETLEFT" | "[" => Some(0xDB),
        "BRACKETRIGHT" | "]" => Some(0xDD),
        "SEMICOLON" | ";" => Some(0xBA),
        "QUOTE" | "'" => Some(0xDE),
        "BACKQUOTE" | "`" => Some(0xC0),
        "BACKSLASH" | "\\" => Some(0xDC),
        "COMMA" | "," => Some(0xBC),
        "PERIOD" | "." => Some(0xBE),
        "SLASH" | "/" => Some(0xBF),
        // Numpad
        "NUMPAD0" => Some(0x60),
        "NUMPAD1" => Some(0x61),
        "NUMPAD2" => Some(0x62),
        "NUMPAD3" => Some(0x63),
        "NUMPAD4" => Some(0x64),
        "NUMPAD5" => Some(0x65),
        "NUMPAD6" => Some(0x66),
        "NUMPAD7" => Some(0x67),
        "NUMPAD8" => Some(0x68),
        "NUMPAD9" => Some(0x69),
        "NUMPADDECIMAL" => Some(0x6E),
        "NUMPADMULTIPLY" => Some(0x6A),
        "NUMPADADD" => Some(0x6B),
        "NUMPADSUBTRACT" => Some(0x6D),
        "NUMPADDIVIDE" => Some(0x6F),
        // Bare modifier keys (for Send Hotkey targeting a modifier itself)
        "CTRL" | "CONTROL" => Some(VK_LCONTROL),
        "ALT" => Some(VK_LALT),
        "SHIFT" => Some(VK_LSHIFT),
        "WIN" | "META" => Some(VK_LWIN),
        _ => None,
    }
}

// ── AHK Script Runner ──────────────────────────────────────────────────────

/// Resolve the path to the bundled AutoHotkey executable.
/// `ahk_version`: "v1" (default) or "v2"
fn resolve_ahk_exe(app: &tauri::AppHandle, ahk_version: &str) -> Option<PathBuf> {
    let filename = if ahk_version == "v2" { "AutoHotkey64.exe" } else { "AutoHotkeyU32.exe" };
    // Production: bundled resource
    if let Ok(resource_dir) = app.path().resource_dir() {
        let path = resource_dir.join("ahk").join(filename);
        if path.exists() {
            return Some(path);
        }
    }
    // Dev fallback: assets directory relative to project root
    let dev_paths = [
        PathBuf::from(format!("assets/ahk/{}", filename)),
        PathBuf::from(format!("../assets/ahk/{}", filename)),
    ];
    for p in &dev_paths {
        if p.exists() {
            return Some(p.clone());
        }
    }
    warn!("[Trigr] {} not found — AHK scripts will not execute", filename);
    None
}

/// Get the temp directory for AHK scripts.
fn ahk_scripts_dir(app: &tauri::AppHandle) -> PathBuf {
    app.path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir())
        .join("ahk-scripts")
}

/// Normalise an AHK script body so users can paste scripts copied straight
/// from existing .ahk files. Trigr is the trigger, so any hotkey labels
/// (`^!j::`, `F1::`, etc.) would either swallow the body or sit waiting for
/// a key that never fires. Strips standalone label lines, keeps the body of
/// one-liner hotkeys, and drops persistence directives that conflict with
/// the one-shot run model. Scripts pasted without labels pass through unchanged.
fn normalise_ahk_script(script: &str) -> String {
    let label_only = regex_lite::Regex::new(r"^\s*[!^+#<>*~\$&\w]+::\s*$").unwrap();
    let label_with_body = regex_lite::Regex::new(r"^\s*[!^+#<>*~\$&\w]+::(.+)$").unwrap();
    script
        .lines()
        .filter_map(|line| {
            if label_only.is_match(line) {
                return None;
            }
            if let Some(caps) = label_with_body.captures(line) {
                return Some(caps.get(1).unwrap().as_str().to_string());
            }
            let lc = line.trim().to_lowercase();
            if lc.starts_with("#persistent") || lc.starts_with("#singleinstance") {
                return None;
            }
            Some(line.to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Execute an AHK script (fire-and-forget for standalone actions).
/// If trigger_key is provided, previous AHK instance for that key is killed first.
fn execute_ahk_script(script: &str, ahk_version: &str, trigger_key: Option<&str>, app: &tauri::AppHandle) {
    let ahk_path = match resolve_ahk_exe(app, ahk_version) {
        Some(p) => p,
        None => return,
    };

    // Kill previous instance for this trigger key (re-trigger)
    if let Some(key) = trigger_key {
        kill_ahk_for_key(key);
    }

    // Write temp .ahk file
    let script_dir = ahk_scripts_dir(app);
    let _ = std::fs::create_dir_all(&script_dir);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let script_path = script_dir.join(format!("trigr-ahk-{}.ahk", timestamp));

    let script = normalise_ahk_script(script);

    // UTF-8 BOM for AHK v1 Unicode support
    let mut content = vec![0xEF, 0xBB, 0xBF];
    content.extend_from_slice(script.as_bytes());
    // Append ExitApp if not present (ensures one-shot scripts exit cleanly)
    let lower = script.to_lowercase();
    if !lower.contains("exitapp") {
        content.extend_from_slice(b"\nExitApp\n");
    }

    if let Err(e) = std::fs::write(&script_path, &content) {
        warn!("[Trigr] AHK: failed to write temp script: {}", e);
        return;
    }

    // Spawn AHK process
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    let child = std::process::Command::new(&ahk_path)
        .arg(&script_path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();

    match child {
        Ok(child) => {
            info!("[Trigr] AHK: spawned process (pid: {})", child.id());
            if let Some(key) = trigger_key {
                let key_str = key.to_string();
                let path_clone = script_path.clone();
                let pid = child.id();
                AHK_PROCESSES.lock().unwrap().insert(
                    key_str.clone(),
                    AhkProcess {
                        child,
                        script_path: script_path,
                    },
                );
                // Spawn cleanup thread: wait for process exit, then clean up
                let key_for_cleanup = key_str;
                thread::spawn(move || {
                    // Wait up to 5 minutes for the process to finish
                    thread::sleep(Duration::from_secs(300));
                    let mut procs = AHK_PROCESSES.lock().unwrap();
                    if let Some(mut entry) = procs.remove(&key_for_cleanup) {
                        let _ = entry.child.kill();
                        let _ = entry.child.wait();
                        let _ = std::fs::remove_file(&path_clone);
                        info!("[Trigr] AHK: cleaned up stale process (pid: {})", pid);
                    }
                });
            } else {
                // No trigger key (called from macro step context as fire-and-forget fallback)
                let path_clone = script_path;
                thread::spawn(move || {
                    // Orphan cleanup after 5 minutes
                    thread::sleep(Duration::from_secs(300));
                    let _ = std::fs::remove_file(&path_clone);
                });
            }
        }
        Err(e) => {
            warn!("[Trigr] AHK: failed to spawn process: {}", e);
            let _ = std::fs::remove_file(&script_path);
        }
    }
}

/// Execute an AHK script synchronously (for macro steps — waits for completion).
fn execute_ahk_script_sync(script: &str, ahk_version: &str, app: &tauri::AppHandle) {
    let ahk_path = match resolve_ahk_exe(app, ahk_version) {
        Some(p) => p,
        None => return,
    };

    let script_dir = ahk_scripts_dir(app);
    let _ = std::fs::create_dir_all(&script_dir);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let script_path = script_dir.join(format!("trigr-ahk-{}.ahk", timestamp));

    let script = normalise_ahk_script(script);

    let mut content = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM
    content.extend_from_slice(script.as_bytes());
    let lower = script.to_lowercase();
    if !lower.contains("exitapp") {
        content.extend_from_slice(b"\nExitApp\n");
    }

    if let Err(e) = std::fs::write(&script_path, &content) {
        warn!("[Trigr] AHK sync: failed to write temp script: {}", e);
        return;
    }

    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;

    match std::process::Command::new(&ahk_path)
        .arg(&script_path)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
    {
        Ok(mut child) => {
            info!("[Trigr] AHK sync: waiting for process (pid: {})", child.id());
            // Wait for process to finish (up to 60s for macro step context)
            match child.wait() {
                Ok(status) => {
                    info!("[Trigr] AHK sync: process exited with {}", status);
                }
                Err(e) => {
                    warn!("[Trigr] AHK sync: wait failed: {}", e);
                    let _ = child.kill();
                }
            }
            let _ = std::fs::remove_file(&script_path);
        }
        Err(e) => {
            warn!("[Trigr] AHK sync: failed to spawn: {}", e);
            let _ = std::fs::remove_file(&script_path);
        }
    }
}

/// Kill the AHK process associated with a trigger key (re-trigger support).
fn kill_ahk_for_key(key: &str) {
    let mut procs = AHK_PROCESSES.lock().unwrap();
    if let Some(mut entry) = procs.remove(key) {
        let _ = entry.child.kill();
        let _ = entry.child.wait();
        let _ = std::fs::remove_file(&entry.script_path);
        info!("[Trigr] AHK: killed previous instance for key: {}", key);
    }
}

/// Kill all running AHK processes and clean up temp files. Called on app quit.
pub fn kill_all_ahk_processes() {
    let mut procs = AHK_PROCESSES.lock().unwrap();
    let count = procs.len();
    for (key, mut entry) in procs.drain() {
        let _ = entry.child.kill();
        let _ = entry.child.wait();
        let _ = std::fs::remove_file(&entry.script_path);
        info!("[Trigr] AHK: killed process for key: {}", key);
    }
    if count > 0 {
        info!("[Trigr] AHK: cleaned up {} process(es) on quit", count);
    }
}

/// Delete leftover AHK temp script files from previous sessions. Called on startup.
pub fn cleanup_stale_ahk_scripts(app_data_dir: PathBuf) {
    let dir = app_data_dir.join("ahk-scripts");
    if !dir.exists() {
        return;
    }
    let mut cleaned = 0;
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "ahk").unwrap_or(false) {
                let _ = std::fs::remove_file(&path);
                cleaned += 1;
            }
        }
    }
    if cleaned > 0 {
        info!("[Trigr] AHK: cleaned up {} stale script(s) from previous session", cleaned);
    }
}

// ── Public wrappers for overlay use ─────────────────────────────────────────

pub fn read_clipboard_pub() -> Option<String> {
    read_clipboard()
}

pub fn write_clipboard_pub(text: &str) -> bool {
    write_clipboard(text)
}

pub fn write_clipboard_recordable_pub(text: &str) -> bool {
    write_clipboard_recordable(text)
}

pub fn send_vk_key_pub(vk: u16, key_up: bool) {
    send_vk_key(vk, key_up);
}
