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

/// Extract {fillIn:Label} tokens from text. Returns list of field labels.
fn extract_fill_in_fields(text: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{fillIn:") {
        let after = &rest[start + 8..];
        if let Some(end) = after.find('}') {
            let label = after[..end].to_string();
            if !label.is_empty() && !fields.contains(&label) {
                fields.push(label);
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    fields
}

/// Substitute {fillIn:Label} tokens with user-supplied values.
fn resolve_fill_in_tokens(text: &str, values: &HashMap<String, String>) -> String {
    let mut result = text.to_string();
    for (label, value) in values {
        let token = format!("{{fillIn:{}}}", label);
        result = result.replace(&token, value);
    }
    result
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
    autocorrect_enabled: bool,
    global_variables: HashMap<String, String>,
}

impl Default for ExpansionState {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            assignments: HashMap::new(),
            space_triggers: HashSet::new(),
            autocorrect_enabled: true,
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

// ── Buffer management (called from hotkeys.rs) ─────────────────────────────

/// Recompute EXPANSION_PENDING_SPACE from the current buffer state. Called from
/// every path that mutates the buffer or the space_triggers set. The flag is
/// the only thing the LL hook reads to decide whether to pre-swallow a Space.
fn refresh_pending_flag(s: &ExpansionState) {
    if s.buffer.is_empty() || s.space_triggers.is_empty() {
        EXPANSION_PENDING_SPACE.store(false, Ordering::SeqCst);
        return;
    }
    let buf_lower = s.buffer.to_lowercase();
    EXPANSION_PENDING_SPACE.store(s.space_triggers.contains(&buf_lower), Ordering::SeqCst);
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
pub fn buffer_pop() {
    let mut s = state().lock().unwrap();
    s.buffer.pop();
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

    // Priority 1: Custom autocorrect — DISABLED FOR ALPHA
    // let ac_key = format!("GLOBAL::AUTOCORRECT::{}", buffer_lower);
    // if let Some(entry) = s.assignments.get(&ac_key).cloned() {
    //     let correction = entry
    //         .get("data")
    //         .and_then(|d| d.get("correction"))
    //         .and_then(|v| v.as_str())
    //         .unwrap_or("")
    //         .to_string();
    //     let trigger_len = s.buffer.len();
    //     let global_vars = s.global_variables.clone();
    //     s.buffer.clear();
    //     drop(s);
    //
    //     info!("[Trigr] Autocorrect: \"{}\" → \"{}\"", buffer_lower, correction);
    //     let replacement = format!("{}", correction);
    //     fire_expansion(&buffer_lower, trigger_len, true, &replacement, &global_vars);
    //     return true;
    // }

    // Priority 2: Built-in autocorrect — DISABLED FOR ALPHA
    // if s.autocorrect_enabled {
    //     if let Some(correction) = builtin_autocorrect(&buffer_lower) {
    //         let trigger_len = s.buffer.len();
    //         let global_vars = s.global_variables.clone();
    //         s.buffer.clear();
    //         drop(s);
    //
    //         info!("[Trigr] Autocorrect (built-in): \"{}\" → \"{}\"", buffer_lower, correction);
    //         let replacement = format!("{}", correction);
    //         fire_expansion(&buffer_lower, trigger_len, true, &replacement, &global_vars);
    //         return true;
    //     }
    // }

    // Priority 3: Text expansion (space-triggered)
    let exp_key = format!("GLOBAL::EXPANSION::{}", buffer_lower);
    if let Some(entry) = s.assignments.get(&exp_key).cloned() {
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
                info!("[Trigr] Image expansion skipped (Free): \"{}\"", buffer_lower);
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

            info!("[Trigr] Image expansion: \"{}\" → \"{}\"", buffer_lower, image_path);
            fire_image_expansion(&buffer_lower, trigger_len, delete_extra, &image_path, image_scale);
            return true;
        }

        // Check for variant options
        let options = entry
            .get("data")
            .and_then(|d| d.get("options"))
            .and_then(|v| v.as_array())
            .cloned();

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

                    info!("[Trigr] Variant expansion (Free → options[0]): \"{}\"", trigger_str);
                    let case_pattern = detect_case(&original_buffer);
                    let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
                    fire_expansion(&trigger_str, trigger_len, delete_extra, &text, html_opt, &global_vars, case_pattern);
                    return true;
                }

                s.buffer.clear();
                drop(s);

                info!("[Trigr] Variant expansion: \"{}\" with {} options", trigger_str, opts.len());
                if crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
                    return true;
                }
                thread::spawn(move || {
                    fire_variant_expansion(&trigger_str, trigger_len, delete_extra, &opts, &global_vars);
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

        info!("[Trigr] Expansion: \"{}\" → \"{}\"", buffer_lower, text);
        let case_pattern = detect_case(&original_buffer);
        let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
        fire_expansion(&buffer_lower, trigger_len, delete_extra, &text, html_opt, &global_vars, case_pattern);
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

/// Called after each character is added to the buffer. Checks for immediate-mode triggers.
/// Returns true if an immediate expansion fired.
pub fn check_immediate_triggers() -> bool {
    let mut s = state().lock().unwrap();
    if s.buffer.is_empty() {
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

                        info!("[Trigr] Variant expansion (immediate, Free → options[0]): \"{}\"", trigger_str);
                        let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
                        fire_expansion(&trigger_str, trigger_len, false, &text, html_opt, &global_vars, case_pattern);
                        return true;
                    }

                    s.buffer.clear();
                    drop(s);

                    info!("[Trigr] Variant expansion (immediate): \"{}\" with {} options", trigger_str, opts.len());
                    if !crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
                        thread::spawn(move || {
                            fire_variant_expansion(&trigger_str, trigger_len, false, &opts, &global_vars);
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
                    info!("[Trigr] Image expansion (immediate) skipped (Free): \"{}\"", imm.trigger);
                    return true;
                }

                let image_path = imm.image_path.clone();
                let image_scale = imm.image_scale;
                s.buffer.clear();
                drop(s);

                info!("[Trigr] Image expansion (immediate): \"{}\" → \"{}\"", imm.trigger, image_path);
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

            info!("[Trigr] Expansion (immediate): \"{}\" → \"{}\"", trigger, text);
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
            log::warn!("[Trigr] Fire Text Expansion: trigger \"{}\" not found, skipping", trigger);
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
            log::info!("[Trigr] Fire Text Expansion (image, Free): \"{}\" — no-op", trigger);
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
        log::info!("[Trigr] Fire Text Expansion (image): \"{}\" → \"{}\"", trigger, image_path);
        fire_image_expansion(trigger, 0, false, &image_path, image_scale);
        return;
    }

    let options = entry
        .get("data")
        .and_then(|d| d.get("options"))
        .and_then(|v| v.as_array())
        .cloned();

    let global_vars = get_global_variables();

    if let Some(opts) = options {
        if !opts.is_empty() {
            if !crate::licence::is_pro() {
                let first = &opts[0];
                let text = first.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let html = first.get("html").and_then(|v| v.as_str()).unwrap_or("").to_string();
                log::info!("[Trigr] Fire Text Expansion (variant, Free → options[0]): \"{}\"", trigger);
                let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
                fire_expansion(trigger, 0, false, &text, html_opt, &global_vars, CasePattern::Lower);
                return;
            }
            if crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
                log::info!("[Trigr] Fire Text Expansion (variant): \"{}\" skipped — fill-in already active", trigger);
                return;
            }
            log::info!("[Trigr] Fire Text Expansion (variant): \"{}\" with {} options", trigger, opts.len());
            let trigger_str = trigger.to_string();
            thread::spawn(move || {
                fire_variant_expansion(&trigger_str, 0, false, &opts, &global_vars);
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
    log::info!("[Trigr] Fire Text Expansion (text): \"{}\" → \"{}\"", trigger, text);
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
    // Check for {fillIn:...} tokens — if present, spawn a dedicated thread for the
    // entire fill-in + injection flow so the processor thread is never blocked.
    // Fill-in flow is plain-text only (rich text inside fill-in fields isn't supported yet).
    let fill_in_fields = extract_fill_in_fields(text);
    if !fill_in_fields.is_empty() {
        if crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let text = text.to_string();
        let global_vars = global_vars.clone();
        let trigger_len = trigger_len;
        let trigger_str = _trigger.to_string();
        thread::spawn(move || {
            fire_expansion_with_fillin(fill_in_fields, &text, trigger_len, delete_extra, &global_vars, &trigger_str, case_pattern);
        });
        return;
    }

    // No fill-in tokens — resolve and inject directly
    let (resolved, cursor_back) = resolve_tokens(text, global_vars);
    let resolved = apply_case(&resolved, case_pattern);

    // Resolve HTML in parallel. Only used when target app accepts CF_HTML —
    // CF_UNICODETEXT always wins on plain-text apps via Windows clipboard fallback.
    // Skip HTML if the expansion uses inline key tokens (those need per-segment
    // injection that doesn't compose with a single paste).
    let resolved_html: Option<String> = html.and_then(|h| {
        if h.is_empty() || h.contains("{key:") {
            None
        } else {
            Some(resolve_tokens_html(h, global_vars))
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
            thread::sleep(Duration::from_millis(50));
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

        // Replay any keystrokes that were buffered during injection
        let buffered: Vec<crate::hotkeys::BufferedKey> =
            crate::hotkeys::injection_buffer().lock().unwrap().drain(..).collect();
        if !buffered.is_empty() {
            crate::hotkeys::SUPPRESS_SIMULATED
                .store(true, std::sync::atomic::Ordering::SeqCst);
            for key in &buffered {
                send_vk_key(key.vk_code as u16, !key.is_keydown);
                thread::sleep(Duration::from_millis(2));
            }
            crate::hotkeys::SUPPRESS_SIMULATED
                .store(false, std::sync::atomic::Ordering::SeqCst);

            // Feed replayed keystrokes into the expansion buffer
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

        // Sync modifier atomics with actual physical key state after replay
        crate::hotkeys::sync_modifier_state_from_os();

        // _guard drops here → INJECTION_IN_PROGRESS = false
    });
}

/// Fill-in flow: runs entirely on a dedicated thread so the processor thread is never blocked.
/// Sequence: show window → wait for response → resolve tokens → inject.
fn fire_expansion_with_fillin(
    fill_in_fields: Vec<String>,
    text: &str,
    trigger_len: usize,
    delete_extra: bool,
    global_vars: &HashMap<String, String>,
    trigger_str: &str,
    case_pattern: CasePattern,
) {
    crate::hotkeys::FILL_IN_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);

    let app = match APP_HANDLE.get() {
        Some(a) => a,
        None => {
            println!("[EXP] No app handle — cannot show fill-in window");
            return;
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

        let _ = win.show();
        let _ = win.set_focus();

        // Ask renderer to signal ready (handles subsequent shows after first mount)
        let _ = win.emit("fill-in-request-ready", serde_json::json!({}));

        // Wait for FillInWindow.jsx to signal it's mounted and listening (5s timeout)
        let (ready_tx, ready_rx) = mpsc::channel();
        *fill_in_ready_tx().lock().unwrap() = Some(ready_tx);
        let _ = ready_rx.recv_timeout(Duration::from_secs(5));
        *fill_in_ready_tx().lock().unwrap() = None;

        // Renderer is ready — emit field data
        let _ = win.emit("fill-in-show", serde_json::json!({
            "fields": fill_in_fields,
            "theme": theme,
        }));
    }

    // Block on this dedicated thread waiting for user response (60s timeout)
    let response = rx.recv_timeout(Duration::from_secs(60));
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

    let text_after_fillin = match response {
        Ok(Some(values)) => {
            resolve_fill_in_tokens(text, &values)
        }
        Ok(None) => {
            return;
        }
        Err(_) => {
            return;
        }
    };

    // Resolve remaining tokens
    let (resolved, cursor_back) = resolve_tokens(&text_after_fillin, global_vars);
    let resolved = apply_case(&resolved, case_pattern);

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
        thread::sleep(Duration::from_millis(50));
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
            inject_via_clipboard(&resolved, None, target_hwnd);
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

    // Replay any keystrokes that were buffered during injection
    let buffered: Vec<crate::hotkeys::BufferedKey> =
        crate::hotkeys::injection_buffer().lock().unwrap().drain(..).collect();
    if !buffered.is_empty() {
        crate::hotkeys::SUPPRESS_SIMULATED
            .store(true, std::sync::atomic::Ordering::SeqCst);
        for key in &buffered {
            send_vk_key(key.vk_code as u16, !key.is_keydown);
            thread::sleep(Duration::from_millis(2));
        }
        crate::hotkeys::SUPPRESS_SIMULATED
            .store(false, std::sync::atomic::Ordering::SeqCst);

        let last_was_space = buffered.last()
            .map(|k| k.vk_code == 0x20 && k.is_keydown)
            .unwrap_or(false);
        for key in &buffered {
            if !key.is_keydown { continue; }
            if key.vk_code == 0x20 { continue; }
            if key.vk_code == 0x08 { buffer_pop(); continue; }
            if key.vk_code == 0x0D || key.vk_code == 0x1B || key.vk_code == 0x09 {
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

    crate::hotkeys::sync_modifier_state_from_os();
    // _guard drops here → INJECTION_IN_PROGRESS = false
}

// ── Token resolution ────────────────────────────────────────────────────────

pub fn resolve_tokens(text: &str, global_vars: &HashMap<String, String>) -> (String, usize) {
    let mut result = text.to_string();

    // Substitute {{varName}} global variables — Pro-only.
    // (Dynamic tokens — date, time, clipboard, cursor — are unlocked for everyone:
    // too many free competitors offer them for the gate to be defensible.)
    if crate::licence::is_pro() && result.contains("{{") {
        for (name, value) in global_vars {
            let token = format!("{{{{{}}}}}", name); // {{name}}
            result = result.replace(&token, value);
        }
    }

    // {clipboard} and {clipboard:transform} tokens — read clipboard once
    if result.contains("{clipboard") {
        let clip = read_clipboard().unwrap_or_default();
        // Replace specific variants BEFORE bare {clipboard} to prevent prefix matching
        result = result.replace("{clipboard:uppercase}", &clip.to_uppercase());
        result = result.replace("{clipboard:lowercase}", &clip.to_lowercase());
        result = result.replace("{clipboard:trim}", clip.trim());
        result = result.replace("{clipboard:urlencode}", &url_encode(&clip));
        result = result.replace("{clipboard}", &clip);
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

    // {date:+Nd}, {date:-Nm}, {date:+Ny} — date/time math with optional format suffix
    if result.contains("{date:+") || result.contains("{date:-") {
        let re = regex_lite::Regex::new(r"\{date:([+-]\d+)([dmy])(?::([^}]+))?\}").unwrap();
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

    // {cursor} — track position, then remove token
    let mut cursor_back = 0;
    if let Some(idx) = result.find("{cursor}") {
        cursor_back = result.len() - idx - "{cursor}".len();
        result = result.replace("{cursor}", "");
    }

    (result, cursor_back)
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

/// Cached CF_HTML clipboard format ID (registered once with the OS).
fn cf_html_format_id() -> u32 {
    static FORMAT_ID: OnceLock<u32> = OnceLock::new();
    *FORMAT_ID.get_or_init(|| {
        let name: Vec<u16> = "HTML Format".encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { RegisterClipboardFormatW(name.as_ptr()) }
    })
}

/// Wrap an HTML fragment in the CF_HTML clipboard format with the required
/// Version / StartHTML / EndHTML / StartFragment / EndFragment byte offsets.
fn build_cf_html(fragment: &str) -> Vec<u8> {
    // Placeholder offsets get patched after we know the actual byte positions
    let header = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    let prefix = "<html>\r\n<body>\r\n<!--StartFragment-->";
    let suffix = "<!--EndFragment-->\r\n</body>\r\n</html>";

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
fn write_clipboard_dual(text: &str, html: Option<&str>) -> bool {
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

            // Keep Trigr's injected text out of Windows Clipboard History (Win+V)
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
/// Cloud Clipboard skip Trigr's own injected content. MUST be called while the
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
// user's next Ctrl+V pastes Trigr's expansion text instead of their image.
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

/// Restore a snapshot by clearing the clipboard and re-writing every captured
/// format. Sets `SUPPRESS_NEXT_CLIPBOARD_WRITE` true before opening so the
/// listener ignores the WM_CLIPBOARDUPDATE that fires from EmptyClipboard +
/// SetClipboardData. Caller is responsible for clearing the suppress flag
/// after the listener has had a chance to process the event.
///
/// An empty snapshot still calls EmptyClipboard — this matches the pre-state
/// of "clipboard was empty before we wrote our text" and removes Trigr's
/// expansion text from the Windows clipboard. Returns false if the clipboard
/// couldn't be opened (clipboard contents are then left as Trigr wrote them).
pub(crate) fn restore_clipboard_snapshot(snapshot: &[(u32, Vec<u8>)]) -> bool {
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
        log::warn!("[Trigr] restore_clipboard_snapshot: OpenClipboard failed after retries");
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
/// clipboard contents change (any process — Trigr's own writes count too).
pub(crate) fn clipboard_sequence_number() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

/// HTML-escape a string and convert newlines into <br>.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&'  => out.push_str("&amp;"),
            '<'  => out.push_str("&lt;"),
            '>'  => out.push_str("&gt;"),
            '"'  => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            '\n' => out.push_str("<br>"),
            '\r' => {}
            _    => out.push(ch),
        }
    }
    out
}

/// Resolve token chips inside an HTML expansion. Each
/// `<span class="rte-token" data-token="{...}">display</span>` is replaced
/// with the HTML-escaped resolved value of its token.
///
/// `{cursor}` chips are stripped entirely — the cursor_back count is derived
/// from the plain-text path which runs alongside.
fn resolve_tokens_html(html: &str, global_vars: &HashMap<String, String>) -> String {
    let re = match regex_lite::Regex::new(
        r#"<span\b[^>]*?\bdata-token="([^"]*)"[^>]*>[^<]*</span>"#
    ) {
        Ok(r) => r,
        Err(_) => return html.to_string(),
    };
    let mut result = String::with_capacity(html.len());
    let mut last_end = 0;
    for caps in re.captures_iter(html) {
        let Some(m) = caps.get(0) else { continue };
        let token = caps.get(1).map(|t| t.as_str()).unwrap_or("");
        result.push_str(&html[last_end..m.start()]);
        if token != "{cursor}" {
            let (resolved, _) = resolve_tokens(token, global_vars);
            result.push_str(&html_escape(&resolved));
        }
        last_end = m.end();
    }
    result.push_str(&html[last_end..]);
    result
}

// ── Hybrid injection — SendInput for short text, clipboard for long/terminal ─

const TERMINAL_PROCS: &[&str] = &[
    "cmd", "powershell", "pwsh", "windowsterminal", "wt", "mintty", "conhost",
];

fn is_terminal_process(proc_name: &str) -> bool {
    TERMINAL_PROCS.iter().any(|&t| proc_name == t)
}

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

/// Inject text via batched KEYEVENTF_UNICODE SendInput (single call).
fn inject_via_sendinput(text: &str, target_hwnd: isize) {
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
    // Snapshot every format currently on the clipboard. Text-only save misses
    // CF_DIB images (Snagit, Snipping Tool), CF_HDROP file drops, RTF from
    // Word, and registered formats from Office/Chromium — leaving Trigr's
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
        let trimmed = h.trim_end();
        if let Some(idx) = trimmed.rfind("</") {
            let before = &trimmed[..idx];
            let close_tag = &trimmed[idx..];
            format!("{}&nbsp;{}", before, close_tag)
        } else {
            format!("{}&nbsp;", trimmed)
        }
    });

    // Write replacement to clipboard — if this fails, do NOT paste (would paste old clipboard content)
    if !write_clipboard_dual(&payload_text, payload_html.as_deref()) {
        log::warn!("[Trigr] write_clipboard FAILED — skipping paste to avoid pasting wrong content");
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

    // Restore clipboard after paste settles.
    // 150ms: Excel (and other Office apps) process clipboard paste via their message
    // queue — slower than most apps. 50ms was not enough, causing Excel to read the
    // restored-old-content instead of the expansion text.
    thread::sleep(Duration::from_millis(150));
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

        // Keep Trigr's injected image out of Win+V / Cloud Clipboard (same as text).
        mark_clipboard_excluded();
        CloseClipboard();
        // Record the seqnum so the listener skips this image write's update event.
        crate::actions::record_self_clipboard_write();
        true
    }
}

/// Fire a variant expansion: show selection popup, wait for user choice, inject selected text.
fn fire_variant_expansion(
    trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    options: &[serde_json::Value],
    global_vars: &HashMap<String, String>,
) {
    crate::hotkeys::FILL_IN_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);

    let app = match APP_HANDLE.get() {
        Some(a) => a.clone(),
        None => {
            crate::hotkeys::FILL_IN_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            return;
        }
    };

    // Capture target HWND before showing popup
    let target_hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };

    // Erase the trigger text
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

    let theme = crate::config::load_config()
        .and_then(|c| c.get("theme").and_then(|v| v.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "dark".to_string());

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
    }

    // Wait for selection (60s timeout)
    let response = rx.recv_timeout(Duration::from_secs(60));
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
    let (selected_text, selected_html) = match response {
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
    };

    if selected_text.is_empty() {
        return;
    }

    // If the selected variant contains {fillIn:LABEL} tokens, re-prompt the
    // user for those values before injecting. Mirrors the main expansion
    // path's fill-in flow (fire_expansion_with_fillin) — variant flow had no
    // fill-in handling before, so tokens were pasted literally as text.
    //
    // Plain-text only when fill-in is involved: matches the main path
    // (rich text inside fill-in fields isn't supported yet, see line 522).
    let fill_in_fields = extract_fill_in_fields(&selected_text);
    let (final_text, final_html) = if !fill_in_fields.is_empty() {
        // Re-acquire FILL_IN_ACTIVE before re-showing the window
        crate::hotkeys::FILL_IN_ACTIVE.store(true, std::sync::atomic::Ordering::SeqCst);

        let (tx2, rx2) = mpsc::channel();
        *fill_in_tx().lock().unwrap() = Some(tx2);

        if let Some(win) = app.get_webview_window("fillin") {
            use tauri::Emitter;

            if let Ok(hwnd) = win.hwnd() {
                crate::hotkeys::FILLIN_HWND.store(hwnd.0 as isize, std::sync::atomic::Ordering::SeqCst);
            }

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
        }

        let response2 = rx2.recv_timeout(Duration::from_secs(60));
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
        (substituted, None) // plain-text only when fill-in is involved
    } else {
        (selected_text, selected_html)
    };

    let (resolved, cursor_back) = resolve_tokens(&final_text, global_vars);
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
            Some(resolve_tokens_html(&h, global_vars))
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
    use image::GenericImageView;

    // Check file exists
    if !std::path::Path::new(image_path).exists() {
        log::warn!("[Trigr] Image expansion: file not found at \"{}\"", image_path);
        return;
    }

    // Read file bytes
    let file_bytes = match std::fs::read(image_path) {
        Ok(b) => b,
        Err(e) => {
            log::warn!("[Trigr] Image expansion: failed to read \"{}\": {}", image_path, e);
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
            log::warn!("[Trigr] Image expansion: unsupported format \"{}\"", ext);
            return;
        }
    };

    // Decode image
    let mut img = match image::load_from_memory_with_format(&file_bytes, format) {
        Ok(i) => i,
        Err(e) => {
            log::warn!("[Trigr] Image expansion: failed to decode \"{}\": {}", image_path, e);
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

        // Replay any keystrokes buffered during injection
        let buffered: Vec<crate::hotkeys::BufferedKey> =
            crate::hotkeys::injection_buffer().lock().unwrap().drain(..).collect();
        if !buffered.is_empty() {
            crate::hotkeys::SUPPRESS_SIMULATED
                .store(true, std::sync::atomic::Ordering::SeqCst);
            for key in &buffered {
                send_vk_key(key.vk_code as u16, !key.is_keydown);
                thread::sleep(Duration::from_millis(2));
            }
            crate::hotkeys::SUPPRESS_SIMULATED
                .store(false, std::sync::atomic::Ordering::SeqCst);

            let last_was_space = buffered.last()
                .map(|k| k.vk_code == 0x20 && k.is_keydown)
                .unwrap_or(false);
            for key in &buffered {
                if !key.is_keydown { continue; }
                if key.vk_code == 0x20 { continue; }
                if key.vk_code == 0x08 { buffer_pop(); continue; }
                if key.vk_code == 0x0D || key.vk_code == 0x1B || key.vk_code == 0x09 {
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

        crate::hotkeys::sync_modifier_state_from_os();
        // _guard drops here → INJECTION_IN_PROGRESS = false
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

fn builtin_autocorrect(word: &str) -> Option<&'static str> {
    match word {
        "teh" => Some("the"),
        "hte" => Some("the"),
        "adn" => Some("and"),
        "nad" => Some("and"),
        "ahve" => Some("have"),
        "hvae" => Some("have"),
        "taht" => Some("that"),
        "tath" => Some("that"),
        "wiht" => Some("with"),
        "iwth" => Some("with"),
        "whic" => Some("which"),
        "whihc" => Some("which"),
        "thier" => Some("their"),
        "theri" => Some("their"),
        "form" => None, // Not a typo — "form" is a real word
        "fomr" => Some("from"),
        "frome" => Some("from"),
        "jsut" => Some("just"),
        "juts" => Some("just"),
        "knwo" => Some("know"),
        "konw" => Some("know"),
        "lik" => Some("like"),
        "liek" => Some("like"),
        "mroe" => Some("more"),
        "moer" => Some("more"),
        "soem" => Some("some"),
        "smoe" => Some("some"),
        "thsi" => Some("this"),
        "htis" => Some("this"),
        "waht" => Some("what"),
        "hwat" => Some("what"),
        "wehn" => Some("when"),
        "hwen" => Some("when"),
        "woudl" => Some("would"),
        "wuold" => Some("would"),
        "yoru" => Some("your"),
        "yuor" => Some("your"),
        "abotu" => Some("about"),
        "baout" => Some("about"),
        "becuase" => Some("because"),
        "becasue" => Some("because"),
        "befoer" => Some("before"),
        "befroe" => Some("before"),
        "coudl" => Some("could"),
        "cuold" => Some("could"),
        "doesnt" => Some("doesn't"),
        "dont" => Some("don't"),
        "didnt" => Some("didn't"),
        "hasnt" => Some("hasn't"),
        "hadnt" => Some("hadn't"),
        "isnt" => Some("isn't"),
        "wasnt" => Some("wasn't"),
        "wont" => Some("won't"),
        "wouldnt" => Some("wouldn't"),
        "cant" => Some("can't"),
        "shouldnt" => Some("shouldn't"),
        _ => None,
    }
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
    info!(
        "[Trigr] Expansion assignments updated: {} entries ({} space-triggers)",
        s.assignments.len(),
        s.space_triggers.len()
    );
    refresh_pending_flag(&s);
}

pub fn set_autocorrect_enabled(enabled: bool) {
    state().lock().unwrap().autocorrect_enabled = enabled;
    info!("[Trigr] Autocorrect config: {} (engine disabled for Alpha)", enabled);
}

pub fn update_global_variables(vars: HashMap<String, String>) {
    state().lock().unwrap().global_variables = vars;
}

pub fn get_global_variables() -> HashMap<String, String> {
    state().lock().unwrap().global_variables.clone()
}
