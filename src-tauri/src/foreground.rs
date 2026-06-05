use log::info;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock, RwLock};
use std::thread;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HANDLE, RECT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowTextW,
    GetWindowThreadProcessId, PostThreadMessageW, GWL_STYLE, WS_CAPTION,
};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};

const POLL_INTERVAL_MS: u64 = 1500;

// ── State ───────────────────────────────────────────────────────────────────

static WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);
static LAST_FG_HWND: AtomicIsize = AtomicIsize::new(0);
static LAST_FG_TITLE: OnceLock<Mutex<String>> = OnceLock::new();

/// True while the user has an editor surface open (mapping right-panel, expansion
/// add/edit form, quick action edit form, or radial segment edit via MacroPanel).
/// When set, the foreground watcher does not auto-switch profiles, so the user
/// can test their work against another app without the profile snapping away.
/// Pushed from the frontend via `set_editing_active`.
static EDITING_ACTIVE: AtomicBool = AtomicBool::new(false);

fn last_fg_title() -> &'static Mutex<String> {
    LAST_FG_TITLE.get_or_init(|| Mutex::new(String::new()))
}

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

/// Retrieve the window title text for a given HWND (lowercase).
fn get_window_title(hwnd: isize) -> String {
    unsafe {
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd as _, buf.as_mut_ptr(), 512);
        if len <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..len as usize]).to_lowercase()
    }
}

// ── Foreground change handler ───────────────────────────────────────────────

fn handle_foreground_change(proc_name: &str, window_title: &str, app: &AppHandle) {
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

    // Suppress auto-switching while the user is actively editing in any action
    // editor (mapping right-panel, expansion form, quick action form, radial
    // segment via MacroPanel). They may be testing their work in another app
    // and we don't want the profile to snap away mid-build. When no editor is
    // open, Trigr behaves the same whether the main window is visible (parked
    // on a side monitor) or hidden — auto-switching runs normally.
    if EDITING_ACTIVE.load(Ordering::SeqCst) {
        return;
    }

    // Find linked profiles — tuple: (profile_name, app_name, optional_title_filter)
    let linked: Vec<(String, String, Option<String>)> = state
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
                    let title_filter = settings
                        .get("linkedWindowTitle")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_lowercase());
                    (profile.clone(), app_name, title_filter)
                })
        })
        .collect();

    // Note: do NOT early-return when `linked` is empty. Free users (no Pro
    // app-linking) should still snap back to active_global_profile when they
    // unfocus Trigr after manually clicking a different profile in the sidebar.
    // Otherwise manual sidebar selection becomes a free workaround for the
    // Pro auto-switch feature.

    // Match foreground process to linked app (+ optional title filter)
    // Note: window_title is already lowercase from get_window_title()
    let matched = linked
        .iter()
        .find(|(_, app_name, title_filter)| {
            *app_name == name
                && title_filter
                    .as_ref()
                    .map_or(true, |filter| window_title.contains(filter.as_str()))
        })
        .map(|(profile, _, _)| profile.clone());

    // Target selection:
    //   - Pro users: matched linked profile, or fallback to active_global_profile.
    //   - Free users: always fallback to active_global_profile (no linked-app
    //     activation — preserves Pro gating even if linked apps were configured
    //     during a lapsed Pro trial). Snap-back to fallback still happens so
    //     manually-clicked profiles don't stick when the user unfocuses Trigr.
    let target = if crate::licence::is_pro() {
        matched
            .clone()
            .unwrap_or_else(|| state.active_global_profile.clone())
    } else {
        state.active_global_profile.clone()
    };

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

/// True iff the given HWND is fullscreen / borderless-fullscreen / chrome-less.
///
/// Distinguishes:
///   exclusive-fullscreen DirectX game  (no WS_CAPTION, rect == monitor) → TRUE
///   borderless-windowed game           (no WS_CAPTION, rect == monitor) → TRUE
///   F11 browser / video fullscreen     (no WS_CAPTION, rect == monitor) → TRUE
///   maximised normal window            (has WS_CAPTION)                  → FALSE
///   any normal windowed app            (rect != monitor)                 → FALSE
///
/// Used by the foreground watcher to pause the LL mouse hook while a game
/// has focus — games using SetCursorPos-recentering for camera rotation
/// (e.g. World of Warcraft) misbehave with the per-event latency our hook
/// adds. Read-only: this function does not modify any system state.
fn is_window_fullscreen(hwnd: isize) -> bool {
    if hwnd == 0 {
        return false;
    }
    unsafe {
        // 1) No-chrome check: any window with WS_CAPTION (title bar) is a
        //    normal app, even when maximised. Fast bail-out.
        let style = GetWindowLongW(hwnd as _, GWL_STYLE) as u32;
        if (style & WS_CAPTION) != 0 {
            return false;
        }

        // 2) Window rect vs monitor rect. We use rcMonitor (the full monitor
        //    bounds) not rcWork (which excludes the taskbar) — borderless
        //    games cover the taskbar too.
        let mut wr: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd as _, &mut wr) == 0 {
            return false;
        }
        let hmon = MonitorFromWindow(hwnd as _, MONITOR_DEFAULTTONEAREST);
        if hmon.is_null() {
            return false;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) == 0 {
            return false;
        }
        let mr = mi.rcMonitor;

        // 3) Equality with ~2px tolerance — some games' window rects are
        //    off-by-one on the right/bottom edges.
        const TOL: i32 = 2;
        (wr.left - mr.left).abs() <= TOL
            && (wr.top - mr.top).abs() <= TOL
            && (wr.right - mr.right).abs() <= TOL
            && (wr.bottom - mr.bottom).abs() <= TOL
    }
}

/// Signal the hook thread to pause or resume the LL mouse hook. Best-effort:
/// if HOOK_THREAD_ID has been cleared (mid-teardown), PostThreadMessageW(0,..)
/// is a silent no-op.
fn set_mouse_hook_paused(paused: bool) {
    // Set the atomic BEFORE posting so the watchdog observes the paused state
    // before any heartbeat tick that might otherwise trigger a reinstall.
    crate::hotkeys::MOUSE_HOOK_PAUSED.store(paused, Ordering::SeqCst);
    let tid = crate::hotkeys::hook_thread_id();
    if tid == 0 {
        return;
    }
    let msg = if paused {
        crate::hotkeys::WM_TRIGR_MOUSE_HOOK_PAUSE
    } else {
        crate::hotkeys::WM_TRIGR_MOUSE_HOOK_RESUME
    };
    unsafe {
        PostThreadMessageW(tid as u32, msg, 0, 0);
    }
}

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
            // Local transition tracker for fullscreen-detected mouse-hook pause.
            // Starts false (normal startup state); only flips on observed change.
            let mut fs_last: bool = false;

            while WATCHER_RUNNING.load(Ordering::Relaxed) {
                unsafe {
                    let hwnd = GetForegroundWindow();
                    let hwnd_val = hwnd as isize;

                    if hwnd_val != 0 {
                        let hwnd_changed = hwnd_val != LAST_FG_HWND.load(Ordering::Relaxed);
                        let title = get_window_title(hwnd_val);
                        let title_changed = {
                            let last = last_fg_title().lock().unwrap();
                            *last != title
                        };

                        if hwnd_changed || title_changed {
                            LAST_FG_HWND.store(hwnd_val, Ordering::Relaxed);
                            *last_fg_title().lock().unwrap() = title.clone();
                            if let Some(name) = get_fg_proc_name(hwnd_val) {
                                // Cache PID for linked-app detection from the hook
                                cache_linked_pid_if_match(hwnd_val, &name);
                                handle_foreground_change(&name, &title, &app);
                            }
                        }

                        // Fullscreen-detect mouse hook pause/resume. Checked
                        // every poll (not gated on hwnd_changed) so a window
                        // toggling fullscreen in-place — e.g. browser F11 —
                        // is still caught.
                        let fs_now = is_window_fullscreen(hwnd_val);
                        if fs_now != fs_last {
                            set_mouse_hook_paused(fs_now);
                            fs_last = fs_now;
                        }
                    } else if fs_last {
                        // No foreground window (rare — e.g. desktop has focus).
                        // Treat as not-fullscreen so the hook resumes.
                        set_mouse_hook_paused(false);
                        fs_last = false;
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
    *last_fg_title().lock().unwrap() = String::new();
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

/// Live lookup: resolve a window HWND to its lowercase process basename
/// (no `.exe` suffix). Used by expansion injection to pick the right paste
/// shortcut per target app — Electron+xterm.js terminals need Shift+Insert
/// because bash readline intercepts raw Ctrl+V as `quoted-insert`.
pub fn proc_name_for_hwnd(hwnd: isize) -> Option<String> {
    get_fg_proc_name(hwnd)
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
    let title = get_window_title(hwnd);
    *last_fg_title().lock().unwrap() = title.clone();
    if let Some(name) = unsafe { get_fg_proc_name(hwnd) } {
        cache_linked_pid_if_match(hwnd, &name);
        handle_foreground_change(&name, &title, app);
    }
}

pub fn set_active_global_profile(profile: String) {
    let mut state = fg_state().lock().unwrap();
    state.active_global_profile = profile;
}

/// Toggle the editing-active gate. While true, the foreground watcher suppresses
/// auto-switching so the user can test profile assignments against another app
/// without snapping away. Frontend pushes this from App.jsx when any action
/// editor opens or closes.
pub fn set_editing_active(active: bool) {
    EDITING_ACTIVE.store(active, Ordering::SeqCst);
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
