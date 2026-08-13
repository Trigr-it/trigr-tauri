use log::{info, warn};
use serde_json::Value;
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tauri::{Emitter, Manager};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
};
use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE,
    KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE,
    MOUSEINPUT, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP, VIRTUAL_KEY,
};
use windows_sys::Win32::Foundation::CloseHandle as CloseHandleWin;
use windows_sys::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetForegroundWindow, GetSystemMetrics, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, MessageBoxW, SetForegroundWindow, SetWindowPos,
    ShowWindow, IDNO, IDOK, IDYES, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MB_OKCANCEL,
    MB_SETFOREGROUND, MB_TOPMOST, MB_YESNOCANCEL,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOZORDER, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
};
use windows_sys::Win32::System::Shutdown::{
    ExitWindowsEx, LockWorkStation, EWX_FORCEIFHUNG, EWX_LOGOFF, EWX_SHUTDOWN,
    SHTDN_REASON_FLAG_USER_DEFINED, SHTDN_REASON_MAJOR_OTHER, SHTDN_REASON_MINOR_OTHER,
};

/// Future clipboard manager checks this flag and skips logging if set.
pub static SUPPRESS_NEXT_CLIPBOARD_WRITE: AtomicBool = AtomicBool::new(false);

// ── Self-write clipboard suppression (robust to async WM_CLIPBOARDUPDATE) ────
// SUPPRESS_NEXT_CLIPBOARD_WRITE is a level flag — it's cleared synchronously
// after a write/restore, but Windows delivers WM_CLIPBOARDUPDATE asynchronously,
// so the listener can process the event AFTER the flag is cleared and record
// Keyfire's own injected text into history (the H3 leak). To fix this precisely,
// every internal write records the resulting clipboard sequence number here; the
// listener skips any update whose seqnum we produced. This is exact: a real user
// copy (or a `Copy to Clipboard` macro step, which the target app performs) gets
// a seqnum we never recorded, so it is always still captured.
static SELF_CLIPBOARD_SEQNUMS: LazyLock<Mutex<VecDeque<u32>>> =
    LazyLock::new(|| Mutex::new(VecDeque::new()));

/// Record the current clipboard sequence number as a Keyfire-internal write so the
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

/// True if `seq` was produced by a Keyfire-internal write. Consumes the match so a
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
// leave SUPPRESS_SIMULATED stuck true, silently disabling all Keyfire hotkeys.

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

// ── Paste-op re-entrancy guard ─────────────────────────────────────────────
// `paste_clipboard_item`, `paste_text`, and `copy_clipboard_item` each spawn a
// fresh thread per call and do a read-prev / write-text / sleep / restore-prev
// dance against the system clipboard. Concurrent invocations (the LL hook can
// emit clipboard-overlay-key on Windows key-repeat, or a user clicks repeatedly
// while the UI is laggy) interleave their reads/writes — observed on 2026-06-05
// producing thousands of alternating-pair clipboard rows because each thread's
// `prev` snapshot captures another thread's mid-flight write.
//
// One shared AtomicBool gates all three paths: the first call acquires, every
// concurrent call drops out instantly. Released on thread exit (including
// panic) via Drop, so a stuck paste can't deadlock future ones.
pub static PASTE_OP_ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) struct PasteOpGuard;

impl PasteOpGuard {
    /// Returns Some(guard) if no other paste/copy op is running. Returns None
    /// if one is already in flight — the caller MUST return without touching
    /// the clipboard.
    pub(crate) fn try_acquire() -> Option<Self> {
        match PASTE_OP_ACTIVE.compare_exchange(
            false, true, Ordering::SeqCst, Ordering::SeqCst,
        ) {
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

// ── Macro re-entrancy guard + loop cancel state ───────────────────────────
// `ACTIVE_MACRO_KEYS` blocks the H1 re-entrancy path: a manual re-press of a
// trigger while its macro is still running spawns a second thread that races
// the first across shared clipboard/SUPPRESS state (BricsCAD Ctrl+Shift+F7
// freeze, confirmed in-wild 2026-06-11). Per-storage-key — different macros
// can still fire concurrently. Nested Fire Trigger / Fire Text Expansion calls
// inside execute_macro_step bypass this guard (they go straight to
// execute_action, not fire_macro) and are bounded instead by FIRE_DEPTH.
//
// `LOOPING_MACROS` holds the cancel flag for each currently-looping macro,
// keyed by storage_key. The hotkey re-press path (fire_macro) checks this map
// FIRST: if the trigger is already looping, we set its cancel flag and return
// without spawning a new fire. `LOOPING_COUNT` is the cheap "any loop active?"
// predicate the LL hook polls on every Esc keydown — single atomic read, no
// mutex contention. `ESC_LOOP_BREAK` is the global cancel signal set by the
// hook on Esc and reset when the last loop exits.
static ACTIVE_MACRO_KEYS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

static LOOPING_MACROS: LazyLock<Mutex<HashMap<String, Arc<AtomicBool>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub static LOOPING_COUNT: AtomicUsize = AtomicUsize::new(0);
pub static ESC_LOOP_BREAK: AtomicBool = AtomicBool::new(false);

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

// ── Unified app launcher (path or AppsFolder AUMID) ───────────────────────
//
// ── AHK Script Runner process tracking ─────────────────────────────────────

use std::collections::{HashMap, HashSet, VecDeque};
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
        info!("[Keyfire] Released held key: {}", state.label);
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
        info!("[Keyfire] Released held key on mouse-up: {}", state.label);
        Some(state.label)
    } else {
        if allow_pending {
            // Hold not stored yet — record that the button was released so the
            // hold action can release immediately when it finishes setting up.
            info!("[Keyfire] Mouse-up for {} but no held key yet — setting pending release", mouse_id);
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
        info!("[Keyfire] Stopped repeating: {}", state.label);
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
// Per-character typing delay lives in keystroke_delay_ms() — preset-resolved,
// no longer a constant (the Keystroke delay slider was dead until v0.6.11).

// Open URL launches the default browser via ShellExecute, which is async.
// Without a settle pause the next macro step targets Keyfire's HWND instead of
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
            // Pre-execution slider applies in full (clamped to the UI max) —
            // "Pause before sending any output" must mean what it says.
            let pre = state.custom_pre_execution_delay.min(500);
            // Scale foreground settle and clipboard restore proportionally to pre-execution
            let fg = if pre == 0 { 5 } else { (pre / 10).max(5) };
            let clip = if pre == 0 { 25 } else { (pre / 3).max(25) };
            (pre, pre.min(10), fg, clip)
        }
        _         => (10, 10, 10, 50), // "safe" (default)
    }
}

/// Per-character delay for direct (Type Each Key) text injection.
/// Presets: safe 10 / fast 5 / instant 0. Custom reads the Keystroke delay
/// slider (clamped to the UI max). The frontend preset cards mirror these
/// numbers — keep MACRO_SPEED_PRESETS in SettingsPanel.jsx in sync.
fn keystroke_delay_ms() -> u64 {
    let state = crate::hotkeys::engine_state().lock().unwrap();
    match state.macro_speed.as_str() {
        "fast"    => 5,
        "instant" => 0,
        "custom"  => state.custom_keystroke_delay.min(200),
        _         => 10, // "safe" (default)
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
            "[Keyfire] Fire recursion limit hit (depth {}, max {}) — aborting. A trigger or text expansion is calling itself directly or via a chain.",
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
    info!("[Keyfire] Firing: [{}] {} (depth {})", macro_type, label, depth);

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

        // Fire an existing text expansion by trigger word. Same dispatch as
        // the "Fire Text Expansion" macro step — routes through
        // expansions::fire_expansion_by_trigger which handles text / image /
        // variants / fill-in fields uniformly. The FIRE_DEPTH guard at the
        // top of this function caps chains (a key fires an expansion that
        // fires another, etc.) at MAX_FIRE_DEPTH.
        "expansion" => {
            let trigger = data
                .and_then(|d| d.get("trigger"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if trigger.is_empty() {
                warn!("[Keyfire] expansion action: empty trigger, skipping");
            } else {
                if step_settle_ms > 0 { thread::sleep(Duration::from_millis(step_settle_ms)); }
                crate::expansions::fire_expansion_by_trigger(trigger);
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
            let monitor = crate::window_target::parse_monitor_target(data, target_hwnd);
            // Single-action path — user pressed a hotkey. No follow-up step to
            // sequence with, so the completion receiver is discarded.
            let _ = crate::window_target::launch_with_monitor_target(
                crate::window_target::LaunchKind::App { kind, path, app_id, args: "" },
                monitor,
            );
        }

        "folder" => {
            if let Some(path) = data.and_then(|d| d.get("path")).and_then(|v| v.as_str()) {
                let monitor = crate::window_target::parse_monitor_target(data, target_hwnd);
                let _ = crate::window_target::launch_with_monitor_target(
                    crate::window_target::LaunchKind::Folder { path },
                    monitor,
                );
            }
        }

        "macro" => {
            if let Some(steps) = data.and_then(|d| d.get("steps")).and_then(|v| v.as_array()) {
                let method = resolve_input_method(data);
                let uses_clipboard = method != "send-input" && method != "direct";
                let mut current_hwnd = target_hwnd;
                let (_, settle_ms, _, clip_restore_ms) = speed_delays();

                // Clear any stale Esc-cancel flag so a pre-press doesn't
                // immediately abort the macro we're about to fire. The flag
                // is set globally on every real Esc keydown — once we're
                // running, any subsequent Esc press will set it again and
                // the per-step check below will catch it.
                ESC_LOOP_BREAK.store(false, Ordering::SeqCst);

                // Loop config — backward compatible: missing `loop` = single fire.
                // `count` clamped to >= 1; `forever` runs until cancelled.
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

                // Register loop handle only when looping AND we have a trigger key
                // to cancel against. Without a trigger key (e.g. nested Fire Trigger
                // chain) the re-press cancel path can't reach us — we run uncancelled.
                let loop_handle = if loop_enabled && max_iters > 1 {
                    trigger_key.map(LoopHandle::register)
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

                // For clipboard method: snapshot once, batch pastes, restore once.
                // Snapshot captures EVERY format (CF_DIB, RTF, CF_HDROP, registered
                // formats) so non-text clipboard content (e.g. an image from Snagit)
                // is preserved across the macro — text-only save would silently drop
                // the image and leak the expansion text into the Windows clipboard.
                // For LOOPED macros the same snapshot covers all iterations (the user's
                // clipboard state is preserved across the whole loop, not per-iter).
                let saved_snapshot = if uses_clipboard {
                    crate::expansions::snapshot_clipboard()
                } else {
                    Vec::new()
                };
                let mut clipboard_dirty = false;
                let mut cancelled = false;
                let mut iter_index: u64 = 0;

                'outer: while iter_index < max_iters {
                    // Per-iteration cancel checks.
                    //   1) Loop-specific flag (re-press) — only applies when looping.
                    //   2) Global ESC_LOOP_BREAK — applies to BOTH loops and
                    //      one-shots, so Esc can cancel any running macro.
                    if let Some(ref lh) = loop_handle {
                        if lh.is_cancelled() {
                            info!("[Keyfire] Macro loop cancelled at iter {}", iter_index);
                            cancelled = true;
                            break;
                        }
                    }
                    if ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                        info!("[Keyfire] Macro cancelled (Esc) at iter {}", iter_index);
                        cancelled = true;
                        break;
                    }

                    if iter_index > 0 && loop_delay_ms > 0 {
                        // Polled sleep — chunks of 100ms so Esc/re-press/pause
                        // is honoured within 100ms even on very long delays
                        // (e.g. 5-minute inter-iteration waits). A single
                        // thread::sleep here would block all cancel paths
                        // for the entire delay.
                        let sleep_chunk = std::time::Duration::from_millis(100);
                        let total = std::time::Duration::from_millis(loop_delay_ms);
                        let start = std::time::Instant::now();
                        while start.elapsed() < total {
                            if let Some(ref lh) = loop_handle {
                                if lh.is_cancelled() {
                                    info!("[Keyfire] Macro loop cancelled during inter-iter delay");
                                    cancelled = true;
                                    break 'outer;
                                }
                            }
                            if ESC_LOOP_BREAK.load(Ordering::SeqCst) {
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
                    // Inter-step cancel poll — keeps Esc/re-press response time bounded
                    // by step duration even inside long macros. Mirrors the per-iter
                    // checks above (loop-specific flag + global ESC_LOOP_BREAK).
                    if let Some(ref lh) = loop_handle {
                        if lh.is_cancelled() {
                            info!("[Keyfire] Macro loop cancelled mid-iter at step {}/{}", i + 1, steps.len());
                            cancelled = true;
                            break 'outer;
                        }
                    }
                    if ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                        info!("[Keyfire] Macro cancelled (Esc) at step {}/{}", i + 1, steps.len());
                        cancelled = true;
                        break 'outer;
                    }
                    let step_type = step.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    let step_value = step.get("value").and_then(|v| v.as_str()).unwrap_or("");
                    info!("[Keyfire]   Step {}/{}: [{}] \"{}\"", i + 1, steps.len(), step_type, step_value);

                    if matches!(step_type, "Type Text" | "Dynamic Text") && uses_clipboard && !step_value.is_empty() {
                        if clipboard_dirty {
                            // A previous step's paste may still be unread by an async
                            // target (Chromium renderer) — sync the target's queue and
                            // wait out the read before overwriting the clipboard.
                            // Too early and either (a) the earlier Ctrl+V reads THIS
                            // step's text (double-paste, earlier text lost), or (b) our
                            // write COLLIDES with the app's open-clipboard read — but
                            // (b) is now absorbed by clipboard_paste_core's write
                            // retries, so this cap only guards (a). 100ms (vs the
                            // 200ms restore cap) keeps multi-text macros feeling snappy
                            // when the read is too fast for the poll to observe —
                            // warmed-up eM Client reads finish in <3ms and would
                            // otherwise pay the full cap on every step (2026-08-05).
                            crate::expansions::settle_paste(current_hwnd, settle_ms.max(100));
                        } else if settle_ms > 0 {
                            thread::sleep(Duration::from_millis(settle_ms));
                        }
                        let resolved = resolve_type_text_tokens(step_value);
                        clipboard_paste_core(&resolved, current_hwnd);
                        clipboard_dirty = true;
                    } else {
                        // Restore the user's clipboard ONLY before steps that actually
                        // read or write it; everything else defers to the single
                        // restore after the loop. Restoring before EVERY non-text step
                        // (pre-v0.7.3) raced async paste handlers: Chromium targets
                        // (eM Client, Slack, browsers) read the clipboard tens of ms
                        // after the Ctrl+V keydown, so a 25-50ms restore could win the
                        // race and the app pasted the user's OLD clipboard content
                        // instead of the step text (beta report 2026-08-05, eM Client).
                        // No seqnum guard — the macro is a controlled sequential flow;
                        // the user isn't expected to copy something mid-macro.
                        if clipboard_dirty && matches!(step_type, "Copy to Clipboard" | "Paste Clipboard" | "Wait for Input" | "Run AHK Script") {
                            crate::expansions::settle_paste(current_hwnd, clip_restore_ms.max(crate::expansions::PASTE_RESTORE_SETTLE_MS));
                            crate::expansions::restore_clipboard_snapshot(&saved_snapshot);
                            SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
                            clipboard_dirty = false;
                        }
                        let cont = execute_macro_step(step, &mut current_hwnd, &method, app);
                        if !cont {
                            info!("[Keyfire] Macro aborted at step {}/{} ({})", i + 1, steps.len(), step_type);
                            // Abort propagates out of the outer loop too — an explicit
                            // user-cancel (e.g. Wait for Input cancel) shouldn't restart.
                            // Post-loop clipboard restore (just below) still runs.
                            cancelled = true;
                            break 'outer;
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
                    if matches!(step_type, "Wait (ms)" | "Wait for Input" | "Open App" | "Focus Window" | "Wait for Window" | "Click at Position" | "Open URL" | "Fire Trigger" | "Fire Text Expansion" | "Record Macro") {
                        if step_type == "Open URL" {
                            thread::sleep(Duration::from_millis(OPEN_URL_FOCUS_SETTLE_MS));
                        }
                        let new_hwnd = unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
                        };
                        if new_hwnd != 0 && new_hwnd != current_hwnd {
                            current_hwnd = new_hwnd;
                        } else if step_type == "Open URL" {
                            warn!("[Keyfire] Open URL: foreground HWND unchanged after {}ms settle. Subsequent steps will target the pre-launch window. Add a Wait step if the browser is slow to focus.", OPEN_URL_FOCUS_SETTLE_MS);
                        }
                    }
                }

                    iter_index += 1;
                }

                // Final restore after all iterations. Queue-sync + settle floor so
                // the last paste is consumed before the restore lands (async-read
                // race — see settle_paste). Seqnum guard: if the user copied
                // something during the final paste window, leave their content.
                if clipboard_dirty {
                    let post_seq = crate::expansions::clipboard_sequence_number();
                    crate::expansions::settle_paste(current_hwnd, clip_restore_ms.max(crate::expansions::PASTE_RESTORE_SETTLE_MS));
                    if crate::expansions::clipboard_sequence_number() == post_seq {
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
                    info!(
                        "[Keyfire] Macro loop ended: {} iter(s), cancelled={}",
                        iter_index, cancelled
                    );
                }
                // loop_handle dropped at end of scope — removes from LOOPING_MACROS map,
                // decrements LOOPING_COUNT, resets ESC_LOOP_BREAK if this was the last
                // active loop.
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
            warn!("[Keyfire] Unknown macro type: {}", macro_type);
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
    let empty_fillin: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let (resolved, _cursor_back) = crate::expansions::resolve_tokens(text, &global_vars, &empty_fillin);
    resolved
}

fn output_text(text: &str, method: &str, target_hwnd: isize) {
    match method {
        "send-input" | "direct" => {
            // Character-by-character fallback for apps that don't support paste
            info!("[Keyfire] Output text (sendinput): \"{}\"", crate::expansions::log_preview(text));
            send_unicode_text(text, target_hwnd);
        }
        _ => {
            // Default: clipboard paste (instant)
            info!("[Keyfire] Output text (clipboard): \"{}\"", crate::expansions::log_preview(text));
            inject_via_clipboard(text, target_hwnd);
        }
    }
}

// ── Clipboard paste injection ───────────────────────────────────────────────
// CRITICAL: SUPPRESS_SIMULATED must be set true before any SendInput call.
// SUPPRESS_NEXT_CLIPBOARD_WRITE must be set before any internal clipboard write.
// New injection paths must follow this pattern or Keyfire will intercept its own
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
    // Queue-sync + settle floor before restoring — async paste handlers
    // (Chromium renderers) read the clipboard well after the Ctrl+V keydown;
    // restoring on the raw preset delay (25-50ms) pasted the user's OLD
    // clipboard content instead of the step text. See expansions::settle_paste.
    crate::expansions::settle_paste(target_hwnd, clip_restore_ms.max(crate::expansions::PASTE_RESTORE_SETTLE_MS));
    if crate::expansions::clipboard_sequence_number() == post_write_seq {
        crate::expansions::restore_clipboard_snapshot(&snapshot);
    }
    SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
}

/// Core clipboard paste: write text to clipboard + send paste keystroke.
/// Does NOT save/restore the clipboard — caller is responsible for that.
fn clipboard_paste_core(text: &str, target_hwnd: isize) {
    let mut write_ok = write_clipboard(text);
    if !write_ok {
        // The target app (reading a previous paste) or a clipboard manager may
        // be holding the clipboard open right now — transient contention, not
        // a hard failure. Retry briefly before giving up: a skipped Type Text
        // step (silently missing recipient/text) is far worse than a short
        // stutter (eM Client multi-recipient skips, 2026-08-05).
        for attempt in 1..=10u32 {
            thread::sleep(Duration::from_millis(20));
            write_ok = write_clipboard(text);
            if write_ok {
                info!("[Keyfire] Clipboard write recovered on retry {}", attempt);
                break;
            }
        }
    }
    info!("[Keyfire] Clipboard write (actions, ok={}): \"{}\"", write_ok, crate::expansions::log_preview(text));
    if !write_ok {
        warn!("[Keyfire] Skipping paste — clipboard write failed after retries, would paste wrong content");
        return;
    }

    let (_, _, fg_settle_ms, _) = speed_delays();

    let _suppress = SuppressionGuard::new();
    let held = release_held_modifiers();

    // Refocus the captured target ONLY if focus has left its process entirely.
    // If the current foreground is a DIFFERENT window of the SAME process, the
    // macro itself moved focus there (e.g. Press Key Ctrl+N opened a new
    // eM Client draft) and the captured HWND is stale — forcing it back to the
    // old window intermittently stole focus from the draft and the paste (and
    // following keystrokes) landed in the wrong window, so Type Text steps
    // appeared to skip (beta report 2026-08-05).
    if target_hwnd != 0 {
        let fg = unsafe { GetForegroundWindow() as isize };
        let same_process = if fg == target_hwnd {
            true
        } else if fg != 0 {
            let mut fg_pid: u32 = 0;
            let mut tgt_pid: u32 = 0;
            unsafe {
                GetWindowThreadProcessId(fg as _, &mut fg_pid);
                GetWindowThreadProcessId(target_hwnd as _, &mut tgt_pid);
            }
            fg_pid != 0 && fg_pid == tgt_pid
        } else {
            false
        };
        if !same_process {
            unsafe {
                SetForegroundWindow(target_hwnd as _);
            }
            if fg_settle_ms > 0 { thread::sleep(Duration::from_millis(fg_settle_ms)); }
        }
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
            // For Keyfire's own paste writes (suppress_listener = true), keep the
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
    let key_delay_ms = keystroke_delay_ms();
    let _suppress = SuppressionGuard::new();
    let held = release_held_modifiers();

    // Restore focus to target window
    if target_hwnd != 0 {
        unsafe {
            SetForegroundWindow(target_hwnd as _);
        }
        if fg_settle_ms > 0 { thread::sleep(Duration::from_millis(fg_settle_ms)); }
    }

    let mut char_count: u64 = 0;
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
        char_count += 1;
        if key_delay_ms > 0 {
            thread::sleep(Duration::from_millis(key_delay_ms));
        }
    }

    // Drain buffer — `SendInput` only queues keystrokes into the OS input
    // queue; the target app drains them via its message pump at its own
    // pace. Without this wait, a subsequent macro step that bypasses the
    // input queue (Open URL, Open App, Click at Position, Focus Window)
    // can act on the target window before the last few characters have
    // been processed, producing the visible "next action fires before text
    // finishes typing" bug. Scale = half the typing time, capped so very
    // long text doesn't drag — empirically enough for browsers/IDEs.
    // 10ms floor keeps a minimal drain even at 0ms keystroke delay (instant) —
    // the sequencing bug this guards against doesn't care how fast we typed.
    if char_count > 0 {
        let drain_ms = (char_count * key_delay_ms / 2).clamp(10, 500);
        thread::sleep(Duration::from_millis(drain_ms));
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

/// Resolve a VK to its scancode-mode SendInput payload.
///
/// Returns (wVk, wScan, base_flags). When the scancode lookup succeeds (the
/// common path for all normal keys), returns scancode mode with the extended
/// flag set for arrows / INS / DEL / HOME / END / PG keys / RAlt / RCtrl /
/// Win / NumLock / PrtScn. Scancode mode is what DirectInput / Raw Input
/// game engines read directly — VK-only injection is invisible to them.
///
/// Falls back to VK-only mode if MapVirtualKeyW returns 0 (rare: some
/// media-key VKs have no hardware mapping). Standard Win32 apps see the
/// same WM_KEYDOWN either way because Windows synthesises the cooked
/// message from the scancode lookup.
fn vk_to_sendinput_parts(vk: u16) -> (u16, u16, u32) {
    let scan = unsafe { MapVirtualKeyW(vk as u32, 0) } as u16;
    if scan == 0 {
        return (vk, 0, 0);
    }
    let is_extended = matches!(vk as u32,
        0x21..=0x28 | 0x2C..=0x2E | 0x5B | 0x5C | 0x90 | 0xA3 | 0xA5
    );
    let mut flags = KEYEVENTF_SCANCODE;
    if is_extended { flags |= KEYEVENTF_EXTENDEDKEY; }
    (0, scan, flags)
}

/// Build a single VK keyboard INPUT struct (key-down or key-up).
fn make_vk_input(vk: u16, key_up: bool) -> INPUT {
    let (w_vk, w_scan, base_flags) = vk_to_sendinput_parts(vk);
    let flags = if key_up { base_flags | KEYEVENTF_KEYUP } else { base_flags };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: w_vk as VIRTUAL_KEY,
                wScan: w_scan,
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
    // Done outside SuppressionGuard so the key-ups update Keyfire's modifier tracking.
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

    // Parse modifiers. Keep them for mouse output too so combos like
    // Shift+LButton or Ctrl+RButton fire the click with the modifier held —
    // captured from the mouse hook (handle_mouse_down CAPTURING_KEY branch).
    // The normal-mode mouse path below wraps the click with mod_down/up
    // SendInput batches; hold-mode and repeat-mode mouse paths still ignore
    // modifiers (edge case, follow-up).
    let modifiers: Vec<String> = data
        .get("modifiers")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();

    // Bare-modifier mode: when the user captured a sole modifier (Ctrl / Shift
    // / Alt / Win alone), the frontend sends key="" with non-empty modifiers.
    // target_vk stays 0 to signal "no main key" — keystroke chains below skip
    // the main-key tap and only press/release the modifier chord.
    let target_vk: u16 = if is_mouse {
        0
    } else if key_name.is_empty() {
        if modifiers.is_empty() {
            warn!("[Keyfire] Send Hotkey has no key or modifiers — nothing to send");
            return;
        }
        0
    } else {
        match display_name_to_vk(key_name) {
            Some(vk) => vk,
            None => {
                warn!("[Keyfire] Unknown Send Hotkey key: {}", key_name);
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
    // synthetic that matches a Keyfire binding). 5ms covers typical dispatch.
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
        info!("[Keyfire] Repeat started: {} ({}ms)", combo_label, repeat_interval);

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
            info!("[Keyfire] Hold released: {}", combo_label);
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
            info!("[Keyfire] Hold released (switching): {}", state.label);
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
        info!("[Keyfire] Send Hotkey → mouse click: {}", combo_label);
        // Build the click as two SendInput batches inside one SuppressionGuard:
        //   1) mod_downs + mouse_down
        //   2) mouse_up + mod_ups (reversed)
        // The 15ms mid-sleep matches send_mouse_click's Chromium-fix hold time
        // per [[feedback_synthetic_key_hold_time]]. Not calling send_mouse_click
        // directly because it toggles SUPPRESS_SIMULATED off before returning,
        // which would unmask the mod_up batch as real input to our own hook.
        let (down_flag, up_flag) = match key_name {
            "LButton" => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            "RButton" => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            "MButton" => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
            _ => {
                warn!("[Keyfire] Unknown mouse button: {}", key_name);
                return;
            }
        };
        let make_mouse = |flag: u32| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0, dy: 0, mouseData: 0, dwFlags: flag, time: 0, dwExtraInfo: 0,
                },
            },
        };
        let held = release_held_modifiers();
        {
            let _guard = SuppressionGuard::new();
            let mut down_batch: Vec<INPUT> = Vec::with_capacity(mod_vks.len() + 1);
            for &vk in &mod_vks { down_batch.push(make_vk_input(vk, false)); }
            down_batch.push(make_mouse(down_flag));
            unsafe {
                SendInput(down_batch.len() as u32, down_batch.as_ptr(), std::mem::size_of::<INPUT>() as i32);
            }
            thread::sleep(Duration::from_millis(15));
            let mut up_batch: Vec<INPUT> = Vec::with_capacity(mod_vks.len() + 1);
            up_batch.push(make_mouse(up_flag));
            for &vk in mod_vks.iter().rev() { up_batch.push(make_vk_input(vk, true)); }
            unsafe {
                SendInput(up_batch.len() as u32, up_batch.as_ptr(), std::mem::size_of::<INPUT>() as i32);
            }
        }
        restore_modifiers(&held);
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
    // IVirtualDesktopManager to filter out matches on non-current desktops.
    // Focusing a window on another virtual desktop would yank the user across
    // desktops. None = COM failed to init, permissive fallback (no filtering).
    vdm: Option<windows::Win32::UI::Shell::IVirtualDesktopManager>,
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

    // All criteria matched — but only accept if this window lives on the
    // current virtual desktop. Otherwise SetForegroundWindow would drag the
    // user across desktops. Skip and keep looking for another candidate.
    if !crate::distill::is_hwnd_on_current_desktop(state.vdm.as_ref(), hwnd as isize) {
        return 1;
    }
    state.found_hwnd = hwnd as isize;
    0 // stop enumeration
}

fn find_window_by_criteria(process_name: &str, title: &str) -> Option<isize> {
    let mut state = FindWindowState {
        target_process_lower: process_name.to_lowercase(),
        target_title_lower: title.to_lowercase(),
        vdm: crate::distill::make_vdm(),
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

/// Resolve a Minimise / Maximise / Resize Window step's target. If the value
/// contains a non-empty process or title, matches an existing window via
/// EnumWindows. Otherwise falls back to the macro's current *target_hwnd
/// (updated by any prior Focus Window step) or the current foreground window.
/// Returns 0 if no window can be resolved.
fn resolve_window_target(parsed: &Value, current_target: isize) -> isize {
    let process = parsed.get("process").and_then(|v| v.as_str()).unwrap_or("");
    let title = parsed.get("title").and_then(|v| v.as_str()).unwrap_or("");
    if !process.is_empty() || !title.is_empty() {
        return find_window_by_criteria(process, title).unwrap_or(0);
    }
    if current_target != 0 {
        return current_target;
    }
    unsafe { GetForegroundWindow() as isize }
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
            warn!("[Keyfire] Unknown mouse button: {}", button);
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
        // Small hold between down and up — fused back-to-back SendInputs are
        // invisible to some Chromium-based targets (Arc browser's right-click
        // context menu, observed 2026-06-18). Mirrors the keyboard hold-time
        // pattern per [[feedback_synthetic_key_hold_time]].
        thread::sleep(Duration::from_millis(15));
        SendInput(1, &input_up, std::mem::size_of::<INPUT>() as i32);
    }

    crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
    info!("[Keyfire] Mouse click: {}", button);
}

/// Synthesize a full click at the current cursor position — used by the mouse
/// ::hold trigger's early-release passthrough (the hook suppressed the user's
/// physical button-down, so on a quick tap the app still needs its native
/// click). 15ms down-up split per the synthetic hold-time rule; button names
/// are the replay_mouse_button set ("Left".."Side2").
pub fn send_passthrough_click(button: &str) {
    crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
    replay_mouse_button(button, true);
    thread::sleep(Duration::from_millis(15));
    replay_mouse_button(button, false);
    thread::sleep(Duration::from_millis(5));
    crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
    info!("[Keyfire] [HOLD] mouse passthrough click: {}", button);
}

// ── Recorder replay helpers ─────────────────────────────────────────────────
//
// Used only by the "Record Macro" macro step's replay path. Unlike send_mouse_click,
// these helpers send a single button-down OR a single button-up — never
// fused — because the recording carries down + up as separate events with
// their original gap. The caller wraps the whole replay in a SuppressionGuard.

fn replay_mouse_button(button: &str, is_down: bool) {
    let (flag, mouse_data) = match (button, is_down) {
        ("Left",   true)  => (MOUSEEVENTF_LEFTDOWN,   0_u32),
        ("Left",   false) => (MOUSEEVENTF_LEFTUP,     0_u32),
        ("Right",  true)  => (MOUSEEVENTF_RIGHTDOWN,  0_u32),
        ("Right",  false) => (MOUSEEVENTF_RIGHTUP,    0_u32),
        ("Middle", true)  => (MOUSEEVENTF_MIDDLEDOWN, 0_u32),
        ("Middle", false) => (MOUSEEVENTF_MIDDLEUP,   0_u32),
        ("Side1",  true)  => (MOUSEEVENTF_XDOWN,      1_u32),
        ("Side1",  false) => (MOUSEEVENTF_XUP,        1_u32),
        ("Side2",  true)  => (MOUSEEVENTF_XDOWN,      2_u32),
        ("Side2",  false) => (MOUSEEVENTF_XUP,        2_u32),
        _ => {
            warn!("[Keyfire] Replay: unknown mouse button name: {}", button);
            return;
        }
    };
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: mouse_data,
                dwFlags: flag,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32); }
}

/// Replay a mouse cursor move during macro playback. Crucially uses SendInput
/// with MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK so
/// the OS generates real WM_MOUSEMOVE messages to whatever window the cursor
/// is over. SetCursorPos alone would teleport the pointer but emit no move
/// messages, so apps that detect drags by tracking WM_MOUSEMOVE between
/// LBUTTONDOWN and LBUTTONUP (Excel image drag, Explorer drag-drop, Paint
/// strokes, lasso-select etc.) see a click-in-place instead of a drag.
///
/// Coords are absolute virtual-desktop pixels (multi-monitor aware). We
/// normalise to the 0..=65535 range Windows expects for absolute mouse
/// SendInput, mapped over the virtual screen rect from GetSystemMetrics.
fn replay_mouse_move(x: i32, y: i32) {
    let (vsx, vsy, vsw, vsh) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    // Guard against pathological zero sizes — fall back to SetCursorPos so the
    // replay still advances visually rather than dividing by zero. Realistic
    // monitors always return non-zero metrics; this is a defensive belt.
    if vsw <= 1 || vsh <= 1 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(x, y);
        }
        return;
    }
    let nx = (((x - vsx) as i64 * 65535) / (vsw - 1) as i64) as i32;
    let ny = (((y - vsy) as i64 * 65535) / (vsh - 1) as i64) as i32;
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: nx,
                dy: ny,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32); }
}

fn replay_wheel(delta: i32) {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: delta as u32,
                dwFlags: MOUSEEVENTF_WHEEL,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32); }
}

// ── Distilled Click-at-Position anchor transform ────────────────────────────
//
// Real apps don't uniformly scale their content when the window resizes:
// sidebars, scrollbars, toolbars stay a fixed pixel size while the content
// area reflows. Pure proportional scaling misses fixed-position UI in split-
// screen or half-monitor windows.
//
// This heuristic looks at each axis independently. If the recorded click was
// within `ANCHOR_THRESHOLD_FRAC` of an edge, we anchor to that edge — preserving
// the distance from the closer edge. Otherwise we scale proportionally.
//
// Empirically handles Slack/VS Code/Chrome/Outlook resize much better than pure
// ratio scaling. Trade-off: a click that happens to be near an edge in a
// uniform-scaling app will edge-anchor when it should scale — rare, and the
// per-click override UI (later work) handles the tail cases.

const ANCHOR_THRESHOLD_FRAC: f32 = 0.20;

/// Transform a single-axis click coord from recorded → current window size.
/// Returns `(new_coord, anchor_label)` where `anchor_label` is one of
/// "start" (top/left-anchored), "end" (bottom/right-anchored), or "prop".
fn anchor_transform_axis(rec: i32, rec_size: i32, cur_size: i32, _axis: &str) -> (i32, &'static str) {
    if rec_size <= 0 || cur_size <= 0 {
        return (rec, "prop");
    }
    let threshold = ((rec_size as f32) * ANCHOR_THRESHOLD_FRAC).max(50.0) as i32;
    let dist_start = rec;
    let dist_end = rec_size - rec;
    if dist_start < threshold && dist_start <= dist_end {
        // Anchored to the start edge (top or left) — keep same distance from it
        (dist_start, "start")
    } else if dist_end < threshold && dist_end < dist_start {
        // Anchored to the end edge (bottom or right)
        (cur_size - dist_end, "end")
    } else {
        // Middle of the axis — proportional scaling
        let scaled = (rec as f32 * cur_size as f32 / rec_size as f32).round() as i32;
        (scaled, "prop")
    }
}

// ── Macro sequence step executor ────────────────────────────────────────────

/// Blocking OK/Cancel confirmation dialog for destructive System macro steps
/// (Sleep, Log Off, Shut Down). Runs on the macro thread — MessageBoxW is
/// synchronous and blocks that thread until the user answers, which is what we
/// want. Returns true if the user confirmed. TopMost + SetForeground so the
/// dialog surfaces even if the target app has focus.
fn confirm_destructive_step(title: &str, message: &str) -> bool {
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let caption: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OKCANCEL | MB_ICONWARNING | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    result == IDOK as i32
}

// OK/Cancel plan-preview dialog for the Sort Files step — informational icon
// rather than the warning triangle, otherwise the same topmost/foreground
// treatment as confirm_destructive_step.
fn confirm_plan_dialog(title: &str, message: &str) -> bool {
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let caption: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OKCANCEL | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    result == IDOK as i32
}

// Fire-and-forget information dialog (Sort Files completion report / Pro
// gate notice). Blocks the macro thread until dismissed, which is fine —
// it's the last thing the step does.
fn info_dialog(title: &str, message: &str) {
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let caption: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
        );
    }
}

// Three-way clash dialog for the Sort Files step: Yes = overwrite the
// existing files, No = keep both (date + time suffix), Cancel = stop.
// MessageBoxW buttons can't be relabelled without hooks, so the message
// body spells out the mapping.
enum ClashChoice {
    Overwrite,
    AppendDate,
    Cancel,
}

fn clash_choice_dialog(title: &str, message: &str) -> ClashChoice {
    let text: Vec<u16> = message.encode_utf16().chain(std::iter::once(0)).collect();
    let caption: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let result = unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_YESNOCANCEL | MB_ICONINFORMATION | MB_TOPMOST | MB_SETFOREGROUND,
        )
    };
    if result == IDYES as i32 {
        ClashChoice::Overwrite
    } else if result == IDNO as i32 {
        ClashChoice::AppendDate
    } else {
        ClashChoice::Cancel
    }
}

// Resolve the source file list for the file-management steps ("selected in
// Explorer" vs "folder + wildcard pattern"), plus the base folder relative
// destinations resolve against (the Explorer folder for selected mode, the
// source folder otherwise). Err(true) = skip the step and continue the
// macro (config incomplete); Err(false) = abort the macro (Explorer context
// required but unavailable) — callers `return` the Err value directly.
fn resolve_file_step_sources(
    parsed: &Value,
    target_hwnd: isize,
    step_type: &str,
) -> Result<(Vec<String>, Option<String>), bool> {
    let source_mode = parsed.get("sourceMode").and_then(|v| v.as_str()).unwrap_or("selected");
    if source_mode == "folder" {
        let dir = parsed
            .get("sourcePath")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if dir.is_empty() {
            warn!("[Keyfire] {}: no source folder set — skipping step", step_type);
            return Err(true);
        }
        let pattern = parsed.get("pattern").and_then(|v| v.as_str()).unwrap_or("*");
        let files = crate::shell_files::list_matching_files(&dir, pattern);
        Ok((files, Some(dir)))
    } else {
        match crate::shell_files::explorer_context(target_hwnd) {
            Some(ctx) if !ctx.selected.is_empty() => {
                // Virtual locations (search results, libraries) have no
                // folder path — fall back to the first selected item's
                // parent, which is where the files really are.
                let base = ctx.folder.clone().or_else(|| {
                    std::path::Path::new(&ctx.selected[0])
                        .parent()
                        .map(|p| p.to_string_lossy().into_owned())
                });
                Ok((ctx.selected, base))
            }
            Some(_) => {
                warn!(
                    "[Keyfire] {}: nothing selected in File Explorer — aborting macro",
                    step_type
                );
                Err(false)
            }
            None => {
                warn!(
                    "[Keyfire] {}: foreground window isn't File Explorer — aborting macro",
                    step_type
                );
                Err(false)
            }
        }
    }
}

// Send a media key (VK_VOLUME_UP/DOWN/MUTE) via SendInput as VK-only with the
// extended-key flag. Bypasses the scancode-mode path send_vk_key uses because
// media keys don't have reliable hardware scancodes — MapVirtualKeyW can
// return non-zero garbage that Windows won't recognise as a media event. The
// KEYEVENTF_EXTENDEDKEY flag is what routes it through the shell's WM_APPCOMMAND
// handler, which is also what triggers the Windows volume OSD overlay.
//
// Used by the Change Volume arm — the OSD wouldn't appear from COM-only
// SetMasterVolumeLevelScalar calls (the OSD is bound to the media-key /
// APPCOMMAND path in Explorer's tray process). Firing one of these before/
// after the COM adjustment gives the same visual feedback as pressing the
// physical FN volume keys.
fn send_media_key(vk: u16) {
    let ki_down = KEYBDINPUT {
        wVk: vk as VIRTUAL_KEY,
        wScan: 0,
        dwFlags: KEYEVENTF_EXTENDEDKEY,
        time: 0,
        dwExtraInfo: 0,
    };
    let ki_up = KEYBDINPUT {
        wVk: vk as VIRTUAL_KEY,
        wScan: 0,
        dwFlags: KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
        time: 0,
        dwExtraInfo: 0,
    };
    let down = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: ki_down } };
    let up = INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: ki_up } };
    unsafe {
        SendInput(1, &down, std::mem::size_of::<INPUT>() as i32);
        SendInput(1, &up, std::mem::size_of::<INPUT>() as i32);
    }
}

// Emit a Win+Key or Win+Shift+Key chord via SendInput. Used by the
// "Minimise All" / "Restore All" system steps which map to Win+M / Win+Shift+M.
fn send_win_chord(vk: u16, with_shift: bool) {
    crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
    send_vk_key(VK_LWIN, false);
    if with_shift { send_vk_key(VK_LSHIFT, false); }
    send_vk_key(vk, false);
    send_vk_key(vk, true);
    if with_shift { send_vk_key(VK_LSHIFT, true); }
    send_vk_key(VK_LWIN, true);
    crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
}

/// Rewrite a distilled Click at Position step from `windowClient` mode to
/// plain `absolute` mode using its recorded fallback coords. Returns None if
/// the step is already absolute (no change needed) or the value can't be
/// parsed. Used when the macro has no target binding: without a binding,
/// resolving windowClient against whatever live window happens to match the
/// recorded exe/class produces wildly wrong clicks after anchor scaling.
fn neutralize_click_to_absolute(step: &Value) -> Option<Value> {
    let value_str = step.get("value").and_then(|v| v.as_str())?;
    let mut inner: Value = serde_json::from_str(value_str).ok()?;
    if inner.get("mode").and_then(|v| v.as_str()) != Some("windowClient") {
        return None;
    }
    let fx = inner.get("fallbackX").and_then(|v| v.as_i64())?;
    let fy = inner.get("fallbackY").and_then(|v| v.as_i64())?;
    let obj = inner.as_object_mut()?;
    obj.insert("x".to_string(), serde_json::json!(fx));
    obj.insert("y".to_string(), serde_json::json!(fy));
    obj.insert("mode".to_string(), serde_json::json!("absolute"));
    obj.remove("targetWindow");
    obj.remove("resizeBehavior");
    obj.remove("fallbackX");
    obj.remove("fallbackY");
    let mut clone = step.clone();
    if let Some(step_obj) = clone.as_object_mut() {
        step_obj.insert("value".to_string(), serde_json::json!(inner.to_string()));
    }
    Some(clone)
}

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
            let value = if step_value.is_empty() { "LButton" } else { step_value };
            // Strip Down / Up suffix to isolate the button base name. Bare
            // "LButton" / "RButton" / "MButton" means a full click (down + up)
            // — that's the pre-v0.6.5 shape and still the default when a new
            // Click Mouse step is created. The suffixed variants fire just
            // one phase so users can chain e.g. LButtonDown → mouse move
            // steps → LButtonUp to script a drag.
            let (button, phase) = if let Some(base) = value.strip_suffix("Down") {
                (base, "down")
            } else if let Some(base) = value.strip_suffix("Up") {
                (base, "up")
            } else {
                (value, "full")
            };
            if is_mouse_button(button) {
                for i in 0..repeat_count {
                    match phase {
                        "down" => send_mouse_event(button, false),
                        "up"   => send_mouse_event(button, true),
                        _      => send_mouse_click(button),
                    }
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
                            warn!("[Keyfire] Unknown macro step key: {}", key_name);
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
            // Polled sleep — chunks of 100ms so Esc / pause toggle reach
            // the user within 100ms even on long waits. A single uninter-
            // ruptible thread::sleep here would block all cancel paths
            // for the entire wait. Mirror of the loop-delay treatment.
            let total = Duration::from_millis(ms);
            let start = std::time::Instant::now();
            while start.elapsed() < total {
                if ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                    info!("[Keyfire] Wait (ms) cancelled (Esc)");
                    return false;  // abort whole macro
                }
                if !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
                    info!("[Keyfire] Wait (ms) aborted (macros disabled)");
                    return false;
                }
                let remaining = total.saturating_sub(start.elapsed());
                thread::sleep(Duration::from_millis(100).min(remaining));
            }
        }

        // Ctrl+C / Ctrl+V / Ctrl+A as first-class macro steps. Implemented as
        // a synthetic LCTRL + letter pulse — same path Press Key takes for the
        // equivalent chord, but exposed with a clearer label in the editor.
        // Doesn't touch Keyfire's own clipboard write path; the OS handles paste
        // semantics for whatever was last copied (per feedback_paste_architecture
        // memory — that rule is about Keyfire-injected content, not raw Ctrl+V).
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
            let process_raw = parsed.get("process").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
            let target_proc = process_raw.trim_end_matches(".exe").to_string();
            let target_title = parsed.get("title").and_then(|v| v.as_str()).unwrap_or("").trim().to_lowercase();
            if target_proc.is_empty() && target_title.is_empty() {
                warn!("[Keyfire] Wait for Window step: both process and title are empty");
                return true;
            }
            // 30s hardcoded — kept off the UI per design. If a typo or stale
            // criterion never matches, the macro continues to the next step
            // instead of hanging Keyfire indefinitely.
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
                            "[Keyfire] Wait for Window: matched (process='{}' title~='{}') after {:?}",
                            target_proc, target_title, start.elapsed()
                        );
                        break;
                    }
                }
                if start.elapsed() >= Duration::from_millis(timeout_ms) {
                    warn!(
                        "[Keyfire] Wait for Window: timeout ({} ms) waiting for process='{}' title~='{}' — aborting macro",
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
            if step_value.is_empty() {
                return true;
            }
            // Backward compat: legacy macros stored step.value as a plain path string.
            // New writes emit JSON {path, monitor}. Detect by trying to parse JSON;
            // fall back to treating the whole value as a bare path.
            let trimmed = step_value.trim_start();
            let (path_owned, monitor) = if trimmed.starts_with('{') {
                match serde_json::from_str::<Value>(step_value) {
                    Ok(parsed) => {
                        let p = parsed.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let m = crate::window_target::parse_monitor_target(Some(&parsed), *target_hwnd);
                        (p, m)
                    }
                    Err(_) => (step_value.to_string(), crate::window_target::MonitorTarget::None),
                }
            } else {
                (step_value.to_string(), crate::window_target::MonitorTarget::None)
            };
            if !path_owned.is_empty() {
                let rx = crate::window_target::launch_with_monitor_target(
                    crate::window_target::LaunchKind::Folder { path: &path_owned },
                    monitor,
                );
                // Block macro progression until the folder window has been
                // moved to the target monitor. Otherwise the next macro step
                // (Minimise All, Focus Window, snap keystroke, etc.) races
                // the async window-placement and fires before the folder is
                // even visible. `rx` is `None` when no monitor target is set —
                // nothing to wait on. 5s ceiling in case the launch failed.
                if let Some(rx) = rx {
                    let _ = rx.recv_timeout(Duration::from_secs(5));
                }
            }
        }

        // ── Files steps ─────────────────────────────────────────────────────
        // Create Folder / Copy Files / Move Files — Explorer-integrated file
        // management via crate::shell_files. "Current folder" and "selected
        // files" read the foreground Explorer window (or desktop) through
        // IShellWindows; transfers run through IFileOperation so the user
        // gets the native progress + conflict dialogs and Recycle-Bin undo.
        // Hard failures (no Explorer window when the step depends on one,
        // transfer error) abort the macro — later steps may assume the file
        // work happened. Soft empties (no files matched a pattern) continue.

        // Value: JSON { name, promptForName, locationMode: "current"|"custom",
        // path, templateEnabled, templatePath }.
        // Tokens in the name resolve at run time ({date}, {clipboard}, ...)
        // so "Invoices {date:YYYY-MM-DD}" stamps itself, same behaviour as
        // Type Text. {inc}/{inc:N} then numbers the name against what
        // already exists in the target directory. promptForName opens the
        // fill-in window at run time with the configured name as the
        // editable default — cancel aborts the macro (matching fill-in Esc
        // semantics). templateEnabled seeds the new folder by copying
        // templatePath's contents into it via IFileOperation.
        "Create Folder" => {
            let parsed: Value = serde_json::from_str(step_value).unwrap_or(Value::Null);
            let name_raw = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let prompt_for_name = parsed
                .get("promptForName")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if name_raw.trim().is_empty() && !prompt_for_name {
                warn!("[Keyfire] Create Folder: no folder name set — skipping step");
                return true;
            }
            // Parent first: {inc} needs it, and the Explorer context should
            // be read via the trigger-time hint before any prompt shows.
            let mode = parsed.get("locationMode").and_then(|v| v.as_str()).unwrap_or("current");
            let parent = if mode == "custom" {
                parsed.get("path").and_then(|v| v.as_str()).unwrap_or("").trim().to_string()
            } else {
                crate::shell_files::explorer_context(*target_hwnd)
                    .and_then(|ctx| ctx.folder)
                    .unwrap_or_default()
            };
            if parent.is_empty() {
                warn!(
                    "[Keyfire] Create Folder: no target directory (mode={} — is a File Explorer window focused?) — aborting macro",
                    mode
                );
                return false;
            }
            let name_seed = resolve_type_text_tokens(name_raw);
            let name = if prompt_for_name {
                match crate::expansions::prompt_single_text("New folder name", &name_seed) {
                    Some(v) if !v.trim().is_empty() => resolve_type_text_tokens(&v),
                    _ => {
                        info!("[Keyfire] Create Folder: name prompt cancelled — aborting macro");
                        return false;
                    }
                }
            } else {
                name_seed
            };
            let name = crate::shell_files::resolve_increment(&parent, &name);
            let created = match crate::shell_files::create_folder(&parent, &name) {
                Ok(full) => {
                    info!("[Keyfire] Create Folder: {}", full);
                    full
                }
                Err(e) => {
                    warn!("[Keyfire] Create Folder: {} — aborting macro", e);
                    return false;
                }
            };
            // Template seed — copy the template folder's contents (not the
            // folder itself) into the new folder.
            let template_enabled = parsed
                .get("templateEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let template_path = parsed
                .get("templatePath")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if template_enabled && !template_path.is_empty() {
                let entries = crate::shell_files::list_dir_entries(&template_path);
                if entries.is_empty() {
                    info!(
                        "[Keyfire] Create Folder: template {} is empty or unreadable — nothing to copy",
                        template_path
                    );
                } else {
                    match crate::shell_files::transfer_files(&entries, &created, false) {
                        Ok(n) => info!(
                            "[Keyfire] Create Folder: {} template item(s) copied into {}",
                            n, created
                        ),
                        Err(e) => {
                            warn!("[Keyfire] Create Folder: template copy failed: {} — aborting macro", e);
                            return false;
                        }
                    }
                }
            }
        }

        // Value: JSON { sourceMode: "selected"|"folder", sourcePath, pattern,
        // destMode: "path"|"subfolder", destPath, destSubfolder,
        // createSubfolder }. "selected" = whatever is highlighted in the
        // foreground Explorer window (files and folders); "folder" = files in
        // sourcePath matching the `;`-separated wildcard pattern (subfolders
        // excluded). destMode "subfolder" resolves destSubfolder against the
        // folder the sources live in (the Explorer folder for selected mode,
        // sourcePath for folder mode) — the "file into .\Superceded\ wherever
        // I am" workflow. A missing subfolder ABORTS the macro unless
        // createSubfolder is set, so chains only run where the folder
        // convention exists.
        "Copy Files" | "Move Files" => {
            let is_move = step_type == "Move Files";
            let parsed: Value = serde_json::from_str(step_value).unwrap_or(Value::Null);
            let (sources, base_folder) = match resolve_file_step_sources(&parsed, *target_hwnd, step_type) {
                Ok(v) => v,
                Err(cont) => return cont,
            };
            if sources.is_empty() {
                info!("[Keyfire] {}: no files matched — nothing to do", step_type);
                return true;
            }

            let dest_mode = parsed.get("destMode").and_then(|v| v.as_str()).unwrap_or("path");
            let dest = if dest_mode == "subfolder" {
                let sub = parsed
                    .get("destSubfolder")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if sub.is_empty() {
                    warn!("[Keyfire] {}: no subfolder name set — skipping step", step_type);
                    return true;
                }
                let create = parsed
                    .get("createSubfolder")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let Some(base) = base_folder else {
                    warn!(
                        "[Keyfire] {}: current folder has no filesystem path — aborting macro",
                        step_type
                    );
                    return false;
                };
                match crate::shell_files::resolve_subfolder(&base, sub, create) {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("[Keyfire] {}: {} — aborting macro", step_type, e);
                        return false;
                    }
                }
            } else {
                let d = parsed
                    .get("destPath")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                if d.is_empty() {
                    warn!("[Keyfire] {}: no destination folder set — skipping step", step_type);
                    return true;
                }
                if let Err(e) = std::fs::create_dir_all(&d) {
                    warn!(
                        "[Keyfire] {}: destination {} unavailable: {} — aborting macro",
                        step_type, d, e
                    );
                    return false;
                }
                d
            };
            match crate::shell_files::transfer_files(&sources, &dest, is_move) {
                Ok(n) => info!(
                    "[Keyfire] {}: {} item(s) → {}",
                    step_type, n, dest
                ),
                Err(e) => {
                    warn!("[Keyfire] {}: {} — aborting macro", step_type, e);
                    return false;
                }
            }
        }

        // Sort Files (Pro) — route each file to the folder its NAME points
        // at. Value: JSON { sourceMode, sourcePath, pattern, rootPath,
        // searchDepth, keyMode: "prefix"|"segment", keyLength, keySegment,
        // keySeparator, routeEnabled, codeSegment, codeSeparator,
        // mappings: [{code, folder}], confirm, collision:
        // "timestamp"|"ask"|"skip" }.
        //
        // Flow: extract a folder key from each filename (first N chars or
        // the Nth separator-delimited segment), find the first folder under
        // rootPath whose name contains that key (BFS, depth-limited, cached
        // per run), optionally descend into a mapped subfolder by a second
        // code segment (DR → "- Drawings"), then execute every move as ONE
        // IFileOperation. Per-file problems SKIP that file with a reason —
        // a sorter should sort what it can and report the rest — so unlike
        // Copy/Move this arm only aborts on hard failures (root missing,
        // transfer error).
        "Sort Files" => {
            if !crate::licence::is_pro() {
                warn!("[Keyfire] Sort Files: Pro feature, no valid licence — skipping step");
                info_dialog(
                    "Keyfire — Sort Files",
                    "Sort Files is a Pro feature.\n\nAdd a licence key in Settings to enable it.",
                );
                return true;
            }
            let parsed: Value = serde_json::from_str(step_value).unwrap_or(Value::Null);
            let root = parsed
                .get("rootPath")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if root.is_empty() {
                warn!("[Keyfire] Sort Files: no search folder set — skipping step");
                return true;
            }
            if !std::path::Path::new(&root).is_dir() {
                warn!("[Keyfire] Sort Files: search folder {} not found — aborting macro", root);
                return false;
            }
            let depth = parsed
                .get("searchDepth")
                .and_then(|v| v.as_u64())
                .unwrap_or(3)
                .clamp(1, 8) as u32;
            let key_mode = parsed.get("keyMode").and_then(|v| v.as_str()).unwrap_or("prefix");
            let key_length = parsed
                .get("keyLength")
                .and_then(|v| v.as_u64())
                .unwrap_or(6)
                .clamp(1, 64) as usize;
            let key_segment = parsed
                .get("keySegment")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .clamp(1, 32) as usize;
            let key_sep_raw = parsed.get("keySeparator").and_then(|v| v.as_str()).unwrap_or("-");
            let key_separator = if key_sep_raw.is_empty() { "-" } else { key_sep_raw };
            let route_enabled = parsed
                .get("routeEnabled")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let code_segment = parsed
                .get("codeSegment")
                .and_then(|v| v.as_u64())
                .unwrap_or(3)
                .clamp(1, 32) as usize;
            let code_sep_raw = parsed.get("codeSeparator").and_then(|v| v.as_str()).unwrap_or("-");
            let code_separator = if code_sep_raw.is_empty() { "-" } else { code_sep_raw };
            let mappings: Vec<(String, String)> = parsed
                .get("mappings")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|m| {
                            let code = m.get("code")?.as_str()?.trim().to_string();
                            let folder = m.get("folder")?.as_str()?.trim().to_string();
                            if code.is_empty() || folder.is_empty() {
                                None
                            } else {
                                Some((code, folder))
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();
            let confirm = parsed.get("confirm").and_then(|v| v.as_bool()).unwrap_or(true);
            // "prompt" (default): one Yes/No/Cancel dialog covering every
            // clash in the run — overwrite / keep-both-with-date-suffix /
            // stop. Legacy "ask" (pre-release native-dialog mode) maps here.
            let collision = match parsed.get("collision").and_then(|v| v.as_str()).unwrap_or("prompt") {
                "ask" => "prompt",
                c => c,
            };
            // "file.pdf" → "file (20260717-104500).pdf"
            let stamp_name = |name: &str| -> String {
                let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
                match name.rsplit_once('.') {
                    Some((stem, ext)) => format!("{} ({}).{}", stem, stamp, ext),
                    None => format!("{} ({})", name, stamp),
                }
            };

            let (sources, _base) = match resolve_file_step_sources(&parsed, *target_hwnd, step_type) {
                Ok(v) => v,
                Err(cont) => return cont,
            };
            if sources.is_empty() {
                info!("[Keyfire] Sort Files: no files matched — nothing to do");
                return true;
            }

            // ── Plan ────────────────────────────────────────────────────
            let mut moves: Vec<crate::shell_files::PlannedMove> = Vec::new();
            let mut skips: Vec<(String, String)> = Vec::new();
            // Indices into `moves` whose destination already has the file —
            // resolved after planning via the single clash dialog.
            let mut clash_idx: Vec<usize> = Vec::new();
            // key (lowercased) → matched folder. One tree search per
            // distinct key per run, like the AHK projectCache.
            let mut folder_cache: std::collections::HashMap<String, Option<String>> =
                std::collections::HashMap::new();

            for src in &sources {
                let p = std::path::Path::new(src);
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if name.is_empty() {
                    skips.push((src.clone(), "no file name".to_string()));
                    continue;
                }
                if p.is_dir() {
                    skips.push((name, "is a folder, not a file".to_string()));
                    continue;
                }
                let key = if key_mode == "segment" {
                    name.split(key_separator)
                        .nth(key_segment - 1)
                        .unwrap_or("")
                        .trim()
                        .to_string()
                } else {
                    name.chars().take(key_length).collect::<String>().trim().to_string()
                };
                if key.is_empty() {
                    skips.push((name, "couldn't extract a folder key from the name".to_string()));
                    continue;
                }
                let folder = folder_cache
                    .entry(key.to_lowercase())
                    .or_insert_with(|| crate::shell_files::find_folder_by_key(&root, &key, depth))
                    .clone();
                let Some(mut dest_dir) = folder else {
                    skips.push((name, format!("no folder matching '{}' found", key)));
                    continue;
                };
                if route_enabled {
                    // Extension guard: if the code segment is the last one it
                    // carries ".ext" — folder codes never contain dots, so
                    // cut at the first one.
                    let code = name
                        .split(code_separator)
                        .nth(code_segment - 1)
                        .unwrap_or("")
                        .split('.')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    if code.is_empty() {
                        skips.push((name, "couldn't extract a code segment from the name".to_string()));
                        continue;
                    }
                    let Some((_, sub)) =
                        mappings.iter().find(|(c, _)| c.eq_ignore_ascii_case(&code))
                    else {
                        skips.push((name, format!("code '{}' not mapped", code)));
                        continue;
                    };
                    let sub_path = std::path::Path::new(&dest_dir).join(sub);
                    if !sub_path.is_dir() {
                        skips.push((name, format!("subfolder '{}' missing in {}", sub, dest_dir)));
                        continue;
                    }
                    dest_dir = sub_path.to_string_lossy().into_owned();
                }
                if p.parent()
                    .map(|pp| pp.to_string_lossy().eq_ignore_ascii_case(&dest_dir))
                    .unwrap_or(false)
                {
                    skips.push((name, "already in its destination folder".to_string()));
                    continue;
                }
                let new_name = if std::path::Path::new(&dest_dir).join(&name).exists() {
                    match collision {
                        "skip" => {
                            skips.push((name, "already exists in destination".to_string()));
                            continue;
                        }
                        "timestamp" => Some(stamp_name(&name)),
                        // "prompt": queue as-is, marked for the clash dialog.
                        _ => {
                            clash_idx.push(moves.len());
                            None
                        }
                    }
                } else {
                    None
                };
                moves.push(crate::shell_files::PlannedMove {
                    src: src.clone(),
                    dest_dir,
                    new_name,
                });
            }

            // ── Confirm ─────────────────────────────────────────────────
            let display_name = |src: &str| {
                std::path::Path::new(src)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| src.to_string())
            };
            if moves.is_empty() {
                let mut msg = format!(
                    "Nothing to move — all {} file(s) were skipped:\n\n",
                    skips.len()
                );
                for (name, reason) in skips.iter().take(12) {
                    msg.push_str(&format!("{}\n    {}\n", name, reason));
                }
                if skips.len() > 12 {
                    msg.push_str(&format!("…and {} more (see log)\n", skips.len() - 12));
                }
                info!("[Keyfire] Sort Files: nothing to move, {} skipped", skips.len());
                for (name, reason) in &skips {
                    info!("[Keyfire] Sort Files skip: {} — {}", name, reason);
                }
                if confirm {
                    info_dialog("Keyfire — Sort Files", &msg);
                }
                return true;
            }
            // Clash resolution — one dialog covering every clash in the run.
            let mut silent_overwrite = false;
            if !clash_idx.is_empty() {
                let mut msg = format!(
                    "{} file(s) already exist in their destination:\n\n",
                    clash_idx.len()
                );
                for &i in clash_idx.iter().take(10) {
                    msg.push_str(&format!("{}\n", display_name(&moves[i].src)));
                }
                if clash_idx.len() > 10 {
                    msg.push_str(&format!("…and {} more\n", clash_idx.len() - 10));
                }
                msg.push_str(
                    "\nYes — overwrite the existing files\n\
                     No — keep both (a date + time suffix is added to the new file)\n\
                     Cancel — stop without moving anything",
                );
                match clash_choice_dialog("Keyfire — Sort Files", &msg) {
                    ClashChoice::Overwrite => silent_overwrite = true,
                    ClashChoice::AppendDate => {
                        for &i in &clash_idx {
                            let n = display_name(&moves[i].src);
                            moves[i].new_name = Some(stamp_name(&n));
                        }
                    }
                    ClashChoice::Cancel => {
                        info!("[Keyfire] Sort Files: cancelled at the clash dialog");
                        return true;
                    }
                }
            }

            if confirm {
                let mut msg = format!("Move {} file(s):\n\n", moves.len());
                for m in moves.iter().take(12) {
                    msg.push_str(&format!("{}\n    → {}\n", display_name(&m.src), m.dest_dir));
                }
                if moves.len() > 12 {
                    msg.push_str(&format!("…and {} more\n", moves.len() - 12));
                }
                if !skips.is_empty() {
                    msg.push_str(&format!("\nSkipping {} file(s):\n\n", skips.len()));
                    for (name, reason) in skips.iter().take(8) {
                        msg.push_str(&format!("{}\n    {}\n", name, reason));
                    }
                    if skips.len() > 8 {
                        msg.push_str(&format!("…and {} more (see log)\n", skips.len() - 8));
                    }
                }
                msg.push_str("\nProceed?");
                if !confirm_plan_dialog("Keyfire — Sort Files", &msg) {
                    info!("[Keyfire] Sort Files: cancelled at the plan dialog");
                    return true;
                }
            }

            // ── Execute + report ────────────────────────────────────────
            for (name, reason) in &skips {
                info!("[Keyfire] Sort Files skip: {} — {}", name, reason);
            }
            match crate::shell_files::perform_moves(&moves, silent_overwrite) {
                Ok(n) => {
                    info!(
                        "[Keyfire] Sort Files: {} file(s) sorted, {} skipped",
                        n,
                        skips.len()
                    );
                    if confirm {
                        let mut msg = format!("Moved {} file(s).", n);
                        if !skips.is_empty() {
                            msg.push_str(&format!(" Skipped {} (see log).", skips.len()));
                        }
                        info_dialog("Keyfire — Sort Files", &msg);
                    }
                }
                Err(e) => {
                    warn!("[Keyfire] Sort Files: {} — aborting macro", e);
                    return false;
                }
            }
        }

        // Play Audio File / Play Video File — shell-open the file via the OS
        // default handler (Windows Media Player, Groove, VLC, whatever's
        // associated). Uses LaunchKind::App with kind="path" so the launcher
        // captures the player's PID and applies the standard 400ms
        // restore-race delay before moving to the target monitor. Existing
        // player instances that reuse their window (Groove typically does
        // this for playlist adds) may not surface a new window, in which
        // case the monitor target is a no-op.
        "Play Audio File" | "Play Video File" => {
            if step_value.is_empty() {
                warn!("[Keyfire] {} step: empty value", step_type);
                return true;
            }
            let parsed: Value = match serde_json::from_str(step_value) {
                Ok(v) => v,
                Err(e) => {
                    warn!("[Keyfire] {} step: invalid JSON: {}", step_type, e);
                    return true;
                }
            };
            let path = parsed.get("path").and_then(|v| v.as_str()).unwrap_or("");
            if path.is_empty() {
                warn!("[Keyfire] {}: empty path", step_type);
                return true;
            }
            let monitor = crate::window_target::parse_monitor_target(Some(&parsed), *target_hwnd);
            let rx = crate::window_target::launch_with_monitor_target(
                crate::window_target::LaunchKind::App { kind: "path", path, app_id: "", args: "" },
                monitor,
            );
            if let Some(rx) = rx {
                let _ = rx.recv_timeout(Duration::from_secs(5));
            }
            info!("[Keyfire] {}: {}", step_type, path);
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
            let kind = parsed.get("kind").and_then(|v| v.as_str()).unwrap_or("path");
            let app_id = parsed.get("appId").and_then(|v| v.as_str()).unwrap_or("");
            let path = parsed.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let args = parsed.get("args").and_then(|v| v.as_str()).unwrap_or("");
            let monitor = crate::window_target::parse_monitor_target(Some(&parsed), *target_hwnd);
            let rx = crate::window_target::launch_with_monitor_target(
                crate::window_target::LaunchKind::App { kind, path, app_id, args },
                monitor,
            );
            // See Open Folder comment above — same rationale, same 5s ceiling.
            if let Some(rx) = rx {
                let _ = rx.recv_timeout(Duration::from_secs(5));
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
            if process.is_empty() && title.is_empty() {
                warn!("[Keyfire] Focus Window step: both process and title are empty");
                return true;
            }
            match find_window_by_criteria(process, title) {
                Some(hwnd) => {
                    let (_, _, fg_settle_ms, _) = speed_delays();
                    // Skip the focus dance entirely if this window is already
                    // foreground. Otherwise SetForegroundWindow + BringWindowToTop
                    // + AttachThreadInput causes a visible flicker each time —
                    // and a distilled macro can fire Focus Window many times.
                    let already_focused = unsafe { GetForegroundWindow() as isize == hwnd };
                    if already_focused {
                        *target_hwnd = hwnd;
                        info!("[Keyfire] Focus Window: HWND {} already foreground — skipping", hwnd);
                    } else {
                        // SW_RESTORE ONLY if minimised — otherwise it un-maximises
                        // maximised windows. IsIconic gates it correctly.
                        unsafe {
                            if IsIconic(hwnd as _) != 0 {
                                ShowWindow(hwnd as _, SW_RESTORE);
                            }
                        }
                        let ok = set_foreground_robust(hwnd);
                        let settle = fg_settle_ms.max(20) * 3;
                        thread::sleep(Duration::from_millis(settle));
                        *target_hwnd = hwnd;
                        info!("[Keyfire] Focus Window: HWND {} (ok={}, settle={}ms, process='{}' title='{}')", hwnd, ok, settle, process, title);
                    }
                }
                None => {
                    warn!("[Keyfire] Focus Window: no matching window found for process='{}' title='{}'", process, title);
                }
            }
        }

        // Minimise / Maximise Window — resolve target via
        // resolve_window_target (process/title → EnumWindows match, else
        // fall through to *target_hwnd or GetForegroundWindow). Empty
        // process AND empty title = "the currently focused window", which
        // is the natural default for a bare-minimise hotkey binding.
        "Minimise Window" | "Maximise Window" => {
            let parsed: Value = if step_value.trim().is_empty() {
                serde_json::json!({})
            } else {
                match serde_json::from_str(step_value) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[Keyfire] {} step: invalid JSON: {}", step_type, e);
                        return true;
                    }
                }
            };
            let hwnd = resolve_window_target(&parsed, *target_hwnd);
            if hwnd == 0 {
                warn!("[Keyfire] {}: no matching window found", step_type);
                return true;
            }
            let cmd = if step_type == "Minimise Window" { SW_MINIMIZE } else { SW_MAXIMIZE };
            unsafe { ShowWindow(hwnd as _, cmd); }
            info!("[Keyfire] {}: HWND {:x}", step_type, hwnd as usize);
        }

        // Resize Window — same target-resolution + width/height (+ optional
        // x/y). SW_RESTORE first so minimised/maximised windows show the new
        // size immediately (SetWindowPos on a minimised window updates the
        // "restored" placement metadata but the window stays minimised
        // visually).
        "Resize Window" => {
            let parsed: Value = if step_value.trim().is_empty() {
                serde_json::json!({})
            } else {
                match serde_json::from_str(step_value) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("[Keyfire] Resize Window step: invalid JSON: {}", e);
                        return true;
                    }
                }
            };
            let hwnd = resolve_window_target(&parsed, *target_hwnd);
            if hwnd == 0 {
                warn!("[Keyfire] Resize Window: no matching window found");
                return true;
            }
            let width = parsed.get("width").and_then(|v| v.as_i64()).unwrap_or(1200).clamp(100, 10000) as i32;
            let height = parsed.get("height").and_then(|v| v.as_i64()).unwrap_or(800).clamp(100, 10000) as i32;
            let use_position = parsed.get("usePosition").and_then(|v| v.as_bool()).unwrap_or(false);
            let x = parsed.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let y = parsed.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            unsafe {
                ShowWindow(hwnd as _, SW_RESTORE);
                let mut flags = SWP_NOZORDER | SWP_NOACTIVATE;
                if !use_position { flags |= SWP_NOMOVE; }
                SetWindowPos(hwnd as _, std::ptr::null_mut(), x, y, width, height, flags);
            }
            info!(
                "[Keyfire] Resize Window: HWND {:x} → {}x{}{}",
                hwnd as usize, width, height,
                if use_position { format!(" at ({}, {})", x, y) } else { String::new() }
            );
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
                    warn!("[Keyfire] Run AHK Script step: invalid JSON");
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
                    // Recorded press duration (distilled recordings). 0/absent
                    // = plain click. Above the threshold we press-hold-release
                    // so long-press UI and games replay faithfully.
                    let hold_ms = parsed.get("holdMs").and_then(|v| v.as_u64()).unwrap_or(0);
                    // Drag end point (distilled drags). Present = replay as a
                    // real drag: down at (x,y), interpolated moves, up here.
                    let drag_to = match (
                        parsed.get("dragToX").and_then(|v| v.as_i64()),
                        parsed.get("dragToY").and_then(|v| v.as_i64()),
                    ) {
                        (Some(dx), Some(dy)) => Some((dx as i32, dy as i32)),
                        _ => None,
                    };
                    // Modifiers held during the recorded click (Shift+drag to
                    // constrain to a straight line, Ctrl+click multi-select…).
                    // Pressed before the button-down, released after the up.
                    // Name→VK mirrors distill::modifier_vk_names.
                    let step_mods: Vec<u16> = parsed
                        .get("modifiers")
                        .and_then(|v| v.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|m| match m.as_str() {
                                    Some("Ctrl") => Some(0xA2u16),  // VK_LCONTROL
                                    Some("Alt") => Some(0xA4u16),   // VK_LMENU
                                    Some("Shift") => Some(0xA0u16), // VK_LSHIFT
                                    Some("Win") => Some(0x5Bu16),   // VK_LWIN
                                    _ => None,
                                })
                                .collect()
                        })
                        .unwrap_or_default();

                    let (abs_x, abs_y) = if mode == "windowClient" {
                        // Pro-gated: client-relative coords + stored target window
                        // identity. Free tier falls back to the `fallbackX/Y` absolute
                        // coords the distiller stored alongside — never the (x,y)
                        // fields, which are client-rel and would land off-screen.
                        let fx = parsed.get("fallbackX").and_then(|v| v.as_i64()).unwrap_or(x as i64) as i32;
                        let fy = parsed.get("fallbackY").and_then(|v| v.as_i64()).unwrap_or(y as i64) as i32;
                        if !crate::licence::is_pro() {
                            info!("[Keyfire] Click at Position: windowClient mode requires Pro — using fallback absolute ({}, {})", fx, fy);
                            (fx, fy)
                        } else if let Some(tw_json) = parsed.get("targetWindow") {
                            let target = crate::distill::TargetWindow {
                                title: tw_json.get("title").and_then(|v| v.as_str()).unwrap_or("").into(),
                                exe:   tw_json.get("exe").and_then(|v| v.as_str()).unwrap_or("").into(),
                                class: tw_json.get("class").and_then(|v| v.as_str()).unwrap_or("").into(),
                                client_w: tw_json.get("clientW").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                                client_h: tw_json.get("clientH").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                            };
                            match crate::distill::resolve_target_window(&target) {
                                Some(hwnd) => {
                                    // Skip focus if this window is already the
                                    // foreground — avoids a per-click flicker
                                    // when a macro fires many clicks against the
                                    // same window.
                                    let already_focused = unsafe { GetForegroundWindow() as isize == hwnd };
                                    if !already_focused {
                                        unsafe {
                                            if IsIconic(hwnd as _) != 0 {
                                                ShowWindow(hwnd as _, SW_RESTORE);
                                            }
                                        }
                                        set_foreground_robust(hwnd);
                                        thread::sleep(Duration::from_millis(80));
                                    }

                                    // Resize handling: "proportional" (default)
                                    // now uses anchor-by-closest-edge per axis
                                    // — sidebars/toolbars/scrollbars anchor to
                                    // their nearest edge, content-area clicks
                                    // fall back to proportional. Empirically
                                    // handles Slack/VSCode/Chrome/Outlook far
                                    // better than pure ratio scaling. "static"
                                    // skips the transform entirely (fixed-anchor
                                    // dialogs opt out).
                                    let behavior = parsed.get("resizeBehavior")
                                        .and_then(|v| v.as_str())
                                        .unwrap_or("proportional");
                                    let (click_x, click_y) = if behavior == "proportional"
                                        && target.client_w > 0
                                        && target.client_h > 0
                                    {
                                        let mut cr = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
                                        let ok = unsafe {
                                            windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd as _, &mut cr)
                                        };
                                        if ok != 0 {
                                            let cur_w = (cr.right - cr.left).max(1);
                                            let cur_h = (cr.bottom - cr.top).max(1);
                                            let (cx, ax) = anchor_transform_axis(
                                                x, target.client_w, cur_w, "x",
                                            );
                                            let (cy, ay) = anchor_transform_axis(
                                                y, target.client_h, cur_h, "y",
                                            );
                                            info!(
                                                "[Keyfire] Click at Position: anchor xy=({}/{},{}/{}) ({},{})→({},{}) [rec {}×{}, live {}×{}]",
                                                ax, x, ay, y, x, y, cx, cy, target.client_w, target.client_h, cur_w, cur_h
                                            );
                                            (cx, cy)
                                        } else {
                                            (x, y)
                                        }
                                    } else {
                                        (x, y)
                                    };

                                    match crate::distill::client_to_screen(hwnd, click_x, click_y) {
                                        Some((sx, sy)) => (sx, sy),
                                        None => {
                                            warn!("[Keyfire] Click at Position: ClientToScreen failed, using fallback ({}, {})", fx, fy);
                                            (fx, fy)
                                        }
                                    }
                                }
                                None => {
                                    warn!("[Keyfire] Click at Position: target window '{}' ({}) not found, using fallback ({}, {})", target.title, target.exe, fx, fy);
                                    (fx, fy)
                                }
                            }
                        } else {
                            warn!("[Keyfire] Click at Position: windowClient mode without targetWindow — using fallback");
                            (fx, fy)
                        }
                    } else if mode == "relative" {
                        // Relative to target window
                        let mut rect = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
                        unsafe {
                            windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(*target_hwnd as _, &mut rect);
                        }
                        (rect.left + x, rect.top + y)
                    } else {
                        (x, y)
                    };

                    info!("[Keyfire] Click at Position: ({}, {}) mode={} button={}", abs_x, abs_y, mode, button);

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

                    // Press recorded modifiers before the button action so
                    // Shift+drag (straight-line constraint), Ctrl+click etc.
                    // reach the app with the modifier held. Released after
                    // the button-up below — ALWAYS, so keys can never stick.
                    if !step_mods.is_empty() {
                        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        for &vk in &step_mods {
                            send_vk_key(vk, false);
                        }
                        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                        thread::sleep(Duration::from_millis(15));
                    }

                    // Click — three shapes, most specific first:
                    //   drag:  down at (x,y) → interpolated REAL mouse moves
                    //          (SendInput MOVE, never SetCursorPos — apps
                    //          detect drags by tracking WM_MOUSEMOVE between
                    //          down and up, see the v0.6.3 raw-replay fix) →
                    //          up at the drag end point.
                    //   hold:  press-hold-release when the recording captured
                    //          a hold longer than a normal click.
                    //   plain: everything else.
                    // 150ms cutoff: everyday clicks are 60-120ms and should
                    // stay snappy; anything longer was deliberate. Durations
                    // capped at 10s. Esc / macros-disabled shortens a hold or
                    // drag but the button-up ALWAYS fires — a stuck mouse
                    // button is never acceptable.
                    const HOLD_THRESHOLD_MS: u64 = 150;
                    if let Some((to_x, to_y)) = drag_to {
                        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        replay_mouse_move(abs_x, abs_y);
                        send_mouse_event(click_button, false); // down
                        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                        let total_ms = hold_ms.clamp(200, 10_000);
                        const DRAG_STEPS: i64 = 16;
                        let step_sleep = (total_ms / DRAG_STEPS as u64).max(10);
                        for i in 1..=DRAG_STEPS {
                            if ESC_LOOP_BREAK.load(Ordering::SeqCst)
                                || !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst)
                            {
                                break;
                            }
                            let fx = abs_x + ((to_x - abs_x) as i64 * i / DRAG_STEPS) as i32;
                            let fy = abs_y + ((to_y - abs_y) as i64 * i / DRAG_STEPS) as i32;
                            replay_mouse_move(fx, fy);
                            thread::sleep(Duration::from_millis(step_sleep));
                        }
                        // Land exactly on the end point, then release —
                        // unconditional so the button can never stick.
                        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        replay_mouse_move(to_x, to_y);
                        send_mouse_event(click_button, true); // up
                        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                    } else if hold_ms > HOLD_THRESHOLD_MS {
                        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        send_mouse_event(click_button, false); // down
                        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                        let total = hold_ms.min(10_000);
                        let mut waited = 0u64;
                        while waited < total {
                            if ESC_LOOP_BREAK.load(Ordering::SeqCst)
                                || !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst)
                            {
                                break;
                            }
                            let chunk = 50.min(total - waited);
                            thread::sleep(Duration::from_millis(chunk));
                            waited += chunk;
                        }
                        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        send_mouse_event(click_button, true); // up — unconditional
                        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                    } else {
                        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        send_mouse_click(click_button);
                        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                    }

                    // Release recorded modifiers (reverse order) — mirror of
                    // the press above. Unconditional: even if the hold/drag
                    // was cut short by Esc, held modifiers must not survive.
                    if !step_mods.is_empty() {
                        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                        for &vk in step_mods.iter().rev() {
                            send_vk_key(vk, true);
                        }
                        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
                    }

                    // Restore cursor to original position
                    thread::sleep(Duration::from_millis(20));
                    unsafe {
                        windows_sys::Win32::UI::WindowsAndMessaging::SetCursorPos(original_pos.x, original_pos.y);
                    }
                } else {
                    warn!("[Keyfire] Click at Position: invalid JSON");
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
                    if settle_ms > 0 { thread::sleep(Duration::from_millis(settle_ms)); }
                    // is_bare=false, is_altgr=false — we're firing programmatically,
                    // not from a real keypress, so no dead character to erase and no
                    // bare-key handling needed. trigger_key=None for the same reason.
                    execute_action(&target_macro, false, *target_hwnd, false, None, app);
                }
                None => {
                    warn!("[Keyfire] Fire Trigger: assignment \"{}\" not found, skipping", step_value);
                }
            }
        }

        // Fire an existing text expansion by trigger word. Routes through the
        // shared dispatch in expansions.rs which honours variants / fill-in /
        // image / tokens / case patterns — same fire paths the space-trigger
        // and immediate-trigger entry points use, so parity is automatic.
        "Fire Text Expansion" => {
            if step_value.is_empty() {
                warn!("[Keyfire] Fire Text Expansion: empty step value, skipping");
                return true;
            }
            if settle_ms > 0 { thread::sleep(Duration::from_millis(settle_ms)); }
            crate::expansions::fire_expansion_by_trigger(step_value);
        }

        // Phase 1 macro recorder — literal replay of a captured event stream.
        // Plays back exactly what was recorded with the same inter-event gaps,
        // capped at MAX_GAP_MS so absurd waits (clock drift, broken JSON) can't
        // freeze the macro forever. SUPPRESS_SIMULATED is held for the whole
        // replay so our injected events don't bounce back into Keyfire's hook
        // processing and fire other assignments. Captured coordinates are
        // absolute screen pixels — Phase 2 will introduce window-relative
        // coords + a Focus Window step in front of clicks targeted at a
        // specific window.
        "Record Macro" => {
            if step_value.is_empty() {
                warn!("[Keyfire] Record Macro: empty step value, skipping");
                return true;
            }
            // Phase 2: value can be either a bare Vec<RecordedEvent> (legacy)
            // or a wrapper object with events + distilled + playbackMode +
            // targetApp. parse_record_macro_value handles both shapes.
            let value = match crate::distill::parse_record_macro_value(step_value) {
                Some(v) => v,
                None => {
                    warn!("[Keyfire] Record Macro: invalid step value JSON");
                    return true;
                }
            };
            let use_distilled = value.playback_mode == "distilled"
                && value.distilled.as_ref().map(|s| !s.is_empty()).unwrap_or(false);

            if !use_distilled {
                replay_recorded_events(&value.events, "Record Macro");
                return true;
            }

            // Distilled playback is Pro-gated. Fall back to raw (absolute-only)
            // for free tier — cannot bypass by hand-editing playback_mode.
            if !crate::licence::is_pro() {
                info!("[Keyfire] Record Macro: distilled playback is Pro — using raw replay");
                replay_recorded_events(&value.events, "Record Macro (free-tier raw)");
                return true;
            }

            // target_app precheck. Read only from the wrapper's stored target_app.
            // The distiller populates this at distil time and the Clear button
            // in the editor sets it to null when the user wants no binding.
            //
            // No fallback to event-stream extraction here: that fallback was
            // broken for macros whose target app is transient (opened BY the
            // macro's first steps, like Search or a new Explorer window), and
            // it silently overrode the user's Clear action.
            let effective_target = if value.disable_target_binding {
                info!("[Keyfire] Record Macro: target binding cleared by user — no precheck");
                None
            } else {
                value.target_app.clone()
            };

            match &effective_target {
                Some(target) => {
                    let fake_tw = crate::distill::TargetWindow {
                        title: target.window_title_hint.clone().unwrap_or_default(),
                        exe: target.exe.clone(),
                        class: String::new(),
                        client_w: 0,
                        client_h: 0,
                    };
                    match crate::distill::resolve_target_window(&fake_tw) {
                        Some(hwnd) => {
                            info!(
                                "[Keyfire] Record Macro: target app '{}' found (hwnd=0x{:x}) — proceeding",
                                target.exe, hwnd
                            );
                        }
                        None => {
                            warn!(
                                "[Keyfire] Record Macro: target app '{}' (hint='{}') not found — aborting + emitting record-macro-app-missing",
                                target.exe,
                                target.window_title_hint.as_deref().unwrap_or("")
                            );
                            let _ = app.emit(
                                "record-macro-app-missing",
                                serde_json::json!({
                                    "exe": target.exe,
                                    "hint": target.window_title_hint,
                                }),
                            );
                            return true;
                        }
                    }
                }
                None => {
                    info!("[Keyfire] Record Macro: no target_app in wrapper or events — no precheck");
                }
            }

            // Option C: distilled steps ARE manual macro steps. Walk the array
            // and recurse into execute_macro_step so every existing arm
            // (Type Text, Press Key, Click at Position, Focus Window, Wait,
            // Mouse Scroll) runs identically to a hand-built sequence.
            //
            // When there's no target app (user cleared the binding), we ALSO
            // neutralize per-step window references: Focus Window steps are
            // skipped entirely, and Click at Position steps in windowClient
            // mode are rewritten to plain absolute mode using their fallback
            // coords. Otherwise a live Explorer window resolved on the current
            // desktop can be dramatically larger than the recorded one, and
            // the anchor-by-edge transform then places clicks hundreds of
            // pixels away from where the user actually meant them to land.
            let distilled = value.distilled.as_ref().unwrap();
            let strip_window_refs = effective_target.is_none();
            info!(
                "[Keyfire] Record Macro: replaying {} distilled step(s) (strip_window_refs={})",
                distilled.len(), strip_window_refs
            );
            for (i, dstep) in distilled.iter().enumerate() {
                if !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
                    info!("[Keyfire] Record Macro: aborted (macros disabled) at step {}", i);
                    break;
                }
                if ESC_LOOP_BREAK.load(Ordering::SeqCst) {
                    info!("[Keyfire] Record Macro: aborted (Esc) at step {}", i);
                    break;
                }
                // Optionally rewrite the step to strip window-specific behaviour.
                // Cow so we only clone the step JSON when the rewrite kicks in.
                let effective_step: std::borrow::Cow<'_, Value> = if strip_window_refs {
                    let step_type = dstep.get("type").and_then(|v| v.as_str()).unwrap_or("");
                    if step_type == "Focus Window" {
                        info!("[Keyfire] Record Macro: skipping Focus Window step {} (no target binding)", i);
                        continue;
                    }
                    if step_type == "Click at Position" {
                        match neutralize_click_to_absolute(dstep) {
                            Some(v) => std::borrow::Cow::Owned(v),
                            None => std::borrow::Cow::Borrowed(dstep),
                        }
                    } else {
                        std::borrow::Cow::Borrowed(dstep)
                    }
                } else {
                    std::borrow::Cow::Borrowed(dstep)
                };
                if !execute_macro_step(&effective_step, target_hwnd, method, app) {
                    info!("[Keyfire] Record Macro: step {} requested abort", i);
                    break;
                }
            }
        }

        // System group: no-config leaves + destructive-with-prompt.
        // VKs inlined: 0x4D = M. Win+M / Win+Shift+M are the classic
        // Windows chords for "minimise everything" and "undo minimise all".

        "Minimise All" => {
            send_win_chord(0x4D, false); // Win+M
        }

        "Restore All" => {
            send_win_chord(0x4D, true); // Win+Shift+M
        }

        "Lock Computer" => {
            unsafe { LockWorkStation(); }
        }

        "Sleep Computer" => {
            if !confirm_destructive_step(
                "Keyfire — Sleep Computer",
                "Put this computer to sleep now?",
            ) {
                info!("[Keyfire] Sleep Computer cancelled by user");
                return true;
            }
            // powrprof.dll's SetSuspendState is the standard entry point.
            // Args: hibernate=0, force=1, wakeup-events-disabled=0.
            let _ = std::process::Command::new("rundll32.exe")
                .args(["powrprof.dll,SetSuspendState", "0,1,0"])
                .spawn();
        }

        "Log Off" => {
            if !confirm_destructive_step(
                "Keyfire — Log Off",
                "Log off this session now?\n\nAny unsaved work will be lost.",
            ) {
                info!("[Keyfire] Log Off cancelled by user");
                return true;
            }
            unsafe {
                ExitWindowsEx(
                    EWX_LOGOFF | EWX_FORCEIFHUNG,
                    SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER
                        | SHTDN_REASON_FLAG_USER_DEFINED,
                );
            }
        }

        "Shut Down Computer" => {
            if !confirm_destructive_step(
                "Keyfire — Shut Down",
                "Shut down this computer now?\n\nAny unsaved work will be lost.",
            ) {
                info!("[Keyfire] Shut Down cancelled by user");
                return true;
            }
            unsafe {
                ExitWindowsEx(
                    EWX_SHUTDOWN | EWX_FORCEIFHUNG,
                    SHTDN_REASON_MAJOR_OTHER | SHTDN_REASON_MINOR_OTHER
                        | SHTDN_REASON_FLAG_USER_DEFINED,
                );
            }
        }

        "Control Panel" => {
            let _ = std::process::Command::new("control.exe").spawn();
        }

        // Change Volume — exact system volume control via IAudioEndpointVolume
        // COM (see crate::volume). Value is JSON `{ mode, amount }`:
        //   mode="set", amount=0-100  → SetMasterVolumeLevelScalar(amount/100)
        //   mode="increase", amount=0-10 → read + add amount + set (clamped 0-100)
        //   mode="decrease", amount=0-10 → read + subtract amount + set
        //   mode="mute"               → toggle mute (read + invert + set)
        // Backward-compat: legacy plain-string values ("up"/"down"/"mute") map
        // to increase-by-2 / decrease-by-2 / mute so pre-rewrite macros keep
        // working. repeat_count multiplies the delta — repeat=5 with amount=5
        // increases by 25 (still clamped 0-100).
        "Change Volume" => {
            let (mode, amount) = if step_value.trim_start().starts_with('{') {
                match serde_json::from_str::<Value>(step_value) {
                    Ok(parsed) => {
                        let m = parsed.get("mode").and_then(|v| v.as_str()).unwrap_or("increase").to_string();
                        let a = parsed.get("amount").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                        (m, a)
                    }
                    Err(_) => ("increase".to_string(), 2),
                }
            } else {
                match step_value {
                    "up"   => ("increase".to_string(), 2),
                    "down" => ("decrease".to_string(), 2),
                    "mute" => ("mute".to_string(), 0),
                    other  => (other.to_string(), 2),
                }
            };
            let clamped = amount.clamp(0, 100);
            let scalar = (clamped as f32) / 100.0;
            // VK constants for media keys — 0xAD=MUTE, 0xAE=DOWN, 0xAF=UP.
            // We send a media keystroke to trigger the Windows volume OSD
            // overlay (same one you get with the FN keys), then COM-set to
            // the exact target so the OSD ends showing the correct value.
            // For Increase/Decrease we compute the target BEFORE the
            // keystroke so we don't cascade the keystroke's ~2-unit nudge
            // into the delta.
            match mode.as_str() {
                "set" => {
                    let cur = crate::volume::get_master_volume_scalar().unwrap_or(0.5);
                    let vk = if scalar >= cur { 0xAF } else { 0xAE };
                    send_media_key(vk);
                    crate::volume::set_master_volume_scalar(scalar);
                }
                "increase" => {
                    let cur = crate::volume::get_master_volume_scalar().unwrap_or(0.5);
                    let target = (cur + (clamped as f32) / 100.0).clamp(0.0, 1.0);
                    send_media_key(0xAF);
                    crate::volume::set_master_volume_scalar(target);
                }
                "decrease" => {
                    let cur = crate::volume::get_master_volume_scalar().unwrap_or(0.5);
                    let target = (cur - (clamped as f32) / 100.0).clamp(0.0, 1.0);
                    send_media_key(0xAE);
                    crate::volume::set_master_volume_scalar(target);
                }
                "mute" => {
                    // Native VK toggles OS mute + shows OSD; no COM needed.
                    send_media_key(0xAD);
                }
                other => {
                    warn!("[Keyfire] Change Volume: unknown mode '{}'", other);
                }
            }
        }

        // Change Audio Output — set the default render endpoint to the pinned
        // device via IPolicyConfig (see crate::audio_devices). Value is JSON
        // `{ deviceId: string, deviceName: string }` — deviceId is Windows'
        // stable endpoint ID; deviceName is display-only for the toast.
        //
        // Success: log, let Windows show its own default-device change bubble.
        // Missing device (unplugged/disabled): toast the user via `system-action-toast`
        // event — App.jsx listens and pipes it through showNotification. Other
        // failure modes (COM broken, IPolicyConfig refused) log only; they're
        // catastrophic and not the user's problem.
        "Change Audio Output" => {
            let parsed: Value = match serde_json::from_str(step_value) {
                Ok(v) => v,
                Err(e) => {
                    warn!("[Keyfire] Change Audio Output: bad JSON \"{}\": {}", step_value, e);
                    return true;
                }
            };
            let device_id = parsed.get("deviceId").and_then(|v| v.as_str()).unwrap_or("");
            let device_name = parsed.get("deviceName").and_then(|v| v.as_str()).unwrap_or("(unnamed)");
            if device_id.is_empty() {
                warn!("[Keyfire] Change Audio Output: no device pinned — step is a no-op");
                return true;
            }
            match crate::audio_devices::set_default_output_device(device_id) {
                Ok(name) => {
                    info!("[Keyfire] Change Audio Output: switched to \"{}\"", name);
                }
                Err(e) => {
                    warn!("[Keyfire] Change Audio Output: {}", e);
                    if matches!(e, crate::audio_devices::SetOutputError::DeviceNotFound(_)) {
                        use tauri::Emitter;
                        let _ = app.emit("system-action-toast", serde_json::json!({
                            "level": "error",
                            "message": format!("Audio device \"{}\" not connected", device_name),
                        }));
                    }
                }
            }
        }

        // Mouse Scroll — SendInput with MOUSEEVENTF_WHEEL / _HWHEEL. Value is
        // JSON `{ direction: "up|down|left|right", amount: <notches> }`. Each
        // notch = WHEEL_DELTA (120). Amount defaults to 3 if omitted. repeat
        // fires the scroll gesture multiple times (with settle between);
        // amount is notches PER gesture — the two multiply.
        "Mouse Scroll" => {
            const WHEEL_DELTA: i32 = 120;
            let parsed: Value = match serde_json::from_str(step_value) {
                Ok(v) => v,
                Err(_) => {
                    // Backward-compat / empty value: default to 3 notches down.
                    serde_json::json!({ "direction": "down", "amount": 3 })
                }
            };
            let direction = parsed.get("direction").and_then(|v| v.as_str()).unwrap_or("down");
            let amount = parsed.get("amount").and_then(|v| v.as_i64()).unwrap_or(3).max(1).min(999) as i32;
            let (flag, delta) = match direction {
                "up"    => (MOUSEEVENTF_WHEEL,   WHEEL_DELTA * amount),
                "down"  => (MOUSEEVENTF_WHEEL,  -WHEEL_DELTA * amount),
                "right" => (MOUSEEVENTF_HWHEEL,  WHEEL_DELTA * amount),
                "left"  => (MOUSEEVENTF_HWHEEL, -WHEEL_DELTA * amount),
                other => {
                    warn!("[Keyfire] Mouse Scroll: unknown direction '{}'", other);
                    return true;
                }
            };
            let input = INPUT {
                r#type: INPUT_MOUSE,
                Anonymous: INPUT_0 {
                    mi: MOUSEINPUT {
                        dx: 0,
                        dy: 0,
                        mouseData: delta as u32,
                        dwFlags: flag,
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            };
            for i in 0..repeat_count {
                unsafe { SendInput(1, &input, std::mem::size_of::<INPUT>() as i32); }
                if i + 1 < repeat_count && settle_ms > 0 {
                    thread::sleep(Duration::from_millis(settle_ms));
                }
            }
        }

        _ => {
            warn!("[Keyfire] Unknown macro step type: {}", step_type);
        }
    }
    true
}

// ── Recorded-event replay ────────────────────────────────────────────────────

/// Replay a captured RecordedEvent stream via SendInput, preserving original
/// inter-event gaps (capped at MAX_GAP_MS so absurd waits can't freeze the
/// macro forever). INTENTIONALLY NO SuppressionGuard — replayed events fire
/// Keyfire's own hotkey assignments / text expansions / radial triggers.
/// Recursion bounded by FIRE_DEPTH inside execute_action. Esc cancels mid-
/// stream via ESC_LOOP_BREAK; macros-disabled also aborts. Always finishes
/// with a defensive modifier release so a buffer that ended mid-modifier-
/// press doesn't leave Ctrl/Shift/Alt/Win stuck down. Shared by the macro
/// step path AND the global temp-macro play hotkey.
pub fn replay_recorded_events(events: &[crate::recorder::RecordedEvent], label: &str) {
    info!("[Keyfire] {}: replaying {} events", label, events.len());

    let mut prev_t: u64 = 0;
    const MAX_GAP_MS: u64 = 5000;

    for evt in events.iter() {
        if !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst) {
            info!("[Keyfire] {}: aborted (macros disabled)", label);
            break;
        }
        if ESC_LOOP_BREAK.load(Ordering::SeqCst) {
            info!("[Keyfire] {}: aborted (Esc)", label);
            break;
        }
        let evt_t = match evt {
            crate::recorder::RecordedEvent::KeyDown { t, .. }
            | crate::recorder::RecordedEvent::KeyUp { t, .. }
            | crate::recorder::RecordedEvent::MouseDown { t, .. }
            | crate::recorder::RecordedEvent::MouseUp { t, .. }
            | crate::recorder::RecordedEvent::MouseMove { t, .. }
            | crate::recorder::RecordedEvent::Wheel { t, .. }
            | crate::recorder::RecordedEvent::ForegroundChanged { t, .. } => *t,
        };
        let gap = evt_t.saturating_sub(prev_t).min(MAX_GAP_MS);
        if gap > 0 {
            thread::sleep(Duration::from_millis(gap));
        }
        prev_t = evt_t;

        match evt {
            crate::recorder::RecordedEvent::KeyDown { vk, .. } => {
                send_vk_key(*vk as u16, false);
            }
            crate::recorder::RecordedEvent::KeyUp { vk, .. } => {
                send_vk_key(*vk as u16, true);
            }
            crate::recorder::RecordedEvent::MouseDown { button, x, y, .. } => {
                // Real WM_MOUSEMOVE first so apps see the cursor arrive at
                // the click target, then the button-down. SendInput preserves
                // event ordering so this is a clean move-then-down sequence.
                replay_mouse_move(*x, *y);
                replay_mouse_button(button, true);
            }
            crate::recorder::RecordedEvent::MouseUp { button, x, y, .. } => {
                // Ensure cursor is at the release point (via a real move
                // message) before the button-up fires, so the OS reports the
                // up coords consistently with what the recording captured.
                replay_mouse_move(*x, *y);
                replay_mouse_button(button, false);
            }
            crate::recorder::RecordedEvent::MouseMove { x, y, .. } => {
                // SendInput-with-MOVE — NOT SetCursorPos alone — so apps under
                // the cursor receive WM_MOUSEMOVE messages and detect drags
                // between LBUTTONDOWN and LBUTTONUP. See replay_mouse_move().
                replay_mouse_move(*x, *y);
            }
            crate::recorder::RecordedEvent::Wheel { delta, x, y, .. } => {
                replay_mouse_move(*x, *y);
                replay_wheel(*delta);
            }
            // ForegroundChanged is metadata for Phase 2 distillation, not a
            // replayable action. Raw-mode replay ignores it — the recording's
            // input events already land in whatever window is foreground at
            // replay time.
            crate::recorder::RecordedEvent::ForegroundChanged { .. } => {}
        }
    }
    // Defensive cleanup: release all modifiers. A buffer that ended mid-
    // modifier-press would otherwise leave the OS with stuck modifiers
    // (every subsequent keypress garbled). Keyup on already-up key is a
    // harmless no-op.
    const VK_LSHIFT: u16 = 0xA0;
    const VK_RSHIFT: u16 = 0xA1;
    const VK_LCTRL: u16 = 0xA2;
    const VK_RCTRL: u16 = 0xA3;
    const VK_LALT: u16 = 0xA4;
    const VK_RALT: u16 = 0xA5;
    const VK_LWIN: u16 = 0x5B;
    const VK_RWIN: u16 = 0x5C;
    for vk in [VK_LSHIFT, VK_RSHIFT, VK_LCTRL, VK_RCTRL, VK_LALT, VK_RALT, VK_LWIN, VK_RWIN] {
        send_vk_key(vk, true);
    }
    info!("[Keyfire] {}: complete", label);
}

/// Continuous-replay wrapper for the Quick Record temp macro. Runs
/// `replay_recorded_events` in a loop until the user presses the configured
/// Loop hotkey again, presses Esc (via the global ESC_LOOP_BREAK gate
/// per [[feedback_esc_global_macro_cancel]]), or disables macros entirely.
///
/// Inter-iteration pause polled in 100ms chunks per
/// [[feedback_polled_sleep_for_cancel]] so a stop signal mid-pause is honoured
/// without waiting the full 500ms.
/// Returns the number of iterations that ran (analytics credit at the call site).
pub fn replay_recorded_events_loop(events: &[crate::recorder::RecordedEvent], label: &str) -> u64 {
    crate::recorder::TEMP_MACRO_LOOP_ACTIVE.store(true, Ordering::SeqCst);
    info!("[Keyfire] {}: loop started", label);
    let mut iter: u64 = 0;
    while crate::recorder::TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst)
        && !ESC_LOOP_BREAK.load(Ordering::SeqCst)
        && crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst)
    {
        iter += 1;
        let iter_label = format!("{} (loop iter {})", label, iter);
        replay_recorded_events(events, &iter_label);
        // 500ms breathing room between iterations, polled cancellable.
        for _ in 0..5 {
            if !crate::recorder::TEMP_MACRO_LOOP_ACTIVE.load(Ordering::SeqCst)
                || ESC_LOOP_BREAK.load(Ordering::SeqCst)
                || !crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst)
            {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    crate::recorder::TEMP_MACRO_LOOP_ACTIVE.store(false, Ordering::SeqCst);
    info!("[Keyfire] {}: loop stopped after {} iter(s)", label, iter);
    iter
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
    let input = make_vk_input(vk, key_up);
    unsafe {
        SendInput(1, &input, std::mem::size_of::<INPUT>() as i32);
    }
}

fn send_vk_key_checked(vk: u16, key_up: bool) -> u32 {
    let input = make_vk_input(vk, key_up);
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

/// Re-press modifiers that were held at release time — but ONLY those the
/// user is still physically holding. Without this guard, fire-on-press leaves
/// the modifier stuck-down in the target app: fire dispatches at keydown
/// while Shift is held, we release it for the injection, and by the time we
/// reach restore the user has typically lifted their finger — re-pressing
/// then sends Shift-down to the app with no matching physical release.
pub fn restore_modifiers(held: &[u16]) {
    for &vk in held {
        if is_key_down(vk) {
            send_vk_key(vk, false);
        }
    }
}

/// Restore foreground to `hwnd` using the AttachThreadInput trick to defeat
/// Windows' cross-process SetForegroundWindow restrictions. Returns whether
/// SetForegroundWindow actually succeeded; on false the helper logs a
/// `[FOCUS-DIAG]` warn so the strip-cycle can grep for refusing apps. Mirrors
/// the tray.rs:show_window pattern but inverted (we want target_hwnd to become
/// foreground, not our own window).
pub fn set_foreground_robust(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe {
        let mut pid: u32 = 0;
        let target_tid = GetWindowThreadProcessId(hwnd as _, &mut pid);
        let current_tid = GetCurrentThreadId();

        let ok = if target_tid != 0 && target_tid != current_tid {
            let attached = AttachThreadInput(current_tid, target_tid, 1);
            let r = SetForegroundWindow(hwnd as _);
            let _ = BringWindowToTop(hwnd as _);
            if attached != 0 {
                AttachThreadInput(current_tid, target_tid, 0);
            }
            r != 0
        } else {
            SetForegroundWindow(hwnd as _) != 0
        };
        if !ok {
            log::warn!(
                "[Keyfire] [FOCUS-DIAG] SetForegroundWindow failed for hwnd 0x{:x}",
                hwnd
            );
        }
        ok
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
    warn!("[Keyfire] {} not found — AHK scripts will not execute", filename);
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
/// from existing .ahk files. Keyfire is the trigger, so any hotkey labels
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
        warn!("[Keyfire] AHK: failed to write temp script: {}", e);
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
            info!("[Keyfire] AHK: spawned process (pid: {})", child.id());
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
                        info!("[Keyfire] AHK: cleaned up stale process (pid: {})", pid);
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
            warn!("[Keyfire] AHK: failed to spawn process: {}", e);
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
        warn!("[Keyfire] AHK sync: failed to write temp script: {}", e);
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
            info!("[Keyfire] AHK sync: waiting for process (pid: {})", child.id());
            // Wait for process to finish (up to 60s for macro step context)
            match child.wait() {
                Ok(status) => {
                    info!("[Keyfire] AHK sync: process exited with {}", status);
                }
                Err(e) => {
                    warn!("[Keyfire] AHK sync: wait failed: {}", e);
                    let _ = child.kill();
                }
            }
            let _ = std::fs::remove_file(&script_path);
        }
        Err(e) => {
            warn!("[Keyfire] AHK sync: failed to spawn: {}", e);
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
        info!("[Keyfire] AHK: killed previous instance for key: {}", key);
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
        info!("[Keyfire] AHK: killed process for key: {}", key);
    }
    if count > 0 {
        info!("[Keyfire] AHK: cleaned up {} process(es) on quit", count);
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
        info!("[Keyfire] AHK: cleaned up {} stale script(s) from previous session", cleaned);
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
