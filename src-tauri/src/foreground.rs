use log::info;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowThreadProcessId,
};

const POLL_INTERVAL_MS: u64 = 1500;

// ── State ───────────────────────────────────────────────────────────────────

static WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_FG_HWND: AtomicIsize = AtomicIsize::new(0);

static FG_STATE: OnceLock<Mutex<FgState>> = OnceLock::new();

fn fg_state() -> &'static Mutex<FgState> {
    FG_STATE.get_or_init(|| Mutex::new(FgState::default()))
}

struct FgState {
    current_fg_proc: String,
    active_global_profile: String,
    profile_settings: HashMap<String, Value>,
    self_proc_names: Vec<String>,
}

impl Default for FgState {
    fn default() -> Self {
        // Build self-detection names
        let mut self_names = vec!["trigr".to_string()];
        if let Ok(exe) = std::env::current_exe() {
            if let Some(stem) = exe.file_stem() {
                self_names.push(stem.to_string_lossy().to_lowercase());
            }
        }

        Self {
            current_fg_proc: String::new(),
            active_global_profile: "Default".to_string(),
            profile_settings: HashMap::new(),
            self_proc_names: self_names,
        }
    }
}

// ── Win32 process name resolution ───────────────────────────────────────────

/// Resolve foreground HWND to process base name (lowercase, no .exe).
fn get_fg_proc_name(hwnd: isize) -> Option<String> {
    unsafe {
        // Step 1: Get PID from HWND
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd as _, &mut pid);
        if pid == 0 {
            return None;
        }

        // Step 2: Open process handle
        let h_proc: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h_proc.is_null() {
            return None;
        }

        // Step 3: Query full process image name (UTF-16)
        let mut buf = [0u16; 260]; // MAX_PATH
        let mut size: u32 = 260;
        let ok: BOOL =
            QueryFullProcessImageNameW(h_proc, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(h_proc);

        if ok == 0 || size == 0 {
            return None;
        }

        let full_path = String::from_utf16_lossy(&buf[..size as usize]);

        // Extract basename without .exe, lowercase
        let file_name = Path::new(&full_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())?;

        Some(file_name)
    }
}

// ── Foreground change handler ───────────────────────────────────────────────

fn handle_foreground_change(proc_name: &str, app: &AppHandle) {
    let name = proc_name
        .to_lowercase()
        .trim_end_matches(".exe")
        .to_string();

    let mut state = fg_state().lock().unwrap();
    state.current_fg_proc = name.clone();

    // Never auto-switch when Trigr itself is focused
    if state.self_proc_names.iter().any(|s| s == &name) {
        return;
    }

    // Find linked profiles
    let linked: Vec<(String, String)> = state
        .profile_settings
        .iter()
        .filter_map(|(profile, settings)| {
            settings
                .get("linkedApp")
                .and_then(|v| v.as_str())
                .map(|app_path| {
                    let app_name = Path::new(app_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    (profile.clone(), app_name)
                })
        })
        .collect();

    if linked.is_empty() {
        return;
    }

    // Match foreground process to linked app
    let matched = linked
        .iter()
        .find(|(_, app_name)| *app_name == name)
        .map(|(profile, _)| profile.clone());

    // Target: matched profile or fallback to global
    let target = matched
        .clone()
        .unwrap_or_else(|| state.active_global_profile.clone());

    // Get current active profile from hotkeys module
    let current_profile = crate::hotkeys::get_active_profile();

    if target != current_profile {
        info!(
            "[Trigr] Auto-switched to profile \"{}\" (foreground: {})",
            target, proc_name
        );

        // Release any held/repeating key before switching — a simulated mouse
        // button held in an app-linked profile must not persist into Default.
        crate::actions::release_held_key();
        crate::actions::stop_repeating_key();

        // Update hotkeys module with new profile
        crate::hotkeys::set_active_profile(target.clone());

        // Notify frontend
        drop(state); // release lock before emit
        let _ = app.emit(
            "profile-switched",
            serde_json::json!({ "profile": target }),
        );
    }
}

// ── Watcher lifecycle ───────────────────────────────────────────────────────

pub fn start_watcher(app: AppHandle) {
    if WATCHER_RUNNING.load(Ordering::Relaxed) {
        return;
    }
    WATCHER_RUNNING.store(true, Ordering::Relaxed);

    thread::Builder::new()
        .name("trigr-fg-watcher".to_string())
        .spawn(move || {
            info!("[Trigr] Foreground watcher started ({}ms poll)", POLL_INTERVAL_MS);

            while WATCHER_RUNNING.load(Ordering::Relaxed) {
                unsafe {
                    let hwnd = GetForegroundWindow();
                    let hwnd_val = hwnd as isize;

                    // Skip if unchanged from last poll
                    if hwnd_val != 0
                        && hwnd_val != LAST_FG_HWND.load(Ordering::Relaxed)
                    {
                        LAST_FG_HWND.store(hwnd_val, Ordering::Relaxed);
                        if let Some(name) = get_fg_proc_name(hwnd_val) {
                            handle_foreground_change(&name, &app);
                        }
                    }
                }

                thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
            }

            info!("[Trigr] Foreground watcher stopped");
        })
        .expect("Failed to spawn foreground watcher thread");
}

pub fn stop_watcher() {
    WATCHER_RUNNING.store(false, Ordering::Relaxed);
    LAST_FG_HWND.store(0, Ordering::Relaxed);
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn get_current_fg_proc() -> String {
    fg_state().lock().unwrap().current_fg_proc.clone()
}

/// The HWND the foreground watcher last confirmed as foreground.
/// Used by the hook to verify the linked app is still focused before
/// suppressing bare mouse buttons (avoids the 1500ms poll lag).
pub fn last_fg_hwnd() -> isize {
    LAST_FG_HWND.load(Ordering::Relaxed)
}

pub fn set_active_global_profile(profile: String) {
    let mut state = fg_state().lock().unwrap();
    state.active_global_profile = profile;
}

pub fn update_profile_settings(settings: HashMap<String, Value>) {
    let mut state = fg_state().lock().unwrap();
    state.profile_settings = settings;
}
