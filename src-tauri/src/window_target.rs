//! Per-action target-monitor support for Open App / Open Folder actions.
//!
//! Resolves `data.monitor` (default / primary / cursor / foreground / "<device-name>")
//! into a work-area rect, pre-positions the cursor to that monitor's centre,
//! launches the app/folder, and then asynchronously finds the new top-level
//! window and centres it on the work area via `SetWindowPos`.
//!
//! Path-app launches go through `ShellExecuteExW` with `SEE_MASK_NOCLOSEPROCESS`
//! so we can pull the PID. With a PID we install `SetWinEventHook(EVENT_OBJECT_SHOW)`
//! scoped to that PID — the new window event fires within ~50-200ms typically.
//! AUMID launches and folder (Explorer) launches are shell-brokered and don't
//! yield a usable PID, so they fall back to snapshot-diff polling of
//! `EnumWindows` at 50ms cadence (3s budget).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::mem;
use std::ptr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use log::{info, warn};
use serde::Serialize;
use serde_json::Value;

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, POINT, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, RedrawWindow,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
    RDW_ALLCHILDREN, RDW_INVALIDATE, RDW_UPDATENOW,
};

// windows-sys 0.59 omits this constant from Win32::Graphics::Gdi; it's a stable
// Win32 SDK value so we inline it. dwFlags & this bit == primary monitor.
const MONITORINFOF_PRIMARY: u32 = 0x00000001;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, GetProcessId, OpenProcess, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteExW, ShellExecuteW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetClassNameW, GetCursorPos, GetParent, GetWindowLongW,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    PeekMessageW, SendMessageW, SetCursorPos, SetWindowPos, ShowWindow, TranslateMessage,
    EVENT_OBJECT_SHOW, GWL_EXSTYLE, MSG, OBJID_WINDOW, PM_REMOVE, SWP_NOACTIVATE, SWP_NOSIZE,
    SWP_NOZORDER, SW_RESTORE, SW_SHOW, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
    WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WS_EX_TOOLWINDOW,
};

// ── Public types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum MonitorTarget {
    None,
    Primary,
    Cursor,
    Foreground(isize),
    Named(String),
}

pub enum LaunchKind<'a> {
    App { kind: &'a str, path: &'a str, app_id: &'a str, args: &'a str },
    Folder { path: &'a str },
}

#[derive(Serialize, Clone)]
pub struct MonitorInfo {
    #[serde(rename = "deviceName")]
    pub device_name: String,
    #[serde(rename = "friendlyName")]
    pub friendly_name: String,
    #[serde(rename = "isPrimary")]
    pub is_primary: bool,
    pub number: u32,
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn parse_monitor_target(data: Option<&Value>, foreground_hwnd: isize) -> MonitorTarget {
    let s = data
        .and_then(|d| d.get("monitor"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    match s {
        "" | "default" => MonitorTarget::None,
        "primary" => MonitorTarget::Primary,
        "cursor" => MonitorTarget::Cursor,
        "foreground" => MonitorTarget::Foreground(foreground_hwnd),
        other => MonitorTarget::Named(other.to_string()),
    }
}

pub fn enum_monitors() -> Vec<MonitorInfo> {
    let mut state = EnumMonState { monitors: Vec::new() };
    unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(enum_monitors_cb),
            &mut state as *mut _ as LPARAM,
        );
    }
    state.monitors.sort_by_key(|m| m.number);
    state.monitors
}

/// Fire the launch and set up the watcher. Returns a receiver that gets a
/// single `()` the first time a window is actually moved to the target
/// monitor — macro steps use `recv_timeout` on it to sequence "Open App" then
/// "Minimise All" correctly. Single-action callers ignore the return (their
/// caller is a key press, nothing to sequence with). `None` is returned when
/// no monitor target is set (default launch — nothing to wait on).
pub fn launch_with_monitor_target(kind: LaunchKind, target: MonitorTarget) -> Option<Receiver<()>> {
    let rc = resolve_target_rect(&target);

    // Launch-or-focus: if the target app already has a visible top-level
    // window, restore + foreground it instead of launching a second instance.
    // Applies only to exe-backed launches (see launch_target_exe_name for the
    // exact rules — documents, .lnk, args-carrying and true-UWP launches all
    // fall through to a normal launch). The window is deliberately NOT moved
    // to the action's monitor target: an already-running app comes to the
    // front wherever the user last put it (Rory's call, 2026-08-06). Monitor
    // targets only place NEW windows.
    if let Some(exe) = launch_target_exe_name(&kind) {
        if focus_running_instance(&exe) {
            if rc.is_some() {
                // Macro Open App steps recv_timeout on the receiver to
                // sequence follow-up steps after window placement. Nothing
                // async happens on this path, so signal completion now.
                let (tx, rx) = channel::<()>();
                let _ = tx.send(());
                return Some(rx);
            }
            return None;
        }
    }

    // Fast path: no targeting — preserve historical behaviour exactly.
    if rc.is_none() {
        do_simple_launch(&kind);
        return None;
    }
    let rc = rc.unwrap();

    // Pre-position cursor to centre of the target work area. Many cursor-aware
    // apps (Explorer, modern dialogs, some UWP) honour this with zero post-move
    // latency.
    let cx = rc.left + (rc.right - rc.left) / 2;
    let cy = rc.top + (rc.bottom - rc.top) / 2;
    unsafe { SetCursorPos(cx, cy); }

    // Snapshot existing top-level HWNDs so the watcher can diff against them.
    let existing: HashSet<isize> = enum_top_level_hwnds()
        .into_iter()
        .map(|h| h as isize)
        .collect();
    let own_pid = unsafe { GetCurrentProcessId() };

    // Folder launches don't hit the restore-race the delay was built to defeat
    // (Explorer folder windows don't persist per-path window rects the way
    // apps like Chrome / Word do). Move immediately so subsequent macro steps
    // like Win+Arrow quadrant snaps land after our move, not before.
    let is_folder = matches!(kind, LaunchKind::Folder { .. });
    let delay_ms = if is_folder { 0 } else { MOVE_DELAY_MS };

    // Launch synchronously on the calling thread (so the LaunchKind borrow stays
    // alive). PID returned for path launches; None for AUMID / folder / failures.
    let pid_opt = launch_and_get_pid(&kind);

    // Completion channel — signalled the first time a delayed-move fires
    // (spawn_delayed_move sends `()` after its SetWindowPos). Each spawned
    // move gets a clone; extra sends after the first are received by whoever
    // still holds the receiver or dropped silently. If the watcher exits its
    // 3s budget without moving anything, all senders drop and recv_timeout
    // returns Disconnected before its timeout — caller then falls through.
    let (tx, rx) = channel::<()>();

    // Watcher thread: finds the new window and moves it. Lives at most 3s.
    thread::spawn(move || {
        if let Some(pid) = pid_opt {
            watch_via_winevent(pid, rc, existing, tx);
        } else {
            watch_via_poll(existing, own_pid, rc, delay_ms, tx);
        }
    });

    Some(rx)
}

// ── Monitor enumeration ─────────────────────────────────────────────────────

struct EnumMonState {
    monitors: Vec<MonitorInfo>,
}

unsafe extern "system" fn enum_monitors_cb(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let state = &mut *(lparam as *mut EnumMonState);
    let mut info: MONITORINFOEXW = mem::zeroed();
    info.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut info as *mut _ as *mut MONITORINFO) == 0 {
        return TRUE;
    }
    let device_w = &info.szDevice;
    let len = device_w.iter().position(|&c| c == 0).unwrap_or(device_w.len());
    let device_name = String::from_utf16_lossy(&device_w[..len]);
    let number = device_name
        .strip_prefix(r"\\.\DISPLAY")
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let is_primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;
    let friendly_name = if number > 0 {
        format!("Monitor {}", number)
    } else {
        device_name.clone()
    };
    state.monitors.push(MonitorInfo { device_name, friendly_name, is_primary, number });
    TRUE
}

struct FindMonState {
    target: String,
    found: Option<HMONITOR>,
}

unsafe extern "system" fn find_hmonitor_cb(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let state = &mut *(lparam as *mut FindMonState);
    let mut info: MONITORINFOEXW = mem::zeroed();
    info.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut info as *mut _ as *mut MONITORINFO) == 0 {
        return TRUE;
    }
    let device_w = &info.szDevice;
    let len = device_w.iter().position(|&c| c == 0).unwrap_or(device_w.len());
    let device_name = String::from_utf16_lossy(&device_w[..len]);
    if device_name == state.target {
        state.found = Some(hmon);
        return 0;
    }
    TRUE
}

fn find_hmonitor_by_device(name: &str) -> Option<HMONITOR> {
    let mut state = FindMonState { target: name.to_string(), found: None };
    unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(find_hmonitor_cb),
            &mut state as *mut _ as LPARAM,
        );
    }
    state.found
}

fn resolve_target_rect(target: &MonitorTarget) -> Option<RECT> {
    let hmon = match target {
        MonitorTarget::None => return None,
        MonitorTarget::Primary => unsafe {
            MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)
        },
        MonitorTarget::Cursor => {
            let mut pt = POINT { x: 0, y: 0 };
            unsafe { GetCursorPos(&mut pt); }
            unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) }
        }
        MonitorTarget::Foreground(hwnd) => {
            if *hwnd == 0 {
                unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }
            } else {
                unsafe { MonitorFromWindow(*hwnd as HWND, MONITOR_DEFAULTTONEAREST) }
            }
        }
        MonitorTarget::Named(name) => match find_hmonitor_by_device(name) {
            Some(h) => h,
            None => {
                warn!("[WINDOW-TARGET] target monitor '{}' not connected, falling back to primary", name);
                unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }
            }
        },
    };

    if hmon.is_null() {
        return None;
    }
    let mut info: MONITORINFO = unsafe { mem::zeroed() };
    info.cbSize = mem::size_of::<MONITORINFO>() as u32;
    if unsafe { GetMonitorInfoW(hmon, &mut info) } == 0 {
        return None;
    }
    Some(info.rcWork)
}

// ── Window enumeration helpers ──────────────────────────────────────────────

unsafe extern "system" fn collect_hwnd_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let vec = &mut *(lparam as *mut Vec<HWND>);
    if is_top_level_window(hwnd) {
        vec.push(hwnd);
    }
    TRUE
}

fn enum_top_level_hwnds() -> Vec<HWND> {
    let mut vec: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect_hwnd_cb), &mut vec as *mut _ as LPARAM); }
    vec
}

fn is_top_level_window(hwnd: HWND) -> bool {
    unsafe {
        if IsWindowVisible(hwnd) == 0 { return false; }
        if !GetParent(hwnd).is_null() { return false; }
        let exstyle = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        if (exstyle & WS_EX_TOOLWINDOW) != 0 { return false; }
        // Filter zero/tiny windows (off-screen hidden popups, system notifiers).
        let mut rect: RECT = mem::zeroed();
        if GetWindowRect(hwnd, &mut rect) == 0 { return false; }
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w < 50 || h < 50 { return false; }
        true
    }
}

fn find_new_top_level_for_pid(existing: &HashSet<isize>, target_pid: u32) -> Option<HWND> {
    let current = enum_top_level_hwnds();
    for hwnd in current {
        if existing.contains(&(hwnd as isize)) { continue; }
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid); }
        if pid == target_pid {
            return Some(hwnd);
        }
    }
    None
}

// ── Launch-or-focus ─────────────────────────────────────────────────────────

/// Resolve the exe filename (lowercase) a launch would start, or None when
/// launch-or-focus shouldn't apply:
/// - kind="path" must point at an .exe. Documents (report.xlsx) and .lnk
///   shortcuts open via their handler app whose process name we can't know —
///   they fall through to a normal launch.
/// - kind="aumid": Get-StartApps returns Win32 apps as folder-GUID-prefixed
///   exe paths ("{GUID}\Vendor\app.exe") — those match by basename. True UWP
///   AUMIDs ("Package!App") are skipped: UWP shell activation is
///   single-instance and foregrounds the running app natively.
/// - Launches carrying args are skipped — focusing an existing window would
///   silently swallow the arguments.
fn launch_target_exe_name(kind: &LaunchKind) -> Option<String> {
    let LaunchKind::App { kind: lk, path, app_id, args } = kind else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let candidate = if *lk == "aumid" && !app_id.is_empty() { *app_id } else { *path };
    let name = candidate
        .rsplit(|c| c == '\\' || c == '/')
        .next()
        .unwrap_or(candidate)
        .trim()
        .to_lowercase();
    if name.ends_with(".exe") && name.len() > 4 {
        Some(name)
    } else {
        None
    }
}

struct FindAppWindowState {
    exe_lower: String,
    own_pid: u32,
    found: isize,
}

unsafe extern "system" fn find_app_window_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let state = &mut *(lparam as *mut FindAppWindowState);
    // Deliberately NOT is_top_level_window(): its 50px size floor rejects
    // minimized windows (Windows parks them at -32000 with a ~160x28 rect),
    // and restoring a minimized instance is exactly this feature's job.
    if IsWindowVisible(hwnd) == 0 {
        return TRUE;
    }
    if !GetParent(hwnd).is_null() {
        return TRUE;
    }
    let exstyle = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
    if (exstyle & WS_EX_TOOLWINDOW) != 0 {
        return TRUE;
    }
    // Require a non-empty title — filters technically-visible helper/host
    // windows that aren't user-facing.
    let mut title_buf = [0u16; 2];
    if GetWindowTextW(hwnd, title_buf.as_mut_ptr(), title_buf.len() as i32) <= 0 {
        return TRUE;
    }
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 || pid == state.own_pid {
        return TRUE;
    }
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if handle.is_null() {
        return TRUE;
    }
    let mut buf = [0u16; 260];
    let mut size: u32 = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
    CloseHandle(handle);
    if ok == 0 || size == 0 {
        return TRUE;
    }
    let full = String::from_utf16_lossy(&buf[..size as usize]);
    let base = full
        .rsplit(|c| c == '\\' || c == '/')
        .next()
        .unwrap_or("")
        .to_lowercase();
    if base == state.exe_lower {
        state.found = hwnd as isize;
        return 0; // stop enumeration — EnumWindows walks Z-order, first hit is topmost
    }
    TRUE
}

/// Find a visible top-level window belonging to a running `exe_lower` process
/// and bring it to the foreground in place (restoring from minimized first).
/// Returns false when no instance is running — caller falls through to a
/// normal launch.
fn focus_running_instance(exe_lower: &str) -> bool {
    let mut state = FindAppWindowState {
        exe_lower: exe_lower.to_string(),
        own_pid: unsafe { GetCurrentProcessId() },
        found: 0,
    };
    unsafe {
        EnumWindows(Some(find_app_window_cb), &mut state as *mut _ as LPARAM);
    }
    if state.found == 0 {
        return false;
    }
    let hwnd = state.found as HWND;
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
    }
    crate::actions::set_foreground_robust(state.found);
    info!(
        "[Keyfire] Open App: {} already running — focused existing window (HWND 0x{:X})",
        exe_lower, state.found
    );
    true
}

// Classic UWP apps (Calculator, Clock, Calendar, Photos, Store, Settings,
// etc.) are hosted in ApplicationFrameHost.exe with a top-level class name of
// "ApplicationFrameWindow". They render via DirectComposition, and any
// programmatic SetWindowPos leaves their compositor surface bound to the
// source monitor's DXGI output — the window moves visually but the content
// area shows nothing. Neither RedrawWindow nor bracketing with
// WM_ENTERSIZEMOVE / WM_EXITSIZEMOVE was enough to prod it back. Skipping
// these windows is the honest fix: monitor targeting doesn't work for UWP
// system apps. Third-party UWP apps that use their own top-level window
// (WinUI 3, some newer Store apps) don't have this class name and continue
// to be moved normally.
fn is_uwp_frame_window(hwnd: HWND) -> bool {
    let mut buf: [u16; 64] = [0; 64];
    let len = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len == 0 { return false; }
    let name = String::from_utf16_lossy(&buf[..len as usize]);
    name == "ApplicationFrameWindow"
}

fn move_window_centered(hwnd: HWND, rc: RECT) {
    if is_uwp_frame_window(hwnd) {
        warn!(
            "[WINDOW-TARGET] skipping UWP app (ApplicationFrameWindow HWND {:x}) — \
             DirectComposition doesn't survive programmatic monitor moves; \
             app will open wherever Windows placed it",
            hwnd as usize
        );
        return;
    }

    let mut wrect: RECT = unsafe { mem::zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut wrect) } == 0 { return; }
    let w = wrect.right - wrect.left;
    let h = wrect.bottom - wrect.top;
    let work_w = rc.right - rc.left;
    let work_h = rc.bottom - rc.top;
    let new_x = rc.left + (work_w - w) / 2;
    let new_y = rc.top + (work_h - h) / 2;
    unsafe {
        // Bracket the move with WM_ENTERSIZEMOVE / WM_EXITSIZEMOVE so UWP
        // frameworks (Calculator, Photos, Store apps, etc.) treat this as an
        // interactive positioning session — same as a manual title-bar drag.
        // Without the bracket, DirectComposition sees a lone
        // WM_WINDOWPOSCHANGED and leaves its swap chain bound to the source
        // monitor's DXGI output, painting nothing. With the bracket, it
        // properly re-binds at exit and re-presents.
        SendMessageW(hwnd, WM_ENTERSIZEMOVE, 0, 0);
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            new_x,
            new_y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        SendMessageW(hwnd, WM_EXITSIZEMOVE, 0, 0);
        // Post-move redraw nudge — invalidates the whole window including all
        // child windows and forces an immediate paint pass. Belt-and-braces
        // for anything the bracket didn't already handle.
        RedrawWindow(
            hwnd,
            ptr::null(),
            ptr::null_mut(),
            RDW_INVALIDATE | RDW_ALLCHILDREN | RDW_UPDATENOW,
        );
    }
}

// Schedule a `move_window_centered` `delay_ms` in the future. Lets the app
// finish restoring its own last-known window rect before we override. HWND
// crosses the thread boundary as isize (raw pointer is !Send). If the app
// closes the window before the delay fires, `move_window_centered`'s
// GetWindowRect returns 0 and the call is a no-op — no crash. delay_ms=0 is
// supported (folder launches use it to avoid racing macro-driven snaps that
// follow the Open Folder step). `on_move_complete` is `Some` for the macro
// path so subsequent steps can sequence after the actual move; `None` for
// the last-ditch snapshot-diff fallback which signals its own completion.
fn spawn_delayed_move(
    hwnd: HWND,
    rc: RECT,
    log_msg: String,
    delay_ms: u64,
    on_move_complete: Option<Sender<()>>,
) {
    let hwnd_isize = hwnd as isize;
    thread::spawn(move || {
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        let hwnd = hwnd_isize as HWND;
        move_window_centered(hwnd, rc);
        info!("{}", log_msg);
        // Send is best-effort — if receiver already dropped (macro moved on or
        // timed out), Err is expected and swallowed.
        if let Some(tx) = on_move_complete {
            let _ = tx.send(());
        }
    });
}

// ── Launchers ───────────────────────────────────────────────────────────────

fn launch_and_get_pid(kind: &LaunchKind) -> Option<u32> {
    match kind {
        LaunchKind::App { kind: lk, path, app_id, args } => {
            if *lk == "aumid" && !app_id.is_empty() {
                // Shell-brokered (AUMID via AppsFolder) — no PID available.
                let target = format!("shell:AppsFolder\\{}", app_id);
                shell_execute_no_pid(&target, args);
                None
            } else {
                if path.is_empty() {
                    warn!("[Keyfire] Open App: empty target (kind={})", lk);
                    return None;
                }
                shell_execute_ex_capture_pid(path, args)
            }
        }
        LaunchKind::Folder { path } => {
            if path.is_empty() {
                warn!("[Keyfire] Open Folder: empty path");
                return None;
            }
            // Explorer is shell-brokered too; no reliable child PID.
            shell_execute_no_pid(path, "");
            None
        }
    }
}

fn shell_execute_ex_capture_pid(target: &str, args: &str) -> Option<u32> {
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let params_wide: Vec<u16> = if !args.is_empty() {
        args.encode_utf16().chain(std::iter::once(0)).collect()
    } else {
        Vec::new()
    };
    let params_ptr = if !args.is_empty() { params_wide.as_ptr() } else { ptr::null() };

    let mut sei: SHELLEXECUTEINFOW = unsafe { mem::zeroed() };
    sei.cbSize = mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI;
    sei.lpVerb = verb.as_ptr();
    sei.lpFile = file.as_ptr();
    sei.lpParameters = params_ptr;
    sei.nShow = SW_SHOW as i32;

    let ok = unsafe { ShellExecuteExW(&mut sei) };
    if ok == 0 {
        warn!("[Keyfire] Open App: ShellExecuteExW failed for {}", target);
        return None;
    }
    if sei.hProcess.is_null() {
        info!("[Keyfire] Open App: launched {} (no process handle returned)", target);
        return None;
    }
    let pid = unsafe { GetProcessId(sei.hProcess) };
    unsafe { CloseHandle(sei.hProcess); }
    info!("[Keyfire] Open App: launched {} (pid {})", target, pid);
    if pid == 0 { None } else { Some(pid) }
}

fn shell_execute_no_pid(target: &str, args: &str) {
    let verb: Vec<u16> = "open\0".encode_utf16().collect();
    let file: Vec<u16> = target.encode_utf16().chain(std::iter::once(0)).collect();
    let params_wide: Vec<u16> = if !args.is_empty() {
        args.encode_utf16().chain(std::iter::once(0)).collect()
    } else {
        Vec::new()
    };
    let params_ptr = if !args.is_empty() { params_wide.as_ptr() } else { ptr::null() };

    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params_ptr,
            ptr::null(),
            SW_SHOW as i32,
        )
    };
    if (result as usize) > 32 {
        info!("[Keyfire] Launched (shell): {}", target);
    } else {
        warn!("[Keyfire] ShellExecuteW failed for {} (code {})", target, result as usize);
    }
}

fn do_simple_launch(kind: &LaunchKind) {
    match kind {
        LaunchKind::App { kind: lk, path, app_id, args } => {
            let target = if *lk == "aumid" && !app_id.is_empty() {
                format!("shell:AppsFolder\\{}", app_id)
            } else {
                path.to_string()
            };
            if target.is_empty() {
                warn!("[Keyfire] Open App: empty target (kind={})", lk);
                return;
            }
            shell_execute_no_pid(&target, args);
        }
        LaunchKind::Folder { path } => {
            if !path.is_empty() {
                shell_execute_no_pid(path, "");
            }
        }
    }
}

// ── Watchers ────────────────────────────────────────────────────────────────

// Watchers detect every new visible top-level window that matches the launched
// PID (or, for brokered launches, every window not owned by us) throughout the
// 3s budget. Each detection SCHEDULES A DELAYED MOVE — MOVE_DELAY_MS after
// detection — so the app has time to run its own position-restore logic
// before we override. Otherwise apps like Word, Excel, Chrome, VS Code (which
// write their last window rect to disk on close and restore on launch) win
// the race: they re-set the window's position after our initial move, leaving
// it on its OS-remembered monitor.
//
// Some apps (Google Earth, Adobe products, Office) show a small bootstrap /
// loader window first, then surface the real main window 1-2 seconds later.
// Each SHOW event gets its own scheduled delayed move, so both are handled.
const MOVE_DELAY_MS: u64 = 400;

thread_local! {
    static TARGET_PID: Cell<u32> = Cell::new(0);
    static TARGET_RECT: Cell<Option<RECT>> = Cell::new(None);
    static MOVE_COUNT: Cell<u32> = Cell::new(0);
    // Sender clone slot — win_event_proc reads it (via clone) to hand a Sender
    // to each spawn_delayed_move. RefCell because Sender is not Copy.
    static TX_ON_MOVE: RefCell<Option<Sender<()>>> = RefCell::new(None);
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread_id: u32,
    _event_ms: u32,
) {
    if event != EVENT_OBJECT_SHOW { return; }
    if id_object != OBJID_WINDOW as i32 { return; }
    if id_child != 0 { return; }
    if !is_top_level_window(hwnd) { return; }

    let target_pid = TARGET_PID.with(|p| p.get());
    let mut pid: u32 = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid != target_pid { return; }

    if let Some(rc) = TARGET_RECT.with(|r| r.get()) {
        MOVE_COUNT.with(|c| c.set(c.get() + 1));
        let log_msg = format!(
            "[WINDOW-TARGET] moved HWND {:x} (pid {}) to centre of target monitor",
            hwnd as usize, pid
        );
        let tx = TX_ON_MOVE.with(|t| t.borrow().clone());
        spawn_delayed_move(hwnd, rc, log_msg, MOVE_DELAY_MS, tx);
    }
}

fn watch_via_winevent(pid: u32, rc: RECT, existing: HashSet<isize>, tx: Sender<()>) {
    TARGET_PID.with(|p| p.set(pid));
    TARGET_RECT.with(|r| r.set(Some(rc)));
    MOVE_COUNT.with(|c| c.set(0));
    // Retain the Sender in thread-local storage so win_event_proc can clone it
    // into each spawn_delayed_move. Cleared at function exit — the outer send
    // in the post-hook diff branch uses a fresh clone taken before Unhook.
    TX_ON_MOVE.with(|t| *t.borrow_mut() = Some(tx.clone()));

    let hook = unsafe {
        SetWinEventHook(
            EVENT_OBJECT_SHOW,
            EVENT_OBJECT_SHOW,
            ptr::null_mut(),
            Some(win_event_proc),
            pid,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.is_null() {
        warn!("[WINDOW-TARGET] SetWinEventHook returned null for pid {}, falling back to poll", pid);
        // Clear the thread-local — the poll fallback owns the Sender now.
        TX_ON_MOVE.with(|t| *t.borrow_mut() = None);
        watch_via_poll_for_pid(existing, pid, rc, tx);
        return;
    }

    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if Instant::now() >= deadline { break; }

        // Drain any queued messages so OUTOFCONTEXT events flow through.
        let mut msg: MSG = unsafe { mem::zeroed() };
        let r = unsafe { PeekMessageW(&mut msg, ptr::null_mut(), 0, 0, PM_REMOVE) };
        if r != 0 {
            unsafe { TranslateMessage(&msg); DispatchMessageW(&msg); }
        } else {
            thread::sleep(Duration::from_millis(10));
        }
    }

    unsafe { UnhookWinEvent(hook); }

    let moved = MOVE_COUNT.with(|c| c.get());
    if moved == 0 {
        // Hook never fired — last-ditch snapshot diff. Covers the (rare) race
        // where the new window was created between snapshot and hook install.
        if let Some(hwnd) = find_new_top_level_for_pid(&existing, pid) {
            move_window_centered(hwnd, rc);
            info!("[WINDOW-TARGET] post-hook diff moved HWND {:x} (pid {})", hwnd as usize, pid);
            // Signal completion since we did move the window even though the
            // hook never fired — the macro-side receiver would otherwise wait
            // its full timeout for an already-completed move.
            let _ = tx.send(());
        } else {
            warn!("[WINDOW-TARGET] pid {} didn't surface a window within 3s — no move", pid);
        }
    } else {
        info!("[WINDOW-TARGET] pid {} moved {} window(s) during launch window", pid, moved);
    }

    // Clear thread-local so nothing outlives this watcher invocation.
    TX_ON_MOVE.with(|t| *t.borrow_mut() = None);
}

fn watch_via_poll_for_pid(existing: HashSet<isize>, pid: u32, rc: RECT, tx: Sender<()>) {
    let mut moved: HashSet<isize> = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        for hwnd in enum_top_level_hwnds() {
            let key = hwnd as isize;
            if existing.contains(&key) || moved.contains(&key) { continue; }
            let mut wpid: u32 = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut wpid); }
            if wpid != pid { continue; }
            let log_msg = format!("[WINDOW-TARGET] poll moved HWND {:x} (pid {})", hwnd as usize, pid);
            spawn_delayed_move(hwnd, rc, log_msg, MOVE_DELAY_MS, Some(tx.clone()));
            moved.insert(key);
        }
        thread::sleep(Duration::from_millis(50));
    }
    if moved.is_empty() {
        warn!("[WINDOW-TARGET] pid {} poll didn't surface a window within 3s — no move", pid);
    }
}

fn watch_via_poll(existing: HashSet<isize>, exclude_pid: u32, rc: RECT, delay_ms: u64, tx: Sender<()>) {
    let mut moved: HashSet<isize> = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        for hwnd in enum_top_level_hwnds() {
            let key = hwnd as isize;
            if existing.contains(&key) || moved.contains(&key) { continue; }
            let mut pid: u32 = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid); }
            if pid == 0 || pid == exclude_pid { continue; }
            let log_msg = format!("[WINDOW-TARGET] poll (brokered) moved HWND {:x}", hwnd as usize);
            spawn_delayed_move(hwnd, rc, log_msg, delay_ms, Some(tx.clone()));
            moved.insert(key);
        }
        thread::sleep(Duration::from_millis(50));
    }
    if moved.is_empty() {
        warn!("[WINDOW-TARGET] brokered launch didn't surface a window within 3s — no move");
    }
}
