use log::info;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, OpenClipboard, SetClipboardData, EmptyClipboard,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
};

const MAX_BUFFER_LENGTH: usize = 50;
const VK_BACKSPACE: u16 = 0x08;
const VK_SPACE: u16 = 0x20;
const VK_LEFT: u16 = 0x25;
const VK_LSHIFT: u16 = 0xA0;
const VK_INSERT: u16 = 0x2D;
const CF_UNICODETEXT: u32 = 13;

// ── Injection guard — ensures INJECTION_IN_PROGRESS is always cleared ──────

struct InjectionGuard;

impl InjectionGuard {
    fn new() -> Self {
        crate::hotkeys::INJECTION_IN_PROGRESS
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Self
    }
}

impl Drop for InjectionGuard {
    fn drop(&mut self) {
        crate::hotkeys::INJECTION_IN_PROGRESS
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

// ── State ───────────────────────────────────────────────────────────────────

static EXPANSION_STATE: OnceLock<Mutex<ExpansionState>> = OnceLock::new();

fn state() -> &'static Mutex<ExpansionState> {
    EXPANSION_STATE.get_or_init(|| Mutex::new(ExpansionState::default()))
}

struct ExpansionState {
    buffer: String,
    assignments: HashMap<String, Value>,
    autocorrect_enabled: bool,
    global_variables: HashMap<String, String>,
}

impl Default for ExpansionState {
    fn default() -> Self {
        Self {
            buffer: String::new(),
            assignments: HashMap::new(),
            autocorrect_enabled: true,
            global_variables: HashMap::new(),
        }
    }
}

// ── Buffer management (called from hotkeys.rs) ─────────────────────────────

/// Append a character to the buffer. Called for bare (unmodified) key presses.
pub fn buffer_push(ch: char) {
    let mut s = state().lock().unwrap();
    s.buffer.push(ch);
    if s.buffer.len() > MAX_BUFFER_LENGTH {
        let start = s.buffer.len() - MAX_BUFFER_LENGTH;
        s.buffer = s.buffer[start..].to_string();
    }
}

/// Remove the last character (Backspace).
pub fn buffer_pop() {
    let mut s = state().lock().unwrap();
    s.buffer.pop();
}

/// Clear the buffer entirely.
pub fn buffer_clear() {
    state().lock().unwrap().buffer.clear();
}

// ── Trigger detection ───────────────────────────────────────────────────────

/// Called when Space is pressed. Returns true if an expansion/autocorrect fired.
pub fn check_space_trigger() -> bool {
    let mut s = state().lock().unwrap();
    if s.buffer.is_empty() {
        return false;
    }

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
            return false;
        }

        let text = entry
            .get("data")
            .and_then(|d| d.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let trigger_len = s.buffer.len();
        let global_vars = s.global_variables.clone();
        s.buffer.clear();
        drop(s);

        info!("[Trigr] Expansion: \"{}\" → \"{}\"", buffer_lower, text);
        fire_expansion(&buffer_lower, trigger_len, true, &text, &global_vars);
        return true;
    }

    s.buffer.clear();
    false
}

/// Called after each character is added to the buffer. Checks for immediate-mode triggers.
/// Returns true if an immediate expansion fired.
pub fn check_immediate_triggers() -> bool {
    let mut s = state().lock().unwrap();
    if s.buffer.is_empty() {
        return false;
    }

    let buf_lower = s.buffer.to_lowercase();

    // Collect immediate triggers sorted by length (longest first)
    let mut immediate_triggers: Vec<(String, String)> = s
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
            let trigger = k["GLOBAL::EXPANSION::".len()..].to_string();
            let text = v
                .get("data")
                .and_then(|d| d.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            (trigger, text)
        })
        .collect();
    immediate_triggers.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    for (trigger, text) in &immediate_triggers {
        if buf_lower.ends_with(trigger) {
            let trigger_len = trigger.len();
            let global_vars = s.global_variables.clone();
            s.buffer.clear();
            drop(s);

            info!("[Trigr] Expansion (immediate): \"{}\" → \"{}\"", trigger, text);
            // deleteExtra=false for immediate (no trailing space to delete)
            fire_expansion(trigger, trigger_len, false, text, &global_vars);
            return true;
        }
    }

    false
}

// ── Fire expansion ──────────────────────────────────────────────────────────

fn fire_expansion(
    trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    text: &str,
    global_vars: &HashMap<String, String>,
) {
    // Resolve tokens in the replacement text
    let (resolved, cursor_back) = resolve_tokens(text, global_vars);

    println!(
        "[EXP] fire_expansion trigger=\"{}\" len={} delete_extra={} resolved=\"{}\" ({} chars) cursor_back={}",
        trigger, trigger_len, delete_extra,
        &resolved[..resolved.len().min(80)], resolved.len(), cursor_back
    );

    if resolved.is_empty() {
        println!("[EXP] Resolved text is empty — skipping injection");
        return;
    }

    // Capture target HWND NOW before spawning the thread
    let target_hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };
    println!("[EXP] Captured target HWND: 0x{:X}", target_hwnd);

    // Wait for any prior injection to finish (handles sequential autocorrects)
    while crate::hotkeys::INJECTION_IN_PROGRESS.load(std::sync::atomic::Ordering::Relaxed) {
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
            .store(true, std::sync::atomic::Ordering::Relaxed);

        // Delete trigger word + space (if applicable)
        let delete_count = trigger_len + if delete_extra { 1 } else { 0 };
        println!("[EXP] Deleting {} chars (trigger {} + extra {})", delete_count, trigger_len, delete_extra as u8);
        for _ in 0..delete_count {
            send_vk_tap(VK_BACKSPACE);
            thread::sleep(Duration::from_millis(5));
        }

        thread::sleep(Duration::from_millis(10));

        // Inject replacement via clipboard
        println!("[EXP] Calling inject_via_clipboard with {} chars, HWND=0x{:X}", resolved.len(), target_hwnd);
        inject_via_clipboard(&resolved, target_hwnd);

        // Move cursor back if {cursor} was present
        if cursor_back > 0 {
            thread::sleep(Duration::from_millis(10));
            for _ in 0..cursor_back {
                send_vk_tap(VK_LEFT);
                thread::sleep(Duration::from_millis(5));
            }
        }

        crate::hotkeys::SUPPRESS_SIMULATED
            .store(false, std::sync::atomic::Ordering::Relaxed);
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(false, std::sync::atomic::Ordering::Relaxed);

        // Replay any keystrokes that were buffered during injection
        let buffered: Vec<crate::hotkeys::BufferedKey> =
            crate::hotkeys::injection_buffer().lock().unwrap().drain(..).collect();
        if !buffered.is_empty() {
            println!("[EXP] Replaying {} buffered keystrokes", buffered.len());
            crate::hotkeys::SUPPRESS_SIMULATED
                .store(true, std::sync::atomic::Ordering::Relaxed);
            for key in &buffered {
                send_vk_key(key.vk_code as u16, !key.is_keydown);
                thread::sleep(Duration::from_millis(2));
            }
            crate::hotkeys::SUPPRESS_SIMULATED
                .store(false, std::sync::atomic::Ordering::Relaxed);

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
                if let Some(ch) = crate::hotkeys::vk_to_char(key.vk_code) {
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

// ── Token resolution ────────────────────────────────────────────────────────

pub fn resolve_tokens(text: &str, global_vars: &HashMap<String, String>) -> (String, usize) {
    let mut result = text.to_string();

    // Substitute {{varName}} global variables
    if result.contains("{{") {
        for (name, value) in global_vars {
            let token = format!("{{{{{}}}}}", name); // {{name}}
            result = result.replace(&token, value);
        }
    }

    // {clipboard} — read current clipboard
    if result.contains("{clipboard}") {
        let clip = read_clipboard().unwrap_or_default();
        result = result.replace("{clipboard}", &clip);
    }

    // {date:...} and {time:...} tokens
    let now = chrono::Local::now();
    result = result.replace("{date:DD/MM/YYYY}", &now.format("%d/%m/%Y").to_string());
    result = result.replace("{date:MM/DD/YYYY}", &now.format("%m/%d/%Y").to_string());
    result = result.replace("{date:YYYY-MM-DD}", &now.format("%Y-%m-%d").to_string());
    result = result.replace("{time:HH:MM:SS}", &now.format("%H:%M:%S").to_string());
    result = result.replace("{time:HH:MM}", &now.format("%H:%M").to_string());
    result = result.replace("{dayofweek}", &now.format("%A").to_string());

    // {cursor} — track position, then remove token
    let mut cursor_back = 0;
    if let Some(idx) = result.find("{cursor}") {
        cursor_back = result.len() - idx - "{cursor}".len();
        result = result.replace("{cursor}", "");
    }

    (result, cursor_back)
}

// ── Clipboard operations (Win32) ────────────────────────────────────────────

fn read_clipboard() -> Option<String> {
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
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
        Some(text)
    }
}

fn write_clipboard(text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let byte_len = wide.len() * 2;
    // Set suppress BEFORE touching clipboard so any listener skips this write
    crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
        .store(true, std::sync::atomic::Ordering::Relaxed);
    unsafe {
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            return false;
        }
        EmptyClipboard();

        let h_mem = GlobalAlloc(GMEM_MOVEABLE, byte_len);
        if h_mem.is_null() {
            CloseClipboard();
            return false;
        }
        let ptr = GlobalLock(h_mem) as *mut u16;
        if ptr.is_null() {
            CloseClipboard();
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        GlobalUnlock(h_mem);

        SetClipboardData(CF_UNICODETEXT, h_mem);
        CloseClipboard();
        true
    }
}

/// Inject text via clipboard paste, restoring clipboard afterwards.
fn inject_via_clipboard(text: &str, target_hwnd: isize) {
    // Save current clipboard
    let prev = read_clipboard().unwrap_or_default();

    // Write replacement to clipboard (suppress set inside write_clipboard)
    let write_ok = write_clipboard(text);
    println!("[EXP] write_clipboard({} chars) → {}", text.chars().count(), write_ok);

    // Release physically held modifiers
    let held = crate::actions::release_held_modifiers();

    // Restore focus to target window
    if target_hwnd != 0 {
        unsafe {
            let ok = windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(
                target_hwnd as _,
            );
            println!("[EXP] SetForegroundWindow(0x{:X}) → {}", target_hwnd, ok);
        }
        thread::sleep(Duration::from_millis(10));
    }

    // Check if Ctrl+V is mapped as a hotkey — if so, use Shift+Insert
    let use_ctrl_v = !is_ctrl_v_mapped();
    if use_ctrl_v {
        println!("[EXP] Pasting with Ctrl+V");
        send_vk_key(0xA2, false); // LCtrl
        send_vk_key(0x56, false); // V
        send_vk_key(0x56, true);
        send_vk_key(0xA2, true);
    } else {
        println!("[EXP] Pasting with Shift+Insert (Ctrl+V is mapped)");
        send_vk_key(VK_LSHIFT, false);
        send_vk_key(VK_INSERT, false);
        send_vk_key(VK_INSERT, true);
        send_vk_key(VK_LSHIFT, true);
    }

    // Send trailing space as a synthetic keystroke (not via clipboard — some apps strip trailing whitespace from paste)
    send_vk_tap(VK_SPACE);

    // Re-press modifiers that were physically held
    crate::actions::restore_modifiers(&held);

    // Restore clipboard after paste settles
    thread::sleep(Duration::from_millis(50));
    write_clipboard(&prev);
    crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
        .store(false, std::sync::atomic::Ordering::Relaxed);
    println!("[EXP] clipboard restored");
}

fn is_ctrl_v_mapped() -> bool {
    let state = crate::hotkeys::engine_state().lock().unwrap();
    let profile = &state.active_profile;
    let key = format!("{}::Ctrl::KeyV", profile);
    state.assignments.contains_key(&key)
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
    info!(
        "[Trigr] Expansion assignments updated: {} entries",
        s.assignments.len()
    );
}

pub fn set_autocorrect_enabled(enabled: bool) {
    state().lock().unwrap().autocorrect_enabled = enabled;
    info!("[Trigr] Autocorrect enabled: {}", enabled);
}

pub fn update_global_variables(vars: HashMap<String, String>) {
    state().lock().unwrap().global_variables = vars;
}
