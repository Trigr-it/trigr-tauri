use log::info;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager};

use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, EnumClipboardFormats, GetClipboardData, GetClipboardSequenceNumber,
    OpenClipboard, SetClipboardData, EmptyClipboard, RegisterClipboardFormatW,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

// windows-sys 0.59 omits GlobalFree from Win32::System::Memory. Declare it
// directly. Only used to free an HGLOBAL after SetClipboardData fails (rare;
// otherwise Windows takes ownership on success).
#[link(name = "kernel32")]
extern "system" {
    fn GlobalFree(hmem: *mut core::ffi::c_void) -> *mut core::ffi::c_void;
}
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE,
};

const MAX_BUFFER_LENGTH: usize = 50;
const VK_BACKSPACE: u16 = 0x08;
const VK_SPACE: u16 = 0x20;
const VK_LEFT: u16 = 0x25;
const VK_LSHIFT: u16 = 0xA0;
const VK_LCONTROL: u16 = 0xA2;
const VK_LALT: u16 = 0xA4;
const VK_LWIN: u16 = 0x5B;
const VK_INSERT: u16 = 0x2D;
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;

// ── Injection guard — ensures INJECTION_IN_PROGRESS is always cleared ──────

struct InjectionGuard;

impl InjectionGuard {
    fn new() -> Self {
        crate::hotkeys::mark_injection_start();
        crate::hotkeys::INJECTION_IN_PROGRESS
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}

impl Drop for InjectionGuard {
    fn drop(&mut self) {
        crate::hotkeys::INJECTION_IN_PROGRESS
            .store(false, std::sync::atomic::Ordering::SeqCst);
        crate::hotkeys::clear_injection_start();
    }
}

/// Truncated single-line preview of injected/written text for log lines.
/// 40 chars max, newlines flattened, so the log never floods or breaks lines.
pub(crate) fn log_preview(s: &str) -> String {
    let mut p: String = s.chars().take(40).collect();
    p = p.replace('\r', "").replace('\n', "\\n");
    if s.chars().count() > 40 {
        p.push_str("...");
    }
    p
}

/// Hard circuit breaker on expansion fire rate. If fires arrive faster than
/// any human could plausibly trigger them, the engine is being fed its own
/// output (feedback loop) — refuse to fire rather than flood the target app
/// and clipboard. 8 fires inside a rolling 2s window is well above legitimate
/// burst use (fast typist with short triggers ≈ 2-3/s) and cuts a runaway
/// (observed at ~34/s on 2026-06-04) within a fraction of a second.
fn fire_rate_ok(context: &str) -> bool {
    static FIRE_TIMES: std::sync::Mutex<Vec<std::time::Instant>> =
        std::sync::Mutex::new(Vec::new());
    let now = std::time::Instant::now();
    let mut times = FIRE_TIMES.lock().unwrap();
    times.retain(|t| now.duration_since(*t) < Duration::from_secs(2));
    if times.len() >= 8 {
        log::error!(
            "[Keyfire] FIRE-RATE BREAKER: {} fires in 2s — suppressing \"{}\" (feedback loop suspected)",
            times.len(),
            context
        );
        return false;
    }
    times.push(now);
    true
}

/// Replay keystrokes that were buffered while an injection was in progress,
/// then re-run trigger checks. Takes ownership of the injection guard so the
/// re-checks provably run AFTER it drops.
///
/// Two loop-prevention rules, both learned from the 2026-06-04 runaway:
/// 1. SUPPRESS_SIMULATED must stay true until the LL hook has drained the
///    replayed events. Clearing it immediately after SendInput let the tail
///    of the replay re-enter the injection buffer (the hook buffers when
///    INJECTION_IN_PROGRESS && !SUPPRESS_SIMULATED, and the guard is still
///    held here) and replay again — self-sustaining. Same bug class as the
///    v0.4.20 repeat-mode SUPPRESS_DRAIN_MS fix.
/// 2. check_immediate_triggers/check_space_trigger must run AFTER the guard
///    drops. Firing inside the guard made the new fire's wait-loop block on
///    this thread's own INJECTION_IN_PROGRESS — a self-deadlock only broken
///    by the 5s watchdog force-clear.
fn replay_buffered_and_recheck(guard: InjectionGuard) {
    let buffered: Vec<crate::hotkeys::BufferedKey> =
        crate::hotkeys::injection_buffer().lock().unwrap().drain(..).collect();

    if !buffered.is_empty() {
        log::info!("[Keyfire] Replaying {} buffered keystrokes", buffered.len());
        // The user outran the injection — the replayed keys land as plain
        // synthetic events, so the one-shot Backspace undo would miscount.
        // Drop it for this fire rather than corrupt text.
        disarm_undo();
        crate::hotkeys::SUPPRESS_SIMULATED
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for key in &buffered {
            send_vk_key(key.vk_code as u16, !key.is_keydown);
            thread::sleep(Duration::from_millis(2));
        }
        // Drain: give the LL hook time to process every replayed event while
        // SUPPRESS_SIMULATED is still true (rule 1 above).
        thread::sleep(Duration::from_millis(30));
        crate::hotkeys::SUPPRESS_SIMULATED
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }

    // Sync modifier atomics with actual physical key state after replay
    crate::hotkeys::sync_modifier_state_from_os();

    // Injection complete — release the guard BEFORE trigger re-checks (rule 2).
    drop(guard);

    if buffered.is_empty() {
        return;
    }

    // Feed replayed keystrokes into the expansion buffer and re-check triggers.
    let last_was_space = buffered.last()
        .map(|k| k.vk_code == 0x20 && k.is_keydown)
        .unwrap_or(false);
    for key in &buffered {
        if !key.is_keydown { continue; }
        if key.vk_code == 0x20 { continue; } // Space handled after loop
        if key.vk_code == 0x08 { buffer_pop(); continue; } // Backspace
        if key.vk_code == 0x0D || key.vk_code == 0x1B || key.vk_code == 0x09 {
            // Enter, Escape, Tab — clear buffer and stop
            buffer_clear();
            break;
        }
        if crate::hotkeys::is_modifier_vk(key.vk_code) { continue; }
        if let Some(ch) = crate::hotkeys::resolve_char(key.vk_code, key.scan_code) {
            buffer_push(ch);
            check_immediate_triggers();
        }
    }
    if last_was_space {
        check_space_trigger();
        buffer_clear();
    }
}

// ── App handle for fill-in IPC ──────────────────────────────────────────────

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

pub fn init_app_handle(handle: AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

// ── Fill-in response channel ───────────────────────────────────────────────

static FILL_IN_TX: OnceLock<Mutex<Option<mpsc::Sender<Option<HashMap<String, String>>>>>> =
    OnceLock::new();

pub fn fill_in_tx() -> &'static Mutex<Option<mpsc::Sender<Option<HashMap<String, String>>>>> {
    FILL_IN_TX.get_or_init(|| Mutex::new(None))
}

// ── Fill-in ready signal (renderer → Rust handshake) ───────────────────────

static FILL_IN_READY_TX: OnceLock<Mutex<Option<mpsc::Sender<()>>>> = OnceLock::new();

pub fn fill_in_ready_tx() -> &'static Mutex<Option<mpsc::Sender<()>>> {
    FILL_IN_READY_TX.get_or_init(|| Mutex::new(None))
}

// ── Fill-in shown signal (renderer → Rust "picker actually rendered") ──────
//
// Failure mode this catches: in dev, WebView2 sometimes doesn't fully wake
// from idle suspension, OR Vite HMR replaces the FillInWindow.jsx listener
// exactly when Rust emits `fill-in-show`. The JS side never renders the
// picker, no selection/cancel ever comes back, and Rust would sit blocked on
// the response recv_timeout for the full duration (previously 60s) with
// FILL_IN_ACTIVE=true — bricking every subsequent expansion in the meantime.
//
// Fix: after emitting `fill-in-show`, wait up to 2s for the JS side to
// invoke the `fill_in_shown_ack` Tauri command. If it doesn't, the picker
// didn't render — self-abort cleanly instead of hanging.
static FILL_IN_SHOWN_TX: OnceLock<Mutex<Option<mpsc::Sender<()>>>> = OnceLock::new();

pub fn fill_in_shown_tx() -> &'static Mutex<Option<mpsc::Sender<()>>> {
    FILL_IN_SHOWN_TX.get_or_init(|| Mutex::new(None))
}

/// Typed fill-in field. Parsed from `{fillIn:Label[:type[:options][:default=value]]}`.
/// Backward-compat: bare `{fillIn:Label}` parses as `FillInField { label, kind: "text", options: [], default: None }`.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct FillInField {
    pub label: String,
    /// One of: "text", "multiline", "dropdown", "checkbox", "number", "date".
    /// Unknown values fall back to "text" at render time.
    pub kind: String,
    /// Comma-separated values for `dropdown`. Empty for other kinds.
    pub options: Vec<String>,
    /// Default seed value. For `checkbox`, "yes"/"no". For `dropdown`, must match an option.
    pub default: Option<String>,
}

/// Parse the content between `{fillIn:` and `}` into a FillInField.
/// Grammar: `<label>[:<kind>[:<options>][:default=<value>]]`
/// - `<options>` is comma-separated, only meaningful for kind=dropdown
/// - `:default=` is always the last segment if present (so `default=` values can contain `:`)
/// - Labels MAY NOT contain `:` or `}` (documented limitation)
pub(crate) fn parse_fillin_token(content: &str) -> FillInField {
    // Extract trailing `:default=...` suffix first so values can contain colons
    let (head, default) = match content.rfind(":default=") {
        Some(idx) => {
            let val = content[idx + 9..].to_string();
            (&content[..idx], Some(val))
        }
        None => (content, None),
    };

    // Remaining grammar: `<label>[:<kind>[:<options>]]`
    let mut parts = head.splitn(3, ':');
    let label = parts.next().unwrap_or("").to_string();
    let kind_raw = parts.next().unwrap_or("").to_string();
    let options_raw = parts.next().unwrap_or("");

    // Normalise kind. Empty (legacy `{fillIn:Label}`) becomes "text".
    let kind = if kind_raw.is_empty() {
        "text".to_string()
    } else {
        kind_raw
    };

    let options: Vec<String> = if !options_raw.is_empty() && kind == "dropdown" {
        options_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        Vec::new()
    };

    FillInField { label, kind, options, default }
}

/// Extract every `{fillIn:...}` token's field spec from text, deduped by label
/// (first occurrence wins — if a label appears twice with different specs, the
/// first one's type/options/default are used).
fn extract_fill_in_fields(text: &str) -> Vec<FillInField> {
    let mut fields: Vec<FillInField> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{fillIn:") {
        let after = &rest[start + 8..];
        if let Some(end) = after.find('}') {
            let content = &after[..end];
            let field = parse_fillin_token(content);
            if !field.label.is_empty() && !fields.iter().any(|f| f.label == field.label) {
                fields.push(field);
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    fields
}

/// Substitute every `{fillIn:...}` token in `text` with the user-supplied value
/// keyed by the token's label. Scans the source per-token rather than per-value
/// so typed tokens (`{fillIn:Name:text:default=John}`) get matched alongside
/// legacy `{fillIn:Name}` — both resolve to the same value from `values["Name"]`.
fn resolve_fill_in_tokens(text: &str, values: &HashMap<String, String>) -> String {
    let mut result = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{fillIn:") {
        result.push_str(&rest[..start]);
        let after = &rest[start + 8..];
        if let Some(end) = after.find('}') {
            let content = &after[..end];
            let field = parse_fillin_token(content);
            let raw_value = values
                .get(&field.label)
                .cloned()
                .or_else(|| field.default.clone())
                .unwrap_or_default();
            // Date fill-ins come back in ISO YYYY-MM-DD format (the HTML5
            // date input's native shape). Reformat them on substitution to
            // the user's preferred display format from Settings → Date
            // Format. The raw ISO value stays in the values map so formula
            // tokens like {=dateadd(label, 7)} can still parse it.
            let value = if field.kind == "date" {
                format_date_for_display(&raw_value)
            } else {
                raw_value
            };
            result.push_str(&value);
            rest = &after[end + 1..];
        } else {
            // Unterminated — append the rest verbatim and stop
            result.push_str(&rest[start..]);
            return result;
        }
    }
    result.push_str(rest);
    result
}

/// Convert a YYYY-MM-DD ISO date string to the user's preferred display
/// format (DD/MM/YYYY, MM/DD/YYYY, YYYY-MM-DD). Returns the original string
/// unchanged if it doesn't parse as ISO (e.g. empty, already formatted, or
/// a fill-in default that wasn't a real date).
fn format_date_for_display(iso: &str) -> String {
    if iso.is_empty() { return String::new(); }
    let trimmed = iso.trim();
    let parsed = match chrono::NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return iso.to_string(),
    };
    let pattern = {
        let state = crate::hotkeys::engine_state().lock().unwrap();
        state.default_date_format.clone()
    };
    let chrono_fmt = match pattern.as_str() {
        "MM/DD/YYYY"  => "%m/%d/%Y",
        "YYYY-MM-DD"  => "%Y-%m-%d",
        "DD/MM/YY"    => "%d/%m/%y",
        "D MMMM YYYY" => "%-d %B %Y",
        _             => "%d/%m/%Y", // DD/MM/YYYY default
    };
    parsed.format(chrono_fmt).to_string()
}

// ── State ───────────────────────────────────────────────────────────────────

static EXPANSION_STATE: OnceLock<Mutex<ExpansionState>> = OnceLock::new();

fn state() -> &'static Mutex<ExpansionState> {
    EXPANSION_STATE.get_or_init(|| Mutex::new(ExpansionState::default()))
}

struct ExpansionState {
    buffer: String,
    assignments: HashMap<String, Value>,
    /// Lowercase exact triggers for space-mode expansions. Used by the LL hook
    /// to decide whether to pre-swallow a Space keystroke before it leaks to
    /// the target app. Rebuilt whenever assignments change.
    space_triggers: HashSet<String>,
    /// Lowercase misspellings from GLOBAL::AUTOCORRECT:: assignment keys.
    /// Rebuilt whenever assignments change — drives AUTOCORRECT_PENDING.
    autocorrect_words: HashSet<String>,
    /// Master autocorrect toggle (Pro). Gates the custom map, the built-in
    /// dictionary, and double-caps correction.
    autocorrect_enabled: bool,
    /// "Common typos" — the built-in starter dictionary sub-toggle.
    builtin_typos_enabled: bool,
    /// "Extended dictionary" — the bundled ~4k-entry Wikipedia list sub-toggle.
    extended_typos_enabled: bool,
    /// Double-caps sub-toggle: HEllo → Hello at word completion.
    double_caps_enabled: bool,
    /// Lowercase words exempt from double-caps AND the Caps Lock fix
    /// ("ids", "pcs", "tvs"...).
    double_caps_exceptions: HashSet<String>,
    /// Caps Lock accident sub-toggle: tHE → The when Caps Lock is
    /// physically on, plus a synthetic tap to switch Caps Lock off.
    caps_lock_fix_enabled: bool,
    /// Sentence-capitalization sub-toggle: lowercase word starts get
    /// capitalized when the previous word ended a sentence (. ! ? Enter).
    sentence_caps_enabled: bool,
    /// True when the next completed word starts a sentence. Set by the
    /// terminator checks, cleared on consumption, clicks and caret moves.
    sentence_start_pending: bool,
    /// Lowercase exe basenames (no .exe) where autocorrect must never fire —
    /// code editors, terminals, games. Checked against the foreground
    /// watcher's cached process name (may lag a fast alt-tab by one 1500ms
    /// poll, same tolerance as app-profile switching).
    excluded_apps: HashSet<String>,
    /// Lowercase typo keys the user has switched off individually in the
    /// bundled dictionaries. Custom-map entries are never checked against
    /// this — users delete those outright instead.
    disabled_entries: HashSet<String>,
    /// "Days of the week" bundled pack sub-toggle (monday → Monday).
    days_enabled: bool,
    /// "Symbols" bundled pack sub-toggle ((c) → ©, -> → →).
    symbols_enabled: bool,
    /// "Emoji" bundled pack sub-toggle ((smile) → 😄).
    emojis_enabled: bool,
    /// Lowercase exe basenames where TEXT EXPANSIONS never fire — separate
    /// list from the autocorrect one (same normalization, same cached
    /// foreground read).
    expansion_excluded_apps: HashSet<String>,
    global_variables: HashMap<String, String>,
}

impl Default for ExpansionState {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            assignments: HashMap::new(),
            space_triggers: HashSet::new(),
            autocorrect_words: HashSet::new(),
            // Off until the startup settings sync says otherwise — autocorrect
            // is an opt-in Pro feature, never active before config loads.
            autocorrect_enabled: false,
            builtin_typos_enabled: false,
            extended_typos_enabled: false,
            double_caps_enabled: false,
            double_caps_exceptions: HashSet::new(),
            caps_lock_fix_enabled: false,
            sentence_caps_enabled: false,
            sentence_start_pending: false,
            excluded_apps: HashSet::new(),
            disabled_entries: HashSet::new(),
            days_enabled: false,
            symbols_enabled: false,
            emojis_enabled: false,
            expansion_excluded_apps: HashSet::new(),
            global_variables: HashMap::new(),
        }
    }
}

/// When true, the LL hook will swallow the next bare Space keypress because
/// the current expansion buffer exactly matches a space-mode trigger. Avoids
/// the post-hoc "+1 backspace" race that previously caused a leading space
/// to appear in expansions when the target app processed the space slowly.
pub static EXPANSION_PENDING_SPACE: AtomicBool = AtomicBool::new(false);

/// Latched by the LL hook when it actually swallows a Space. Read once and
/// cleared by check_space_trigger to decide whether to skip the extra backspace.
/// If no expansion ends up matching, the swallowed Space is re-injected.
pub static SPACE_PRE_SWALLOWED: AtomicBool = AtomicBool::new(false);

/// When true, the current expansion buffer resolves an autocorrect correction
/// (custom map, built-in dictionary, or double-caps pattern). The LL hook
/// reads this to pre-swallow word-terminator keystrokes (Space, Enter, Tab,
/// unshifted punctuation) so the erase count is exact — the same race fix
/// EXPANSION_PENDING_SPACE provides for space-mode expansions.
pub static AUTOCORRECT_PENDING: AtomicBool = AtomicBool::new(false);

/// Latched by the LL hook when it swallows a non-Space terminator for
/// autocorrect. Read once and cleared by the processor-side terminator check.
/// If nothing ends up firing (layout resolved the key to a non-terminator
/// char, settings changed mid-flight), the swallowed keystroke is re-injected
/// so the user's input is never lost.
pub static AC_KEY_PRE_SWALLOWED: AtomicBool = AtomicBool::new(false);

/// One-shot Backspace undo: true from the moment a correction fires until
/// the next input event. While armed, the LL hook pre-swallows a bare
/// Backspace so try_undo_autocorrect can revert the correction atomically.
pub static AC_UNDO_ARMED: AtomicBool = AtomicBool::new(false);

/// Latched by the LL hook when it swallows the Backspace of an armed undo.
pub static AC_BS_PRE_SWALLOWED: AtomicBool = AtomicBool::new(false);

/// What the last correction did — everything needed to put the typed text
/// back. Kept for exactly one follow-up input event.
struct AcUndo {
    original: String,
    replacement: String,
    term: AcTerminator,
    /// Which rule fired — forwarded to the frontend on undo so repeated
    /// undos of the same correction can suggest the right opt-out.
    source: AcSource,
}

static AC_UNDO: Mutex<Option<AcUndo>> = Mutex::new(None);

// ── Buffer management (called from hotkeys.rs) ─────────────────────────────

/// Recompute EXPANSION_PENDING_SPACE and AUTOCORRECT_PENDING from the current
/// buffer state. Called from every path that mutates the buffer, the trigger
/// sets, or the autocorrect settings. These flags are the only things the LL
/// hook reads to decide whether to pre-swallow a terminator keystroke.
fn refresh_pending_flag(s: &ExpansionState) {
    if s.buffer.is_empty() {
        EXPANSION_PENDING_SPACE.store(false, Ordering::SeqCst);
        AUTOCORRECT_PENDING.store(false, Ordering::SeqCst);
        return;
    }
    let buf_lower = s.buffer.to_lowercase();
    // Excluded-app veto short-circuits last so the cached foreground read
    // only happens when a trigger actually matches — mirrors the autocorrect
    // veto below.
    EXPANSION_PENDING_SPACE.store(
        !s.space_triggers.is_empty()
            && s.space_triggers.contains(&buf_lower)
            && !expansion_app_excluded(s),
        Ordering::SeqCst,
    );
    // Pro-gated at the flag so the hook never swallows for free-tier users
    // (is_pro is a cached atomic load — safe per keystroke).
    let ac = s.autocorrect_enabled
        && crate::licence::is_pro()
        // Dictionary check shares resolve_dict_correction with the resolve
        // path — per-entry disables and identity no-ops (typed "I" matching
        // i→I) never swallow a terminator.
        && (resolve_dict_correction(s, &s.buffer, &buf_lower).is_some()
            || (s.double_caps_enabled
                && double_caps_candidate(&s.buffer)
                && !s.double_caps_exceptions.contains(&buf_lower))
            || (s.caps_lock_fix_enabled
                && caps_lock_on()
                && caps_lock_candidate(&s.buffer)
                && !s.double_caps_exceptions.contains(&buf_lower))
            || (s.sentence_caps_enabled
                && s.sentence_start_pending
                && s.buffer.chars().next().map_or(false, |c| c.is_alphabetic() && c.is_lowercase())
                && s.buffer.chars().all(|c| c.is_alphabetic() || c == '\'')));
    // Excluded apps veto last — the cached-foreground read only happens when
    // a correction would otherwise be pending.
    let ac = ac
        && (s.excluded_apps.is_empty()
            || !s.excluded_apps.contains(&crate::foreground::get_current_fg_proc()));
    AUTOCORRECT_PENDING.store(ac, Ordering::SeqCst);
}

/// Append a character to the buffer. Called for bare (unmodified) key presses.
pub fn buffer_push(ch: char) {
    let mut s = state().lock().unwrap();
    s.buffer.push(ch);
    if s.buffer.len() > MAX_BUFFER_LENGTH {
        let start = s.buffer.len() - MAX_BUFFER_LENGTH;
        s.buffer = s.buffer[start..].to_string();
    }
    refresh_pending_flag(&s);
}

/// Remove the last character (Backspace).
///
/// Clears the pending sentence-start flag too. A terminator (`.` `!` `?`
/// Enter) that armed the flag may have just been backspaced away, or the
/// user may be editing back into a previous word — either way the engine
/// can't see across the caret, so the safe default is to disarm. The
/// one-shot autocorrect undo intercepts Backspace before this runs, so a
/// single-BS "revert that" gesture is unaffected.
pub fn buffer_pop() {
    let mut s = state().lock().unwrap();
    s.buffer.pop();
    s.sentence_start_pending = false;
    refresh_pending_flag(&s);
}

/// Clear the buffer entirely.
pub fn buffer_clear() {
    let mut s = state().lock().unwrap();
    s.buffer.clear();
    refresh_pending_flag(&s);
}

// ── Trigger detection ───────────────────────────────────────────────────────

/// Called when Space is pressed. Returns true if an expansion/autocorrect fired.
///
/// If the LL hook pre-swallowed the Space (SPACE_PRE_SWALLOWED set), we skip
/// the extra backspace and — if no expansion ends up matching — re-inject the
/// Space so the user's keystroke isn't lost.
pub fn check_space_trigger() -> bool {
    let was_pre_swallowed = SPACE_PRE_SWALLOWED.swap(false, Ordering::SeqCst);
    let delete_extra = !was_pre_swallowed;

    let mut s = state().lock().unwrap();
    if s.buffer.is_empty() {
        if was_pre_swallowed {
            drop(s);
            reinject_swallowed_space();
        }
        return false;
    }

    let original_buffer = s.buffer.clone();
    let buffer_lower = s.buffer.to_lowercase();

    // Sentence-caps context: only a CLEAN word (letters/apostrophes) consumes
    // the pending sentence-start flag. A buffer like "end." is the tail of
    // the word that just SET the flag — the space after it must not eat the
    // flag meant for the next word.
    let sentence_start = if original_buffer.chars().all(|c| c.is_alphabetic() || c == '\'') {
        let prev = s.sentence_start_pending;
        s.sentence_start_pending = false;
        prev
    } else {
        false
    };

    // Priority 1: Text expansion (space-triggered). Deliberate triggers win
    // over passive autocorrect when a word is somehow both. Excluded-app
    // veto falls through to autocorrect (its own separate list) and then to
    // the normal clear + reinject path.
    let exp_key = format!("GLOBAL::EXPANSION::{}", buffer_lower);
    if let Some(entry) = s
        .assignments
        .get(&exp_key)
        .cloned()
        .filter(|_| !expansion_app_excluded(&s))
    {
        let trigger_mode = entry
            .get("data")
            .and_then(|d| d.get("triggerMode"))
            .and_then(|v| v.as_str())
            .unwrap_or("space");

        // Skip immediate-mode expansions on Space — they already fired
        if trigger_mode == "immediate" {
            s.buffer.clear();
            drop(s);
            if was_pre_swallowed {
                reinject_swallowed_space();
            }
            return false;
        }

        let expansion_type = entry
            .get("data")
            .and_then(|d| d.get("expansionType"))
            .and_then(|v| v.as_str())
            .unwrap_or("text");

        if expansion_type == "image" {
            // Pro gate: Free users silently no-op image expansions. Data preserved
            // in config (imagePath, imageScale) so the expansion returns on upgrade.
            // No fall-through: there's no text body to substitute.
            if !crate::licence::is_pro() {
                s.buffer.clear();
                drop(s);
                info!("[Keyfire] Image expansion skipped (Free): \"{}\"", buffer_lower);
                return true;
            }

            let image_path = entry
                .get("data")
                .and_then(|d| d.get("imagePath"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let image_scale = entry
                .get("data")
                .and_then(|d| d.get("imageScale"))
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as u32;
            let trigger_len = s.buffer.len();
            s.buffer.clear();
            drop(s);

            info!("[Keyfire] Image expansion: \"{}\" → \"{}\"", buffer_lower, image_path);
            fire_image_expansion(&buffer_lower, trigger_len, delete_extra, &image_path, image_scale);
            return true;
        }

        // Check for variant options
        let options = entry
            .get("data")
            .and_then(|d| d.get("options"))
            .and_then(|v| v.as_array())
            .cloned();
        let random_variant = entry
            .get("data")
            .and_then(|d| d.get("randomVariant"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if let Some(opts) = options {
            if !opts.is_empty() {
                let trigger_len = s.buffer.len();
                let global_vars = s.global_variables.clone();
                let trigger_str = buffer_lower.clone();

                // Pro gate: Free users skip the variant picker and silently fire
                // options[0] as a regular text expansion. Variant data is preserved
                // in config and the picker returns on upgrade.
                if !crate::licence::is_pro() {
                    let first = &opts[0];
                    let text = first.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let html = first.get("html").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    s.buffer.clear();
                    drop(s);

                    info!("[Keyfire] Variant expansion (Free → options[0]): \"{}\"", trigger_str);
                    let case_pattern = detect_case(&original_buffer);
                    let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
                    fire_expansion(&trigger_str, trigger_len, delete_extra, &text, html_opt, &global_vars, case_pattern);
                    return true;
                }

                s.buffer.clear();
                drop(s);

                info!(
                    "[Keyfire] Variant expansion: \"{}\" with {} options (mode: {})",
                    trigger_str, opts.len(), if random_variant { "random" } else { "picker" }
                );
                if crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
                    return true;
                }
                thread::spawn(move || {
                    fire_variant_expansion(&trigger_str, trigger_len, delete_extra, &opts, &global_vars, random_variant);
                });
                return true;
            }
        }

        let text = entry
            .get("data")
            .and_then(|d| d.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let html = entry
            .get("data")
            .and_then(|d| d.get("html"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let trigger_len = s.buffer.len();
        let global_vars = s.global_variables.clone();
        s.buffer.clear();
        drop(s);

        info!("[Keyfire] Expansion: \"{}\" → \"{}\"", buffer_lower, text);
        let case_pattern = detect_case(&original_buffer);
        let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
        fire_expansion(&buffer_lower, trigger_len, delete_extra, &text, html_opt, &global_vars, case_pattern);
        return true;
    }

    // Priority 2: Autocorrect (Pro) — custom map, built-in typos, typing fixes.
    if let Some(fix) = resolve_autocorrect(&s, &original_buffer, sentence_start) {
        s.buffer.clear();
        refresh_pending_flag(&s);
        drop(s);
        info!("[Keyfire] Autocorrect: \"{}\" -> \"{}\" (term Space)", original_buffer, fix.text);
        let term = if was_pre_swallowed {
            AcTerminator::SwallowedVk(VK_SPACE)
        } else {
            AcTerminator::AlreadySentChar(' ')
        };
        fire_autocorrect(&original_buffer, &fix.text, term, fix.caps_off, fix.source);
        return true;
    }

    s.buffer.clear();
    drop(s);
    if was_pre_swallowed {
        reinject_swallowed_space();
    }
    false
}

/// Re-inject a Space keystroke that the LL hook swallowed pre-emptively but
/// no expansion ended up consuming. Wrapped in SUPPRESS_SIMULATED so the hook
/// passes our synthetic event through to the target app without re-swallowing.
fn reinject_swallowed_space() {
    crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
    send_vk_tap(VK_SPACE);
    crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
}

// ── Autocorrect engine (Pro, v0.6.11) ───────────────────────────────────────
//
// Word-level correction fired at word terminators. Unlike expansions this
// NEVER touches the clipboard and has no settle sleeps: the erase, the
// corrected word, and the terminator go out as ONE batched SendInput call,
// so a 3-char fix is 8 input events injected in a single syscall. Windows
// guarantees batch order, and the InjectionGuard buffers any keystrokes the
// user types in the same instant (replayed after, same as expansions).

/// Punctuation characters that complete a word for autocorrect purposes.
/// Space / Enter / Tab are handled as keys, not chars.
pub(crate) fn is_terminator_char(ch: char) -> bool {
    matches!(ch, '.' | ',' | '!' | '?' | ';' | ':')
}

/// True when the foreground app is on the TEXT EXPANSION exclusion list
/// (separate list from the autocorrect one). Cached mutex read, cheap; the
/// empty-list check short-circuits it away entirely for users without
/// exclusions.
fn expansion_app_excluded(s: &ExpansionState) -> bool {
    !s.expansion_excluded_apps.is_empty()
        && s.expansion_excluded_apps.contains(&crate::foreground::get_current_fg_proc())
}

/// True when a word matches the double-caps typo shape: two leading capitals
/// followed by lowercase (HEllo, TWo, DOn't). Apostrophes are allowed in the
/// tail so contractions correct cleanly. Words of ALL caps (acronyms) never
/// match — the third char must be lowercase.
fn double_caps_candidate(word: &str) -> bool {
    let mut chars = word.chars();
    let (Some(c0), Some(c1)) = (chars.next(), chars.next()) else { return false; };
    if !(c0.is_alphabetic() && c0.is_uppercase() && c1.is_alphabetic() && c1.is_uppercase()) {
        return false;
    }
    let mut saw_tail = false;
    for c in chars {
        if !(c.is_lowercase() || c == '\'') {
            return false;
        }
        saw_tail = true;
    }
    saw_tail
}

/// True when Caps Lock is physically toggled on.
pub(crate) fn caps_lock_on() -> bool {
    unsafe {
        windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x14 /* VK_CAPITAL */) & 1 != 0
    }
}

/// True when a word matches the accidental-Caps-Lock shape: lowercase first
/// letter, everything after it uppercase (tHE, dON'T). Requires at least two
/// uppercase tail letters so short intentional oddities ("tO") survive.
/// Only meaningful when Caps Lock is actually on — the caller checks that.
fn caps_lock_candidate(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(c0) = chars.next() else { return false; };
    if !(c0.is_alphabetic() && c0.is_lowercase()) {
        return false;
    }
    let mut upper_tail = 0;
    for c in chars {
        if c == '\'' {
            continue;
        }
        if !(c.is_alphabetic() && c.is_uppercase()) {
            return false;
        }
        upper_tail += 1;
    }
    upper_tail >= 2
}

/// Invert an accidental-Caps-Lock word: "tHE" → "The".
fn caps_lock_fix(word: &str) -> String {
    let mut chars = word.chars();
    let first: String = chars.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    let rest: String = chars.as_str().to_lowercase();
    first + &rest
}

/// Lower ONLY the second character: "HEllo" → "Hello".
fn double_caps_fix(word: &str) -> String {
    word.chars()
        .enumerate()
        .flat_map(|(i, c)| {
            let iter: Box<dyn Iterator<Item = char>> = if i == 1 {
                Box::new(c.to_lowercase())
            } else {
                Box::new(std::iter::once(c))
            };
            iter
        })
        .collect()
}

/// Which rule produced a correction. Carried through the undo state so the
/// frontend can offer the right "stop correcting this" action when the user
/// keeps undoing the same fix.
#[derive(Clone, Copy, PartialEq)]
enum AcSource {
    Custom,
    Builtin,
    Extended,
    Days,
    Symbols,
    Emojis,
    DoubleCaps,
    CapsLock,
    SentenceCaps,
}

impl AcSource {
    fn as_str(self) -> &'static str {
        match self {
            AcSource::Custom => "custom",
            AcSource::Builtin => "builtin",
            AcSource::Extended => "extended",
            AcSource::Days => "days",
            AcSource::Symbols => "symbols",
            AcSource::Emojis => "emojis",
            AcSource::DoubleCaps => "doubleCaps",
            AcSource::CapsLock => "capsLock",
            AcSource::SentenceCaps => "sentenceCaps",
        }
    }
}

/// A resolved correction plus its side effects.
struct AcFix {
    text: String,
    /// Tap Caps Lock off inside the correction batch (Caps Lock fix path).
    caps_off: bool,
    source: AcSource,
}

/// Dictionary lookup shared by the pending flag and the resolve path so the
/// two can never disagree about whether a word will fire. Priority: custom
/// map → built-in dictionary → extended dictionary. The bundled packs honour
/// the per-entry disable list; the custom map doesn't. Returns None when the
/// case-carried correction is identical to what was typed (e.g. "I" typed
/// correctly matching the i→I entry) — firing would be a visible no-op and
/// the terminator must not be swallowed for it.
fn resolve_dict_correction(
    s: &ExpansionState,
    original: &str,
    lower: &str,
) -> Option<(String, AcSource)> {
    let mut hit: Option<(&str, AcSource)> = None;

    let ac_key = format!("GLOBAL::AUTOCORRECT::{}", lower);
    if let Some(entry) = s.assignments.get(&ac_key) {
        let correction = entry
            .get("data")
            .and_then(|d| d.get("correction"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !correction.is_empty() {
            hit = Some((correction, AcSource::Custom));
        }
    }

    if hit.is_none() && !s.disabled_entries.contains(lower) {
        if s.builtin_typos_enabled {
            if let Some(c) = builtin_autocorrect(lower) {
                hit = Some((c, AcSource::Builtin));
            }
        }
        if hit.is_none() && s.extended_typos_enabled {
            if let Some(c) = extended_autocorrect(lower) {
                hit = Some((c, AcSource::Extended));
            }
        }
        if hit.is_none() && s.days_enabled {
            if let Some(c) = days_autocorrect(lower) {
                hit = Some((c, AcSource::Days));
            }
        }
        if hit.is_none() && s.symbols_enabled {
            if let Some(c) = symbols_autocorrect(lower) {
                hit = Some((c, AcSource::Symbols));
            }
        }
        if hit.is_none() && s.emojis_enabled {
            if let Some(c) = emoji_autocorrect(lower) {
                hit = Some((c, AcSource::Emojis));
            }
        }
        // Superscript/subscript suffix matching: "m^2" → "m²". The whole
        // buffered word is replaced (typed-case prefix + symbol), so the
        // normal fire/undo machinery applies unchanged. Trigger shapes are
        // unambiguous (^N/_N can't end an English word), and "x^10" matches
        // nothing — no partial corruption possible.
        if hit.is_none() && s.symbols_enabled {
            for (trig, sym) in SUPERSUB_ENTRIES {
                if lower.ends_with(trig) && !s.disabled_entries.contains(*trig) {
                    let prefix_chars = original.chars().count() - trig.chars().count();
                    let prefix: String = original.chars().take(prefix_chars).collect();
                    return Some((format!("{}{}", prefix, sym), AcSource::Symbols));
                }
            }
        }
    }

    let (correction, source) = hit?;
    let text = apply_case(correction, detect_case(original));
    if text == original {
        return None;
    }
    Some((text, source))
}

/// Resolve the correction for a completed word, or None. Priority: custom
/// map → built-in dictionary → Caps Lock fix → double-caps, then sentence
/// capitalization composes over whichever result (or the raw word) so
/// "teh" at a sentence start becomes "The". Case of the typed word carries
/// onto map corrections (Teh → The, TEH → THE); a lowercase typed word takes
/// the stored correction verbatim so deliberate case in corrections survives
/// (e.g. "im" → "I'm").
fn resolve_autocorrect(s: &ExpansionState, original: &str, sentence_start: bool) -> Option<AcFix> {
    if !s.autocorrect_enabled || !crate::licence::is_pro() {
        return None;
    }
    // Excluded app in the foreground — never fire (mirrors the pending-flag
    // veto; this covers the resolve-time race after a fast app switch).
    if !s.excluded_apps.is_empty()
        && s.excluded_apps.contains(&crate::foreground::get_current_fg_proc())
    {
        return None;
    }
    let lower = original.to_lowercase();

    let mut base: Option<AcFix> = resolve_dict_correction(s, original, &lower)
        .map(|(text, source)| AcFix { text, caps_off: false, source });

    // Dict hit typed with Caps Lock accidentally on: screen "aDN" means the
    // user really typed "Adn", so the correction carries the INVERTED word's
    // case ("And", not "and") and the fix also switches Caps Lock off — same
    // treatment as the dedicated Caps Lock arm below. The candidate shape
    // (lower first + upper tail) guarantees detect_case(original) was Lower,
    // so the resolved text is the stored correction verbatim; Capitalized
    // apply_case only touches the first letter, preserving intrinsic case
    // in corrections like "I'm".
    if s.caps_lock_fix_enabled
        && caps_lock_on()
        && caps_lock_candidate(original)
        && !s.double_caps_exceptions.contains(&lower)
    {
        if let Some(f) = base.as_mut() {
            f.text = apply_case(&f.text, detect_case(&caps_lock_fix(original)));
            f.caps_off = true;
        }
    }

    if base.is_none()
        && s.caps_lock_fix_enabled
        && caps_lock_on()
        && caps_lock_candidate(original)
        && !s.double_caps_exceptions.contains(&lower)
    {
        base = Some(AcFix { text: caps_lock_fix(original), caps_off: true, source: AcSource::CapsLock });
    }

    if base.is_none()
        && s.double_caps_enabled
        && double_caps_candidate(original)
        && !s.double_caps_exceptions.contains(&lower)
    {
        base = Some(AcFix { text: double_caps_fix(original), caps_off: false, source: AcSource::DoubleCaps });
    }

    // Sentence capitalization — composes over the resolved text or the raw
    // word when nothing else matched. Only clean words qualify (letters and
    // apostrophes): the buffer can carry punctuation left over from an
    // earlier non-firing terminator ("world.") which must never re-fire.
    if s.sentence_caps_enabled && sentence_start {
        let current = base.as_ref().map(|f| f.text.as_str()).unwrap_or(original);
        if current.chars().next().map_or(false, |c| c.is_alphabetic() && c.is_lowercase())
            && current.chars().all(|c| c.is_alphabetic() || c == '\'')
        {
            let capped = apply_case(current, CasePattern::Capitalized);
            // Keep the base rule's source when composing — undoing "The"
            // that started as a "teh" dictionary hit should point at the
            // dictionary entry, not at sentence caps.
            return Some(AcFix {
                text: capped,
                caps_off: base.as_ref().map_or(false, |f| f.caps_off),
                source: base.map_or(AcSource::SentenceCaps, |f| f.source),
            });
        }
    }

    base
}

/// How the word was completed, and whether the terminator keystroke already
/// reached the target app.
#[derive(Clone, Copy)]
pub(crate) enum AcTerminator {
    /// Hook pre-swallowed the key — re-emit it inside the batch.
    SwallowedVk(u16),
    /// Hook pre-swallowed the keystroke that resolved to this char.
    SwallowedChar(char),
    /// The char already landed in the app (shifted punctuation isn't
    /// pre-swallowed) — erase it too (+1 backspace) and retype it.
    AlreadySentChar(char),
    /// The key (Enter/Tab) already landed in the app — erase it too
    /// (+1 backspace kills the newline/tab) and re-tap it.
    AlreadySentVk(u16),
}

fn push_vk_pair(inputs: &mut Vec<INPUT>, vk: u16) {
    for flags in [0u32, KEYEVENTF_KEYUP] {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
            },
        });
    }
}

fn push_unicode(inputs: &mut Vec<INPUT>, text: &str) {
    for code_unit in text.encode_utf16() {
        for flags in [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP] {
            inputs.push(INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT { wVk: 0, wScan: code_unit, dwFlags: flags, time: 0, dwExtraInfo: 0 },
                },
            });
        }
    }
}

/// Fire a correction: erase the typed word, type the correction, re-emit the
/// terminator — one SendInput batch, zero sleeps before it. Runs on its own
/// thread only for the post-batch suppress drain; the InjectionGuard is
/// created HERE on the processor thread so there is no window for user
/// keystrokes to slip between the decision and the buffering.
/// Also arms the one-shot Backspace undo with everything needed to revert.
fn fire_autocorrect(original: &str, replacement: &str, term: AcTerminator, caps_off: bool, source: AcSource) {
    if !fire_rate_ok(replacement) {
        return;
    }

    let word_char_len = original.chars().count();
    let backspaces = word_char_len
        + match term {
            AcTerminator::AlreadySentChar(_) | AcTerminator::AlreadySentVk(_) => 1,
            _ => 0,
        };

    let mut inputs: Vec<INPUT> =
        Vec::with_capacity(backspaces * 2 + replacement.len() * 2 + 6);
    for _ in 0..backspaces {
        push_vk_pair(&mut inputs, VK_BACKSPACE);
    }
    push_unicode(&mut inputs, replacement);
    match term {
        AcTerminator::SwallowedVk(vk) | AcTerminator::AlreadySentVk(vk) => {
            push_vk_pair(&mut inputs, vk)
        }
        AcTerminator::SwallowedChar(ch) | AcTerminator::AlreadySentChar(ch) => {
            push_unicode(&mut inputs, &ch.to_string())
        }
    }
    if caps_off {
        // Caps Lock accident fix: switch Caps Lock off as part of the batch.
        // The unicode text events above are layout/caps independent, so batch
        // position doesn't affect the corrected text.
        push_vk_pair(&mut inputs, 0x14 /* VK_CAPITAL */);
    }

    crate::analytics::log_action("autocorrect", replacement.chars().count() as u32, replacement, replacement);

    // Arm the one-shot Backspace undo before anything can land.
    *AC_UNDO.lock().unwrap() = Some(AcUndo {
        original: original.to_string(),
        replacement: replacement.to_string(),
        term,
        source,
    });
    AC_UNDO_ARMED.store(true, Ordering::SeqCst);

    // Guard on the processor thread — concurrent real keystrokes buffer from
    // this instant and replay after the batch.
    let guard = InjectionGuard::new();

    thread::spawn(move || {
        let _guard_slot = guard; // moved in; released by replay helper below

        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
        // Ctrl/Alt/Win held would corrupt the batch (Ctrl+Backspace = delete
        // word). Physical modifier release + restore, same as every injector.
        let held = crate::actions::release_held_modifiers();

        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }

        crate::actions::restore_modifiers(&held);

        // Suppress drain — keep SUPPRESS_SIMULATED true until the LL hook has
        // consumed our injected events (rule 1 in replay_buffered_and_recheck).
        thread::sleep(Duration::from_millis(10));
        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);

        replay_buffered_and_recheck(_guard_slot);
    });
}

/// Common English abbreviations that end in `.` mid-sentence. Kept sorted for
/// `binary_search`. Case-insensitive whole-word match against the buffer at
/// the moment a `.` terminator arrives — a hit means the `.` doesn't arm
/// sentence-caps for the next word.
///
/// Titles (`Mr.`, `Mrs.`, `Ms.`, `Dr.`, `Prof.`, `Rev.`, `Hon.`, `Jr.`, `Sr.`,
/// `St.` as in Saint) are DELIBERATELY EXCLUDED — they precede a proper noun
/// name that the user expects capitalised. Same for `am`/`pm` where an
/// end-of-sentence period is common.
const SENTENCE_END_ABBREVIATIONS: &[&str] = &[
    "ave", "blvd", "cf", "co", "corp", "eg", "etc", "fig",
    "gmbh", "ie", "inc", "llc", "ltd", "no", "plc", "rd",
    "viz", "vol", "vs",
];

fn is_sentence_end_abbreviation(word: &str) -> bool {
    let lower = word.to_lowercase();
    SENTENCE_END_ABBREVIATIONS
        .binary_search(&lower.as_str())
        .is_ok()
}

/// Terminator check for a resolved typed character, called by the processor
/// BEFORE the char is pushed into the buffer. Returns true when a correction
/// fired (caller must NOT push the char — the batch already emitted it).
/// Consumes AC_KEY_PRE_SWALLOWED; a swallowed keystroke that doesn't fire is
/// re-injected so the user's input is never lost.
pub fn check_char_terminator(ch: char) -> bool {
    let was_swallowed = AC_KEY_PRE_SWALLOWED.swap(false, Ordering::SeqCst);

    if !is_terminator_char(ch) {
        // Layout surprise: the hook swallowed an OEM key that isn't
        // punctuation on this layout. Give the char back; caller pushes it.
        if was_swallowed {
            crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
            send_unicode_char_tap(ch);
            crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
        }
        return false;
    }

    let mut s = state().lock().unwrap();
    let original = s.buffer.clone();

    // Sentence-caps context: only a CLEAN word (letters/apostrophes) consumes
    // the pending flag — punctuation-carrying buffers ("end.", "wait..") are
    // tails of the word that set it. '!' and '?' always mark a sentence end;
    // '.' only after a clean word of 2+ letters that isn't in the common
    // abbreviation blocklist ("etc.", "Mr.", "Dr." etc. don't end sentences).
    // Ellipsis dots on a non-clean buffer leave a pending flag standing
    // rather than killing it.
    let sentence_start = if original.is_empty() {
        false
    } else {
        let clean = original.chars().all(|c| c.is_alphabetic() || c == '\'');
        let prev = if clean {
            let p = s.sentence_start_pending;
            s.sentence_start_pending = false;
            p
        } else {
            false
        };
        let period_ends_sentence = ch == '.'
            && clean
            && original.chars().count() >= 2
            && !is_sentence_end_abbreviation(&original);
        if matches!(ch, '!' | '?') || period_ends_sentence {
            s.sentence_start_pending = true;
        }
        prev
    };

    let fired = if original.is_empty() {
        None
    } else {
        resolve_autocorrect(&s, &original, sentence_start)
    };

    match fired {
        Some(fix) => {
            s.buffer.clear();
            refresh_pending_flag(&s);
            drop(s);
            info!("[Keyfire] Autocorrect: \"{}\" -> \"{}\" (term '{}')", original, fix.text, ch);
            let term = if was_swallowed {
                AcTerminator::SwallowedChar(ch)
            } else {
                AcTerminator::AlreadySentChar(ch)
            };
            fire_autocorrect(&original, &fix.text, term, fix.caps_off, fix.source);
            true
        }
        None => {
            drop(s);
            if was_swallowed {
                crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                send_unicode_char_tap(ch);
                crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
            }
            false
        }
    }
}

/// Terminator check for Enter / Tab keydowns, called by the processor before
/// it clears the buffer. Returns true when a correction fired. Consumes
/// AC_KEY_PRE_SWALLOWED and re-injects the key when nothing fires.
pub fn check_key_terminator(vk: u16) -> bool {
    let was_swallowed = AC_KEY_PRE_SWALLOWED.swap(false, Ordering::SeqCst);

    let mut s = state().lock().unwrap();
    let original = s.buffer.clone();

    // Sentence-caps context: Enter starts a new sentence, Tab doesn't.
    // Empty buffer (Enter straight after a fired word) leaves the pending
    // flag alone — it still applies to the next real word. Only a clean
    // word consumes the flag; "end." tails must not eat it.
    let sentence_start = if original.is_empty() {
        if vk == 0x0D {
            s.sentence_start_pending = true;
        }
        false
    } else {
        let clean = original.chars().all(|c| c.is_alphabetic() || c == '\'');
        let prev = if clean {
            let p = s.sentence_start_pending;
            s.sentence_start_pending = false;
            p
        } else {
            false
        };
        if vk == 0x0D {
            s.sentence_start_pending = true;
        }
        prev
    };

    let fired = if original.is_empty() {
        None
    } else {
        resolve_autocorrect(&s, &original, sentence_start)
    };

    match fired {
        Some(fix) => {
            s.buffer.clear();
            refresh_pending_flag(&s);
            drop(s);
            info!("[Keyfire] Autocorrect: \"{}\" -> \"{}\" (term vk 0x{:02X})", original, fix.text, vk);
            let term = if was_swallowed {
                AcTerminator::SwallowedVk(vk)
            } else {
                // Key already reached the app (hook didn't swallow — settings
                // changed mid-flight). Erase the newline/tab too, re-tap after.
                AcTerminator::AlreadySentVk(vk)
            };
            fire_autocorrect(&original, &fix.text, term, fix.caps_off, fix.source);
            true
        }
        None => {
            drop(s);
            if was_swallowed {
                crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
                send_vk_tap(vk);
                crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
            }
            false
        }
    }
}

/// Backspace pressed as the very next input after a correction: revert it.
/// Erases the correction + terminator, retypes the original word + the same
/// terminator — one batch, same machinery as the fire. Returns true when the
/// undo consumed the Backspace (caller must not treat it as a deletion).
pub fn try_undo_autocorrect() -> bool {
    let was_swallowed = AC_BS_PRE_SWALLOWED.swap(false, Ordering::SeqCst);
    if !AC_UNDO_ARMED.swap(false, Ordering::SeqCst) {
        // Hook swallowed on a stale armed flag (processor disarmed first) —
        // give the Backspace back so the user's deletion still happens.
        if was_swallowed {
            crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
            send_vk_tap(VK_BACKSPACE);
            crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
            return true;
        }
        return false;
    }
    let Some(u) = AC_UNDO.lock().unwrap().take() else {
        if was_swallowed {
            crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
            send_vk_tap(VK_BACKSPACE);
            crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
            return true;
        }
        return false;
    };

    // Swallowed → the terminator is still on screen and must be erased too.
    // Not swallowed → the physical Backspace already deleted the terminator.
    let replacement_chars = u.replacement.chars().count();
    let backspaces = replacement_chars + if was_swallowed { 1 } else { 0 };

    let mut inputs: Vec<INPUT> =
        Vec::with_capacity(backspaces * 2 + u.original.len() * 2 + 4);
    for _ in 0..backspaces {
        push_vk_pair(&mut inputs, VK_BACKSPACE);
    }
    push_unicode(&mut inputs, &u.original);
    match u.term {
        AcTerminator::SwallowedVk(vk) | AcTerminator::AlreadySentVk(vk) => {
            push_vk_pair(&mut inputs, vk)
        }
        AcTerminator::SwallowedChar(ch) | AcTerminator::AlreadySentChar(ch) => {
            push_unicode(&mut inputs, &ch.to_string())
        }
    }

    info!("[Keyfire] Autocorrect undo: \"{}\" restored over \"{}\"", u.original, u.replacement);

    // Tell the frontend which correction was rejected — the main window
    // counts repeats and offers "stop correcting this" after the second
    // undo of the same word. Main window is never webview-suspended, so a
    // broadcast emit is safe from the processor thread.
    if let Some(app) = APP_HANDLE.get() {
        use tauri::Emitter;
        let _ = app.emit(
            "autocorrect-undone",
            serde_json::json!({
                "original": u.original,
                "replacement": u.replacement,
                "source": u.source.as_str(),
            }),
        );
    }

    let guard = InjectionGuard::new();
    thread::spawn(move || {
        let _guard_slot = guard;
        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
        let held = crate::actions::release_held_modifiers();
        unsafe {
            SendInput(
                inputs.len() as u32,
                inputs.as_ptr(),
                std::mem::size_of::<INPUT>() as i32,
            );
        }
        crate::actions::restore_modifiers(&held);
        thread::sleep(Duration::from_millis(10));
        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
        replay_buffered_and_recheck(_guard_slot);
    });
    true
}

/// Any input other than an immediate Backspace invalidates the one-shot undo.
pub fn disarm_undo() {
    if AC_UNDO_ARMED.swap(false, Ordering::SeqCst) {
        AC_UNDO.lock().unwrap().take();
    }
}

/// Click or caret-moving key: undo no longer applies and the sentence-start
/// context is unknown. Called alongside buffer_clear from those paths.
pub fn on_caret_moved() {
    disarm_undo();
    let mut s = state().lock().unwrap();
    s.sentence_start_pending = false;
    refresh_pending_flag(&s);
}

/// Consume a stale pre-swallow latch for a keystroke that resolved to no
/// character (dead key on this layout) — re-tap the original VK so the
/// user's keystroke isn't lost. No-op when nothing was swallowed.
pub fn reinject_if_swallowed(vk: u16) {
    if AC_KEY_PRE_SWALLOWED.swap(false, Ordering::SeqCst) {
        crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);
        send_vk_tap(vk);
        crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
    }
}

/// Single unicode char tap outside a batch (re-inject path).
fn send_unicode_char_tap(ch: char) {
    let mut inputs: Vec<INPUT> = Vec::with_capacity(4);
    push_unicode(&mut inputs, &ch.to_string());
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

/// Called after each character is added to the buffer. Checks for immediate-mode triggers.
/// Returns true if an immediate expansion fired.
pub fn check_immediate_triggers() -> bool {
    let mut s = state().lock().unwrap();
    if s.buffer.is_empty() {
        return false;
    }

    // Excluded app in the foreground — immediate expansions never fire
    // there. Empty-list short-circuit keeps this free for everyone else.
    if expansion_app_excluded(&s) {
        return false;
    }

    let original_buffer = s.buffer.clone();
    let buf_lower = s.buffer.to_lowercase();

    // Collect immediate triggers sorted by length (longest first)
    struct ImmTrigger {
        trigger: String,
        exp_type: String,
        text: String,
        html: String,
        image_path: String,
        image_scale: u32,
        options: Option<Vec<serde_json::Value>>,
        random_variant: bool,
    }
    let mut immediate_triggers: Vec<ImmTrigger> = s
        .assignments
        .iter()
        .filter(|(k, v)| {
            k.starts_with("GLOBAL::EXPANSION::")
                && v.get("data")
                    .and_then(|d| d.get("triggerMode"))
                    .and_then(|v| v.as_str())
                    == Some("immediate")
        })
        .map(|(k, v)| {
            let data = v.get("data");
            ImmTrigger {
                trigger: k["GLOBAL::EXPANSION::".len()..].to_string(),
                exp_type: data.and_then(|d| d.get("expansionType")).and_then(|v| v.as_str()).unwrap_or("text").to_string(),
                text: data.and_then(|d| d.get("text")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                html: data.and_then(|d| d.get("html")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                image_path: data.and_then(|d| d.get("imagePath")).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                image_scale: data.and_then(|d| d.get("imageScale")).and_then(|v| v.as_u64()).unwrap_or(100) as u32,
                options: data.and_then(|d| d.get("options")).and_then(|v| v.as_array()).cloned(),
                random_variant: data.and_then(|d| d.get("randomVariant")).and_then(|v| v.as_bool()).unwrap_or(false),
            }
        })
        .collect();
    immediate_triggers.sort_by(|a, b| b.trigger.len().cmp(&a.trigger.len()));

    for imm in &immediate_triggers {
        if buf_lower.ends_with(&imm.trigger) {
            let trigger_len = imm.trigger.len();

            // Variant expansion
            if let Some(ref opts) = imm.options {
                if !opts.is_empty() {
                    let opts = opts.clone();
                    let trigger_str = imm.trigger.clone();
                    let global_vars = s.global_variables.clone();
                    let random_variant = imm.random_variant;

                    // Pro gate: Free users silently fire options[0] as a regular
                    // text expansion. Picker returns on upgrade; data preserved.
                    if !crate::licence::is_pro() {
                        let first = &opts[0];
                        let text = first.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let html = first.get("html").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let original_suffix = original_buffer
                            .get(original_buffer.len().saturating_sub(trigger_len)..)
                            .unwrap_or(&original_buffer);
                        let case_pattern = detect_case(original_suffix);
                        s.buffer.clear();
                        drop(s);

                        info!("[Keyfire] Variant expansion (immediate, Free → options[0]): \"{}\"", trigger_str);
                        let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
                        fire_expansion(&trigger_str, trigger_len, false, &text, html_opt, &global_vars, case_pattern);
                        return true;
                    }

                    s.buffer.clear();
                    drop(s);

                    info!(
                        "[Keyfire] Variant expansion (immediate): \"{}\" with {} options (mode: {})",
                        trigger_str, opts.len(), if random_variant { "random" } else { "picker" }
                    );
                    if !crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
                        thread::spawn(move || {
                            fire_variant_expansion(&trigger_str, trigger_len, false, &opts, &global_vars, random_variant);
                        });
                    }
                    return true;
                }
            }

            if imm.exp_type == "image" {
                // Pro gate: Free users silently no-op image expansions. Data preserved.
                if !crate::licence::is_pro() {
                    s.buffer.clear();
                    drop(s);
                    info!("[Keyfire] Image expansion (immediate) skipped (Free): \"{}\"", imm.trigger);
                    return true;
                }

                let image_path = imm.image_path.clone();
                let image_scale = imm.image_scale;
                s.buffer.clear();
                drop(s);

                info!("[Keyfire] Image expansion (immediate): \"{}\" → \"{}\"", imm.trigger, image_path);
                fire_image_expansion(&imm.trigger, trigger_len, false, &image_path, image_scale);
                return true;
            }

            let global_vars = s.global_variables.clone();
            let text = imm.text.clone();
            let html = imm.html.clone();
            let trigger = imm.trigger.clone();
            // Detect case from the original-case suffix of the buffer.
            // Use .get() to avoid panicking if trigger_len falls mid-char (non-ASCII buffer).
            let original_suffix = original_buffer
                .get(original_buffer.len().saturating_sub(trigger_len)..)
                .unwrap_or(&original_buffer);
            let case_pattern = detect_case(original_suffix);
            s.buffer.clear();
            drop(s);

            info!("[Keyfire] Expansion (immediate): \"{}\" → \"{}\"", trigger, text);
            let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
            fire_expansion(&trigger, trigger_len, false, &text, html_opt, &global_vars, case_pattern);
            return true;
        }
    }

    false
}

// ── Smart Case ─────────────────────────────────────────────────────────────

#[derive(Clone, Copy)]
enum CasePattern {
    Lower,       // "brb" → no transform
    Capitalized, // "Brb" → capitalize first letter of output
    Upper,       // "BRB" → all caps
}

fn detect_case(original: &str) -> CasePattern {
    let has_alpha = original.chars().any(|c| c.is_alphabetic());
    if !has_alpha {
        return CasePattern::Lower;
    }
    if original.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()) {
        CasePattern::Upper
    } else if original.chars().next().map_or(false, |c| c.is_uppercase()) {
        CasePattern::Capitalized
    } else {
        CasePattern::Lower
    }
}

fn apply_case(text: &str, pattern: CasePattern) -> String {
    match pattern {
        CasePattern::Lower => text.to_string(),
        CasePattern::Upper => text.to_uppercase(),
        CasePattern::Capitalized => {
            let mut chars = text.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        }
    }
}

// ── Fire expansion ──────────────────────────────────────────────────────────

/// Fire an existing text expansion by trigger word — entry point for the
/// "Fire Text Expansion" macro step in actions.rs. Mirrors the dispatch logic
/// in check_space_trigger (image / variant / plain) but uses trigger_len=0 +
/// delete_extra=false because no characters were typed to consume — the macro
/// step injects on top of the current caret without erasing anything.
///
/// Case detection uses Lower (no typed buffer to read case from). The Pro
/// gating + missing-trigger handling matches the live-typing path so behaviour
/// is identical regardless of how the expansion was invoked.
pub(crate) fn fire_expansion_by_trigger(trigger: &str) {
    let entry = {
        let state = crate::hotkeys::engine_state().lock().unwrap();
        state.assignments
            .get(&format!("GLOBAL::EXPANSION::{}", trigger))
            .cloned()
    };
    let entry = match entry {
        Some(e) => e,
        None => {
            log::warn!("[Keyfire] Fire Text Expansion: trigger \"{}\" not found, skipping", trigger);
            return;
        }
    };

    let expansion_type = entry
        .get("data")
        .and_then(|d| d.get("expansionType"))
        .and_then(|v| v.as_str())
        .unwrap_or("text");

    if expansion_type == "image" {
        if !crate::licence::is_pro() {
            log::info!("[Keyfire] Fire Text Expansion (image, Free): \"{}\" — no-op", trigger);
            return;
        }
        let image_path = entry
            .get("data")
            .and_then(|d| d.get("imagePath"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let image_scale = entry
            .get("data")
            .and_then(|d| d.get("imageScale"))
            .and_then(|v| v.as_u64())
            .unwrap_or(100) as u32;
        log::info!("[Keyfire] Fire Text Expansion (image): \"{}\" → \"{}\"", trigger, image_path);
        fire_image_expansion(trigger, 0, false, &image_path, image_scale);
        return;
    }

    let options = entry
        .get("data")
        .and_then(|d| d.get("options"))
        .and_then(|v| v.as_array())
        .cloned();
    let random_variant = entry
        .get("data")
        .and_then(|d| d.get("randomVariant"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let global_vars = get_global_variables();

    if let Some(opts) = options {
        if !opts.is_empty() {
            if !crate::licence::is_pro() {
                let first = &opts[0];
                let text = first.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let html = first.get("html").and_then(|v| v.as_str()).unwrap_or("").to_string();
                log::info!("[Keyfire] Fire Text Expansion (variant, Free → options[0]): \"{}\"", trigger);
                let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
                fire_expansion(trigger, 0, false, &text, html_opt, &global_vars, CasePattern::Lower);
                return;
            }
            if crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
                log::info!("[Keyfire] Fire Text Expansion (variant): \"{}\" skipped — fill-in already active", trigger);
                return;
            }
            log::info!(
                "[Keyfire] Fire Text Expansion (variant): \"{}\" with {} options (mode: {})",
                trigger, opts.len(), if random_variant { "random" } else { "picker" }
            );
            let trigger_str = trigger.to_string();
            thread::spawn(move || {
                fire_variant_expansion(&trigger_str, 0, false, &opts, &global_vars, random_variant);
            });
            return;
        }
    }

    let text = entry
        .get("data")
        .and_then(|d| d.get("text"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let html = entry
        .get("data")
        .and_then(|d| d.get("html"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    log::info!("[Keyfire] Fire Text Expansion (text): \"{}\" → \"{}\"", trigger, text);
    let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
    fire_expansion(trigger, 0, false, &text, html_opt, &global_vars, CasePattern::Lower);
}

fn fire_expansion(
    _trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    text: &str,
    html: Option<&str>,
    global_vars: &HashMap<String, String>,
    case_pattern: CasePattern,
) {
    if !fire_rate_ok(_trigger) {
        return;
    }
    // Check for {fillIn:...} tokens — if present, spawn a dedicated thread for the
    // entire fill-in + injection flow so the processor thread is never blocked.
    // The HTML version (if present) is forwarded so rich-text formatting is
    // preserved through the fill-in path — same as the no-fill-in path.
    let fill_in_fields = extract_fill_in_fields(text);
    if !fill_in_fields.is_empty() {
        if crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let text = text.to_string();
        let html_owned: Option<String> = html.map(|s| s.to_string());
        let global_vars = global_vars.clone();
        let trigger_len = trigger_len;
        let trigger_str = _trigger.to_string();
        thread::spawn(move || {
            fire_expansion_with_fillin(fill_in_fields, &text, html_owned.as_deref(), trigger_len, delete_extra, &global_vars, &trigger_str, case_pattern);
        });
        return;
    }

    // No fill-in tokens — resolve and inject directly. Empty fill-in map since
    // there are no field values to reference in expressions.
    let empty_fillin: HashMap<String, String> = HashMap::new();
    let (resolved, cursor_back) = resolve_tokens(text, global_vars, &empty_fillin);
    let resolved = apply_case(&resolved, case_pattern);

    // Resolve HTML in parallel. Only used when target app accepts CF_HTML —
    // CF_UNICODETEXT always wins on plain-text apps via Windows clipboard fallback.
    // Skip HTML if the expansion uses inline key tokens (those need per-segment
    // injection that doesn't compose with a single paste).
    let resolved_html: Option<String> = html.and_then(|h| {
        if h.is_empty() || h.contains("{key:") {
            None
        } else {
            Some(resolve_tokens_html(h, global_vars, &empty_fillin))
        }
    });

    if resolved.is_empty() {
        return;
    }

    crate::analytics::log_action("expansion", resolved.chars().filter(|c| *c != '\r').count() as u32, _trigger, _trigger);

    // Capture target HWND NOW before spawning the thread
    let target_hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };

    // Wait for any prior injection to finish (handles sequential autocorrects)
    while crate::hotkeys::INJECTION_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(5));
    }

    // Set flag immediately on the processor thread — no race window for keystrokes to slip through
    let guard = InjectionGuard::new();

    // Spawn on a separate thread to avoid blocking the event processor
    let trigger_len = trigger_len;
    thread::spawn(move || {
        // Move guard into closure — Drop fires at end of injection
        let _guard = guard;

        // Delay to let the Space/character keystroke be processed by the target app
        thread::sleep(Duration::from_millis(30));

        // Suppress hook so our Backspace and paste keystrokes aren't intercepted
        crate::hotkeys::SUPPRESS_SIMULATED
            .store(true, std::sync::atomic::Ordering::SeqCst);

        // Delete trigger word + space (if applicable)
        let delete_count = trigger_len + if delete_extra { 1 } else { 0 };
        for _ in 0..delete_count {
            send_vk_tap(VK_BACKSPACE);
            thread::sleep(Duration::from_millis(5));
        }

        thread::sleep(Duration::from_millis(10));

        if resolved.contains("{key:") {
            // Inline key-token path: inject each text/key segment in order
            let snapshot = snapshot_clipboard();
            let held = crate::actions::release_held_modifiers();
            if target_hwnd != 0 {
                unsafe {
                    windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(target_hwnd as _);
                }
                thread::sleep(Duration::from_millis(10));
            }
            for seg in parse_key_segments(&resolved) {
                match seg {
                    KeySegment::Text(ref t) if !t.is_empty() => {
                        inject_text_segment(t, target_hwnd);
                    }
                    KeySegment::Key { mod_vks, main_vk, repeat } => {
                        for _ in 0..repeat {
                            for &m in &mod_vks { send_vk_key(m, false); }
                            send_vk_tap(main_vk);
                            for m in mod_vks.iter().rev() { send_vk_key(*m, true); }
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                    _ => {}
                }
            }
            crate::actions::restore_modifiers(&held);
            let post_seq = clipboard_sequence_number();
            settle_paste(target_hwnd, PASTE_RESTORE_SETTLE_MS);
            // Skip restore if user copied something during the paste window.
            if clipboard_sequence_number() == post_seq {
                restore_clipboard_snapshot(&snapshot);
            }
            crate::hotkeys::SUPPRESS_SIMULATED
                .store(false, std::sync::atomic::Ordering::SeqCst);
            crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                .store(false, std::sync::atomic::Ordering::SeqCst);
        } else {
            // Normal path: single inject
            let used_clipboard = should_use_clipboard(&resolved);
            if used_clipboard {
                inject_via_clipboard(&resolved, resolved_html.as_deref(), target_hwnd);
            } else {
                inject_via_sendinput(&resolved, target_hwnd);
            }

            // Move cursor back if {cursor} was present — single batched
            // SendInput so the caret snaps instantly instead of walking back.
            if cursor_back > 0 {
                thread::sleep(Duration::from_millis(10));
                send_left_arrows_batch(cursor_back);
            }

            crate::hotkeys::SUPPRESS_SIMULATED
                .store(false, std::sync::atomic::Ordering::SeqCst);
            if used_clipboard {
                crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                    .store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }

        // Replay buffered keystrokes and re-check triggers. The helper takes
        // the guard and releases it BEFORE the re-checks — see its doc comment.
        replay_buffered_and_recheck(_guard);
    });
}

/// Show the fill-in window with `fields` and block until the user submits,
/// cancels, or the 60s timeout elapses. This is the shared prompt surface —
/// the expansion fill-in flow AND macro-step prompts (Create Folder's
/// ask-for-name mode; Macro Inputs when it lands) all route through here.
///
/// Extracted verbatim from fire_expansion_with_fillin — the order of
/// operations is LOAD-BEARING per the fill-in invariants in CLAUDE.md:
/// FILL_IN_ACTIVE before anything, FILLIN_HWND before show,
/// resume_for_show before the first emit, renderer-ready handshake before
/// the fields emit, hide (never destroy) + focus restore after.
///
/// Returns (response, target_hwnd) where response is Ok(Some(values)) on
/// submit, Ok(None) on cancel, Err on timeout — and target_hwnd is the
/// window that had focus before the prompt (callers that inject text need
/// it; focus is already restored to it by the time this returns).
pub(crate) fn run_fill_in_window(
    fill_in_fields: &[FillInField],
) -> (Result<Option<HashMap<String, String>>, mpsc::RecvTimeoutError>, isize) {
    crate::hotkeys::FILL_IN_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);

    let app = match APP_HANDLE.get() {
        Some(a) => a,
        None => {
            log::error!("[EXP] No app handle — cannot show fill-in window");
            crate::hotkeys::FILL_IN_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            return (Ok(None), 0);
        }
    };

    // Capture target HWND BEFORE showing fill-in window (it will steal focus)
    let target_hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };

    // Create response channel
    let (tx, rx) = mpsc::channel();
    *fill_in_tx().lock().unwrap() = Some(tx);

    // Read theme from config for the fill-in window
    let theme = crate::config::load_config()
        .and_then(|c| c.get("theme").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "dark".to_string());

    // Track whether the JS side confirmed the picker rendered. Set true only
    // when the shown-ACK arrives within 2s of the fill-in-show emit. Governs
    // whether we bother waiting on the response channel afterwards — no point
    // blocking 30s for a selection from a picker that never appeared.
    let mut shown_ok = false;

    // Show fill-in window, wait for renderer ready signal, then emit field data
    if let Some(win) = app.get_webview_window("fillin") {
        use tauri::Emitter;

        // Store fill-in HWND before show — stable from window creation, no focus dependency
        if let Ok(hwnd) = win.hwnd() {
            let hwnd_val = hwnd.0 as isize;
            crate::hotkeys::FILLIN_HWND.store(hwnd_val, std::sync::atomic::Ordering::SeqCst);
        }

        // Position fill-in on the active monitor (where cursor is), not just primary
        {
            use windows_sys::Win32::Foundation::POINT;
            use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
            use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

            let scale = win.scale_factor().unwrap_or(1.0);
            let (cx, cy) = unsafe {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                (pt.x, pt.y)
            };
            let (wa_left, wa_top, wa_right, wa_bottom) = unsafe {
                let pt = POINT { x: cx, y: cy };
                let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
                let mut mi: MONITORINFO = std::mem::zeroed();
                mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(hmon, &mut mi) != 0 {
                    (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom)
                } else {
                    (0, 0, 1920, 1080)
                }
            };
            let log_left = wa_left as f64 / scale;
            let log_top  = wa_top  as f64 / scale;
            let log_w = (wa_right  - wa_left)  as f64 / scale;
            let log_h = (wa_bottom - wa_top) as f64 / scale;
            let win_w = 420.0;
            let x = log_left + (log_w - win_w) / 2.0;
            let y = log_top + log_h / 3.0;
            let _ = win.set_position(tauri::LogicalPosition::new(x, y));
        }

        // Wake a suspended webview BEFORE show/emit — see webview_mem.rs invariant.
        crate::webview_mem::resume_for_show(app, "fillin");
        let _ = win.show();
        let _ = win.set_focus();

        // Ask renderer to signal ready (handles subsequent shows after first mount)
        let _ = win.emit("fill-in-request-ready", serde_json::json!({}));

        // Wait for FillInWindow.jsx to signal it's mounted and listening (5s timeout)
        let (ready_tx, ready_rx) = mpsc::channel();
        *fill_in_ready_tx().lock().unwrap() = Some(ready_tx);
        let _ = ready_rx.recv_timeout(Duration::from_secs(5));
        *fill_in_ready_tx().lock().unwrap() = None;

        // Resolve tokens in each field's `default` and dropdown `options` before
        // the webview sees them. Without this, `default={{var:my.name}}` (or a
        // {clipboard} / {date} default) would appear literally in the input box.
        // Uses the full resolve_tokens pipeline (globals + nested expansions +
        // clipboard/selection/date tokens); fillin_values is empty because
        // fill-in tokens don't self-reference at prompt time.
        let global_vars = get_global_variables();
        let empty_fillin: HashMap<String, String> = HashMap::new();
        let resolved_fields: Vec<FillInField> = fill_in_fields
            .iter()
            .map(|f| FillInField {
                label: f.label.clone(),
                kind: f.kind.clone(),
                options: f
                    .options
                    .iter()
                    .map(|o| resolve_tokens(o, &global_vars, &empty_fillin).0)
                    .collect(),
                default: f
                    .default
                    .as_ref()
                    .map(|d| resolve_tokens(d, &global_vars, &empty_fillin).0),
            })
            .collect();

        // Renderer is ready — emit typed field data. Each field carries
        // label/kind/options/default so FillInWindow.jsx can render the right input.
        let _ = win.emit("fill-in-show", serde_json::json!({
            "fields": resolved_fields,
            "theme": theme,
        }));

        // Wait for JS to confirm the picker actually rendered. Guards against
        // WebView2/HMR failures in dev where the emit lands nowhere. If the
        // ACK doesn't arrive in 2s, skip straight to cleanup — no point
        // waiting for a selection from an invisible picker.
        let (shown_tx, shown_rx) = mpsc::channel::<()>();
        *fill_in_shown_tx().lock().unwrap() = Some(shown_tx);
        shown_ok = shown_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        *fill_in_shown_tx().lock().unwrap() = None;
        if !shown_ok {
            log::warn!("[Keyfire] fill-in window did not ACK render within 2s — aborting");
        }
    }

    // Block on this dedicated thread waiting for user response. Cut from 60s
    // to 30s so a stuck picker recovers faster (still generous for typing).
    // If the picker never confirmed render, don't wait at all — treat as an
    // instant timeout so cleanup runs and expansions unbrick immediately.
    let response = if shown_ok {
        rx.recv_timeout(Duration::from_secs(30))
    } else {
        Err(mpsc::RecvTimeoutError::Timeout)
    };
    *fill_in_tx().lock().unwrap() = None;

    // Clear fill-in HWND and hide window, restore focus to the original target app
    crate::hotkeys::FILLIN_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    if let Some(win) = app.get_webview_window("fillin") {
        let _ = win.hide();
    }
    if target_hwnd != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(target_hwnd as _);
        }
        thread::sleep(Duration::from_millis(10));
    }

    // Fill-in UI is fully closed — allow new fill-in invocations
    crate::hotkeys::FILL_IN_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);

    (response, target_hwnd)
}

/// Prompt the user for a single text value via the fill-in window. Used by
/// macro steps (Create Folder's ask-for-name mode). Returns None on cancel,
/// timeout, or when another fill-in is already active.
pub fn prompt_single_text(label: &str, default: &str) -> Option<String> {
    if crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
        log::warn!("[EXP] prompt_single_text: another fill-in is active — declining");
        return None;
    }
    let field = FillInField {
        label: label.to_string(),
        kind: "text".to_string(),
        options: Vec::new(),
        default: if default.is_empty() { None } else { Some(default.to_string()) },
    };
    match run_fill_in_window(std::slice::from_ref(&field)).0 {
        Ok(Some(values)) => values.get(label).cloned(),
        _ => None,
    }
}

/// Fill-in flow: runs entirely on a dedicated thread so the processor thread is never blocked.
/// Sequence: show window → wait for response → resolve tokens → inject.
fn fire_expansion_with_fillin(
    fill_in_fields: Vec<FillInField>,
    text: &str,
    html: Option<&str>,
    trigger_len: usize,
    delete_extra: bool,
    global_vars: &HashMap<String, String>,
    trigger_str: &str,
    case_pattern: CasePattern,
) {
    let (response, target_hwnd) = run_fill_in_window(&fill_in_fields);

    let (text_after_fillin, fillin_values) = match response {
        Ok(Some(values)) => {
            (resolve_fill_in_tokens(text, &values), values)
        }
        Ok(None) => {
            return;
        }
        Err(_) => {
            return;
        }
    };

    // Resolve remaining tokens. fillin_values is passed in so `{=expr}` and
    // `{if}` conditions can reference fields by their label as bare identifiers.
    let (resolved, cursor_back) = resolve_tokens(&text_after_fillin, global_vars, &fillin_values);
    let resolved = apply_case(&resolved, case_pattern);

    // Resolve the HTML alongside the text so rich-text targets (Word, Outlook,
    // Gmail) still receive formatting when the expansion uses fill-in fields.
    // The fillin_values are threaded through resolve_tokens_html so chips that
    // reference fields (e.g. {=upper(name)}) render correctly in HTML too.
    let resolved_html: Option<String> = html.and_then(|h| {
        if h.is_empty() || h.contains("{key:") {
            None
        } else {
            // Fill-in tokens may also appear in HTML as plain text outside of
            // chip spans (legacy expansions). Resolve those first, then walk
            // chip spans and resolve their embedded tokens.
            let html_after_fillin = resolve_fill_in_tokens(h, &fillin_values);
            Some(resolve_tokens_html(&html_after_fillin, global_vars, &fillin_values))
        }
    });

    if resolved.is_empty() {
        return;
    }

    crate::analytics::log_action("expansion", resolved.chars().filter(|c| *c != '\r').count() as u32, trigger_str, trigger_str);

    // Wait for any prior injection to finish
    while crate::hotkeys::INJECTION_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(5));
    }

    let _guard = InjectionGuard::new();

    // Delay to let focus settle after fill-in window hides
    thread::sleep(Duration::from_millis(30));

    crate::hotkeys::SUPPRESS_SIMULATED
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Delete trigger word + space (if applicable)
    let delete_count = trigger_len + if delete_extra { 1 } else { 0 };
    for _ in 0..delete_count {
        send_vk_tap(VK_BACKSPACE);
        thread::sleep(Duration::from_millis(5));
    }

    thread::sleep(Duration::from_millis(10));

    if resolved.contains("{key:") {
        let snapshot = snapshot_clipboard();
        let held = crate::actions::release_held_modifiers();
        if target_hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(target_hwnd as _);
            }
            thread::sleep(Duration::from_millis(10));
        }
        for seg in parse_key_segments(&resolved) {
            match seg {
                KeySegment::Text(ref t) if !t.is_empty() => {
                    inject_text_segment(t, target_hwnd);
                }
                KeySegment::Key { mod_vks, main_vk, repeat } => {
                    for _ in 0..repeat {
                        for &m in &mod_vks { send_vk_key(m, false); }
                        send_vk_tap(main_vk);
                        for m in mod_vks.iter().rev() { send_vk_key(*m, true); }
                        thread::sleep(Duration::from_millis(10));
                    }
                }
                _ => {}
            }
        }
        crate::actions::restore_modifiers(&held);
        let post_seq = clipboard_sequence_number();
        settle_paste(target_hwnd, PASTE_RESTORE_SETTLE_MS);
        if clipboard_sequence_number() == post_seq {
            restore_clipboard_snapshot(&snapshot);
        }
        crate::hotkeys::SUPPRESS_SIMULATED
            .store(false, std::sync::atomic::Ordering::SeqCst);
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(false, std::sync::atomic::Ordering::SeqCst);
    } else {
        let used_clipboard = should_use_clipboard(&resolved);
        if used_clipboard {
            inject_via_clipboard(&resolved, resolved_html.as_deref(), target_hwnd);
        } else {
            inject_via_sendinput(&resolved, target_hwnd);
        }

        if cursor_back > 0 {
            thread::sleep(Duration::from_millis(10));
            send_left_arrows_batch(cursor_back);
        }

        crate::hotkeys::SUPPRESS_SIMULATED
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if used_clipboard {
            crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                .store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    // Replay buffered keystrokes and re-check triggers. The helper takes
    // the guard and releases it BEFORE the re-checks — see its doc comment.
    replay_buffered_and_recheck(_guard);
}

// ── Token resolution ────────────────────────────────────────────────────────

pub fn resolve_tokens(
    text: &str,
    global_vars: &HashMap<String, String>,
    fillin_values: &HashMap<String, String>,
) -> (String, usize) {
    // Strip rich-text-editor artifacts baked into saved expansion text: ZWSP
    // cursor anchors (U+200B, inserted after every token chip) and the NBSPs
    // (U+00A0) contenteditable substitutes for spaces next to chips. The editor
    // now strips these on save, but fire-time stripping covers expansions saved
    // by older versions without a config migration.
    let mut result = text.replace('\u{200B}', "").replace('\u{00A0}', " ");

    // Determine whether to pay the cost of selection capture / clipboard read
    // once. Cached values feed both the legacy `{clipboard}` / `{selection}`
    // tokens AND the expression scope, so a snippet with mixed tokens captures
    // each source at most once per fire.
    let needs_expr  = result.contains("{=") || result.contains("{if ");
    let needs_clip  = result.contains("{clipboard") || needs_expr;
    let needs_sel   = result.contains("{selection") || needs_expr;
    let clipboard_text = if needs_clip {
        read_clipboard().unwrap_or_default()
    } else {
        String::new()
    };
    let selection_text = if needs_sel {
        capture_selection_via_copy().unwrap_or_default()
    } else {
        String::new()
    };

    // Nested expansions: {{expansion:trigger}} inlines another expansion's text.
    // Runs BEFORE global-variable substitution so a nested expansion's text can
    // itself contain {{var:name}} tokens that then resolve in the outer pass.
    // Cycle detection + 5-deep cap prevent A→B→A loops and runaway depth.
    if result.contains("{{expansion:") {
        let mut visited: HashSet<String> = HashSet::new();
        result = resolve_nested_expansions(&result, 0, &mut visited);
    }

    // Substitute global variables. Free tier — every credible competitor ships
    // basic name/email substitution for free, so gating first-name here made no
    // sense. Two forms supported:
    //   - {{var:name}}  (preferred, namespaced; won't collide with expansion refs)
    //   - {{name}}      (legacy bare form; kept forever for backwards compat)
    // Order: prefixed first so a global literally named "var:something" (unlikely
    // but possible via the raw config) doesn't get swallowed by the bare pass.
    if result.contains("{{") {
        for (name, value) in global_vars {
            let prefixed = format!("{{{{var:{}}}}}", name);
            result = result.replace(&prefixed, value);
        }
        for (name, value) in global_vars {
            let bare = format!("{{{{{}}}}}", name);
            result = result.replace(&bare, value);
        }
    }

    // {clipboard} and {clipboard:transform} legacy tokens — use the cached read.
    if result.contains("{clipboard") {
        // Replace specific variants BEFORE bare {clipboard} to prevent prefix matching
        result = result.replace("{clipboard:uppercase}", &clipboard_text.to_uppercase());
        result = result.replace("{clipboard:lowercase}", &clipboard_text.to_lowercase());
        result = result.replace("{clipboard:trim}", clipboard_text.trim());
        result = result.replace("{clipboard:urlencode}", &url_encode(&clipboard_text));
        result = result.replace("{clipboard}", &clipboard_text);
    }

    // {selection} and {selection:transform} legacy tokens — use the cached capture.
    if result.contains("{selection") {
        result = result.replace("{selection:uppercase}", &selection_text.to_uppercase());
        result = result.replace("{selection:lowercase}", &selection_text.to_lowercase());
        result = result.replace("{selection:trim}", selection_text.trim());
        result = result.replace("{selection:urlencode}", &url_encode(&selection_text));
        result = result.replace("{selection}", &selection_text);
    }

    // {date:...} and {time:...} tokens
    let now = chrono::Local::now();
    // Resolve the user's default date format once. Used by bare {date} and by
    // unformatted Date Math tokens ({date:+1d} etc). Explicit-format variants
    // below are unaffected.
    let default_date_format = {
        let state = crate::hotkeys::engine_state().lock().unwrap();
        state.default_date_format.clone()
    };
    let default_chrono_fmt = match default_date_format.as_str() {
        "MM/DD/YYYY" => "%m/%d/%Y",
        "YYYY-MM-DD" => "%Y-%m-%d",
        _ => "%d/%m/%Y", // DD/MM/YYYY fallback for missing/unknown values
    };
    result = result.replace("{date:DD/MM/YYYY}", &now.format("%d/%m/%Y").to_string());
    result = result.replace("{date:DD/MM/YY}", &now.format("%d/%m/%y").to_string());
    result = result.replace("{date:MM/DD/YYYY}", &now.format("%m/%d/%Y").to_string());
    result = result.replace("{date:YYYY-MM-DD}", &now.format("%Y-%m-%d").to_string());
    result = result.replace("{time:HH:MM:SS}", &now.format("%H:%M:%S").to_string());
    result = result.replace("{time:HH:MM}", &now.format("%H:%M").to_string());
    result = result.replace("{dayofweek}", &now.format("%A").to_string());
    result = result.replace("{month}", &now.format("%B").to_string());
    result = result.replace("{year}", &now.format("%Y").to_string());
    result = result.replace("{day}", &now.format("%-d").to_string());
    result = result.replace("{date:D MMMM YYYY}", &now.format("%-d %B %Y").to_string());
    result = result.replace("{isodate}", &now.format("%Y-%m-%dT%H:%M:%S").to_string());
    // Bare {date} — uses the user's default date format setting. Must run AFTER
    // the explicit-format variants above so `{date:DD/MM/YYYY}` etc. are
    // consumed first (otherwise `{date` would prefix-match into them).
    result = result.replace("{date}", &now.format(default_chrono_fmt).to_string());

    // {date:+Nd}, {date:-Nm}, {date:+Ny}, {date:-Nb} — date/time math with
    // optional format suffix. Unit `b` = business day (Mon-Fri), skips
    // weekends. So on Monday, {date:-1b} = last Friday. No holiday calendar —
    // Sat/Sun-only skip covers the common case.
    if result.contains("{date:+") || result.contains("{date:-") {
        let re = regex_lite::Regex::new(r"\{date:([+-]\d+)([dmyb])(?::([^}]+))?\}").unwrap();
        // Collect matches first to avoid mutating result during iteration
        let matches: Vec<(String, String)> = re
            .captures_iter(&result.clone())
            .filter_map(|caps| {
                let full_match = caps.get(0)?.as_str().to_string();
                let sign_and_mag = caps.get(1)?.as_str();
                let unit = caps.get(2)?.as_str();
                let fmt_suffix = caps.get(3).map(|m| m.as_str()).unwrap_or("");

                let n: i64 = sign_and_mag.parse().ok()?;

                let target_date = match unit {
                    "d" => {
                        now.date_naive() + chrono::Duration::days(n)
                    }
                    "m" => {
                        if n >= 0 {
                            now.date_naive().checked_add_months(chrono::Months::new(n as u32))?
                        } else {
                            now.date_naive().checked_sub_months(chrono::Months::new(n.unsigned_abs() as u32))?
                        }
                    }
                    "y" => {
                        if n >= 0 {
                            now.date_naive().checked_add_months(chrono::Months::new((n as u32) * 12))?
                        } else {
                            now.date_naive().checked_sub_months(chrono::Months::new((n.unsigned_abs() as u32) * 12))?
                        }
                    }
                    "b" => {
                        // Business-day walker — steps N weekdays forward or
                        // back from today, skipping Sat + Sun. n=0 returns
                        // today unchanged (matches d/m/y zero-step). Uses %w
                        // format (0=Sun, 6=Sat) to avoid importing Datelike.
                        let mut date = now.date_naive();
                        if n != 0 {
                            let step = if n > 0 { 1i64 } else { -1i64 };
                            let target: u32 = n.unsigned_abs() as u32;
                            let mut counted: u32 = 0;
                            while counted < target {
                                date = date + chrono::Duration::days(step);
                                let wd = date.format("%w").to_string();
                                if wd != "0" && wd != "6" {
                                    counted += 1;
                                }
                            }
                        }
                        date
                    }
                    _ => return None,
                };

                // Unformatted Date Math tokens (Tomorrow / Yesterday / Next
                // Week / Next Month — {date:+1d} etc.) follow the user's
                // default date format from Settings. Explicit-format variants
                // like {date:+1d:YYYY-MM-DD} keep their requested format.
                let chrono_fmt = match fmt_suffix {
                    "DD/MM/YYYY" => "%d/%m/%Y",
                    "DD/MM/YY"   => "%d/%m/%y",
                    "MM/DD/YYYY" => "%m/%d/%Y",
                    "YYYY-MM-DD" => "%Y-%m-%d",
                    "D MMMM YYYY" => "%-d %B %Y",
                    _ => default_chrono_fmt,
                };

                let formatted = target_date.format(chrono_fmt).to_string();
                Some((full_match, formatted))
            })
            .collect();

        for (token, replacement) in matches {
            result = result.replace(&token, &replacement);
        }
    }

    // Expression engine ({set} / {if} / {=}) is Pro-only. Gated here — the one
    // spot both the plain-text and rich-text (resolve_tokens_html delegates to
    // this fn) paths pass through, so there is no second ungated route. For
    // non-Pro users the tokens are left verbatim in the output rather than
    // evaluated, mirroring how {{global vars}} are left literal above.
    // Uses is_pro() so it follows the same entitlement source as every other
    // gate once Paddle online activation lands.
    if crate::licence::is_pro() {
        // Single scope shared across {set}, {if}, and {=} passes. `local_vars`
        // is populated by `{set name = expr}` tokens and read by the later if /
        // expression scans, so the user can chain intermediate calculations.
        let mut scope = crate::expression::Scope {
            fillin_values,
            global_vars,
            local_vars: std::collections::HashMap::new(),
            selection: &selection_text,
            clipboard: &clipboard_text,
        };

        // {set name = expr} — intermediate named values. Runs FIRST so {if}/{=}
        // can reference whatever the user defined. Outputs nothing.
        //
        // First pass is NON-FINAL: any {set} whose expression fails (e.g. because
        // it depends on a value an {ifset} will produce later) is left in the
        // text verbatim. The second {set} pass below — after {ifset} populates
        // scope — retries those and finalises.
        if result.contains("{set ") {
            result = process_set_tokens(&result, &mut scope, false);
        }

        // {if expr}…[{else}…]{endif} — conditional blocks. Runs BEFORE {=expr}
        // so discarded branches don't waste expression evaluation cycles. Nested
        // {if} blocks are tracked by depth in process_if_blocks.
        //
        // Also handles {ifset NAME cond}…{endif} — same logic as {if} plus the
        // chosen branch text gets stored in scope.local_vars[NAME] so the user
        // can reference the conditional's resolved value in later formulas.
        if result.contains("{if ") || result.contains("{ifset ") {
            result = process_if_blocks(&result, &mut scope);
        }

        // Second {set} pass — finalises any sets that depended on an {ifset}
        // value (which is now in scope after the if/ifset pass above). Anything
        // still unresolved at this point is a genuine error and rendered inline.
        if result.contains("{set ") {
            result = process_set_tokens(&result, &mut scope, true);
        }

        // {=expr} — expression substitution. Errors render inline as `«error: msg»`
        // so a single broken formula doesn't kill the whole expansion fire.
        if result.contains("{=") {
            result = process_expr_tokens(&result, &scope);
        }
    }

    // {cursor} — track position, then remove token
    let mut cursor_back = 0;
    if let Some(idx) = result.find("{cursor}") {
        cursor_back = result.len() - idx - "{cursor}".len();
        result = result.replace("{cursor}", "");
    }

    (result, cursor_back)
}

/// Find the matching `}` for a token starting at the byte just past `{=` or
/// `{if `. Respects string literals so `}` inside `"..."` doesn't terminate
/// the expression early. Returns the byte index of the closing `}` relative
/// to the start of the slice, or None if unterminated.
fn find_expression_end(body: &str) -> Option<usize> {
    let bytes = body.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    while i < bytes.len() {
        let b = bytes[i];
        if in_string {
            if b == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if b == b'"' {
                in_string = false;
            }
        } else {
            if b == b'"' {
                in_string = true;
            } else if b == b'}' {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Substitute every `{=expr}` token by evaluating the expression. On parse or
/// evaluation error, the substitution becomes `«error: <msg>»` so the user
/// sees what went wrong in-place rather than losing the whole expansion.
fn process_expr_tokens(text: &str, scope: &crate::expression::Scope<'_>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{=") {
        out.push_str(&rest[..start]);
        let body_start = start + 2;
        let after = &rest[body_start..];
        match find_expression_end(after) {
            Some(end) => {
                let expr_text = &after[..end];
                let rendered = match crate::expression::evaluate(expr_text, scope) {
                    Ok(s) => s,
                    Err(msg) => format!("«error: {}»", msg),
                };
                out.push_str(&rendered);
                rest = &after[end + 1..];
            }
            None => {
                // Unterminated — append everything verbatim and stop scanning.
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Look up an expansion's raw text by its trigger. Returns `None` if no such
/// expansion is registered. Used by `{{expansion:trigger}}` nested resolution.
///
/// Note on locking: this takes the engine_state lock. Safe because `resolve_tokens`
/// (the only caller path) is always invoked with the state lock released — callers
/// clone `global_vars` out of state and then fire, per the codebase convention.
pub fn get_expansion_text_by_trigger(trigger: &str) -> Option<String> {
    let key = format!("GLOBAL::EXPANSION::{}", trigger);
    let s = crate::hotkeys::engine_state().lock().ok()?;
    s.assignments
        .get(&key)
        .and_then(|v| v.get("data"))
        .and_then(|d| d.get("text"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Resolve `{{expansion:trigger}}` nested references. Inlines the target
/// expansion's raw text so downstream token passes (global vars, dates,
/// clipboard, expressions) run over the combined text.
///
/// Cycle detection: `visited` tracks the triggers currently being expanded.
/// A re-entry is replaced with a literal marker rather than looping.
/// Depth cap: 5 levels. Beyond that, the token is left as-is (visible to the
/// user so they see something is wrong instead of getting silent truncation).
///
/// Unknown triggers are left literal (mirrors how unknown `{{var:name}}` behaves).
/// Fill-in tokens inside nested expansions are NOT re-prompted — the outer
/// expansion's fill-in prompts happen before nested resolution, so nested
/// `{fillIn:...}` tokens would appear literal. Users nesting fill-in-bearing
/// expansions should be aware; documented in the help guide.
fn resolve_nested_expansions(text: &str, depth: u32, visited: &mut HashSet<String>) -> String {
    if depth >= 5 {
        return text.to_string();
    }
    // Manual scanner — avoids a regex compile per resolve_tokens call. Matches
    // `{{expansion:<trigger>}}` where <trigger> is anything up to the closing
    // `}}` (triggers can contain colons but not braces, matching config rules).
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{expansion:") {
        out.push_str(&rest[..start]);
        let body_start = start + "{{expansion:".len();
        let after = &rest[body_start..];
        match after.find("}}") {
            Some(end_rel) => {
                let trigger = after[..end_rel].trim().to_string();
                let full_end = body_start + end_rel + 2;
                if trigger.is_empty() {
                    out.push_str(&rest[start..full_end]);
                } else if visited.contains(&trigger) {
                    out.push_str(&format!("«cycle: {{{{expansion:{}}}}}»", trigger));
                } else if let Some(child_text) = get_expansion_text_by_trigger(&trigger) {
                    visited.insert(trigger.clone());
                    let resolved = resolve_nested_expansions(&child_text, depth + 1, visited);
                    visited.remove(&trigger);
                    out.push_str(&resolved);
                } else {
                    // Unknown trigger — leave the token literal so the author
                    // can see they referenced a missing expansion.
                    out.push_str(&rest[start..full_end]);
                }
                rest = &rest[full_end..];
            }
            None => {
                // Unterminated `{{expansion:` — copy the rest verbatim and bail.
                out.push_str(&rest[start..]);
                return out;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `{set name = expr}` — declare a named intermediate value. Evaluates each
/// `expr` against scope, stores results in `scope.local_vars`, and removes
/// the tokens from the output. Subsequent `{=…}` / `{if …}` passes see the
/// names.
///
/// Forward-reference safe: a `{set foo = bar}` can sit BEFORE a `{set bar = 5}`
/// in the snippet. We do a two-pass scan — first collect every `{set}` site,
/// then fixed-point evaluate until no more progress is made (capped at 10
/// iterations so cyclic dependencies bail out cleanly rather than spinning).
///
/// `final_pass` controls what happens to sets whose expressions still can't
/// evaluate after the fixed-point loop:
/// - `false`: leave the `{set name = expr}` token in the output text. The
///   caller will run this function again later (after `{ifset}` blocks have
///   populated more scope entries) and retry these.
/// - `true`: render the last evaluation error inline (`«error: …»`). This is
///   the terminal state — anything still failing genuinely can't be resolved.
fn process_set_tokens(text: &str, scope: &mut crate::expression::Scope<'_>, final_pass: bool) -> String {
    struct SetEntry {
        name: String,
        expr: String,
        start: usize,      // byte offset of `{set` in `text`
        end: usize,        // byte offset just past the closing `}`
        valid_name: bool,
    }

    // ── First pass: collect every {set name = expr} occurrence in source
    // order. We don't evaluate yet — that happens in the fixed-point loop
    // below, where forward references can resolve.
    let mut entries: Vec<SetEntry> = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + 5 <= bytes.len() {
        if &bytes[i..i + 5] == b"{set " {
            let body_start = i + 5;
            let after = &text[body_start..];
            if let Some(end_rel) = find_expression_end(after) {
                let inner = &after[..end_rel];
                if let Some(eq_pos) = inner.find('=') {
                    let name = inner[..eq_pos].trim().to_string();
                    let expr = inner[eq_pos + 1..].trim().to_string();
                    let valid_name = !name.is_empty()
                        && name.chars().next().map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                    entries.push(SetEntry {
                        name, expr,
                        start: i,
                        end: body_start + end_rel + 1,
                        valid_name,
                    });
                } else {
                    entries.push(SetEntry {
                        name: String::new(),
                        expr: String::new(),
                        start: i,
                        end: body_start + end_rel + 1,
                        valid_name: false,
                    });
                }
                i = body_start + end_rel + 1;
                continue;
            } else {
                // Unterminated {set — emit everything from here verbatim and bail.
                let mut out = String::with_capacity(text.len());
                let mut cursor = 0;
                for entry in &entries {
                    out.push_str(&text[cursor..entry.start]);
                    cursor = entry.end;
                }
                out.push_str(&text[cursor..]);
                return out;
            }
        }
        i += 1;
    }

    // ── Fixed-point evaluation. Each iteration tries every unresolved entry;
    // if any newly succeed, repeat (since downstream entries may now have
    // their deps in scope). Cap at 10 iterations — handles realistic chains
    // and trips clean on cyclic / impossible references.
    let mut resolved: Vec<bool> = vec![false; entries.len()];
    let mut last_error: Vec<Option<String>> = vec![None; entries.len()];
    for _ in 0..10 {
        let mut progress = false;
        for (idx, entry) in entries.iter().enumerate() {
            if resolved[idx] || !entry.valid_name { continue; }
            match crate::expression::evaluate(&entry.expr, scope) {
                Ok(value) => {
                    scope.set_local(entry.name.clone(), value);
                    resolved[idx] = true;
                    last_error[idx] = None;
                    progress = true;
                }
                Err(msg) => {
                    last_error[idx] = Some(msg);
                }
            }
        }
        if !progress { break; }
    }

    // ── Third pass: rebuild output text. Successful sets vanish; invalid-name
    // sets render their error inline. Unresolved sets:
    //   - non-final pass: keep the original `{set name = expr}` text so a
    //     later pass can retry them (after {ifset} populates more scope).
    //   - final pass: render the last error inline.
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (idx, entry) in entries.iter().enumerate() {
        out.push_str(&text[cursor..entry.start]);
        if !entry.valid_name {
            if entry.name.is_empty() {
                out.push_str("«error: {set} needs `name = expression`»");
            } else {
                out.push_str(&format!("«error: invalid {{set}} name '{}'»", entry.name));
            }
        } else if !resolved[idx] {
            if final_pass {
                if let Some(msg) = &last_error[idx] {
                    out.push_str(&format!("«error: set {}: {}»", entry.name, msg));
                }
            } else {
                // Keep verbatim for a retry later.
                out.push_str(&text[entry.start..entry.end]);
            }
        }
        cursor = entry.end;
    }
    out.push_str(&text[cursor..]);
    out
}

/// Resolve every `{if expr}…[{else}…]{endif}` block, keeping only the branch
/// that matches the condition. Supports nested `{if}` blocks by depth-tracking
/// — a stray `{else}` or `{endif}` at the same depth as the outer block
/// terminates the branch.
///
/// Also handles `{ifset NAME cond}…{endif}` — the named variant. After
/// picking and recursively processing the chosen branch, the resulting text
/// is stored in `scope.local_vars[NAME]` so subsequent `{=NAME}` references
/// resolve to the conditional's output. Nested `{ifset}` participates in
/// depth tracking just like `{if}`.
///
/// On condition error the entire block is replaced with `«error: msg»` so the
/// user gets a clear in-place signal rather than silent omission of content.
fn process_if_blocks(text: &str, scope: &mut crate::expression::Scope<'_>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;

    loop {
        // Find the earliest if-marker — `{if ` and `{ifset ` are disjoint
        // (different chars at byte 3) so either-or-both may show up. We take
        // whichever appears first; the other gets processed next iteration.
        let if_pos = rest.find("{if ");
        let ifset_pos = rest.find("{ifset ");
        let (start, is_ifset) = match (if_pos, ifset_pos) {
            (None, None) => { out.push_str(rest); return out; }
            (Some(p), None) => (p, false),
            (None, Some(p)) => (p, true),
            (Some(p1), Some(p2)) => if p1 <= p2 { (p1, false) } else { (p2, true) },
        };
        out.push_str(&rest[..start]);

        // Parse the header. `{if cond}` → name=None. `{ifset NAME cond}` →
        // name=Some(NAME), cond starts after the space separating name from
        // condition.
        let (header_start, name_opt): (usize, Option<String>) = if is_ifset {
            let name_start = start + 7; // past "{ifset "
            let after_set = &rest[name_start..];
            // Name terminates at the first space — condition follows.
            let Some(space_pos) = after_set.find(' ') else {
                out.push_str(&rest[start..]);
                return out;
            };
            let name = after_set[..space_pos].to_string();
            (name_start + space_pos + 1, Some(name))
        } else {
            (start + 4, None)
        };

        let after_header = &rest[header_start..];
        let Some(header_end) = find_expression_end(after_header) else {
            out.push_str(&rest[start..]);
            return out;
        };
        let condition_text = &after_header[..header_end];
        let after_open = &after_header[header_end + 1..];

        // Scan body for matching `{else}` / `{endif}` at depth 0.
        // Nested `{if` or `{ifset` opens are depth+1.
        let mut depth = 0usize;
        let mut else_at: Option<usize> = None;
        let mut end_at: Option<usize> = None;
        let bytes = after_open.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i..].starts_with(b"{if ") {
                depth += 1;
                i += 4;
                continue;
            }
            if bytes[i..].starts_with(b"{ifset ") {
                depth += 1;
                i += 7;
                continue;
            }
            if bytes[i..].starts_with(b"{endif}") {
                if depth == 0 {
                    end_at = Some(i);
                    break;
                }
                depth -= 1;
                i += 7;
                continue;
            }
            if bytes[i..].starts_with(b"{else}") && depth == 0 && else_at.is_none() {
                else_at = Some(i);
                i += 6;
                continue;
            }
            i += 1;
        }

        let Some(end_idx) = end_at else {
            out.push_str(&rest[start..]);
            return out;
        };

        let (then_branch, else_branch) = match else_at {
            Some(else_idx) => (
                &after_open[..else_idx],
                &after_open[else_idx + 6..end_idx],
            ),
            None => (&after_open[..end_idx], ""),
        };

        let kept = match crate::expression::evaluate_bool(condition_text, scope) {
            Ok(true)  => then_branch.to_string(),
            Ok(false) => else_branch.to_string(),
            Err(msg)  => format!("«error: {}»", msg),
        };

        // Recurse into the kept branch so nested blocks evaluate too.
        let kept_processed = process_if_blocks(&kept, scope);

        // For named {ifset NAME ...}: stash the kept text in local_vars so
        // downstream {=NAME} resolves to it. Validate the name first; an
        // invalid identifier means the user typed something garbled and we
        // just skip storage (the block still renders normally).
        if let Some(name) = name_opt {
            let valid = !name.is_empty()
                && name.chars().next().map_or(false, |c| c.is_ascii_alphabetic() || c == '_')
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            if valid {
                scope.set_local(name, kept_processed.clone());
            }
        }

        out.push_str(&kept_processed);
        rest = &after_open[end_idx + 7..]; // skip past "{endif}"
    }
}

/// Decode the HTML entities that browsers inject into attribute values when
/// serializing innerHTML. Handles the named entities the editor actually
/// produces (`&lt; &gt; &amp; &quot; &apos;`) plus numeric character
/// references (`&#NN;` decimal, `&#xNN;` hex). Anything else passes through
/// verbatim so we don't accidentally mangle user content.
fn decode_html_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp_pos) = rest.find('&') {
        out.push_str(&rest[..amp_pos]);
        let after_amp = &rest[amp_pos + 1..];
        if let Some(semi_offset) = after_amp.find(';') {
            let entity = &after_amp[..semi_offset];
            let decoded: Option<char> = match entity {
                "lt"   => Some('<'),
                "gt"   => Some('>'),
                "amp"  => Some('&'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if entity.starts_with('#') => {
                    let num = &entity[1..];
                    let cp = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
                        u32::from_str_radix(hex, 16).ok()
                    } else {
                        num.parse::<u32>().ok()
                    };
                    cp.and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(ch) = decoded {
                out.push(ch);
                rest = &after_amp[semi_offset + 1..];
                continue;
            }
        }
        // Not a recognised entity — emit the `&` literally and resume scanning
        // from the char after it.
        out.push('&');
        rest = after_amp;
    }
    out.push_str(rest);
    out
}

/// Percent-encode a string per RFC 3986 (unreserved characters pass through).
fn url_encode(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

// ── Clipboard operations (Win32) ────────────────────────────────────────────

/// Synchronously capture the user's current text selection by sending Ctrl+C,
/// reading the resulting clipboard contents, and then restoring whatever was
/// on the clipboard before. Returns None if no selection (clipboard didn't
/// change) or the read failed.
///
/// Blocks the calling thread for ~50–200ms (waits for the clipboard sequence
/// number to advance after the synthetic Ctrl+C). Callers must be on a thread
/// that can absorb this — typically a spawned injection thread, not the LL
/// hook callback.
///
/// Invariants:
/// - Sets `SUPPRESS_NEXT_CLIPBOARD_WRITE` so the clipboard-history listener
///   doesn't capture the Ctrl+C result OR the snapshot restore.
/// - Sets `SUPPRESS_SIMULATED` around the SendInput burst so the LL hook
///   doesn't re-trigger Ctrl+C-bound hotkeys.
/// - Restores the previous clipboard via `restore_clipboard_snapshot` so the
///   user's prior clipboard state is preserved (paste history, etc.).
fn capture_selection_via_copy() -> Option<String> {
    use std::sync::atomic::Ordering;

    let snapshot = snapshot_clipboard();
    let before_seq = clipboard_sequence_number();

    // Mark BOTH the Ctrl+C clipboard write and the restore as ours so the
    // clipboard-history listener ignores them. Cleared in the same scope on
    // every return path.
    crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(true, Ordering::SeqCst);
    crate::hotkeys::SUPPRESS_SIMULATED.store(true, Ordering::SeqCst);

    // Release any user-held modifiers so the Ctrl+C lands cleanly; restored after.
    let held = crate::actions::release_held_modifiers();

    // Send Ctrl+C — same VK pattern as the Ctrl+V paste path below.
    send_vk_key(0xA2, false); // LCtrl down
    send_vk_key(0x43, false); // C down
    thread::sleep(Duration::from_millis(15));
    send_vk_key(0x43, true);  // C up
    send_vk_key(0xA2, true);  // LCtrl up

    crate::actions::restore_modifiers(&held);

    // Wait for the clipboard sequence number to advance, up to 200ms.
    let mut sel: Option<String> = None;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(200) {
        thread::sleep(Duration::from_millis(15));
        if clipboard_sequence_number() != before_seq {
            sel = read_clipboard();
            break;
        }
    }

    // Restore previous clipboard contents (also suppressed from history).
    restore_clipboard_snapshot(&snapshot);

    crate::hotkeys::SUPPRESS_SIMULATED.store(false, Ordering::SeqCst);
    crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);

    sel.filter(|s| !s.is_empty())
}

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

            // Find null terminator
            let mut len = 0;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            let text = String::from_utf16_lossy(slice);

            GlobalUnlock(handle);
            CloseClipboard();
            return Some(text);
        }
    }
    None
}

fn write_clipboard(text: &str) -> bool {
    write_clipboard_dual(text, None)
}

/// Convert CSS `rgb(r, g, b)` colour functions to `#RRGGBB` hex. Chromium's
/// contenteditable serialises EVERY inline colour in rgb() function notation
/// (foreColor, hiliteColor, all of them) — and Word's legacy HTML paste
/// reader silently drops rgb() values while parsing hex fine. Modern engines
/// (Gmail, eM Client, browsers) accept both, so hex is the universal form.
fn rgb_functions_to_hex(s: &str) -> String {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex_lite::Regex::new(r"rgb\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})\s*\)").unwrap()
    });
    re.replace_all(s, |caps: &regex_lite::Captures| {
        let r: u8 = caps[1].parse().unwrap_or(0);
        let g: u8 = caps[2].parse().unwrap_or(0);
        let b: u8 = caps[3].parse().unwrap_or(0);
        format!("#{:02X}{:02X}{:02X}", r, g, b)
    })
    .to_string()
}

/// Map the RTE highlight-palette hexes to Word's fixed 16-colour highlight
/// names. Word's HTML paste reader IGNORES css backgrounds on inline spans —
/// its only channel for character highlight is the `mso-highlight` property,
/// and that only accepts the classic 16 names (maps to RTF \highlightN).
/// Nearest-distance mapping doesn't work here (every pastel is nearest to
/// white), so the palette is mapped by hand. MUST stay in sync with
/// HIGHLIGHT_COLOURS in TextExpansions.jsx — an unmapped hex just means no
/// Word highlight (modern apps still render the background).
fn mso_highlight_name(hex: &str) -> Option<&'static str> {
    match hex.to_ascii_uppercase().as_str() {
        "FFF59D" | "FFF176" | "FFCC80" => Some("yellow"), // yellow, amber, orange
        "C8E6C9" => Some("green"),
        "B3E5FC" | "80CBC4" => Some("cyan"), // blue, teal
        "F8BBD0" | "E1BEE7" => Some("magenta"), // pink, lavender
        "EF9A9A" => Some("red"),
        "B0BEC5" => Some("lightgray"),
        "F5F5F5" => Some("white"),
        _ => None,
    }
}

/// Append `mso-highlight` to every inline `background:#hex` declaration so
/// highlights survive the paste into Word. Modern engines ignore the unknown
/// mso-* property and keep rendering the background hex.
fn add_mso_highlight(s: &str) -> String {
    static RE: OnceLock<regex_lite::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex_lite::Regex::new(r"background:\s*#([0-9A-Fa-f]{6})").unwrap()
    });
    re.replace_all(s, |caps: &regex_lite::Captures| {
        match mso_highlight_name(&caps[1]) {
            Some(name) => format!("background:#{};mso-highlight:{}", &caps[1], name),
            None => caps[0].to_string(),
        }
    })
    .to_string()
}

/// Cached CF_HTML clipboard format ID (registered once with the OS).
/// pub(crate) so the clipboard listener can read HTML captures using the same
/// format ID this module writes with — otherwise we'd have two separate
/// RegisterClipboardFormatW calls for the same "HTML Format" string, which
/// still returns the same u32 but wastes a syscall on every clipboard event.
pub(crate) fn cf_html_format_id() -> u32 {
    static FORMAT_ID: OnceLock<u32> = OnceLock::new();
    *FORMAT_ID.get_or_init(|| {
        let name: Vec<u16> = "HTML Format".encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    })
}

/// Wrap an HTML fragment in the CF_HTML clipboard format with the required
/// Version / StartHTML / EndHTML / StartFragment / EndFragment byte offsets.
fn build_cf_html(fragment: &str) -> Vec<u8> {
    // Placeholder offsets get patched after we know the actual byte positions.
    // NO whitespace between the wrapper tags and the fragment: consumers that
    // parse the whole body instead of honouring the fragment offsets (Gmail's
    // paste sanitiser) turn each \r\n before the content into a visible
    // leading space in the compose window.
    let header = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let prefix = "<html><body><!--StartFragment-->";
    let suffix = "<!--EndFragment--></body></html>";

    let start_html     = header.len();
    let start_fragment = start_html + prefix.len();
    let end_fragment   = start_fragment + fragment.len();
    let end_html       = end_fragment + suffix.len();

    let mut body = String::with_capacity(end_html);
    body.push_str(header);
    body.push_str(prefix);
    body.push_str(fragment);
    body.push_str(suffix);

    // Patch each "Key:0000000000" placeholder with the real offset
    let patch = |bytes: &mut Vec<u8>, key: &str, val: usize| {
        let needle = format!("{}:0000000000", key);
        if let Some(pos) = bytes.windows(needle.len()).position(|w| w == needle.as_bytes()) {
            let val_str = format!("{:010}", val);
            let start = pos + key.len() + 1;
            for (i, b) in val_str.bytes().enumerate() {
                bytes[start + i] = b;
            }
        }
    };

    let mut out = body.into_bytes();
    patch(&mut out, "StartHTML",     start_html);
    patch(&mut out, "EndHTML",       end_html);
    patch(&mut out, "StartFragment", start_fragment);
    patch(&mut out, "EndFragment",   end_fragment);
    out.push(0); // CF_HTML is a null-terminated ANSI/UTF-8 string
    out
}

/// Write plain text to the clipboard as CF_UNICODETEXT. If `html` is provided,
/// also write CF_HTML so rich-text-aware target apps (Word, Outlook, Gmail,
/// Slack, Teams) receive formatted content. Target apps that don't accept
/// CF_HTML fall back to CF_UNICODETEXT automatically — no extra wiring needed.
///
/// pub(crate) so lib.rs's `paste_clipboard_item` can reuse this for clipboard
/// history rows that were captured with a CF_HTML fragment (rich-text copies
/// from Word / Outlook / browsers). Prior to that wiring, clipboard paste was
/// plain-only even when the source had put rich text on the clipboard.
pub(crate) fn write_clipboard_dual(text: &str, html: Option<&str>) -> bool {
    log::info!(
        "[Keyfire] Clipboard write (expansions{}): \"{}\"",
        if html.is_some() { ", +html" } else { "" },
        log_preview(text)
    );
    // TEMP debug (strip after table-paste wild-verify): the exact HTML fragment
    // handed to CF_HTML — table structure issues in target apps diagnose from
    // this line. log_preview's 40 chars is too short to show table markup, so
    // truncate at 600 here.
    if let Some(h) = html {
        let preview: String = h.chars().take(600).collect();
        log::info!("[Keyfire] CF_HTML fragment ({} chars): \"{}\"", h.chars().count(), preview);
    }
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let text_bytes = wide.len() * 2;
    let html_blob = html.map(build_cf_html);

    crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
        .store(true, std::sync::atomic::Ordering::SeqCst);

    for attempt in 0..10 {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                if attempt < 9 { thread::sleep(Duration::from_millis(3)); continue; }
                return false;
            }
            EmptyClipboard();

            // CF_UNICODETEXT — always written, this is the plain-text fallback
            let h_text = GlobalAlloc(GMEM_MOVEABLE, text_bytes);
            if h_text.is_null() {
                CloseClipboard();
                return false;
            }
            let ptr = GlobalLock(h_text) as *mut u16;
            if ptr.is_null() {
                CloseClipboard();
                return false;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            GlobalUnlock(h_text);
            SetClipboardData(CF_UNICODETEXT, h_text);

            // CF_HTML — only when caller provided HTML
            if let Some(ref blob) = html_blob {
                let h_html = GlobalAlloc(GMEM_MOVEABLE, blob.len());
                if !h_html.is_null() {
                    let p = GlobalLock(h_html) as *mut u8;
                    if !p.is_null() {
                        std::ptr::copy_nonoverlapping(blob.as_ptr(), p, blob.len());
                        GlobalUnlock(h_html);
                        SetClipboardData(cf_html_format_id(), h_html);
                    }
                }
            }

            // Keep Keyfire's injected text out of Windows Clipboard History (Win+V)
            // and Cloud Clipboard. Target apps still read CF_UNICODETEXT/CF_HTML
            // normally — these marker formats are only read by the OS clipboard
            // monitor. Must be set while the clipboard is open, after the content.
            mark_clipboard_excluded();

            CloseClipboard();
            // Record the seqnum this write produced so the listener skips it even
            // if the WM_CLIPBOARDUPDATE arrives after the suppress flag is cleared.
            crate::actions::record_self_clipboard_write();
            return true;
        }
    }
    false
}

/// Mark the currently-open clipboard so Windows Clipboard History (Win+V) and
/// Cloud Clipboard skip Keyfire's own injected content. MUST be called while the
/// clipboard is OPEN and AFTER the real content formats have been set. Best
/// effort: any failure is ignored (paste still works; the payload just isn't
/// excluded). Pasting is unaffected — apps read the content formats, not these.
///
/// Three documented registered formats:
/// - `ExcludeClipboardContentFromMonitorProcessing` — presence alone excludes the
///   payload from clipboard monitors / history.
/// - `CanIncludeInClipboardHistory` — DWORD 0 opts out of Win+V history.
/// - `CanUploadToCloudClipboard` — DWORD 0 opts out of cross-device cloud sync.
pub(crate) unsafe fn mark_clipboard_excluded() {
    const NAMES: [&str; 3] = [
        "ExcludeClipboardContentFromMonitorProcessing",
        "CanIncludeInClipboardHistory",
        "CanUploadToCloudClipboard",
    ];
    for name in NAMES {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let id = RegisterClipboardFormatW(wide.as_ptr());
        if id == 0 {
            continue;
        }
        // Each format takes a small HGLOBAL holding a DWORD 0. For the Exclude*
        // format the value is ignored (presence is the signal); the Can* formats
        // read 0 as "no".
        let h = GlobalAlloc(GMEM_MOVEABLE, 4);
        if h.is_null() {
            continue;
        }
        let p = GlobalLock(h) as *mut u32;
        if p.is_null() {
            GlobalFree(h);
            continue;
        }
        *p = 0;
        GlobalUnlock(h);
        if SetClipboardData(id, h).is_null() {
            GlobalFree(h);
        }
    }
}

// ── Multi-format clipboard snapshot / restore ──────────────────────────────
//
// Text-only save/restore (read CF_UNICODETEXT, write CF_UNICODETEXT) silently
// drops every other format the source app put on the clipboard — most damagingly
// CF_DIB images from screenshot tools (Snagit, Snipping Tool, ShareX). After a
// text expansion fires, the image is gone from the Windows clipboard and the
// user's next Ctrl+V pastes Keyfire's expansion text instead of their image.
//
// The fix below snapshots every HGLOBAL-backed format present on the clipboard
// (CF_DIB, CF_DIBV5, CF_HDROP, CF_UNICODETEXT, CF_HTML, RTF, PNG, app-specific
// registered formats, etc.) into a Vec<(format_id, bytes)>, then restores all
// of them after paste settles. The caller uses GetClipboardSequenceNumber to
// skip restoration when the user copied something new during the paste window.

/// Returns true for clipboard formats whose handle is HGLOBAL-backed and safe
/// to snapshot via GlobalSize/GlobalLock. Excludes formats whose handle is a
/// GDI object (HBITMAP, HPALETTE, HENHMETAFILE, HMETAFILEPICT), owner-display
/// formats (source app renders on demand — not transferable), and private /
/// GDI-object format ranges (app-defined cleanup, not safe to copy generically).
///
/// CF_BITMAP / CF_ENHMETAFILE are commonly accompanied by CF_DIB / CF_DIBV5
/// when an image is on the clipboard, so receiving apps still get a working
/// bitmap from the snapshot.
fn is_snapshottable_format(fmt: u32) -> bool {
    match fmt {
        2 | 3 | 9 | 14 => false,           // CF_BITMAP, CF_METAFILEPICT, CF_PALETTE, CF_ENHMETAFILE
        0x80..=0x83 | 0x8E => false,       // CF_OWNERDISPLAY + CF_DSP* variants
        0x200..=0x2FF => false,            // CF_PRIVATEFIRST..CF_PRIVATELAST (app-defined cleanup)
        0x300..=0x3FF => false,            // CF_GDIOBJFIRST..CF_GDIOBJLAST (GDI handles)
        _ => true,
    }
}

/// Snapshot every HGLOBAL-backed format currently on the clipboard. Returns an
/// empty Vec if the clipboard is empty, can't be opened, or contains only
/// unsupported handle types. Caller restores via `restore_clipboard_snapshot`.
///
/// Read-only — does not modify the clipboard, does not touch
/// `SUPPRESS_NEXT_CLIPBOARD_WRITE` (no WM_CLIPBOARDUPDATE fires from reading).
pub(crate) fn snapshot_clipboard() -> Vec<(u32, Vec<u8>)> {
    let mut out: Vec<(u32, Vec<u8>)> = Vec::new();

    // Retry open up to 5 times — clipboard may be briefly held by another process
    let mut opened = false;
    for attempt in 0..5 {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
        }
        if attempt < 4 { thread::sleep(Duration::from_millis(3)); }
    }
    if !opened { return out; }

    unsafe {
        let mut fmt: u32 = 0;
        loop {
            fmt = EnumClipboardFormats(fmt);
            if fmt == 0 { break; }
            if !is_snapshottable_format(fmt) { continue; }

            let handle = GetClipboardData(fmt);
            if handle.is_null() { continue; }

            let size = GlobalSize(handle);
            if size == 0 { continue; }

            let ptr = GlobalLock(handle) as *const u8;
            if ptr.is_null() { continue; }

            let data = std::slice::from_raw_parts(ptr, size).to_vec();
            GlobalUnlock(handle);

            out.push((fmt, data));
        }
        CloseClipboard();
    }
    out
}

/// Minimum wait between sending a paste keystroke and changing the clipboard
/// again (restore or next write). Chromium/Electron targets (eM Client, Slack,
/// browsers) read the clipboard asynchronously in the renderer, tens of ms
/// after the Ctrl+V keydown is delivered — change the clipboard too early and
/// the app pastes whatever is there when it finally reads (the user's OLD
/// content on restore, or the NEXT step's text in multi-step macros). 150ms
/// was the empirical floor for Excel's message-queue paste; eM Client macro
/// reports (2026-08-05) showed 25-50ms losing the race, so the shared floor
/// is 200ms.
pub(crate) const PASTE_RESTORE_SETTLE_MS: u64 = 200;

/// Sync the target window's message queue, then wait — adaptively — until the
/// paste has been read, capped at `max_ms`.
///
/// Phase 1: WM_NULL round-trip (SendMessageTimeoutW) returns once the target
/// thread has pumped every message queued before it — i.e. the paste
/// keystroke has at least been CONSUMED. Synchronous Win32 paste handlers
/// have fully read the clipboard by the time this returns.
///
/// Phase 2: async readers (Chromium/CEF renderers — eM Client, Slack,
/// browsers) fetch the clipboard on their own schedule after the keydown.
/// While reading they hold the clipboard open, so we poll
/// GetOpenClipboardWindow every 3ms: once a foreign open has been observed
/// AND released (with a short grace re-check for multi-format reads that
/// open/close several times), the read is done and we exit early — typically
/// 40-80ms instead of the full cap. If the read is too fast to observe at
/// 3ms polling we simply wait out `max_ms`, which is never worse than the
/// fixed sleep this replaces.
///
/// Call before every clipboard restore/overwrite that follows a paste
/// keystroke, passing at least PASTE_RESTORE_SETTLE_MS as the cap.
pub(crate) fn settle_paste(target_hwnd: isize, max_ms: u64) {
    if target_hwnd != 0 {
        unsafe {
            let mut result: usize = 0;
            windows_sys::Win32::UI::WindowsAndMessaging::SendMessageTimeoutW(
                target_hwnd as _,
                windows_sys::Win32::UI::WindowsAndMessaging::WM_NULL,
                0,
                0,
                windows_sys::Win32::UI::WindowsAndMessaging::SMTO_ABORTIFHUNG,
                500,
                &mut result,
            );
        }
    }
    if max_ms == 0 {
        return;
    }
    const POLL_MS: u64 = 3;
    // Don't trust an observed read completion before this — the renderer may
    // read text first and HTML a beat later, and clipboard-history managers
    // can produce a brief foreign open right after our write.
    const DEFAULT_MIN_WAIT_MS: u64 = 30;
    // Office (Word, Outlook) dispatches Ctrl+V through the ribbon, so its OS
    // clipboard read fires ~60-150ms after the keystroke. Cloud Clipboard's
    // brief ~15ms clipboard-open right after our write can otherwise trip
    // seen_reader early and cause the restore to race Office's actual paste
    // read → the user sees the PRE-fire clipboard content pasted. Extending
    // the min-wait floor and the cap for Office keeps the restore behind
    // Office's read. Tune here if 800ms feels laggy in real use — 500ms is
    // probably the empirical floor before Word starts to miss it on slower
    // hardware, but nothing under 400ms was reliable in initial testing.
    const OFFICE_MIN_WAIT_MS: u64 = 150;
    const OFFICE_MAX_MS: u64 = 800;
    // After the reader releases, re-check once past this grace in case the
    // same read sequence re-opens the clipboard for another format.
    const POST_READ_GRACE_MS: u64 = 20;

    let office = target_is_office(target_hwnd);
    let cap = if office { max_ms.max(OFFICE_MAX_MS) } else { max_ms };
    let min_wait = if office { OFFICE_MIN_WAIT_MS } else { DEFAULT_MIN_WAIT_MS };

    let start = std::time::Instant::now();
    let mut seen_reader = false;
    loop {
        let elapsed = start.elapsed().as_millis() as u64;
        if elapsed >= cap {
            break;
        }
        let open_wnd = unsafe {
            windows_sys::Win32::System::DataExchange::GetOpenClipboardWindow() as isize
        };
        if open_wnd != 0 {
            seen_reader = true;
        } else if seen_reader && elapsed >= min_wait {
            thread::sleep(Duration::from_millis(POST_READ_GRACE_MS));
            let reopened = unsafe {
                windows_sys::Win32::System::DataExchange::GetOpenClipboardWindow() as isize
            };
            if reopened == 0 {
                break; // read complete — safe to change the clipboard
            }
            continue; // another format read in flight — keep waiting
        }
        thread::sleep(Duration::from_millis(POLL_MS));
    }
    // TEMP settle diagnostics (strip after wild-verify): observed=true means
    // the reader was caught in the act and we exited early; false means we
    // waited out the cap.
    log::info!(
        "[Keyfire] settle_paste: observed={} elapsed={}ms cap={}ms office={}",
        seen_reader,
        start.elapsed().as_millis(),
        cap,
        office,
    );
}

/// Restore a snapshot by clearing the clipboard and re-writing every captured
/// format. Sets `SUPPRESS_NEXT_CLIPBOARD_WRITE` true before opening so the
/// listener ignores the WM_CLIPBOARDUPDATE that fires from EmptyClipboard +
/// SetClipboardData. Caller is responsible for clearing the suppress flag
/// after the listener has had a chance to process the event.
///
/// An empty snapshot still calls EmptyClipboard — this matches the pre-state
/// of "clipboard was empty before we wrote our text" and removes Keyfire's
/// expansion text from the Windows clipboard. Returns false if the clipboard
/// couldn't be opened (clipboard contents are then left as Keyfire wrote them).
pub(crate) fn restore_clipboard_snapshot(snapshot: &[(u32, Vec<u8>)]) -> bool {
    log::info!("[Keyfire] Clipboard restore: {} formats", snapshot.len());
    crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
        .store(true, std::sync::atomic::Ordering::SeqCst);

    // Retry open up to 10 times — clipboard may be briefly held by the listener
    let mut opened = false;
    for attempt in 0..10 {
        unsafe {
            if OpenClipboard(std::ptr::null_mut()) != 0 {
                opened = true;
                break;
            }
        }
        if attempt < 9 { thread::sleep(Duration::from_millis(3)); }
    }
    if !opened {
        log::warn!("[Keyfire] restore_clipboard_snapshot: OpenClipboard failed after retries");
        return false;
    }

    unsafe {
        EmptyClipboard();
        for (fmt, data) in snapshot {
            let h_mem = GlobalAlloc(GMEM_MOVEABLE, data.len());
            if h_mem.is_null() { continue; }
            let ptr = GlobalLock(h_mem) as *mut u8;
            if ptr.is_null() {
                GlobalFree(h_mem);
                continue;
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr, data.len());
            GlobalUnlock(h_mem);
            // SetClipboardData takes ownership of h_mem on success. On failure
            // we own it and must free, otherwise the HGLOBAL leaks.
            if SetClipboardData(*fmt, h_mem).is_null() {
                GlobalFree(h_mem);
            }
        }
        // The restore is a mechanical re-write of the user's prior content, not a
        // fresh user copy — keep it out of Win+V (the original is already in
        // history from when the user first copied it; this also avoids a
        // duplicate and protects sensitive originals we couldn't fully snapshot).
        mark_clipboard_excluded();
        CloseClipboard();
        // Record the seqnum so the listener skips this restore's update event.
        crate::actions::record_self_clipboard_write();
    }
    true
}

/// Wraps `GetClipboardSequenceNumber` for the "did anyone else write to the
/// clipboard during our paste window?" guard. The OS bumps this every time the
/// clipboard contents change (any process — Keyfire's own writes count too).
pub(crate) fn clipboard_sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// Resolve token chips inside an HTML expansion. Each
/// `<span class="rte-token" data-token="{...}">display</span>` is replaced
/// with the HTML-escaped resolved value of its token.
///
/// `{cursor}` chips are stripped entirely — the cursor_back count is derived
/// from the plain-text path which runs alongside.
fn resolve_tokens_html(
    html: &str,
    global_vars: &HashMap<String, String>,
    fillin_values: &HashMap<String, String>,
) -> String {
    // Replace every chip span with its raw data-token text inline. This mirrors
    // the frontend's htmlToPlainText chip-flattening behaviour so the engine
    // sees the same content as the plain-text path.
    let re = match regex_lite::Regex::new(
        r#"<span\b[^>]*?\bdata-token="([^"]*)"[^>]*>[^<]*</span>"#
    ) {
        Ok(r) => r,
        Err(_) => return html.to_string(),
    };
    let html_inline = re.replace_all(html, |caps: &regex_lite::Captures| {
        let raw = caps.get(1).map(|t| t.as_str()).unwrap_or("");
        // {cursor} produces no visible output in HTML — strip its span entirely.
        if raw == "{cursor}" { return String::new(); }
        // Captured group is the RAW attribute text from the serialized HTML,
        // which still has entities. The engine expects literal characters
        // (e.g. `>` not `&gt;`), so decode the common entities here.
        decode_html_entities(raw)
    }).to_string();

    // Run the resolver ONCE across the full inlined HTML. Formatting tags
    // (`<strong>`, `<em>`, `<span style="...">`, etc.) don't match the
    // `{set}`/`{if}`/`{=}` token patterns the engine scans for, so they pass
    // through unchanged. Cross-chip dependencies work because every chip's
    // tokens are now visible to a single resolve pass — a `{set foo = …}` in
    // chip A populates scope before a `{=foo}` in chip B is evaluated.
    //
    // Caveat: substituted values are NOT HTML-escaped, so if a fill-in or
    // formula result contains literal `<` / `&` / `"` chars they'll render as
    // raw HTML in the target. Invoice numbers / dates / typical text are
    // safe; document if a user trips this.
    let (resolved, _) = resolve_tokens(&html_inline, global_vars, fillin_values);

    // Strip residual ZWSPs (editor cursor anchors serialized by innerHTML).
    resolved.replace('\u{200B}', "")
}

// ── Hybrid injection — SendInput for short text, clipboard for long/terminal ─

fn should_use_clipboard(_resolved_text: &str) -> bool {
    true
}

/// Apps whose terminals route Ctrl+V through to a child pty where bash readline
/// interprets it as `quoted-insert` (NOT paste). For these, force Shift+Insert
/// so the terminal emulator (xterm.js etc.) handles the paste in its renderer
/// before the keystroke reaches the shell. Add new entries as reports come in.
pub(crate) fn target_needs_shift_insert(target_hwnd: isize) -> bool {
    if target_hwnd == 0 {
        return false;
    }
    matches!(
        crate::foreground::proc_name_for_hwnd(target_hwnd).as_deref(),
        Some("code"),
    )
}

/// True for Office desktop apps whose paste is dispatched through the ribbon
/// (Word, Outlook confirmed 2026-08-20). The OS clipboard read fires ~60-150ms
/// after Ctrl+V — well after the fast paths (Notepad, Chromium, eM Client)
/// have completed. `settle_paste` uses this to extend its min-wait floor and
/// its cap so the clipboard restore never races Office's delayed paste read.
///
/// If a user reports the same stale-paste symptom in PowerPoint or Excel, add
/// their process name here (`powerpnt`, `excel`).
pub(crate) fn target_is_office(target_hwnd: isize) -> bool {
    if target_hwnd == 0 {
        return false;
    }
    matches!(
        crate::foreground::proc_name_for_hwnd(target_hwnd).as_deref(),
        Some("winword") | Some("outlook"),
    )
}

/// Inject text via batched KEYEVENTF_UNICODE SendInput (single call).
fn inject_via_sendinput(text: &str, target_hwnd: isize) {
    log::info!("[Keyfire] Inject (sendinput): \"{}\"", log_preview(text));
    // Release physically held modifiers
    let held = crate::actions::release_held_modifiers();

    // Restore focus to target window
    if target_hwnd != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(target_hwnd as _);
        }
        thread::sleep(Duration::from_millis(10));
    }

    // Build batched INPUT array — down+up per UTF-16 code unit
    // Surrogate pairs are handled automatically by encode_utf16()
    let utf16: Vec<u16> = text.encode_utf16().collect();
    let mut inputs: Vec<INPUT> = Vec::with_capacity((utf16.len() * 2) + 2);
    for &code_unit in &utf16 {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: code_unit,
                    dwFlags: KEYEVENTF_UNICODE,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: 0,
                    wScan: code_unit,
                    dwFlags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }

    // Trailing space as VK_SPACE (not KEYEVENTF_UNICODE — some apps strip trailing whitespace)
    inputs.push(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_SPACE as _,
                wScan: 0,
                dwFlags: 0,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });
    inputs.push(INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VK_SPACE as _,
                wScan: 0,
                dwFlags: KEYEVENTF_KEYUP,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    });

    // Single SendInput call — atomic delivery, no interleaving
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }

    // Re-press modifiers that were physically held
    crate::actions::restore_modifiers(&held);
}

/// Inject text via clipboard paste, restoring clipboard afterwards.
/// When `html` is provided, also writes CF_HTML so target apps that accept
/// rich text (Word, Outlook, Gmail, Slack, Teams) receive formatted content.
/// Apps that don't accept CF_HTML automatically fall back to CF_UNICODETEXT.
fn inject_via_clipboard(text: &str, html: Option<&str>, target_hwnd: isize) {
    log::info!("[Keyfire] Inject (clipboard): \"{}\"", log_preview(text));
    // Snapshot every format currently on the clipboard. Text-only save misses
    // CF_DIB images (Snagit, Snipping Tool), CF_HDROP file drops, RTF from
    // Word, and registered formats from Office/Chromium — leaving Keyfire's
    // expansion text on the clipboard when the user's next Ctrl+V expected
    // the image they had a moment ago.
    let snapshot = snapshot_clipboard();

    let needs_shift_insert = target_needs_shift_insert(target_hwnd);

    // Always bundle the trailing space into the clipboard payload. Sending it
    // as a separate VK_SPACE keystroke after the paste keys races against
    // async paste handlers in Chromium/Electron apps:
    //   eM Client → keystroke wins the race, lands before paste → " <text>"
    //   Slack/Notion → keystroke arrives during the post-paste React re-render
    //                  and is dropped entirely → "<text>" (no space at all)
    //   Notepad/Word → synchronous paste, keystroke lands after → "<text> "
    // Bundling into the clipboard makes the space part of the atomic paste
    // payload — no separate event for any target app to lose or reorder.
    //
    // HTML uses &nbsp; rather than a literal space because HTML parsers
    // (Word, Outlook, eM Client rich-text editor) strip whitespace between
    // block elements during parse — a literal " " after the closing </p>
    // never makes it into the rendered document. &nbsp; is a character
    // node that survives parsing and renders identically to a regular space.
    //
    // CRITICAL: &nbsp; must go INSIDE the last closing tag, not after it.
    // Appending after `</p>` causes Gmail / eM Client / similar rich-text
    // editors to wrap the stray text node in a new paragraph — producing
    // an awkward blank line below the expansion with the cursor on it.
    // Inserting before the close-tag keeps the nbsp in the same block as
    // the rest of the content so the cursor lands inline at the end.
    let payload_text = format!("{} ", text);
    let payload_html: Option<String> = html.map(|h| {
        // Highlight/colour normalisation for target-app compatibility, applied
        // at fire time so already-saved expansions fix themselves without a
        // config migration:
        //  1. background-color → background shorthand (Chromium's hiliteColor
        //     writes the long form; some readers only take the shorthand).
        //  2. rgb() colour functions → hex (Word's reader drops rgb() values,
        //     for text colours as well as backgrounds; hex parses everywhere).
        //  3. background:#hex gains mso-highlight:<name> — Word IGNORES css
        //     backgrounds on inline spans entirely; mso-highlight is its only
        //     character-highlight channel (16 fixed colours). Modern engines
        //     ignore the mso-* property and render the background hex.
        let normalised = add_mso_highlight(
            &rgb_functions_to_hex(&h.replace("background-color:", "background:")),
        );
        let trimmed = normalised.trim_end();
        if let Some(idx) = trimmed.rfind("</") {
            let close_tag = &trimmed[idx..];
            // Table structural closing tags nest — inserting text between
            // </tbody> and </table> (or between </td> and </tr>) breaks the
            // table and downstream parsers rebuild it as a single flattened
            // cell. Append a fresh paragraph AFTER the fragment instead so
            // the caret still lands below the table without corrupting rows.
            let is_table_close = close_tag.starts_with("</table")
                || close_tag.starts_with("</tbody")
                || close_tag.starts_with("</thead")
                || close_tag.starts_with("</tfoot")
                || close_tag.starts_with("</tr")
                || close_tag.starts_with("</td")
                || close_tag.starts_with("</th");
            if is_table_close {
                format!("{}<p>&nbsp;</p>", trimmed)
            } else {
                let before = &trimmed[..idx];
                format!("{}&nbsp;{}", before, close_tag)
            }
        } else {
            format!("{}&nbsp;", trimmed)
        }
    });

    // Write replacement to clipboard — if this fails, do NOT paste (would paste old clipboard content)
    if !write_clipboard_dual(&payload_text, payload_html.as_deref()) {
        log::warn!("[Keyfire] write_clipboard FAILED — skipping paste to avoid pasting wrong content");
        return;
    }
    let post_write_seq = clipboard_sequence_number();

    // Release physically held modifiers
    let held = crate::actions::release_held_modifiers();

    // Restore focus to target window
    if target_hwnd != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                target_hwnd as _,
            );
        }
        thread::sleep(Duration::from_millis(10));
    }

    let use_ctrl_v = !is_ctrl_v_mapped() && !needs_shift_insert;
    if use_ctrl_v {
        send_vk_key(0xA2, false); // LCtrl
        send_vk_key(0x56, false); // V
        send_vk_key(0x56, true);
        send_vk_key(0xA2, true);
    } else {
        send_vk_key(VK_LSHIFT, false);
        send_vk_key_extended(VK_INSERT, false);
        send_vk_key_extended(VK_INSERT, true);
        send_vk_key(VK_LSHIFT, true);
    }

    // No separate trailing VK_SPACE — the space is bundled into the clipboard
    // payload above to avoid the async-paste race in Chromium/Electron targets.

    // Re-press modifiers that were physically held
    crate::actions::restore_modifiers(&held);

    // Restore clipboard after the paste has settled: queue-sync guarantees the
    // target consumed the Ctrl+V keydown, the floor delay covers async readers
    // (Chromium renderers, Excel's message-queue paste — see the constant doc).
    settle_paste(target_hwnd, PASTE_RESTORE_SETTLE_MS);
    // Only restore if the clipboard still holds our content. If the sequence
    // number advanced, the user (or another process) copied something new during
    // the paste window — leave their content alone.
    if clipboard_sequence_number() == post_write_seq {
        restore_clipboard_snapshot(&snapshot);
    }
    crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
        .store(false, std::sync::atomic::Ordering::SeqCst);
}

// ── Image expansion ────────────────────────────────────────────────────────

/// Write image to the clipboard as CF_DIB + PNG stream (no text formats).
/// CF_DIB provides universal bitmap support. PNG stream is preferred by Word, Outlook, etc.
/// `raw_png_bytes` is the original file bytes when the source is PNG, or re-encoded PNG bytes.
fn write_clipboard_image(pixels: &[u8], width: u32, height: u32, raw_png_bytes: &[u8]) -> bool {
    // BITMAPINFOHEADER is 40 bytes
    let header_size: u32 = 40;
    let row_stride = (width * 4) as usize; // BGRA = 4 bytes per pixel
    let pixel_data_size = row_stride * height as usize;
    let total_size = header_size as usize + pixel_data_size;

    crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
        .store(true, std::sync::atomic::Ordering::SeqCst);

    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();

        // ── CF_DIB: BITMAPINFOHEADER + pixel data ──
        let h_dib = GlobalAlloc(GMEM_MOVEABLE, total_size);
        if h_dib.is_null() {
            CloseClipboard();
            return false;
        }
        let ptr = GlobalLock(h_dib) as *mut u8;
        if ptr.is_null() {
            CloseClipboard();
            return false;
        }

        // Write BITMAPINFOHEADER manually (40 bytes)
        let header_ptr = ptr as *mut u32;
        // biSize
        *header_ptr = header_size;
        // biWidth
        *header_ptr.add(1) = width;
        // biHeight (positive = bottom-up, which is what we provide)
        *header_ptr.add(2) = height;
        // biPlanes (u16) + biBitCount (u16) packed as u32
        let planes_and_bits: u32 = 1 | (32 << 16); // planes=1, bitCount=32
        *header_ptr.add(3) = planes_and_bits;
        // biCompression = BI_RGB = 0
        *header_ptr.add(4) = 0;
        // biSizeImage
        *header_ptr.add(5) = pixel_data_size as u32;
        // biXPelsPerMeter
        *header_ptr.add(6) = 0;
        // biYPelsPerMeter
        *header_ptr.add(7) = 0;
        // biClrUsed
        *header_ptr.add(8) = 0;
        // biClrImportant
        *header_ptr.add(9) = 0;

        // Write pixel data after header
        let pixel_dest = ptr.add(header_size as usize);
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), pixel_dest, pixel_data_size);

        GlobalUnlock(h_dib);
        SetClipboardData(CF_DIB, h_dib as _);

        // ── PNG stream: preferred by Word, Outlook, browsers ──
        if !raw_png_bytes.is_empty() {
            let png_format_name: Vec<u16> = "PNG\0".encode_utf16().collect();
            let png_format_id = RegisterClipboardFormatW(png_format_name.as_ptr());
            if png_format_id != 0 {
                let h_png = GlobalAlloc(GMEM_MOVEABLE, raw_png_bytes.len());
                if !h_png.is_null() {
                    let png_ptr = GlobalLock(h_png) as *mut u8;
                    if !png_ptr.is_null() {
                        std::ptr::copy_nonoverlapping(raw_png_bytes.as_ptr(), png_ptr, raw_png_bytes.len());
                        GlobalUnlock(h_png);
                        SetClipboardData(png_format_id, h_png as _);
                    }
                }
            }
        }

        // Keep Keyfire's injected image out of Win+V / Cloud Clipboard (same as text).
        mark_clipboard_excluded();
        CloseClipboard();
        // Record the seqnum so the listener skips this image write's update event.
        crate::actions::record_self_clipboard_write();
        true
    }
}

/// Fire a variant expansion. `random_variant = false` shows the variant picker;
/// `random_variant = true` picks one option at random with no popup (nanos-based,
/// mirrors `random()` in expression.rs). Either way, the picked variant's text
/// re-prompts for `{fillIn:...}` tokens if it carries any, then resolves globals
/// / clipboard / date / expressions, and injects via clipboard.
fn fire_variant_expansion(
    trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    options: &[serde_json::Value],
    global_vars: &HashMap<String, String>,
    random_variant: bool,
) {
    if !fire_rate_ok(trigger) {
        return;
    }

    let app = match APP_HANDLE.get() {
        Some(a) => a.clone(),
        None => return,
    };

    // Capture target HWND before showing popup / injecting
    let target_hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };

    // Erase the trigger text (runs either way — random skips the picker but still
    // needs the trigger removed from the target buffer).
    {
        let _guard = InjectionGuard::new();
        crate::hotkeys::SUPPRESS_SIMULATED.store(true, std::sync::atomic::Ordering::SeqCst);
        let erase_count = trigger_len + if delete_extra { 1 } else { 0 };
        for _ in 0..erase_count {
            crate::actions::send_vk_key_pub(0x08, false); // VK_BACKSPACE down
            crate::actions::send_vk_key_pub(0x08, true);  // VK_BACKSPACE up
            thread::sleep(Duration::from_millis(2));
        }
        crate::hotkeys::SUPPRESS_SIMULATED.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    // Load theme once for any fill-in window shown by this call (picker OR the
    // post-pick fill-in re-prompt in the tail — both need it).
    let theme = crate::config::load_config()
        .and_then(|c| c.get("theme").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "dark".to_string());

    // Pick a variant. Random mode short-circuits before the popup path so no
    // FILL_IN_ACTIVE toggle is taken and no window is shown; picker mode does
    // the full popup dance. Both branches produce (selected_text, selected_html).
    let (selected_text, selected_html): (String, Option<String>) = if random_variant {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64 ^ (d.as_secs() as u64))
            .unwrap_or(0);
        let idx = (nanos as usize) % options.len();
        let picked = &options[idx];
        let label = picked.get("label").and_then(|v| v.as_str()).unwrap_or("Option");
        log::info!(
            "[Keyfire] Random variant pick: \"{}\" → \"{}\" (index {} of {})",
            trigger, label, idx, options.len()
        );
        let t = picked.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let h = picked.get("html").and_then(|v| v.as_str()).map(String::from);
        (t, h)
    } else {
        crate::hotkeys::FILL_IN_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);

        // Set true only when the JS side ACKs render within 2s. Governs
        // whether we bother waiting on the response channel — see the
        // fill_in_shown_tx() comment for the failure mode this catches.
        let mut shown_ok = false;

        // Build options list for the popup
        let option_labels: Vec<String> = options.iter().map(|opt| {
        opt.get("label").and_then(|v| v.as_str()).unwrap_or("Option").to_string()
    }).collect();

    // Build 1-line plain-text previews for the picker: first non-empty line of
    // the variant body, hard-truncated to 80 chars. Picker CSS clamps further.
    let option_previews: Vec<String> = options.iter().map(|opt| {
        let raw = opt.get("text").and_then(|v| v.as_str()).unwrap_or("");
        let first_line = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
        if first_line.chars().count() > 80 {
            let truncated: String = first_line.chars().take(80).collect();
            format!("{}…", truncated)
        } else {
            first_line.to_string()
        }
    }).collect();

    // Create response channel — reuse fill_in_tx
    let (tx, rx) = mpsc::channel();
    *fill_in_tx().lock().unwrap() = Some(tx);

    // Show fill-in window in variant selection mode
    if let Some(win) = app.get_webview_window("fillin") {
        use tauri::Emitter;

        if let Ok(hwnd) = win.hwnd() {
            crate::hotkeys::FILLIN_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
        }

        // Position fill-in on the active monitor (where cursor is), not just primary
        {
            use windows_sys::Win32::Foundation::POINT;
            use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
            use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

            let scale = win.scale_factor().unwrap_or(1.0);
            let (cx, cy) = unsafe {
                let mut pt = POINT { x: 0, y: 0 };
                GetCursorPos(&mut pt);
                (pt.x, pt.y)
            };
            let (wa_left, wa_top, wa_right, wa_bottom) = unsafe {
                let pt = POINT { x: cx, y: cy };
                let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
                let mut mi: MONITORINFO = std::mem::zeroed();
                mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(hmon, &mut mi) != 0 {
                    (mi.rcWork.left, mi.rcWork.top, mi.rcWork.right, mi.rcWork.bottom)
                } else {
                    (0, 0, 1920, 1080)
                }
            };
            let log_left = wa_left as f64 / scale;
            let log_top  = wa_top  as f64 / scale;
            let log_w = (wa_right  - wa_left)  as f64 / scale;
            let log_h = (wa_bottom - wa_top) as f64 / scale;
            let win_w = 420.0;
            let x = log_left + (log_w - win_w) / 2.0;
            let y = log_top + log_h / 3.0;
            let _ = win.set_position(tauri::LogicalPosition::new(x, y));
        }

        // Wake a suspended webview BEFORE show/emit — see webview_mem.rs invariant.
        crate::webview_mem::resume_for_show(&app, "fillin");
        let _ = win.show();
        let _ = win.set_focus();

        let _ = win.emit("fill-in-request-ready", serde_json::json!({}));
        let (ready_tx, ready_rx) = mpsc::channel();
        *fill_in_ready_tx().lock().unwrap() = Some(ready_tx);
        let _ = ready_rx.recv_timeout(Duration::from_secs(5));
        *fill_in_ready_tx().lock().unwrap() = None;

        // Emit variant-select mode with options
        let _ = win.emit("fill-in-show", serde_json::json!({
            "mode": "variant",
            "options": option_labels,
            "previews": option_previews,
            "theme": theme,
        }));

        // Wait for JS ACK that the picker rendered. Same rationale as
        // run_fill_in_window — in dev, WebView2/HMR can drop the emit and we
        // must not block 60s on a picker that never appeared.
        let (shown_tx, shown_rx) = mpsc::channel::<()>();
        *fill_in_shown_tx().lock().unwrap() = Some(shown_tx);
        shown_ok = shown_rx.recv_timeout(Duration::from_secs(2)).is_ok();
        *fill_in_shown_tx().lock().unwrap() = None;
        if !shown_ok {
            log::warn!("[Keyfire] variant picker did not ACK render within 2s — aborting");
        }
    }

    // Wait for selection (30s timeout, previously 60s). If the picker never
    // rendered, skip the wait entirely so cleanup runs immediately.
    let response = if shown_ok {
        rx.recv_timeout(Duration::from_secs(30))
    } else {
        Err(mpsc::RecvTimeoutError::Timeout)
    };
    *fill_in_tx().lock().unwrap() = None;

    // Clean up
    crate::hotkeys::FILLIN_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
    if let Some(win) = app.get_webview_window("fillin") {
        let _ = win.hide();
    }
    if target_hwnd != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(target_hwnd as _);
        }
        thread::sleep(Duration::from_millis(10));
    }
    crate::hotkeys::FILL_IN_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);

    // Process selection — pull both text and html (html absent in legacy
    // {label, text} variants; we just paste plain in that case).
    match response {
        Ok(Some(values)) => {
            // The fill-in window sends back {"__variant_index": "0"} for variant mode
            if let Some(idx_str) = values.get("__variant_index") {
                if let Ok(idx) = idx_str.parse::<usize>() {
                    if idx < options.len() {
                        let t = options[idx]
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let h = options[idx]
                            .get("html")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        (t, h)
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            } else {
                return;
            }
        }
        _ => return,
    }
    }; // closes: let (selected_text, selected_html) = if random_variant { ... } else { ... }

    if selected_text.is_empty() {
        return;
    }

    // If the selected variant contains {fillIn:LABEL} tokens, re-prompt the
    // user for those values before injecting. HTML is also resolved alongside
    // text using the collected fill-in values so rich-text targets keep their
    // formatting even when fill-ins are involved.
    let fill_in_fields = extract_fill_in_fields(&selected_text);
    let (final_text, final_html, fillin_values) = if !fill_in_fields.is_empty() {
        // Re-acquire FILL_IN_ACTIVE before re-showing the window
        crate::hotkeys::FILL_IN_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);

        let (tx2, rx2) = mpsc::channel();
        *fill_in_tx().lock().unwrap() = Some(tx2);
        // Set true only when JS acks render within 2s — same shown-ACK guard
        // as the picker branch. Failure here means the re-prompt window never
        // appeared, so we short-circuit the response wait.
        let mut shown_ok = false;

        if let Some(win) = app.get_webview_window("fillin") {
            use tauri::Emitter;

            if let Ok(hwnd) = win.hwnd() {
                crate::hotkeys::FILLIN_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
            }

            // Wake a suspended webview BEFORE show/emit — see webview_mem.rs invariant.
            crate::webview_mem::resume_for_show(&app, "fillin");
            let _ = win.show();
            let _ = win.set_focus();

            let _ = win.emit("fill-in-request-ready", serde_json::json!({}));
            let (ready_tx, ready_rx) = mpsc::channel();
            *fill_in_ready_tx().lock().unwrap() = Some(ready_tx);
            let _ = ready_rx.recv_timeout(Duration::from_secs(5));
            *fill_in_ready_tx().lock().unwrap() = None;

            let _ = win.emit("fill-in-show", serde_json::json!({
                "fields": fill_in_fields,
                "theme": theme,
            }));

            let (shown_tx, shown_rx) = mpsc::channel::<()>();
            *fill_in_shown_tx().lock().unwrap() = Some(shown_tx);
            shown_ok = shown_rx.recv_timeout(Duration::from_secs(2)).is_ok();
            *fill_in_shown_tx().lock().unwrap() = None;
            if !shown_ok {
                log::warn!("[Keyfire] variant post-pick fill-in did not ACK render within 2s — aborting");
            }
        }

        // 30s instead of 60s, plus short-circuit if the picker never rendered.
        let response2 = if shown_ok {
            rx2.recv_timeout(Duration::from_secs(30))
        } else {
            Err(mpsc::RecvTimeoutError::Timeout)
        };
        *fill_in_tx().lock().unwrap() = None;

        // Clean up window + restore focus before injection
        crate::hotkeys::FILLIN_HWND.store(0, std::sync::atomic::Ordering::SeqCst);
        if let Some(win) = app.get_webview_window("fillin") {
            let _ = win.hide();
        }
        if target_hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(target_hwnd as _);
            }
            thread::sleep(Duration::from_millis(10));
        }
        crate::hotkeys::FILL_IN_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);

        let values = match response2 {
            Ok(Some(v)) => v,
            _ => return, // user cancelled
        };

        let substituted = resolve_fill_in_tokens(&selected_text, &values);
        // Substitute fill-in tokens in the HTML too (legacy plain-text fillin
        // tokens live in the editor's text content, not chip spans). Chip-
        // embedded fillin references resolve later via resolve_tokens_html
        // using fillin_values.
        let html_with_fillins = selected_html.as_deref()
            .map(|h| resolve_fill_in_tokens(h, &values));
        (substituted, html_with_fillins, values)
    } else {
        (selected_text, selected_html, HashMap::new())
    };

    let (resolved, cursor_back) = resolve_tokens(&final_text, global_vars, &fillin_values);
    if resolved.is_empty() {
        return;
    }

    // Resolve HTML in parallel when present. Skip if the variant uses inline
    // key tokens — those need per-segment injection that doesn't compose with
    // a single paste, same rule as the main expansion path (line 546).
    let resolved_html: Option<String> = final_html.and_then(|h| {
        if h.is_empty() || h.contains("{key:") {
            None
        } else {
            Some(resolve_tokens_html(&h, global_vars, &fillin_values))
        }
    });

    crate::analytics::log_action("expansion", resolved.chars().filter(|c| *c != '\r').count() as u32, trigger, trigger);

    // Inject the selected text
    while crate::hotkeys::INJECTION_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(5));
    }

    let guard = InjectionGuard::new();
    let _guard = guard;

    crate::hotkeys::SUPPRESS_SIMULATED.store(true, std::sync::atomic::Ordering::SeqCst);
    inject_via_clipboard(&resolved, resolved_html.as_deref(), target_hwnd);

    if cursor_back > 0 {
        thread::sleep(Duration::from_millis(10));
        send_left_arrows_batch(cursor_back);
    }

    crate::hotkeys::SUPPRESS_SIMULATED.store(false, std::sync::atomic::Ordering::SeqCst);

    // Trailing space is bundled into the clipboard payload by inject_via_clipboard
    // above. No separate VK_SPACE keystroke is needed — sending one here was
    // redundant (double-space) on top of the bundled space.
}

/// Fire an image expansion: read image from disk, optionally resize, write to clipboard, paste.
fn fire_image_expansion(
    _trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    image_path: &str,
    image_scale: u32,
) {
    if !fire_rate_ok(_trigger) {
        return;
    }
    use image::GenericImageView;

    // Check file exists
    if !std::path::Path::new(image_path).exists() {
        log::warn!("[Keyfire] Image expansion: file not found at \"{}\"", image_path);
        return;
    }

    // Read file bytes
    let file_bytes = match std::fs::read(image_path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("[Keyfire] Image expansion: failed to read \"{}\": {}", image_path, e);
            return;
        }
    };

    // Detect format from extension
    let ext = std::path::Path::new(image_path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    let format = match ext.as_str() {
        "png" => image::ImageFormat::Png,
        "jpg" | "jpeg" => image::ImageFormat::Jpeg,
        _ => {
            log::warn!("[Keyfire] Image expansion: unsupported format \"{}\"", ext);
            return;
        }
    };

    // Decode image
    let mut img = match image::load_from_memory_with_format(&file_bytes, format) {
        Ok(i) => i,
        Err(e) => {
            log::warn!("[Keyfire] Image expansion: failed to decode \"{}\": {}", image_path, e);
            return;
        }
    };

    // Resize if scale < 100
    let scale = image_scale.clamp(10, 100);
    if scale < 100 {
        let (w, h) = img.dimensions();
        let new_w = (w as f64 * scale as f64 / 100.0).round() as u32;
        let new_h = (h as f64 * scale as f64 / 100.0).round() as u32;
        if new_w > 0 && new_h > 0 {
            img = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);
        }
    }

    let (width, height) = img.dimensions();
    let rgba = img.to_rgba8();

    // Convert RGBA → BGRA and flip rows vertically (DIB is bottom-up)
    let row_stride = (width * 4) as usize;
    let mut bgra_bottom_up = vec![0u8; row_stride * height as usize];
    for y in 0..height as usize {
        let src_row = &rgba.as_raw()[y * row_stride..(y + 1) * row_stride];
        let dst_y = (height as usize - 1) - y;
        let dst_row = &mut bgra_bottom_up[dst_y * row_stride..(dst_y + 1) * row_stride];
        for x in 0..width as usize {
            let si = x * 4;
            dst_row[si] = src_row[si + 2];     // B
            dst_row[si + 1] = src_row[si + 1]; // G
            dst_row[si + 2] = src_row[si];     // R
            dst_row[si + 3] = src_row[si + 3]; // A
        }
    }

    // Build PNG bytes for the PNG clipboard stream.
    // If source is PNG and no resize was applied, use the original file bytes directly.
    // Otherwise, re-encode the (possibly resized) image as PNG.
    let png_bytes = if ext == "png" && scale == 100 {
        file_bytes
    } else {
        let mut buf = std::io::Cursor::new(Vec::new());
        if img.write_to(&mut buf, image::ImageFormat::Png).is_ok() {
            buf.into_inner()
        } else {
            Vec::new()
        }
    };

    crate::analytics::log_action("expansion", 0, _trigger, _trigger);

    // Capture target HWND
    let target_hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };

    // Wait for any prior injection to finish
    while crate::hotkeys::INJECTION_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(5));
    }

    let guard = InjectionGuard::new();

    thread::spawn(move || {
        let _guard = guard;

        // Delay to let the trigger keystroke be processed
        thread::sleep(Duration::from_millis(30));

        crate::hotkeys::SUPPRESS_SIMULATED
            .store(true, std::sync::atomic::Ordering::SeqCst);

        // Delete trigger word + space (if applicable)
        let delete_count = trigger_len + if delete_extra { 1 } else { 0 };
        for _ in 0..delete_count {
            send_vk_tap(VK_BACKSPACE);
            thread::sleep(Duration::from_millis(5));
        }

        thread::sleep(Duration::from_millis(10));

        // Write image to clipboard
        write_clipboard_image(&bgra_bottom_up, width, height, &png_bytes);

        // Release physically held modifiers
        let held = crate::actions::release_held_modifiers();

        // Restore focus to target window
        if target_hwnd != 0 {
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                    target_hwnd as _,
                );
            }
            thread::sleep(Duration::from_millis(10));
        }

        // Fire Ctrl+V or Shift+Insert
        let use_ctrl_v = !is_ctrl_v_mapped();
        if use_ctrl_v {
            send_vk_key(0xA2, false); // LCtrl
            send_vk_key(0x56, false); // V
            send_vk_key(0x56, true);
            send_vk_key(0xA2, true);
        } else {
            send_vk_key(VK_LSHIFT, false);
            send_vk_key(VK_INSERT, false);
            send_vk_key(VK_INSERT, true);
            send_vk_key(VK_LSHIFT, true);
        }

        // No trailing space for image paste

        // Re-press modifiers that were physically held
        crate::actions::restore_modifiers(&held);

        // No clipboard restore for images — leave image on clipboard

        crate::hotkeys::SUPPRESS_SIMULATED
            .store(false, std::sync::atomic::Ordering::SeqCst);
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(false, std::sync::atomic::Ordering::SeqCst);

        // Replay buffered keystrokes and re-check triggers. The helper takes
        // the guard and releases it BEFORE the re-checks — see its doc comment.
        replay_buffered_and_recheck(_guard);
    });
}

fn is_ctrl_v_mapped() -> bool {
    let state = crate::hotkeys::engine_state().lock().unwrap();
    let profile = &state.active_profile;
    let key = format!("{}::Ctrl::KeyV", profile);
    state.assignments.contains_key(&key)
}

// ── Key token support ({key:Combo:N}, {key:Combo}, …) ──────────────────────

enum KeySegment {
    Text(String),
    Key { mod_vks: Vec<u16>, main_vk: u16, repeat: u32 },
}

/// Split `text` on `{key:...}` tokens, returning text and key segments.
/// Token format: `{key:Combo:N}` (e.g. `{key:Ctrl+F4:2}`) or legacy `{key:Combo}` (N=1).
fn parse_key_segments(text: &str) -> Vec<KeySegment> {
    let mut segments = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{key:") {
        if start > 0 {
            segments.push(KeySegment::Text(rest[..start].to_string()));
        }
        let after = &rest[start + 5..]; // skip "{key:"
        if let Some(end) = after.find('}') {
            let token_body = &after[..end];
            // Parse optional repeat count: "Combo:N" or just "Combo"
            let (combo, repeat) = if let Some(colon_pos) = token_body.rfind(':') {
                let potential_num = &token_body[colon_pos + 1..];
                if let Ok(n) = potential_num.parse::<u32>() {
                    (&token_body[..colon_pos], n.max(1))
                } else {
                    (token_body, 1u32)
                }
            } else {
                (token_body, 1u32)
            };
            if let Some((mod_vks, main_vk)) = combo_str_to_vks(combo) {
                segments.push(KeySegment::Key { mod_vks, main_vk, repeat });
            }
            rest = &after[end + 1..];
        } else {
            // Malformed token — treat remainder as text
            segments.push(KeySegment::Text(rest[start..].to_string()));
            rest = "";
            break;
        }
    }
    if !rest.is_empty() {
        segments.push(KeySegment::Text(rest.to_string()));
    }
    segments
}

/// Parse a combo string like "Ctrl+F4" or "Shift+Tab" or just "Tab" into
/// (modifier VKs, main VK). Modifier order is: Ctrl, Shift, Alt, Win.
fn combo_str_to_vks(combo: &str) -> Option<(Vec<u16>, u16)> {
    let parts: Vec<&str> = combo.split('+').collect();
    if parts.is_empty() {
        return None;
    }
    let mut mod_vks = Vec::new();
    // Everything before the last part is a modifier; last part is the key.
    let main_part = parts[parts.len() - 1];
    for &part in &parts[..parts.len().saturating_sub(1)] {
        match part {
            "Ctrl"  => mod_vks.push(VK_LCONTROL),
            "Shift" => mod_vks.push(VK_LSHIFT),
            "Alt"   => mod_vks.push(VK_LALT),
            "Win"   => mod_vks.push(VK_LWIN),
            _ => {}
        }
    }
    key_name_to_vk(main_part).map(|vk| (mod_vks, vk))
}

fn key_name_to_vk(name: &str) -> Option<u16> {
    match name {
        "Tab"                          => Some(0x09),
        "Enter"                        => Some(0x0D),
        "Escape" | "Esc"               => Some(0x1B),
        "Backspace"                    => Some(0x08),
        "Delete" | "Del"               => Some(0x2E),
        "Space"                        => Some(0x20),
        // Arrow keys — display names ("Up") and legacy names ("ArrowUp")
        "Left"  | "ArrowLeft"          => Some(0x25),
        "Up"    | "ArrowUp"            => Some(0x26),
        "Right" | "ArrowRight"         => Some(0x27),
        "Down"  | "ArrowDown"          => Some(0x28),
        "Home"                         => Some(0x24),
        "End"                          => Some(0x23),
        "PageUp" | "PgUp"              => Some(0x21),
        "PageDown" | "PgDn"            => Some(0x22),
        "Insert"                       => Some(0x2D),
        "CapsLock" | "Caps"            => Some(0x14),
        "PrintScreen"                  => Some(0x2C),
        "ScrollLock"                   => Some(0x91),
        "Pause"                        => Some(0x13),
        "NumLock"                      => Some(0x90),
        // Function keys
        "F1"  => Some(0x70), "F2"  => Some(0x71), "F3"  => Some(0x72),
        "F4"  => Some(0x73), "F5"  => Some(0x74), "F6"  => Some(0x75),
        "F7"  => Some(0x76), "F8"  => Some(0x77), "F9"  => Some(0x78),
        "F10" => Some(0x79), "F11" => Some(0x7A), "F12" => Some(0x7B),
        // OEM punctuation keys (US layout)
        "`"  => Some(0xC0), "'"  => Some(0xDE), ";"  => Some(0xBA),
        "["  => Some(0xDB), "]"  => Some(0xDD), "\\" => Some(0xDC),
        ","  => Some(0xBC), "."  => Some(0xBE), "/"  => Some(0xBF),
        "-"  => Some(0xBD), "="  => Some(0xBB),
        _ => {
            // Single letter (A–Z) or digit (0–9)
            if name.len() == 1 {
                let b = name.as_bytes()[0].to_ascii_uppercase();
                if b.is_ascii_alphanumeric() {
                    return Some(b as u16);
                }
            }
            None
        }
    }
}

/// Inject a text segment via clipboard paste, without a trailing space or clipboard restore.
/// SUPPRESS_NEXT_CLIPBOARD_WRITE is set true by write_clipboard — caller manages the final reset.
fn inject_text_segment(text: &str, target_hwnd: isize) {
    if !write_clipboard(text) {
        return;
    }

    if target_hwnd != 0 {
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(target_hwnd as _);
        }
        thread::sleep(Duration::from_millis(10));
    }

    let use_ctrl_v = !is_ctrl_v_mapped() && !target_needs_shift_insert(target_hwnd);
    if use_ctrl_v {
        send_vk_key(0xA2, false); // LCtrl down
        send_vk_key(0x56, false); // V down
        send_vk_key(0x56, true);  // V up
        send_vk_key(0xA2, true);  // LCtrl up
    } else {
        send_vk_key(VK_LSHIFT, false);
        send_vk_key_extended(VK_INSERT, false);
        send_vk_key_extended(VK_INSERT, true);
        send_vk_key(VK_LSHIFT, true);
    }

    thread::sleep(Duration::from_millis(30));
}

// ── SendInput helpers ───────────────────────────────────────────────────────

fn send_vk_key(vk: u16, key_up: bool) {
    let flags = if key_up { KEYEVENTF_KEYUP } else { 0 };
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk as _,
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

fn send_vk_tap(vk: u16) {
    send_vk_key(vk, false);
    send_vk_key(vk, true);
}

/// SendInput variant that sets KEYEVENTF_EXTENDEDKEY. Required for the "extended"
/// navigation-cluster keys (Insert, Delete, Home, End, PgUp, PgDn, Arrows) so
/// Chromium-based targets (xterm.js inside VS Code, etc.) resolve the DOM event
/// `code` to e.g. "Insert" rather than "Numpad0" — keybinding handlers for
/// Shift+Insert / paste require this to match.
pub(crate) fn send_vk_key_extended(vk: u16, key_up: bool) {
    let mut flags = KEYEVENTF_EXTENDEDKEY;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk as _,
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

/// Move the cursor back `count` positions instantly via a single batched
/// SendInput call. Used by the `{cursor}` token so the cursor snaps to its
/// final position rather than walking back one character at a time.
fn send_left_arrows_batch(count: usize) {
    if count == 0 {
        return;
    }
    let mut inputs: Vec<INPUT> = Vec::with_capacity(count * 2);
    for _ in 0..count {
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_LEFT as _,
                    wScan: 0,
                    dwFlags: 0,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
        inputs.push(INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VK_LEFT as _,
                    wScan: 0,
                    dwFlags: KEYEVENTF_KEYUP,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        });
    }
    unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

// ── Built-in autocorrect dictionary ─────────────────────────────────────────

/// (typo, correction) pairs — single source of truth shared by the engine
/// lookup and the get_builtin_autocorrect_entries command that feeds the
/// visible "Common typos" list in the UI.
const BUILTIN_TYPOS: &[(&str, &str)] = &[
    ("teh", "the"),
    ("hte", "the"),
    ("adn", "and"),
    ("nad", "and"),
    ("ahve", "have"),
    ("hvae", "have"),
    ("taht", "that"),
    ("tath", "that"),
    ("wiht", "with"),
    ("iwth", "with"),
    ("whic", "which"),
    ("whihc", "which"),
    ("thier", "their"),
    ("theri", "their"),
    // "form" is deliberately absent — it's a real word, not a typo of "from".
    ("fomr", "from"),
    ("frome", "from"),
    ("jsut", "just"),
    ("juts", "just"),
    ("knwo", "know"),
    ("konw", "know"),
    ("lik", "like"),
    ("liek", "like"),
    ("mroe", "more"),
    ("moer", "more"),
    ("soem", "some"),
    ("smoe", "some"),
    ("thsi", "this"),
    ("htis", "this"),
    ("waht", "what"),
    ("hwat", "what"),
    ("wehn", "when"),
    ("hwen", "when"),
    ("woudl", "would"),
    ("wuold", "would"),
    ("yoru", "your"),
    ("yuor", "your"),
    ("abotu", "about"),
    ("baout", "about"),
    ("becuase", "because"),
    ("becasue", "because"),
    ("befoer", "before"),
    ("befroe", "before"),
    ("coudl", "could"),
    ("cuold", "could"),
    ("doesnt", "doesn't"),
    ("dont", "don't"),
    ("didnt", "didn't"),
    ("hasnt", "hasn't"),
    ("hadnt", "hadn't"),
    ("isnt", "isn't"),
    ("wasnt", "wasn't"),
    ("wont", "won't"),
    ("wouldnt", "wouldn't"),
    ("cant", "can't"),
    ("shouldnt", "shouldn't"),
    // Word-style capitalization entries. The identity guard in
    // resolve_dict_correction keeps a correctly typed "I" from firing.
    ("i", "I"),
    ("ive", "I've"),
];

fn builtin_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: std::sync::OnceLock<HashMap<&'static str, &'static str>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| BUILTIN_TYPOS.iter().copied().collect())
}

fn builtin_autocorrect(word: &str) -> Option<&'static str> {
    builtin_map().get(word).copied()
}

/// Days of the week — Word-style capitalization pack. Months are deliberately
/// absent: "may", "march" and "august" are ordinary words, so the pack would
/// misfire constantly. The identity guard keeps a correctly typed "Monday"
/// from firing.
const DAYS_ENTRIES: &[(&str, &str)] = &[
    ("monday", "Monday"),
    ("tuesday", "Tuesday"),
    ("wednesday", "Wednesday"),
    ("thursday", "Thursday"),
    ("friday", "Friday"),
    ("saturday", "Saturday"),
    ("sunday", "Sunday"),
];

fn days_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: std::sync::OnceLock<HashMap<&'static str, &'static str>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| DAYS_ENTRIES.iter().copied().collect())
}

fn days_autocorrect(word: &str) -> Option<&'static str> {
    days_map().get(word).copied()
}

/// Symbol replacements, Typinator-style. Constraint: a trigger must never
/// contain a terminator char (. , ! ? ; :) — those end the buffer word, so
/// such a trigger could never accumulate. Ellipsis and != are impossible for
/// that reason. Triggers fire as standalone "words" only (buffer-boundary
/// matching): "x->y" stays untouched, "foo -> bar" converts.
const SYMBOL_ENTRIES: &[(&str, &str)] = &[
    ("(c)", "©"),
    ("(r)", "®"),
    ("(tm)", "™"),
    ("->", "→"),
    ("<-", "←"),
    ("=>", "⇒"),
    ("+-", "±"),
    ("~=", "≈"),
    ("--", "–"),
    ("1/2", "½"),
    ("1/4", "¼"),
    ("3/4", "¾"),
    ("1/3", "⅓"),
    ("2/3", "⅔"),
    ("1/8", "⅛"),
    ("3/8", "⅜"),
    ("5/8", "⅝"),
    ("7/8", "⅞"),
    // Engineering / maths
    (">=", "≥"),
    ("<=", "≤"),
    ("=/=", "≠"),
    ("(deg)", "°"),
    ("(ohm)", "Ω"),
    ("(mu)", "µ"),
    ("(pi)", "π"),
    ("(sqrt)", "√"),
    // Finance / currency (for layouts without the key — UK has £, US doesn't;
    // € needs AltGr on UK)
    ("(eur)", "€"),
    ("(gbp)", "£"),
    ("(yen)", "¥"),
    ("(cent)", "¢"),
];

/// Superscript/subscript entries fire on words ENDING with the trigger —
/// "m^2" → "m²", "10^3" → "10³", "co_2" → "co₂" — because nobody types "^2"
/// as a standalone word. Suffix semantics are deliberately limited to these
/// ^N/_N shapes: generalizing to the arrow/dash triggers would corrupt
/// code-like tokens ("<!--" must never become "<!–"). Displayed in the
/// Symbols pack; per-entry disable applies by trigger like everything else.
const SUPERSUB_ENTRIES: &[(&str, &str)] = &[
    ("^0", "⁰"),
    ("^1", "¹"),
    ("^2", "²"),
    ("^3", "³"),
    ("^4", "⁴"),
    ("^5", "⁵"),
    ("^6", "⁶"),
    ("^7", "⁷"),
    ("^8", "⁸"),
    ("^9", "⁹"),
    ("_0", "₀"),
    ("_1", "₁"),
    ("_2", "₂"),
    ("_3", "₃"),
    ("_4", "₄"),
    ("_5", "₅"),
    ("_6", "₆"),
    ("_7", "₇"),
    ("_8", "₈"),
    ("_9", "₉"),
];

fn symbols_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: std::sync::OnceLock<HashMap<&'static str, &'static str>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| SYMBOL_ENTRIES.iter().copied().collect())
}

fn symbols_autocorrect(word: &str) -> Option<&'static str> {
    symbols_map().get(word).copied()
}

/// Emoji pack. Trigger convention is (name) — the universal `:name:` is
/// impossible here because ':' is a word terminator and can never sit inside
/// a buffered word. Names are lowercase, unambiguous, and never contain a
/// terminator char. Multi-codepoint emoji (variation selectors, surrogate
/// pairs) inject fine — push_unicode walks UTF-16 code units.
const EMOJI_ENTRIES: &[(&str, &str)] = &[
    // Faces
    ("(smile)", "😄"),
    ("(grin)", "😁"),
    ("(laugh)", "😂"),
    ("(wink)", "😉"),
    ("(happy)", "🙂"),
    ("(sad)", "😢"),
    ("(cry)", "😭"),
    ("(angry)", "😠"),
    ("(love)", "😍"),
    ("(cool)", "😎"),
    ("(shocked)", "😲"),
    ("(thinking)", "🤔"),
    ("(sweat)", "😅"),
    ("(neutral)", "😐"),
    ("(eyeroll)", "🙄"),
    ("(facepalm)", "🤦"),
    ("(shrug)", "🤷"),
    ("(party)", "🥳"),
    ("(mindblown)", "🤯"),
    ("(skull)", "💀"),
    ("(ghost)", "👻"),
    ("(robot)", "🤖"),
    // Hands
    ("(thumbsup)", "👍"),
    ("(thumbsdown)", "👎"),
    ("(ok)", "👌"),
    ("(clap)", "👏"),
    ("(wave)", "👋"),
    ("(pray)", "🙏"),
    ("(muscle)", "💪"),
    ("(crossed)", "🤞"),
    ("(fist)", "✊"),
    ("(handshake)", "🤝"),
    // Hearts & celebration
    ("(heart)", "❤️"),
    ("(brokenheart)", "💔"),
    ("(tada)", "🎉"),
    ("(gift)", "🎁"),
    ("(cake)", "🎂"),
    ("(trophy)", "🏆"),
    ("(crown)", "👑"),
    // Common objects & marks
    ("(fire)", "🔥"),
    ("(star)", "⭐"),
    ("(sparkles)", "✨"),
    ("(check)", "✅"),
    ("(cross)", "❌"),
    ("(warning)", "⚠️"),
    ("(rocket)", "🚀"),
    ("(bulb)", "💡"),
    ("(zap)", "⚡"),
    ("(boom)", "💥"),
    ("(eyes)", "👀"),
    ("(100)", "💯"),
    ("(money)", "💰"),
    ("(coffee)", "☕"),
    ("(beer)", "🍺"),
    ("(pizza)", "🍕"),
    ("(sun)", "☀️"),
    ("(moon)", "🌙"),
    ("(rainbow)", "🌈"),
    ("(dog)", "🐶"),
    ("(cat)", "🐱"),
    ("(bug)", "🐛"),
    ("(poop)", "💩"),
    ("(bell)", "🔔"),
    ("(lock)", "🔒"),
    ("(key)", "🔑"),
    ("(pin)", "📌"),
    ("(link)", "🔗"),
    ("(chart)", "📈"),
    ("(target)", "🎯"),
    ("(music)", "🎵"),
    ("(book)", "📚"),
    ("(email)", "📧"),
    ("(phone)", "📱"),
    ("(laptop)", "💻"),
];

fn emoji_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: std::sync::OnceLock<HashMap<&'static str, &'static str>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| EMOJI_ENTRIES.iter().copied().collect())
}

fn emoji_autocorrect(word: &str) -> Option<&'static str> {
    emoji_map().get(word).copied()
}

/// Extended dictionary — ~4k tab-separated (typo, correction) pairs derived
/// from Wikipedia's machine-readable list of common misspellings (CC BY-SA;
/// credit in the help guide). Filtered at generation time: unambiguous
/// corrections only, lowercase typos of 3+ letters. Bundled at build per the
/// offline-first rule; parsed once on first use.
const EXTENDED_TYPOS_RAW: &str = include_str!("data/autocorrect_extended.txt");

fn extended_map() -> &'static HashMap<&'static str, &'static str> {
    static MAP: std::sync::OnceLock<HashMap<&'static str, &'static str>> =
        std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        EXTENDED_TYPOS_RAW
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .collect()
    })
}

fn extended_autocorrect(word: &str) -> Option<&'static str> {
    extended_map().get(word).copied()
}

/// Every bundled dictionary entry as (typo, correction, pack) for the UI
/// list. Packs: "starter" | "extended".
pub fn builtin_autocorrect_entries() -> Vec<(String, String, String)> {
    let mut v: Vec<(String, String, String)> = BUILTIN_TYPOS
        .iter()
        .map(|(t, c)| (t.to_string(), c.to_string(), "starter".to_string()))
        .collect();
    v.extend(
        EXTENDED_TYPOS_RAW
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .map(|(t, c)| (t.to_string(), c.to_string(), "extended".to_string())),
    );
    v.extend(
        DAYS_ENTRIES
            .iter()
            .map(|(t, c)| (t.to_string(), c.to_string(), "days".to_string())),
    );
    v.extend(
        SYMBOL_ENTRIES
            .iter()
            .map(|(t, c)| (t.to_string(), c.to_string(), "symbols".to_string())),
    );
    v.extend(
        SUPERSUB_ENTRIES
            .iter()
            .map(|(t, c)| (t.to_string(), c.to_string(), "symbols".to_string())),
    );
    v.extend(
        EMOJI_ENTRIES
            .iter()
            .map(|(t, c)| (t.to_string(), c.to_string(), "emojis".to_string())),
    );
    v
}

// ── Public API for Tauri commands ───────────────────────────────────────────

pub fn update_assignments(assignments: HashMap<String, Value>) {
    let mut s = state().lock().unwrap();
    // Only keep expansion and autocorrect entries
    s.assignments = assignments
        .into_iter()
        .filter(|(k, _)| {
            k.starts_with("GLOBAL::EXPANSION::") || k.starts_with("GLOBAL::AUTOCORRECT::")
        })
        .collect();
    // Rebuild the space-trigger set used by the LL hook for pre-swallow.
    // triggerMode defaults to "space" when absent, matching check_space_trigger.
    s.space_triggers = s
        .assignments
        .iter()
        .filter(|(k, v)| {
            if !k.starts_with("GLOBAL::EXPANSION::") {
                return false;
            }
            let mode = v
                .get("data")
                .and_then(|d| d.get("triggerMode"))
                .and_then(|m| m.as_str())
                .unwrap_or("space");
            mode == "space"
        })
        .map(|(k, _)| k["GLOBAL::EXPANSION::".len()..].to_string())
        .collect();
    // Rebuild the misspelling set driving AUTOCORRECT_PENDING.
    s.autocorrect_words = s
        .assignments
        .keys()
        .filter_map(|k| k.strip_prefix("GLOBAL::AUTOCORRECT::"))
        .map(|w| w.to_lowercase())
        .collect();
    info!(
        "[Keyfire] Expansion assignments updated: {} entries ({} space-triggers, {} autocorrect words)",
        s.assignments.len(),
        s.space_triggers.len(),
        s.autocorrect_words.len()
    );
    refresh_pending_flag(&s);
}

pub fn set_autocorrect_enabled(enabled: bool) {
    let mut s = state().lock().unwrap();
    s.autocorrect_enabled = enabled;
    refresh_pending_flag(&s);
    info!("[Keyfire] Autocorrect config: enabled={}", enabled);
}

/// Full autocorrect settings payload — one struct end to end (frontend JSON
/// object → Tauri command → engine) instead of the old positional-parameter
/// list that had grown to 12 arguments. `#[serde(default)]` keeps it forward
/// compatible: a caller omitting a newly added field gets the off/empty
/// default instead of a deserialization error.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct AutocorrectSettings {
    pub enabled: bool,
    pub builtin_typos: bool,
    pub extended_typos: bool,
    pub days: bool,
    pub symbols: bool,
    pub emojis: bool,
    pub double_caps: bool,
    pub double_caps_exceptions: Vec<String>,
    pub caps_lock_fix: bool,
    pub sentence_caps: bool,
    pub excluded_apps: Vec<String>,
    pub disabled_entries: Vec<String>,
}

/// Full autocorrect settings sync — called on startup config load and on
/// every settings change from the frontend.
pub fn set_autocorrect_settings(cfg: AutocorrectSettings) {
    let mut s = state().lock().unwrap();
    s.autocorrect_enabled = cfg.enabled;
    s.builtin_typos_enabled = cfg.builtin_typos;
    s.extended_typos_enabled = cfg.extended_typos;
    s.days_enabled = cfg.days;
    s.symbols_enabled = cfg.symbols;
    s.emojis_enabled = cfg.emojis;
    s.double_caps_enabled = cfg.double_caps;
    s.disabled_entries = cfg
        .disabled_entries
        .into_iter()
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    s.excluded_apps = cfg
        .excluded_apps
        .into_iter()
        .map(|a| a.trim().to_lowercase().trim_end_matches(".exe").to_string())
        .filter(|a| !a.is_empty())
        .collect();
    s.double_caps_exceptions = cfg
        .double_caps_exceptions
        .into_iter()
        .map(|w| w.trim().to_lowercase())
        .filter(|w| !w.is_empty())
        .collect();
    s.caps_lock_fix_enabled = cfg.caps_lock_fix;
    s.sentence_caps_enabled = cfg.sentence_caps;
    refresh_pending_flag(&s);
    info!(
        "[Keyfire] Autocorrect settings: enabled={} builtin={} extended={} days={} symbols={} emojis={} double_caps={} caps_lock_fix={} sentence_caps={} ({} exceptions, {} excluded apps, {} disabled entries)",
        cfg.enabled, cfg.builtin_typos, cfg.extended_typos, cfg.days, cfg.symbols, cfg.emojis, cfg.double_caps, cfg.caps_lock_fix, cfg.sentence_caps,
        s.double_caps_exceptions.len(), s.excluded_apps.len(), s.disabled_entries.len()
    );
}

/// Text-expansion excluded apps — separate list from the autocorrect one.
/// Same normalization (lowercase, no .exe) so the foreground compare works.
pub fn set_expansion_excluded_apps(apps: Vec<String>) {
    let mut s = state().lock().unwrap();
    s.expansion_excluded_apps = apps
        .into_iter()
        .map(|a| a.trim().to_lowercase().trim_end_matches(".exe").to_string())
        .filter(|a| !a.is_empty())
        .collect();
    refresh_pending_flag(&s);
    info!("[Keyfire] Expansion excluded apps: {}", s.expansion_excluded_apps.len());
}

pub fn update_global_variables(vars: HashMap<String, String>) {
    state().lock().unwrap().global_variables = vars;
}

pub fn get_global_variables() -> HashMap<String, String> {
    state().lock().unwrap().global_variables.clone()
}
