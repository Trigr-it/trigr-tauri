use log::info;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Manager};

use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, OpenClipboard, SetClipboardData, EmptyClipboard,
    RegisterClipboardFormatW,
};
use windows_sys::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
};

const MAX_BUFFER_LENGTH: usize = 50;
const VK_BACKSPACE: u16 = 0x08;
const VK_SPACE: u16 = 0x20;
const VK_LEFT: u16 = 0x25;
const VK_LSHIFT: u16 = 0xA0;
const VK_INSERT: u16 = 0x2D;
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;

// ── Injection guard — ensures INJECTION_IN_PROGRESS is always cleared ──────

struct InjectionGuard;

impl InjectionGuard {
    fn new() -> Self {
        crate::hotkeys::INJECTION_IN_PROGRESS
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Self
    }
}

impl Drop for InjectionGuard {
    fn drop(&mut self) {
        crate::hotkeys::INJECTION_IN_PROGRESS
            .store(false, std::sync::atomic::Ordering::SeqCst);
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

        let expansion_type = entry
            .get("data")
            .and_then(|d| d.get("expansionType"))
            .and_then(|v| v.as_str())
            .unwrap_or("text");

        if expansion_type == "image" {
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
            fire_image_expansion(&buffer_lower, trigger_len, true, &image_path, image_scale);
            return true;
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
    // Each entry: (trigger, expansion_type, text, image_path, image_scale)
    let mut immediate_triggers: Vec<(String, String, String, String, u32)> = s
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
            let exp_type = v.get("data")
                .and_then(|d| d.get("expansionType"))
                .and_then(|v| v.as_str())
                .unwrap_or("text")
                .to_string();
            let text = v
                .get("data")
                .and_then(|d| d.get("text"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let image_path = v.get("data")
                .and_then(|d| d.get("imagePath"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let image_scale = v.get("data")
                .and_then(|d| d.get("imageScale"))
                .and_then(|v| v.as_u64())
                .unwrap_or(100) as u32;
            (trigger, exp_type, text, image_path, image_scale)
        })
        .collect();
    immediate_triggers.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    for (trigger, exp_type, text, image_path, image_scale) in &immediate_triggers {
        if buf_lower.ends_with(trigger) {
            let trigger_len = trigger.len();

            if exp_type == "image" {
                let image_path = image_path.clone();
                let image_scale = *image_scale;
                s.buffer.clear();
                drop(s);

                info!("[Trigr] Image expansion (immediate): \"{}\" → \"{}\"", trigger, image_path);
                fire_image_expansion(trigger, trigger_len, false, &image_path, image_scale);
                return true;
            }

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
    _trigger: &str,
    trigger_len: usize,
    delete_extra: bool,
    text: &str,
    global_vars: &HashMap<String, String>,
) {
    // Check for {fillIn:...} tokens — if present, spawn a dedicated thread for the
    // entire fill-in + injection flow so the processor thread is never blocked.
    let fill_in_fields = extract_fill_in_fields(text);
    if !fill_in_fields.is_empty() {
        // Prevent concurrent fill-in invocations
        if crate::hotkeys::FILL_IN_ACTIVE.load(std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        let text = text.to_string();
        let global_vars = global_vars.clone();
        let trigger_len = trigger_len;
        let trigger_str = _trigger.to_string();
        thread::spawn(move || {
            fire_expansion_with_fillin(fill_in_fields, &text, trigger_len, delete_extra, &global_vars, &trigger_str);
        });
        return;
    }

    // No fill-in tokens — resolve and inject directly
    let (resolved, cursor_back) = resolve_tokens(text, global_vars);

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

        let used_clipboard = should_use_clipboard(&resolved);
        if used_clipboard {
            inject_via_clipboard(&resolved, target_hwnd);
        } else {
            inject_via_sendinput(&resolved, target_hwnd);
        }

        // Move cursor back if {cursor} was present
        if cursor_back > 0 {
            thread::sleep(Duration::from_millis(10));
            for _ in 0..cursor_back {
                send_vk_tap(VK_LEFT);
                thread::sleep(Duration::from_millis(5));
            }
        }

        crate::hotkeys::SUPPRESS_SIMULATED
            .store(false, std::sync::atomic::Ordering::SeqCst);
        if used_clipboard {
            crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
                .store(false, std::sync::atomic::Ordering::SeqCst);
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

/// Fill-in flow: runs entirely on a dedicated thread so the processor thread is never blocked.
/// Sequence: show window → wait for response → resolve tokens → inject.
fn fire_expansion_with_fillin(
    fill_in_fields: Vec<String>,
    text: &str,
    trigger_len: usize,
    delete_extra: bool,
    global_vars: &HashMap<String, String>,
    trigger_str: &str,
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

    let used_clipboard = should_use_clipboard(&resolved);
    if used_clipboard {
        inject_via_clipboard(&resolved, target_hwnd);
    } else {
        inject_via_sendinput(&resolved, target_hwnd);
    }

    // Move cursor back if {cursor} was present
    if cursor_back > 0 {
        thread::sleep(Duration::from_millis(10));
        for _ in 0..cursor_back {
            send_vk_tap(VK_LEFT);
            thread::sleep(Duration::from_millis(5));
        }
    }

    crate::hotkeys::SUPPRESS_SIMULATED
        .store(false, std::sync::atomic::Ordering::SeqCst);
    if used_clipboard {
        crate::actions::SUPPRESS_NEXT_CLIPBOARD_WRITE
            .store(false, std::sync::atomic::Ordering::SeqCst);
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

    crate::hotkeys::sync_modifier_state_from_os();
    // _guard drops here → INJECTION_IN_PROGRESS = false
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
    result = result.replace("{date:DD/MM/YY}", &now.format("%d/%m/%y").to_string());
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
        .store(true, std::sync::atomic::Ordering::SeqCst);
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
fn inject_via_clipboard(text: &str, target_hwnd: isize) {
    // Save current clipboard
    let prev = read_clipboard().unwrap_or_default();

    // Write replacement to clipboard (suppress set inside write_clipboard)
    write_clipboard(text);

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

    // Check if Ctrl+V is mapped as a hotkey — if so, use Shift+Insert
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

    // Send trailing space as a synthetic keystroke (not via clipboard — some apps strip trailing whitespace from paste)
    send_vk_tap(VK_SPACE);

    // Re-press modifiers that were physically held
    crate::actions::restore_modifiers(&held);

    // Restore clipboard after paste settles — skip if previous was empty to avoid blank clipboard entry
    thread::sleep(Duration::from_millis(50));
    if !prev.is_empty() {
        write_clipboard(&prev);
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

        CloseClipboard();
        true
    }
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
    info!("[Trigr] Autocorrect config: {} (engine disabled for Alpha)", enabled);
}

pub fn update_global_variables(vars: HashMap<String, String>) {
    state().lock().unwrap().global_variables = vars;
}

pub fn get_global_variables() -> HashMap<String, String> {
    state().lock().unwrap().global_variables.clone()
}
