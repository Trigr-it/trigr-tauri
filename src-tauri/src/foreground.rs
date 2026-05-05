use log::info;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
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

/// Cache of linked-app PIDs → profile name.  Populated when the foreground
/// watcher detects a linked app.  The hook reads this (via try_read) to check
/// if the cursor is over a linked app that isn't currently the foreground —
/// fixing the "click to refocus" missed-remap issue.
static LINKED_APP_PIDS: OnceLock<RwLock<HashMap<u32, String>>> = OnceLock::new();

fn linked_app_pids() -> &'static RwLock<HashMap<u32, String>> {
    LINKED_APP_PIDS.get_or_init(|| RwLock::new(HashMap::new()))
}

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

    // Suppress auto-switching while Trigr's main window is visible and not minimized —
    // user may be editing a profile and clicking between apps to test.
    // Only resume auto-switching when the window is hidden to tray or minimized.
    if let Some(win) = app.get_webview_window("main") {
        let visible = win.is_visible().unwrap_or(false);
        let minimized = win.is_minimized().unwrap_or(false);
        if visible && !minimized {
            return;
        }
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

            let mut prune_counter: u32 = 0;

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
                            // Cache PID for linked-app detection from the hook
                            cache_linked_pid_if_match(hwnd_val, &name);
                            handle_foreground_change(&name, &app);
                        }
                    }
                }

                // Prune stale PID cache entries every ~20 polls (~30s).
                // Validates each cached PID still maps to the expected linked app.
                prune_counter += 1;
                if prune_counter >= 20 {
                    prune_counter = 0;
                    prune_stale_pids();
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

/// Resolve a PID to process base name (lowercase, no .exe).
/// Returns None if the process no longer exists or access is denied.
fn get_proc_name_by_pid(pid: u32) -> Option<String> {
    unsafe {
        let h_proc: HANDLE = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h_proc.is_null() { return None; }
        let mut buf = [0u16; 260];
        let mut size: u32 = 260;
        let ok: BOOL = QueryFullProcessImageNameW(h_proc, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(h_proc);
        if ok == 0 || size == 0 { return None; }
        let full_path = String::from_utf16_lossy(&buf[..size as usize]);
        Path::new(&full_path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
    }
}

// ── Linked-app PID cache ───────────────────────────────────────────────────

/// If the foreground process name matches a linked app, cache its PID so the
/// mouse hook can detect "cursor over linked app" even when the app isn't the
/// current foreground (click-to-refocus scenario).
fn cache_linked_pid_if_match(hwnd_val: isize, proc_name: &str) {
    let name = proc_name.to_lowercase();
    let state = fg_state().lock().unwrap();
    let matched_profile = state
        .profile_settings
        .iter()
        .find_map(|(profile, settings)| {
            settings
                .get("linkedApp")
                .and_then(|v| v.as_str())
                .and_then(|app_path| {
                    let app_name = Path::new(app_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    if app_name == name { Some(profile.clone()) } else { None }
                })
        });
    drop(state);

    if let Some(profile) = matched_profile {
        unsafe {
            let mut pid: u32 = 0;
            GetWindowThreadProcessId(hwnd_val as _, &mut pid);
            if pid != 0 {
                if let Ok(mut cache) = linked_app_pids().write() {
                    cache.insert(pid, profile);
                }
            }
        }
    }
}

/// Evict PID cache entries whose process has exited or been replaced by a
/// different executable (PID reuse).  Called periodically from the watcher loop.
fn prune_stale_pids() {
    let state = fg_state().lock().unwrap();
    let settings = &state.profile_settings;
    // Build a map: profile → expected lowercase process name
    let expected: HashMap<String, String> = settings
        .iter()
        .filter_map(|(profile, s)| {
            s.get("linkedApp")
                .and_then(|v| v.as_str())
                .and_then(|app_path| {
                    Path::new(app_path)
                        .file_stem()
                        .map(|s| (profile.clone(), s.to_string_lossy().to_lowercase()))
                })
        })
        .collect();
    drop(state);

    if let Ok(mut cache) = linked_app_pids().write() {
        cache.retain(|&pid, profile| {
            let Some(expected_name) = expected.get(profile) else { return false; };
            match get_proc_name_by_pid(pid) {
                Some(name) => &name == expected_name,
                None => false, // process exited
            }
        });
    }
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

/// Check if a PID belongs to a known linked app.  Returns the profile name.
/// Called from the mouse hook — must be non-blocking (try_read).
pub fn linked_profile_for_pid(pid: u32) -> Option<String> {
    linked_app_pids()
        .try_read()
        .ok()
        .and_then(|cache| cache.get(&pid).cloned())
}

/// Force an immediate foreground check and profile switch if needed.
/// Called before showing the radial menu to avoid the 1500ms poll lag.
pub fn force_check(app: &AppHandle) {
    let hwnd = unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow() as isize
    };
    if hwnd == 0 { return; }
    LAST_FG_HWND.store(hwnd, Ordering::Relaxed);
    if let Some(name) = unsafe { get_fg_proc_name(hwnd) } {
        cache_linked_pid_if_match(hwnd, &name);
        handle_foreground_change(&name, app);
    }
}

pub fn set_active_global_profile(profile: String) {
    let mut state = fg_state().lock().unwrap();
    state.active_global_profile = profile;
}

pub fn update_profile_settings(settings: HashMap<String, Value>) {
    let mut state = fg_state().lock().unwrap();
    state.profile_settings = settings;
    // Prune PID cache: remove entries whose profile is no longer linked
    if let Ok(mut cache) = linked_app_pids().write() {
        cache.retain(|_, profile| {
            state.profile_settings
                .get(profile)
                .and_then(|s| s.get("linkedApp"))
                .and_then(|v| v.as_str())
                .is_some()
        });
    }
}
