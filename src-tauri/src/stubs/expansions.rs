//! Non-Windows twin of expansions.rs — on macOS this is the REAL text
//! expansion engine (Mac port Phase 2, modules 3-4).
//!
//! Ported from the Windows original by copy-and-surgery: the keystroke
//! buffer, trigger matching, smart case, fill-in parsing and the whole token
//! resolver ({date}, {clipboard}, {selection}, {set}/{if}/{=expr}, {cursor},
//! {key:...}) are byte-for-byte the Windows logic. Only the OS seams changed:
//!
//!   * Clipboard: NSPasteboard replaces the Win32 clipboard. Dual-format
//!     writes put `public.utf8-plain-text` + `public.html` on the pasteboard
//!     (raw HTML fragment — macOS has no CF_HTML container). Snapshot/restore
//!     round-trips EVERY flavor via types()/dataForType so images and RTF
//!     survive an expansion. The `org.nspasteboard.ConcealedType` marker is
//!     the mac analogue of ExcludeClipboardContentFromMonitorProcessing —
//!     third-party clipboard managers honour it; Keyfire's own listener skips
//!     our writes via the changeCount queue (actions::record_self_clipboard_write).
//!   * Injection: tagged CGEvents via stubs/actions.rs (INJECTED_EVENT_MAGIC).
//!     There is NO SUPPRESS_SIMULATED on macOS — the tag on every synthetic
//!     event is per-event suppression, so the Windows replay-buffer machinery
//!     (keystrokes typed mid-injection get buffered + replayed) is not ported:
//!     real keystrokes typed during the ~200ms injection window pass straight
//!     through to the app and feed the buffer live from the tap.
//!   * Paste is always ⌘V. The Windows is_ctrl_v_mapped / Shift+Insert dance
//!     is unnecessary: a user-mapped ⌘V hotkey can't eat our synthetic paste
//!     because the tap drops tagged events before matching.
//!   * Focus: no HWNDs. The fill-in / variant picker flows capture the
//!     frontmost app's PID before showing the window and re-activate it
//!     after (same PID hand-back as the quick-search overlay).
//!
//! Buffer feeding comes from the CGEventTap processor in stubs/hotkeys.rs
//! (layout-aware chars via CGEventKeyboardGetUnicodeString). The Space
//! pre-swallow contract (EXPANSION_PENDING_SPACE / SPACE_PRE_SWALLOWED) is
//! identical to Windows: the tap callback drops the Space keydown when the
//! buffer exactly matches a space-mode trigger.
#![allow(dead_code, unused_variables)]

use log::info;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::AppHandle;

const MAX_BUFFER_LENGTH: usize = 50;

// ── Injection guard — ensures INJECTION_IN_PROGRESS is always cleared ──────

struct InjectionGuard;

impl InjectionGuard {
    fn new() -> Self {
        crate::hotkeys::mark_injection_start();
        crate::hotkeys::INJECTION_IN_PROGRESS.store(true, Ordering::SeqCst);
        Self
    }
}

impl Drop for InjectionGuard {
    fn drop(&mut self) {
        crate::hotkeys::INJECTION_IN_PROGRESS.store(false, Ordering::SeqCst);
        crate::hotkeys::clear_injection_start();
    }
}

/// Wait for any prior injection to finish. Bounded at 5s: the Windows build
/// has a watchdog thread that force-clears a stuck INJECTION_IN_PROGRESS; on
/// mac the bound lives here in the only wait loop.
fn wait_for_injection_clear() {
    let start = std::time::Instant::now();
    while crate::hotkeys::INJECTION_IN_PROGRESS.load(Ordering::SeqCst) {
        if start.elapsed() > Duration::from_secs(5) {
            log::error!("[Keyfire] injection flag stuck >5s — force-clearing (watchdog)");
            crate::hotkeys::INJECTION_IN_PROGRESS.store(false, Ordering::SeqCst);
            break;
        }
        thread::sleep(Duration::from_millis(5));
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
/// burst use and cuts a runaway within a fraction of a second (same breaker
/// as Windows, which observed ~34/s on 2026-06-04).
fn fire_rate_ok(context: &str) -> bool {
    static FIRE_TIMES: Mutex<Vec<std::time::Instant>> = Mutex::new(Vec::new());
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
    /// Lowercase exact triggers for space-mode expansions. Used by the event
    /// tap to decide whether to pre-swallow a Space keystroke before it leaks
    /// to the target app. Rebuilt whenever assignments change.
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

/// When true, the event tap will swallow the next bare Space keypress because
/// the current expansion buffer exactly matches a space-mode trigger. Avoids
/// the post-hoc "+1 backspace" race that previously caused a leading space
/// to appear in expansions when the target app processed the space slowly.
pub static EXPANSION_PENDING_SPACE: AtomicBool = AtomicBool::new(false);

/// Latched by the tap callback when it actually swallows a Space. Read once and
/// cleared by check_space_trigger to decide whether to skip the extra backspace.
/// If no expansion ends up matching, the swallowed Space is re-injected.
pub static SPACE_PRE_SWALLOWED: AtomicBool = AtomicBool::new(false);

// ── Buffer management (called from stubs/hotkeys.rs processor thread) ──────

/// Recompute EXPANSION_PENDING_SPACE from the current buffer state. Called from
/// every path that mutates the buffer or the space_triggers set. The flag is
/// the only thing the tap callback reads to decide whether to pre-swallow a Space.
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
        // Trim from the front, snapped to a char boundary (buffer chars are
        // layout-resolved and may be multi-byte).
        let mut start = s.buffer.len() - MAX_BUFFER_LENGTH;
        while !s.buffer.is_char_boundary(start) {
            start += 1;
        }
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

/// Called when Space is pressed. Returns true if an expansion fired.
///
/// If the tap pre-swallowed the Space (SPACE_PRE_SWALLOWED set), we skip
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

    // Autocorrect (custom + built-in) is DISABLED FOR ALPHA — same as Windows.

    // Text expansion (space-triggered)
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
            let trigger_len = s.buffer.chars().count();
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

        if let Some(opts) = options {
            if !opts.is_empty() {
                let trigger_len = s.buffer.chars().count();
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

                info!("[Keyfire] Variant expansion: \"{}\" with {} options", trigger_str, opts.len());
                if crate::hotkeys::FILL_IN_ACTIVE.load(Ordering::SeqCst) {
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
        let trigger_len = s.buffer.chars().count();
        let global_vars = s.global_variables.clone();
        s.buffer.clear();
        drop(s);

        info!("[Keyfire] Expansion: \"{}\" → \"{}\"", buffer_lower, log_preview(&text));
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

/// Re-inject a Space keystroke that the tap swallowed pre-emptively but no
/// expansion ended up consuming. The synthetic event is tagged, so the tap
/// passes it through to the target app without re-swallowing.
fn reinject_swallowed_space() {
    crate::actions::send_vk_key_pub(0x20, false);
    crate::actions::send_vk_key_pub(0x20, true);
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
            let trigger_len = imm.trigger.chars().count();

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
                            .get(original_buffer.len().saturating_sub(imm.trigger.len())..)
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

                    info!("[Keyfire] Variant expansion (immediate): \"{}\" with {} options", trigger_str, opts.len());
                    if !crate::hotkeys::FILL_IN_ACTIVE.load(Ordering::SeqCst) {
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
            // Use .get() to avoid panicking if the length falls mid-char (non-ASCII buffer).
            let original_suffix = original_buffer
                .get(original_buffer.len().saturating_sub(imm.trigger.len())..)
                .unwrap_or(&original_buffer);
            let case_pattern = detect_case(original_suffix);
            s.buffer.clear();
            drop(s);

            info!("[Keyfire] Expansion (immediate): \"{}\" → \"{}\"", trigger, log_preview(&text));
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
            if crate::hotkeys::FILL_IN_ACTIVE.load(Ordering::SeqCst) {
                log::info!("[Keyfire] Fire Text Expansion (variant): \"{}\" skipped — fill-in already active", trigger);
                return;
            }
            log::info!("[Keyfire] Fire Text Expansion (variant): \"{}\" with {} options", trigger, opts.len());
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
    log::info!("[Keyfire] Fire Text Expansion (text): \"{}\" → \"{}\"", trigger, log_preview(&text));
    let html_opt = if html.is_empty() { None } else { Some(html.as_str()) };
    fire_expansion(trigger, 0, false, &text, html_opt, &global_vars, CasePattern::Lower);
}

/// Dispatch an expansion fire. `trigger_len` is in CHARS (each becomes one
/// backspace — the Windows original counts bytes, which over-deletes on
/// non-ASCII triggers; chars is the correct currency for keystrokes).
fn fire_expansion(
    _trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    text: &str,
    html: Option<&str>,
    global_vars: &HashMap<String, String>,
    case_pattern: CasePattern,
) {
    #[cfg(target_os = "macos")]
    {
        mac::fire_expansion(_trigger, trigger_len, delete_extra, text, html, global_vars, case_pattern);
    }
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("[stub] fire_expansion: expansion engine is not available on this platform yet");
    }
}

fn fire_variant_expansion(
    trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    options: &[serde_json::Value],
    global_vars: &HashMap<String, String>,
) {
    #[cfg(target_os = "macos")]
    {
        mac::fire_variant_expansion(trigger, trigger_len, delete_extra, options, global_vars);
    }
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("[stub] fire_variant_expansion: not available on this platform yet");
    }
}

fn fire_image_expansion(
    trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    image_path: &str,
    image_scale: u32,
) {
    #[cfg(target_os = "macos")]
    {
        mac::fire_image_expansion(trigger, trigger_len, delete_extra, image_path, image_scale);
    }
    #[cfg(not(target_os = "macos"))]
    {
        log::warn!("[stub] fire_image_expansion: not available on this platform yet");
    }
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

    // Substitute {{varName}} global variables — Pro-only.
    // (Dynamic tokens — date, time, clipboard, cursor — are unlocked for everyone:
    // too many free competitors offer them for the gate to be defensible.)
    if crate::licence::is_pro() && result.contains("{{") {
        for (name, value) in global_vars {
            let token = format!("{{{{{}}}}}", name); // {{name}}
            result = result.replace(&token, value);
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

                // Unformatted Date Math tokens follow the user's default date
                // format from Settings. Explicit-format variants like
                // {date:+1d:YYYY-MM-DD} keep their requested format.
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

    // {cursor} — track position, then remove token
    let mut cursor_back = 0;
    if let Some(idx) = result.find("{cursor}") {
        // Count CHARS after the token (each becomes one Left-arrow press).
        cursor_back = result[idx + "{cursor}".len()..].chars().count();
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

// ── Clipboard seams (NSPasteboard on macOS; no-ops elsewhere) ───────────────

fn read_clipboard() -> Option<String> {
    crate::actions::read_clipboard_pub()
}

fn capture_selection_via_copy() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        mac::capture_selection_via_copy()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// Write plain text (+ optional raw HTML flavor) to the general pasteboard.
/// The mac analogue of the Windows CF_UNICODETEXT + CF_HTML dual write —
/// rich-text targets (Mail, Pages, browsers) read `public.html`, everything
/// else falls back to the plain string. pub(crate) because lib.rs's
/// `paste_clipboard_item` reuses this for clipboard-history rows captured
/// with an HTML fragment.
pub(crate) fn write_clipboard_dual(text: &str, html: Option<&str>) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::write_clipboard_dual(text, html)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// Snapshot every flavor currently on the general pasteboard as (UTI, bytes)
/// pairs. The mac twin of the Windows multi-format HGLOBAL snapshot — images,
/// RTF and app-private flavors all survive an expansion fire. pub(crate) so
/// stubs/actions.rs's clipboard injection restores non-text clipboards too.
pub(crate) fn snapshot_clipboard() -> Vec<(String, Vec<u8>)> {
    #[cfg(target_os = "macos")]
    {
        mac::snapshot_clipboard()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Vec::new()
    }
}

/// Restore a snapshot by clearing the pasteboard and re-writing every flavor.
/// Records the resulting changeCount as a self-write so the clipboard
/// listener skips it. An empty snapshot still clears — matching the
/// "clipboard was empty before we wrote" pre-state.
pub(crate) fn restore_clipboard_snapshot(snapshot: &[(String, Vec<u8>)]) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac::restore_clipboard_snapshot(snapshot)
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

/// The Windows original registers private clipboard formats to keep Keyfire's
/// writes out of Win+V history — and must be called with the clipboard OPEN.
/// On macOS the equivalent (`org.nspasteboard.ConcealedType`) is written
/// inline by `write_clipboard_dual` during the write itself, so this is a
/// structural no-op kept for call-site compatibility. `unsafe` matches the
/// original signature — callers invoke it inside unsafe blocks.
pub(crate) unsafe fn mark_clipboard_excluded() {}

// ── Key token support ({key:Combo:N}, {key:Combo}, …) ──────────────────────

enum KeySegment {
    Text(String),
    /// Modifier + main key as NATIVE mac keycodes.
    Key { mod_kcs: Vec<u16>, main_kc: u16, repeat: u32 },
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
            if let Some((mod_kcs, main_kc)) = combo_str_to_keycodes(combo) {
                segments.push(KeySegment::Key { mod_kcs, main_kc, repeat });
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
/// (modifier mac keycodes, main mac keycode). Modifier tokens use accelerator
/// semantics, matching send_vk_key_pub: "Ctrl" and "Win" both mean ⌘ on
/// macOS ({key:Ctrl+C} authored on Windows should copy on a Mac), "Alt" is
/// ⌥, "Shift" ⇧.
fn combo_str_to_keycodes(combo: &str) -> Option<(Vec<u16>, u16)> {
    const KC_LCMD: u16 = 55;
    const KC_LSHIFT: u16 = 56;
    const KC_LOPTION: u16 = 58;
    let parts: Vec<&str> = combo.split('+').collect();
    if parts.is_empty() {
        return None;
    }
    let mut mod_kcs: Vec<u16> = Vec::new();
    // Everything before the last part is a modifier; last part is the key.
    let main_part = parts[parts.len() - 1];
    for &part in &parts[..parts.len().saturating_sub(1)] {
        let kc = match part {
            "Ctrl" | "Win" => KC_LCMD,
            "Shift" => KC_LSHIFT,
            "Alt" => KC_LOPTION,
            _ => continue,
        };
        if !mod_kcs.contains(&kc) {
            mod_kcs.push(kc);
        }
    }
    key_name_to_keycode(main_part).map(|kc| (mod_kcs, kc))
}

/// Resolve a {key:} key name to a mac keycode. Names are the same universe
/// the Windows key_name_to_vk accepts; aliases map onto the key_id names the
/// hotkeys table resolves. Keys with no mac equivalent (PrintScreen,
/// ScrollLock, Pause) return None and the segment is dropped — same graceful
/// degradation as an unknown name on Windows.
fn key_name_to_keycode(name: &str) -> Option<u16> {
    #[cfg(target_os = "macos")]
    {
        let canonical = match name {
            "Esc" => "Escape",
            "Del" => "Delete",
            "PgUp" => "PageUp",
            "PgDn" => "PageDown",
            "Caps" => "CapsLock",
            other => other,
        };
        crate::hotkeys::display_name_to_keycode(canonical)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

// ── Built-in autocorrect dictionary (engine disabled for Alpha, kept for parity) ─

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

/// Strip rich-text-editor chrome from expansion HTML before it reaches the
/// pasteboard. The contenteditable serializes the app's OWN UI font as
/// inline `font-family: var(--font-body)` declarations — CSS variables mean
/// nothing outside the app, so paste targets fall back to their HTML
/// default font (Times) instead of the caret font. Remove every
/// font-family declaration whose value is a var() reference, then drop
/// style attributes left empty so a chrome-only span reads as plain.
fn strip_editor_font_styles(html: &str) -> String {
    let re_font = regex_lite::Regex::new(r"font-family:\s*var\([^)]*\)\s*;?\s*").unwrap();
    let stripped = re_font.replace_all(html, "");
    let re_empty_style = regex_lite::Regex::new(r#"\s*style="\s*""#).unwrap();
    re_empty_style.replace_all(&stripped, "").to_string()
}

/// True when a resolved HTML fragment carries real formatting worth pasting.
///
/// The rich-text editor stores an `html` body for EVERY expansion, including
/// ones the user never formatted. Pasting that unstyled HTML makes rich-text
/// targets fall back to the HTML default font (Times in TextEdit's WebKit
/// conversion) instead of the caret's font — so semantically plain fragments
/// paste better as text-only. Structural tags (<p>, <div>, <br>, bare
/// <span>) don't count as formatting; inline styles and any styling/content
/// tag do. Conservative on unknown tags: keep the HTML.
fn html_has_formatting(html: &str) -> bool {
    let lower = html.to_lowercase();
    if lower.contains("style=") {
        return true;
    }
    let mut rest = lower.as_str();
    while let Some(start) = rest.find('<') {
        let after = &rest[start + 1..];
        // Tag name: letters up to whitespace / '>' / '/'; skip closers.
        let name_src = after.strip_prefix('/').unwrap_or(after);
        let name: String = name_src
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !matches!(name.as_str(), "" | "p" | "div" | "br" | "span") {
            return true;
        }
        match after.find('>') {
            Some(end) => rest = &after[end + 1..],
            None => break,
        }
    }
    false
}

/// Resolve token chips inside an HTML expansion. Each
/// `<span class="rte-token" data-token="{...}">display</span>` is replaced
/// with the resolved value of its token.
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

    // Strip residual ZWSPs (editor cursor anchors serialized by innerHTML)
    // and the editor's own var()-based font-family chrome — the single
    // chokepoint all three fire paths resolve through.
    strip_editor_font_styles(&resolved.replace('\u{200B}', ""))
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
    // Rebuild the space-trigger set used by the tap callback for pre-swallow.
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
        "[Keyfire] Expansion assignments updated: {} entries ({} space-triggers)",
        s.assignments.len(),
        s.space_triggers.len()
    );
    refresh_pending_flag(&s);
}

pub fn set_autocorrect_enabled(enabled: bool) {
    state().lock().unwrap().autocorrect_enabled = enabled;
    info!("[Keyfire] Autocorrect config: {} (engine disabled for Alpha)", enabled);
}

pub fn update_global_variables(vars: HashMap<String, String>) {
    state().lock().unwrap().global_variables = vars;
}

pub fn get_global_variables() -> HashMap<String, String> {
    state().lock().unwrap().global_variables.clone()
}

// ── macOS implementation: pasteboard + injection + fire paths ───────────────
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use log::info;
    use objc2_app_kit::{
        NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypePNG, NSPasteboardTypeString,
        NSPasteboardTypeTIFF,
    };
    use objc2_foundation::{NSData, NSString};

    /// mac virtual keycodes used directly by the engine.
    const KC_BACKSPACE_VK: u16 = 0x08; // Windows VK — translated by send_vk_key_pub
    const VK_SPACE: u16 = 0x20;
    const VK_LEFT: u16 = 0x25;
    const VK_LCONTROL: u16 = 0xA2; // → ⌘ (accelerator mapping)
    const VK_C: u16 = 0x43;
    const VK_V: u16 = 0x56;

    /// Marker type honoured by third-party clipboard managers (Maccy, Paste,
    /// Alfred, …): payloads carrying it are treated as transient/concealed
    /// and skipped — the community convention from nspasteboard.org, the mac
    /// analogue of ExcludeClipboardContentFromMonitorProcessing.
    const CONCEALED_TYPE: &str = "org.nspasteboard.ConcealedType";

    fn tap_vk(vk: u16) {
        crate::actions::send_vk_key_pub(vk, false);
        crate::actions::send_vk_key_pub(vk, true);
    }

    /// Send the ⌘V chord via the VK translation layer (LCtrl → ⌘).
    fn paste_cmd_v() {
        crate::actions::send_vk_key_pub(VK_LCONTROL, false);
        crate::actions::send_vk_key_pub(VK_V, false);
        crate::actions::send_vk_key_pub(VK_V, true);
        crate::actions::send_vk_key_pub(VK_LCONTROL, true);
    }

    fn change_count() -> i64 {
        pasteboard().changeCount() as i64
    }

    // NSPasteboard::generalPasteboard is `unsafe fn` in objc2-app-kit 0.3;
    // the pasteboard itself is safe to use from any thread for these ops
    // (same contract as stubs/actions.rs / stubs/clipboard.rs).
    #[allow(unused_unsafe)]
    fn pasteboard() -> objc2::rc::Retained<NSPasteboard> {
        unsafe { NSPasteboard::generalPasteboard() }
    }

    pub(super) fn write_clipboard_dual(text: &str, html: Option<&str>) -> bool {
        info!(
            "[Keyfire] Clipboard write (expansions{}): \"{}\"",
            if html.is_some() { ", +html" } else { "" },
            log_preview(text)
        );
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(true, Ordering::SeqCst);

        let pb = pasteboard();
        pb.clearContents();
        let ok = pb.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString });
        if !ok {
            log::warn!("[CLIP] NSPasteboard setString failed (dual write)");
            return false;
        }
        if let Some(h) = html {
            // Raw HTML fragment — macOS rich-text targets read public.html
            // directly; no CF_HTML byte-offset container exists here.
            if !pb.setString_forType(&NSString::from_str(h), unsafe { NSPasteboardTypeHTML }) {
                log::warn!("[CLIP] NSPasteboard setString(HTML) failed — plain text still written");
            }
        }
        // Concealed marker: keeps Keyfire's injected payload out of
        // third-party clipboard managers. Best effort.
        let _ = pb.setString_forType(&NSString::from_str(""), &NSString::from_str(CONCEALED_TYPE));

        crate::actions::record_self_clipboard_write();
        true
    }

    fn write_clipboard(text: &str) -> bool {
        write_clipboard_dual(text, None)
    }

    pub(super) fn snapshot_clipboard() -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<(String, Vec<u8>)> = Vec::new();
        let pb = pasteboard();
        let Some(types) = pb.types() else { return out };
        for t in types.iter() {
            let name = t.to_string();
            if out.iter().any(|(n, _)| n == &name) {
                continue;
            }
            // dataForType resolves lazy/promised flavors, same as the Windows
            // GetClipboardData snapshot. Flavors the owner can't render come
            // back None and are skipped.
            if let Some(data) = pb.dataForType(&t) {
                out.push((name, data.to_vec()));
            }
        }
        out
    }

    pub(super) fn restore_clipboard_snapshot(snapshot: &[(String, Vec<u8>)]) -> bool {
        info!("[Keyfire] Clipboard restore: {} flavors", snapshot.len());
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(true, Ordering::SeqCst);
        let pb = pasteboard();
        pb.clearContents();
        for (uti, bytes) in snapshot {
            let data = NSData::with_bytes(bytes);
            if !pb.setData_forType(Some(&data), &NSString::from_str(uti)) {
                log::warn!("[CLIP] restore: setData failed for {}", uti);
            }
        }
        // The restore is a mechanical re-write of the user's prior content,
        // not a fresh copy — record it so the listener skips the change.
        crate::actions::record_self_clipboard_write();
        true
    }

    /// Synchronously capture the user's current text selection by sending ⌘C,
    /// reading the resulting pasteboard contents, and then restoring whatever
    /// was on the pasteboard before. Returns None if no selection (changeCount
    /// didn't advance) or the read failed.
    ///
    /// Blocks the calling thread for ~50–200ms. Callers must be on a thread
    /// that can absorb this — typically a spawned injection thread or the
    /// processor thread (same budget as the Windows original).
    pub(super) fn capture_selection_via_copy() -> Option<String> {
        // Never under test: this posts a REAL ⌘C to the frontmost app and
        // mutates the live pasteboard outside PASTEBOARD_TEST_LOCK.
        if cfg!(test) {
            return None;
        }
        let snapshot = snapshot_clipboard();
        let before = change_count();

        // Mark BOTH the ⌘C write and the restore as ours so the clipboard
        // listener ignores them.
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(true, Ordering::SeqCst);

        // Release any user-held modifiers so the ⌘C lands cleanly; restored after.
        let held = crate::actions::release_held_modifiers();

        crate::actions::send_vk_key_pub(VK_LCONTROL, false); // → ⌘ down
        crate::actions::send_vk_key_pub(VK_C, false);
        thread::sleep(Duration::from_millis(15));
        crate::actions::send_vk_key_pub(VK_C, true);
        crate::actions::send_vk_key_pub(VK_LCONTROL, true);

        crate::actions::restore_modifiers(&held);

        // Wait for the pasteboard changeCount to advance, up to 200ms.
        let mut sel: Option<String> = None;
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_millis(200) {
            thread::sleep(Duration::from_millis(15));
            if change_count() != before {
                sel = crate::actions::read_clipboard_pub();
                break;
            }
        }

        // Restore previous pasteboard contents (also suppressed from history).
        restore_clipboard_snapshot(&snapshot);
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);

        sel.filter(|s| !s.is_empty())
    }

    // ── Injection paths ──────────────────────────────────────────────────────

    /// Inject text via clipboard paste, restoring the full multi-flavor
    /// pasteboard afterwards. When `html` is provided, `public.html` is
    /// written alongside so rich-text targets receive formatted content.
    fn inject_via_clipboard(text: &str, html: Option<&str>) {
        info!("[Keyfire] Inject (clipboard): \"{}\"", log_preview(text));
        let snapshot = snapshot_clipboard();

        // Always bundle the trailing space into the clipboard payload — a
        // separate Space keystroke races async paste handlers in
        // Chromium/Electron apps (same bundling rationale as Windows).
        //
        // HTML gets &nbsp; INSIDE the last closing tag: appending after it
        // makes rich-text editors wrap the stray text node in a new
        // paragraph (blank line below the expansion).
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

        // If the write fails, do NOT paste (would paste old clipboard content)
        if !write_clipboard_dual(&payload_text, payload_html.as_deref()) {
            log::warn!("[Keyfire] write_clipboard FAILED — skipping paste to avoid pasting wrong content");
            return;
        }
        let post_write = change_count();

        // Release physically held modifiers (hard rule 5), paste, restore.
        let held = crate::actions::release_held_modifiers();
        paste_cmd_v();
        crate::actions::restore_modifiers(&held);

        // Restore clipboard after paste settles. 150ms: slow paste handlers
        // (Office-class apps) read the pasteboard from their event queue.
        thread::sleep(Duration::from_millis(150));
        // Only restore if the pasteboard still holds our content. If the
        // changeCount advanced, the user (or another process) copied
        // something new during the paste window — leave their content alone.
        if change_count() == post_write {
            restore_clipboard_snapshot(&snapshot);
        }
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
    }

    /// Inject a text segment via clipboard paste, without a trailing space or
    /// clipboard restore (caller snapshots/restores around the whole {key:}
    /// segment sequence).
    fn inject_text_segment(text: &str) {
        if !write_clipboard(text) {
            return;
        }
        paste_cmd_v();
        thread::sleep(Duration::from_millis(30));
    }

    /// Move the cursor back `count` positions via Left-arrow taps ({cursor}
    /// token). Individual tagged events — no batched SendInput on mac.
    fn send_left_arrows(count: usize) {
        for _ in 0..count {
            tap_vk(VK_LEFT);
        }
    }

    /// Delete `count` characters with Backspace taps.
    fn send_backspaces(count: usize) {
        for _ in 0..count {
            tap_vk(KC_BACKSPACE_VK);
            thread::sleep(Duration::from_millis(5));
        }
    }

    pub(super) fn fire_expansion(
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
        // Check for {fillIn:...} tokens — if present, spawn a dedicated thread
        // for the entire fill-in + injection flow so the processor thread is
        // never blocked. The HTML version (if present) is forwarded so
        // rich-text formatting is preserved through the fill-in path.
        let fill_in_fields = extract_fill_in_fields(text);
        if !fill_in_fields.is_empty() {
            if crate::hotkeys::FILL_IN_ACTIVE.load(Ordering::SeqCst) {
                return;
            }
            let text = text.to_string();
            let html_owned: Option<String> = html.map(|s| s.to_string());
            let global_vars = global_vars.clone();
            let trigger_str = _trigger.to_string();
            thread::spawn(move || {
                fire_expansion_with_fillin(
                    fill_in_fields,
                    &text,
                    html_owned.as_deref(),
                    trigger_len,
                    delete_extra,
                    &global_vars,
                    &trigger_str,
                    case_pattern,
                );
            });
            return;
        }

        // No fill-in tokens — resolve and inject directly. Empty fill-in map
        // since there are no field values to reference in expressions.
        let empty_fillin: HashMap<String, String> = HashMap::new();
        let (resolved, cursor_back) = resolve_tokens(text, global_vars, &empty_fillin);
        let resolved = apply_case(&resolved, case_pattern);

        // Resolve HTML in parallel. Only used when the target app accepts
        // public.html — plain-text apps read the string flavor. Skip HTML if
        // the expansion uses inline key tokens (those need per-segment
        // injection that doesn't compose with a single paste).
        let resolved_html: Option<String> = html.and_then(|h| {
            if h.is_empty() || h.contains("{key:") {
                None
            } else {
                let r = resolve_tokens_html(h, global_vars, &empty_fillin);
                // Plain fragments paste text-only so the target keeps its
                // caret font — see html_has_formatting.
                html_has_formatting(&r).then_some(r)
            }
        });

        if resolved.is_empty() {
            return;
        }

        crate::analytics::log_action(
            "expansion",
            resolved.chars().filter(|c| *c != '\r').count() as u32,
            _trigger,
            _trigger,
        );

        // Wait for any prior injection to finish (handles sequential fires)
        wait_for_injection_clear();

        // Set flag immediately on the processor thread — no race window
        let guard = InjectionGuard::new();

        // Spawn on a separate thread to avoid blocking the event processor.
        // No SetForegroundWindow dance: the frontmost app keeps focus on mac.
        thread::spawn(move || {
            let _guard = guard;

            // Delay to let the Space/character keystroke be processed by the target app
            thread::sleep(Duration::from_millis(30));

            // Delete trigger word + space (if applicable)
            let delete_count = trigger_len + if delete_extra { 1 } else { 0 };
            send_backspaces(delete_count);

            thread::sleep(Duration::from_millis(10));

            if resolved.contains("{key:") {
                // Inline key-token path: inject each text/key segment in order
                let snapshot = snapshot_clipboard();
                let held = crate::actions::release_held_modifiers();
                for seg in parse_key_segments(&resolved) {
                    match seg {
                        KeySegment::Text(ref t) if !t.is_empty() => {
                            inject_text_segment(t);
                        }
                        KeySegment::Key { ref mod_kcs, main_kc, repeat } => {
                            for _ in 0..repeat {
                                crate::actions::post_chord_keycodes(mod_kcs, Some(main_kc));
                                thread::sleep(Duration::from_millis(10));
                            }
                        }
                        _ => {}
                    }
                }
                crate::actions::restore_modifiers(&held);
                let post = change_count();
                thread::sleep(Duration::from_millis(50));
                // Skip restore if the user copied something during the paste window.
                if change_count() == post {
                    restore_clipboard_snapshot(&snapshot);
                }
                crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
            } else {
                // Normal path: single clipboard-paste inject (expansions always
                // paste — matches the Windows should_use_clipboard()==true).
                inject_via_clipboard(&resolved, resolved_html.as_deref());

                // Move cursor back if {cursor} was present.
                if cursor_back > 0 {
                    thread::sleep(Duration::from_millis(10));
                    send_left_arrows(cursor_back);
                }
            }

            // No replay/re-check on mac: keystrokes typed during injection
            // passed through live (tagged-event filtering — see module docs).
        });
    }

    /// Fill-in flow: runs entirely on a dedicated thread so the processor
    /// thread is never blocked. Sequence: show window → wait for response →
    /// resolve tokens → inject.
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
        crate::hotkeys::FILL_IN_ACTIVE.store(true, Ordering::SeqCst);

        let app = match APP_HANDLE.get() {
            Some(a) => a,
            None => {
                log::error!("[EXP] No app handle — cannot show fill-in window");
                crate::hotkeys::FILL_IN_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Capture the frontmost app BEFORE showing the fill-in (it steals focus).
        let target_pid = crate::foreground::capture_frontmost_pid();

        let (tx, rx) = mpsc::channel();
        *fill_in_tx().lock().unwrap() = Some(tx);

        show_fillin_window(app, serde_json::json!({
            "fields": fill_in_fields,
            "theme": resolved_theme(),
        }));

        // Block on this dedicated thread waiting for user response (60s timeout)
        let response = rx.recv_timeout(Duration::from_secs(60));
        *fill_in_tx().lock().unwrap() = None;

        hide_fillin_window(app, target_pid);
        crate::hotkeys::FILL_IN_ACTIVE.store(false, Ordering::SeqCst);

        let (text_after_fillin, fillin_values) = match response {
            Ok(Some(values)) => (resolve_fill_in_tokens(text, &values), values),
            Ok(None) => return,
            Err(_) => return,
        };

        // Resolve remaining tokens. fillin_values is passed in so `{=expr}` and
        // `{if}` conditions can reference fields by their label as bare identifiers.
        let (resolved, cursor_back) = resolve_tokens(&text_after_fillin, global_vars, &fillin_values);
        let resolved = apply_case(&resolved, case_pattern);

        // Resolve the HTML alongside the text so rich-text targets still
        // receive formatting when the expansion uses fill-in fields.
        let resolved_html: Option<String> = html.and_then(|h| {
            if h.is_empty() || h.contains("{key:") {
                None
            } else {
                let html_after_fillin = resolve_fill_in_tokens(h, &fillin_values);
                let r = resolve_tokens_html(&html_after_fillin, global_vars, &fillin_values);
                html_has_formatting(&r).then_some(r)
            }
        });

        if resolved.is_empty() {
            return;
        }

        crate::analytics::log_action(
            "expansion",
            resolved.chars().filter(|c| *c != '\r').count() as u32,
            trigger_str,
            trigger_str,
        );

        wait_for_injection_clear();
        let _guard = InjectionGuard::new();

        // Delay to let focus settle after the fill-in window hides
        thread::sleep(Duration::from_millis(30));

        let delete_count = trigger_len + if delete_extra { 1 } else { 0 };
        send_backspaces(delete_count);
        thread::sleep(Duration::from_millis(10));

        if resolved.contains("{key:") {
            let snapshot = snapshot_clipboard();
            let held = crate::actions::release_held_modifiers();
            for seg in parse_key_segments(&resolved) {
                match seg {
                    KeySegment::Text(ref t) if !t.is_empty() => {
                        inject_text_segment(t);
                    }
                    KeySegment::Key { ref mod_kcs, main_kc, repeat } => {
                        for _ in 0..repeat {
                            crate::actions::post_chord_keycodes(mod_kcs, Some(main_kc));
                            thread::sleep(Duration::from_millis(10));
                        }
                    }
                    _ => {}
                }
            }
            crate::actions::restore_modifiers(&held);
            let post = change_count();
            thread::sleep(Duration::from_millis(50));
            if change_count() == post {
                restore_clipboard_snapshot(&snapshot);
            }
            crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
        } else {
            inject_via_clipboard(&resolved, resolved_html.as_deref());
            if cursor_back > 0 {
                thread::sleep(Duration::from_millis(10));
                send_left_arrows(cursor_back);
            }
        }
    }

    /// Fire a variant expansion: show selection popup, wait for user choice,
    /// inject selected text.
    pub(super) fn fire_variant_expansion(
        trigger: &str,
        trigger_len: usize,
        delete_extra: bool,
        options: &[serde_json::Value],
        global_vars: &HashMap<String, String>,
    ) {
        if !fire_rate_ok(trigger) {
            return;
        }
        crate::hotkeys::FILL_IN_ACTIVE.store(true, Ordering::SeqCst);

        let app = match APP_HANDLE.get() {
            Some(a) => a.clone(),
            None => {
                crate::hotkeys::FILL_IN_ACTIVE.store(false, Ordering::SeqCst);
                return;
            }
        };

        // Capture the frontmost app before showing the popup
        let target_pid = crate::foreground::capture_frontmost_pid();

        // Erase the trigger text
        {
            let _guard = InjectionGuard::new();
            let erase_count = trigger_len + if delete_extra { 1 } else { 0 };
            for _ in 0..erase_count {
                tap_vk(KC_BACKSPACE_VK);
                thread::sleep(Duration::from_millis(2));
            }
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

        let theme = resolved_theme();
        show_fillin_window(&app, serde_json::json!({
            "mode": "variant",
            "options": option_labels,
            "previews": option_previews,
            "theme": theme,
        }));

        // Wait for selection (60s timeout)
        let response = rx.recv_timeout(Duration::from_secs(60));
        *fill_in_tx().lock().unwrap() = None;

        hide_fillin_window(&app, target_pid);
        crate::hotkeys::FILL_IN_ACTIVE.store(false, Ordering::SeqCst);

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
        // user for those values before injecting.
        let fill_in_fields = extract_fill_in_fields(&selected_text);
        let (final_text, final_html, fillin_values) = if !fill_in_fields.is_empty() {
            // Re-acquire FILL_IN_ACTIVE before re-showing the window
            crate::hotkeys::FILL_IN_ACTIVE.store(true, Ordering::SeqCst);

            let (tx2, rx2) = mpsc::channel();
            *fill_in_tx().lock().unwrap() = Some(tx2);

            show_fillin_window(&app, serde_json::json!({
                "fields": fill_in_fields,
                "theme": theme,
            }));

            let response2 = rx2.recv_timeout(Duration::from_secs(60));
            *fill_in_tx().lock().unwrap() = None;

            hide_fillin_window(&app, target_pid);
            crate::hotkeys::FILL_IN_ACTIVE.store(false, Ordering::SeqCst);

            let values = match response2 {
                Ok(Some(v)) => v,
                _ => return, // user cancelled
            };

            let substituted = resolve_fill_in_tokens(&selected_text, &values);
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

        // Resolve HTML in parallel when present. Skip if the variant uses
        // inline key tokens — same rule as the main expansion path.
        let resolved_html: Option<String> = final_html.and_then(|h| {
            if h.is_empty() || h.contains("{key:") {
                None
            } else {
                let r = resolve_tokens_html(&h, global_vars, &fillin_values);
                html_has_formatting(&r).then_some(r)
            }
        });

        crate::analytics::log_action(
            "expansion",
            resolved.chars().filter(|c| *c != '\r').count() as u32,
            trigger,
            trigger,
        );

        wait_for_injection_clear();
        let _guard = InjectionGuard::new();

        thread::sleep(Duration::from_millis(30));
        inject_via_clipboard(&resolved, resolved_html.as_deref());

        if cursor_back > 0 {
            thread::sleep(Duration::from_millis(10));
            send_left_arrows(cursor_back);
        }
        // Trailing space is bundled into the clipboard payload by
        // inject_via_clipboard — no separate Space keystroke.
    }

    /// Fire an image expansion: read image from disk, optionally resize, write
    /// PNG + TIFF to the pasteboard, paste. No clipboard restore — the image
    /// stays on the pasteboard (same as Windows).
    pub(super) fn fire_image_expansion(
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

        if !std::path::Path::new(image_path).exists() {
            log::warn!("[Keyfire] Image expansion: file not found at \"{}\"", image_path);
            return;
        }

        let file_bytes = match std::fs::read(image_path) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("[Keyfire] Image expansion: failed to read \"{}\": {}", image_path, e);
                return;
            }
        };

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

        // PNG bytes: original file bytes when unmodified PNG, else re-encode.
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
        // TIFF alongside PNG: AppKit-native consumers (TextEdit RTFD, older
        // apps) read public.tiff; modern apps prefer public.png.
        let tiff_bytes = {
            let mut buf = std::io::Cursor::new(Vec::new());
            if img.write_to(&mut buf, image::ImageFormat::Tiff).is_ok() {
                buf.into_inner()
            } else {
                Vec::new()
            }
        };
        if png_bytes.is_empty() && tiff_bytes.is_empty() {
            log::warn!("[Keyfire] Image expansion: could not encode image for the pasteboard");
            return;
        }

        crate::analytics::log_action("expansion", 0, _trigger, _trigger);

        wait_for_injection_clear();
        let guard = InjectionGuard::new();

        thread::spawn(move || {
            let _guard = guard;

            // Delay to let the trigger keystroke be processed
            thread::sleep(Duration::from_millis(30));

            let delete_count = trigger_len + if delete_extra { 1 } else { 0 };
            send_backspaces(delete_count);
            thread::sleep(Duration::from_millis(10));

            // Write image to the pasteboard
            crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(true, Ordering::SeqCst);
            let pb = pasteboard();
            pb.clearContents();
            let mut wrote = false;
            if !png_bytes.is_empty() {
                wrote |= pb.setData_forType(
                    Some(&NSData::with_bytes(&png_bytes)),
                    unsafe { NSPasteboardTypePNG },
                );
            }
            if !tiff_bytes.is_empty() {
                wrote |= pb.setData_forType(
                    Some(&NSData::with_bytes(&tiff_bytes)),
                    unsafe { NSPasteboardTypeTIFF },
                );
            }
            let _ = pb.setString_forType(&NSString::from_str(""), &NSString::from_str(CONCEALED_TYPE));
            crate::actions::record_self_clipboard_write();
            if !wrote {
                log::warn!("[Keyfire] Image expansion: pasteboard write failed — skipping paste");
                crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
                return;
            }

            // Release physically held modifiers, paste, restore.
            let held = crate::actions::release_held_modifiers();
            paste_cmd_v();
            crate::actions::restore_modifiers(&held);

            // No clipboard restore for images — leave image on the pasteboard.
            crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
        });
    }

    // ── Fill-in window helpers ───────────────────────────────────────────────

    /// Resolve the configured theme for popup windows. "auto" resolves against
    /// the OS appearance Rust-side (WKWebView misreports prefers-color-scheme,
    /// same fix as the overlays).
    fn resolved_theme() -> String {
        let raw = crate::config::load_config()
            .and_then(|c| c.get("theme").and_then(|v| v.as_str()).map(String::from))
            .unwrap_or_else(|| "dark".to_string());
        if raw == "auto" {
            if crate::foreground::os_theme_is_dark() { "dark".into() } else { "light".into() }
        } else {
            raw
        }
    }

    /// Show the fill-in window centred on the cursor's monitor, wait for the
    /// renderer ready handshake, then emit the payload. Mirrors the Windows
    /// flow minus HWND bookkeeping: FILLIN_HWND carries 1 as a "visible" flag
    /// (the tap consults it to keep fill-in keystrokes out of the buffer).
    fn show_fillin_window(app: &AppHandle, payload: serde_json::Value) {
        use tauri::{Emitter, Manager};
        let Some(win) = app.get_webview_window("fillin") else {
            log::error!("[EXP] fill-in window missing");
            return;
        };

        crate::hotkeys::FILLIN_HWND.store(1, Ordering::SeqCst);

        // Position on the monitor containing the cursor: centred, one-third
        // from the top (same layout maths as the overlay windows).
        let monitor = app
            .cursor_position()
            .ok()
            .and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten())
            .or_else(|| app.primary_monitor().ok().flatten());
        if let Some(monitor) = monitor {
            let wa = monitor.work_area();
            let scale = monitor.scale_factor();
            let phys_w = (420.0 * scale).round() as i32;
            let phys_x = wa.position.x + ((wa.size.width as i32) - phys_w) / 2;
            let phys_y = wa.position.y + (wa.size.height as i32) / 3;
            let _ = win.set_position(tauri::PhysicalPosition::new(phys_x, phys_y));
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

        let _ = win.emit("fill-in-show", payload);
    }

    /// Hide the fill-in window and hand focus back to the captured target app.
    fn hide_fillin_window(app: &AppHandle, target_pid: i32) {
        use tauri::Manager;
        crate::hotkeys::FILLIN_HWND.store(0, Ordering::SeqCst);
        if let Some(win) = app.get_webview_window("fillin") {
            let _ = win.hide();
        }
        if target_pid > 0 {
            crate::foreground::activate_pid(target_pid);
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fillin_token_parses_typed_and_legacy() {
        let f = parse_fillin_token("Name");
        assert_eq!(f.label, "Name");
        assert_eq!(f.kind, "text");
        assert!(f.options.is_empty());
        assert_eq!(f.default, None);

        let f = parse_fillin_token("Priority:dropdown:Low,Medium,High:default=Medium");
        assert_eq!(f.label, "Priority");
        assert_eq!(f.kind, "dropdown");
        assert_eq!(f.options, vec!["Low", "Medium", "High"]);
        assert_eq!(f.default.as_deref(), Some("Medium"));

        // default values may contain colons
        let f = parse_fillin_token("When:text:default=12:30");
        assert_eq!(f.label, "When");
        assert_eq!(f.default.as_deref(), Some("12:30"));
    }

    #[test]
    fn fillin_values_substitute_with_defaults() {
        let mut values = HashMap::new();
        values.insert("Name".to_string(), "Rory".to_string());
        let out = resolve_fill_in_tokens(
            "Hi {fillIn:Name}, priority {fillIn:Priority:text:default=High}!",
            &values,
        );
        assert_eq!(out, "Hi Rory, priority High!");
    }

    #[test]
    fn smart_case_detection_and_apply() {
        assert!(matches!(detect_case("brb"), CasePattern::Lower));
        assert!(matches!(detect_case("Brb"), CasePattern::Capitalized));
        assert!(matches!(detect_case("BRB"), CasePattern::Upper));
        assert!(matches!(detect_case(":123"), CasePattern::Lower));
        assert_eq!(apply_case("be right back", CasePattern::Upper), "BE RIGHT BACK");
        assert_eq!(apply_case("be right back", CasePattern::Capitalized), "Be right back");
    }

    #[test]
    fn buffer_trims_at_char_boundary() {
        buffer_clear();
        for _ in 0..30 {
            buffer_push('é'); // 2 bytes each — exercises the boundary snap
        }
        {
            let s = state().lock().unwrap();
            assert!(s.buffer.len() <= MAX_BUFFER_LENGTH);
            assert!(s.buffer.chars().all(|c| c == 'é'));
        }
        buffer_clear();
    }

    #[test]
    fn space_trigger_set_and_pending_flag() {
        let mut assignments = HashMap::new();
        assignments.insert(
            "GLOBAL::EXPANSION::brb".to_string(),
            serde_json::json!({"data": {"text": "be right back"}}),
        );
        assignments.insert(
            "GLOBAL::EXPANSION:::sig".to_string(),
            serde_json::json!({"data": {"text": "x", "triggerMode": "immediate"}}),
        );
        update_assignments(assignments);

        buffer_clear();
        buffer_push('b');
        buffer_push('r');
        assert!(!EXPANSION_PENDING_SPACE.load(Ordering::SeqCst));
        buffer_push('b');
        assert!(EXPANSION_PENDING_SPACE.load(Ordering::SeqCst));
        buffer_pop();
        assert!(!EXPANSION_PENDING_SPACE.load(Ordering::SeqCst));
        buffer_clear();
        update_assignments(HashMap::new());
    }

    #[test]
    fn resolve_tokens_dates_and_cursor() {
        let now = chrono::Local::now();
        let gv = HashMap::new();
        let fv = HashMap::new();
        let (out, back) = resolve_tokens("today {date:YYYY-MM-DD} end{cursor}!!", &gv, &fv);
        assert_eq!(out, format!("today {} end!!", now.format("%Y-%m-%d")));
        assert_eq!(back, 2);
    }

    #[test]
    fn resolve_tokens_set_if_expr() {
        // {=} triggers a pasteboard read for the expression scope — hold the
        // pasteboard lock so it doesn't race the mutating clipboard tests.
        #[cfg(target_os = "macos")]
        let _pb = crate::actions::PASTEBOARD_TEST_LOCK.lock().unwrap();
        let gv = HashMap::new();
        let fv = HashMap::new();
        let (out, _) = resolve_tokens(
            "{set n = 6}{if n > 5}big{else}small{endif} {=n * 2}",
            &gv,
            &fv,
        );
        assert_eq!(out, "big 12");
    }

    #[test]
    fn key_segments_parse_with_repeat() {
        let segs = parse_key_segments("abc{key:Tab:2}def");
        assert_eq!(segs.len(), 3);
        match &segs[1] {
            KeySegment::Key { mod_kcs, main_kc, repeat } => {
                assert!(mod_kcs.is_empty());
                assert_eq!(*repeat, 2);
                #[cfg(target_os = "macos")]
                assert_eq!(*main_kc, 48); // Tab
            }
            _ => panic!("expected key segment"),
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn combo_names_use_accelerator_semantics() {
        // Ctrl and Win both mean ⌘ (55); aliases resolve.
        assert_eq!(combo_str_to_keycodes("Ctrl+C"), Some((vec![55], 8)));
        assert_eq!(combo_str_to_keycodes("Win+Tab"), Some((vec![55], 48)));
        assert_eq!(combo_str_to_keycodes("Shift+Esc"), Some((vec![56], 53)));
        assert_eq!(combo_str_to_keycodes("PgUp"), Some((vec![], 116)));
        // Ctrl+Win dedups to a single ⌘
        assert_eq!(combo_str_to_keycodes("Ctrl+Win+A"), Some((vec![55], 0)));
    }

    #[test]
    fn editor_font_chrome_is_stripped_and_reads_plain() {
        // Real shape from a saved expansion: the editor's UI font leaks in
        // as an inline var() font-family — meaningless outside the app.
        let html = r#"<span style="font-family: var(--font-body);">Typed text.</span><div><br></div>"#;
        let stripped = strip_editor_font_styles(html);
        assert!(!stripped.contains("style="), "chrome style must be removed: {stripped}");
        assert!(!html_has_formatting(&stripped), "chrome-only span reads as plain");
        // Other declarations in the same style attribute survive.
        let mixed = r#"<span style="font-family: var(--font-body); color: red;">x</span>"#;
        let stripped = strip_editor_font_styles(mixed);
        assert!(stripped.contains("color: red"), "real styling kept: {stripped}");
        assert!(html_has_formatting(&stripped));
        // Non-var font-family (user-chosen) is kept.
        let user_font = r#"<span style="font-family: Georgia;">x</span>"#;
        assert!(strip_editor_font_styles(user_font).contains("Georgia"));
    }

    #[test]
    fn plain_html_is_detected_and_formatted_html_kept() {
        // Semantically plain — structural tags only → paste text-only so the
        // target keeps its caret font (the TextEdit Times-New-Roman fix).
        assert!(!html_has_formatting("<p>be right back</p>"));
        assert!(!html_has_formatting("<p>line one</p><p>line two<br></p>"));
        assert!(!html_has_formatting("<div><span>plain chip output</span></div>"));
        // Real formatting → keep the HTML flavor.
        assert!(html_has_formatting("<p><strong>bold</strong></p>"));
        assert!(html_has_formatting("<p><em>italic</em> text</p>"));
        assert!(html_has_formatting(r#"<p><span style="color:red">red</span></p>"#));
        assert!(html_has_formatting("<ul><li>item</li></ul>"));
        assert!(html_has_formatting(r#"<p><a href="https://x.y">link</a></p>"#));
        // Unknown tags are conservatively kept.
        assert!(html_has_formatting("<p><custom-thing>x</custom-thing></p>"));
    }

    #[test]
    fn html_chip_resolution_inlines_tokens() {
        #[cfg(target_os = "macos")]
        let _pb = crate::actions::PASTEBOARD_TEST_LOCK.lock().unwrap();
        let gv = HashMap::new();
        let fv = HashMap::new();
        let html = r#"<p>x <span class="rte-token" data-token="{=2 + 3}">2+3</span> y</p>"#;
        let out = resolve_tokens_html(html, &gv, &fv);
        assert_eq!(out, "<p>x 5 y</p>");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn dual_write_snapshot_restore_roundtrip() {
        let _pb = crate::actions::PASTEBOARD_TEST_LOCK.lock().unwrap();
        let before = snapshot_clipboard();

        assert!(write_clipboard_dual("kf-dual-probe", Some("<b>kf-dual-probe</b>")));
        assert_eq!(read_clipboard().as_deref(), Some("kf-dual-probe"));
        // HTML flavor visible on the pasteboard
        let snap = snapshot_clipboard();
        assert!(snap.iter().any(|(t, _)| t == "public.html"));
        // Concealed marker present so clipboard managers skip our write
        assert!(snap.iter().any(|(t, _)| t == "org.nspasteboard.ConcealedType"));

        // Restore a synthetic snapshot and verify the flavor comes back.
        let fake = vec![(
            "public.utf8-plain-text".to_string(),
            b"kf-restored".to_vec(),
        )];
        assert!(restore_clipboard_snapshot(&fake));
        assert_eq!(read_clipboard().as_deref(), Some("kf-restored"));

        // Put the user's original clipboard back.
        restore_clipboard_snapshot(&before);
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE.store(false, Ordering::SeqCst);
    }
}
