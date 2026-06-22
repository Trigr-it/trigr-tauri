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

use std::cell::Cell;
use std::collections::HashSet;
use std::mem;
use std::ptr;
use std::thread;
use std::time::{Duration, Instant};

use log::{info, warn};
use serde::Serialize;
use serde_json::Value;

use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM, POINT, RECT, TRUE};
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow,
    HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
    MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};

// windows-sys 0.59 omits this constant from Win32::Graphics::Gdi; it's a stable
// Win32 SDK value so we inline it. dwFlags & this bit == primary monitor.
const MONITORINFOF_PRIMARY: u32 = 0x00000001;
use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetProcessId};
use windows_sys::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows_sys::Win32::UI::Shell::{
    ShellExecuteExW, ShellExecuteW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS,
    SHELLEXECUTEINFOW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetCursorPos, GetParent, GetWindowLongW, GetWindowRect,
    GetWindowThreadProcessId, IsWindowVisible, PeekMessageW, SetCursorPos, SetWindowPos,
    TranslateMessage, EVENT_OBJECT_SHOW, GWL_EXSTYLE, MSG, OBJID_WINDOW, PM_REMOVE,
    SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER, SW_SHOW, WINEVENT_OUTOFCONTEXT,
    WINEVENT_SKIPOWNPROCESS, WS_EX_TOOLWINDOW,
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

pub fn launch_with_monitor_target(kind: LaunchKind, target: MonitorTarget) {
    let rc = resolve_target_rect(&target);

    // Fast path: no targeting — preserve historical behaviour exactly.
    if rc.is_none() {
        do_simple_launch(&kind);
        return;
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

    // Launch synchronously on the calling thread (so the LaunchKind borrow stays
    // alive). PID returned for path launches; None for AUMID / folder / failures.
    let pid_opt = launch_and_get_pid(&kind);

    // Watcher thread: finds the new window and moves it. Lives at most 3s.
    thread::spawn(move || {
        if let Some(pid) = pid_opt {
            watch_via_winevent(pid, rc, existing);
        } else {
            watch_via_poll(existing, own_pid, rc);
        }
    });
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

fn find_new_top_level_excluding_pid(existing: &HashSet<isize>, exclude_pid: u32) -> Option<HWND> {
    let current = enum_top_level_hwnds();
    for hwnd in current {
        if existing.contains(&(hwnd as isize)) { continue; }
        let mut pid: u32 = 0;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid); }
        if pid == 0 || pid == exclude_pid { continue; }
        return Some(hwnd);
    }
    None
}

fn move_window_centered(hwnd: HWND, rc: RECT) {
    let mut wrect: RECT = unsafe { mem::zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut wrect) } == 0 { return; }
    let w = wrect.right - wrect.left;
    let h = wrect.bottom - wrect.top;
    let work_w = rc.right - rc.left;
    let work_h = rc.bottom - rc.top;
    let new_x = rc.left + (work_w - w) / 2;
    let new_y = rc.top + (work_h - h) / 2;
    unsafe {
        SetWindowPos(
            hwnd,
            ptr::null_mut(),
            new_x,
            new_y,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
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
                    warn!("[Trigr] Open App: empty target (kind={})", lk);
                    return None;
                }
                shell_execute_ex_capture_pid(path, args)
            }
        }
        LaunchKind::Folder { path } => {
            if path.is_empty() {
                warn!("[Trigr] Open Folder: empty path");
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
        warn!("[Trigr] Open App: ShellExecuteExW failed for {}", target);
        return None;
    }
    if sei.hProcess.is_null() {
        info!("[Trigr] Open App: launched {} (no process handle returned)", target);
        return None;
    }
    let pid = unsafe { GetProcessId(sei.hProcess) };
    unsafe { CloseHandle(sei.hProcess); }
    info!("[Trigr] Open App: launched {} (pid {})", target, pid);
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
        info!("[Trigr] Launched (shell): {}", target);
    } else {
        warn!("[Trigr] ShellExecuteW failed for {} (code {})", target, result as usize);
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
                warn!("[Trigr] Open App: empty target (kind={})", lk);
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

// Watchers keep moving every NEW visible top-level window that matches the
// launched PID (or, for brokered launches, every window not owned by us) for
// the full 3s budget. Some apps (Google Earth, Adobe products, Office) show a
// small bootstrap/loader window first, then surface the real main window 1-2
// seconds later — moving only the first match leaves the real window on its
// OS-remembered monitor. Keeping the hook alive catches both.

thread_local! {
    static TARGET_PID: Cell<u32> = Cell::new(0);
    static TARGET_RECT: Cell<Option<RECT>> = Cell::new(None);
    static MOVE_COUNT: Cell<u32> = Cell::new(0);
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
        move_window_centered(hwnd, rc);
        MOVE_COUNT.with(|c| c.set(c.get() + 1));
        info!(
            "[WINDOW-TARGET] moved HWND {:x} (pid {}) to centre of target monitor",
            hwnd as usize, pid
        );
    }
}

fn watch_via_winevent(pid: u32, rc: RECT, existing: HashSet<isize>) {
    TARGET_PID.with(|p| p.set(pid));
    TARGET_RECT.with(|r| r.set(Some(rc)));
    MOVE_COUNT.with(|c| c.set(0));

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
        watch_via_poll_for_pid(existing, pid, rc);
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
        } else {
            warn!("[WINDOW-TARGET] pid {} didn't surface a window within 3s — no move", pid);
        }
    } else {
        info!("[WINDOW-TARGET] pid {} moved {} window(s) during launch window", pid, moved);
    }
}

fn watch_via_poll_for_pid(existing: HashSet<isize>, pid: u32, rc: RECT) {
    let mut moved: HashSet<isize> = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        for hwnd in enum_top_level_hwnds() {
            let key = hwnd as isize;
            if existing.contains(&key) || moved.contains(&key) { continue; }
            let mut wpid: u32 = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut wpid); }
            if wpid != pid { continue; }
            move_window_centered(hwnd, rc);
            moved.insert(key);
            info!("[WINDOW-TARGET] poll moved HWND {:x} (pid {})", hwnd as usize, pid);
        }
        thread::sleep(Duration::from_millis(50));
    }
    if moved.is_empty() {
        warn!("[WINDOW-TARGET] pid {} poll didn't surface a window within 3s — no move", pid);
    }
}

fn watch_via_poll(existing: HashSet<isize>, exclude_pid: u32, rc: RECT) {
    let mut moved: HashSet<isize> = HashSet::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        for hwnd in enum_top_level_hwnds() {
            let key = hwnd as isize;
            if existing.contains(&key) || moved.contains(&key) { continue; }
            let mut pid: u32 = 0;
            unsafe { GetWindowThreadProcessId(hwnd, &mut pid); }
            if pid == 0 || pid == exclude_pid { continue; }
            move_window_centered(hwnd, rc);
            moved.insert(key);
            info!("[WINDOW-TARGET] poll (brokered) moved HWND {:x}", hwnd as usize);
        }
        thread::sleep(Duration::from_millis(50));
    }
    if moved.is_empty() {
        warn!("[WINDOW-TARGET] brokered launch didn't surface a window within 3s — no move");
    }
}
