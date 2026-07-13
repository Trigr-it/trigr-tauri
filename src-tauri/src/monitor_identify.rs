//! Monitor identification overlays — big-number placards shown centred on each
//! physical display while the Open App / Open Folder monitor picker dropdown
//! is open. Native Win32 layered top-most transparent windows, one per
//! monitor, matching the pattern Windows Display Settings' own "Identify"
//! button uses.
//!
//! The number shown is Keyfire's internal monitor number (derived from the
//! GDI `\\.\DISPLAY{N}` device path). Useful when Windows Display Settings'
//! numbering has drifted from GDI's (rearranged monitors, disconnect/reconnect
//! cycles, hybrid graphics) — the user can look at each physical screen and
//! see which number Keyfire uses for it.

use std::mem;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use log::{info, warn};

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, RECT, TRUE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateRoundRectRgn, CreateSolidBrush, DeleteObject, DrawTextW,
    EndPaint, EnumDisplayMonitors, FillRect, GetMonitorInfoW, SelectObject, SetBkMode,
    SetTextColor, SetWindowRgn, UpdateWindow, DEFAULT_CHARSET, DEFAULT_QUALITY, DT_CENTER,
    DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, FW_BOLD, HDC, HMONITOR, MONITORINFO,
    MONITORINFOEXW, PAINTSTRUCT, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW,
    LoadCursorW, RegisterClassExW, SetLayeredWindowAttributes, SetWindowLongPtrW,
    ShowWindow, GWLP_USERDATA, IDC_ARROW, LWA_ALPHA, SW_SHOWNOACTIVATE, WM_ERASEBKGND,
    WM_NCDESTROY, WM_PAINT, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

// Fixed pixel size regardless of monitor DPI. Deliberate: a consistent 260x260
// placard reads about the same at 100% and 175% scaling and always fits inside
// the smallest reasonable work-area rect.
const OVERLAY_W: i32 = 260;
const OVERLAY_H: i32 = 260;
const CORNER_RADIUS: i32 = 24;
const BG_ALPHA: u8 = 235; // ~92% opacity

// Class registration is one-shot for the process lifetime.
static CLASS_REGISTERED: OnceLock<()> = OnceLock::new();

// Vec (linear-scan) is fine — never more than the user's physical monitor count.
static ACTIVE_HWNDS: Mutex<Vec<isize>> = Mutex::new(Vec::new());

// Target visibility state — set by show/hide before their body runs. Guards
// against the invoke-pipeline race where a JS effect fires show then its
// cleanup fires hide, and the two land on different Tauri runtime threads:
// if hide's flag write happens between show's flag write and show's window
// creation, show's tail check catches it and tears everything down.
static SHOULD_BE_SHOWN: AtomicBool = AtomicBool::new(false);

// Per-window payload stored in GWLP_USERDATA as a raw `Box<OverlayPayload>`
// pointer. WM_PAINT reads it (no lock), WM_NCDESTROY frees it. Avoids the
// deadlock that a shared map would cause when UpdateWindow synchronously
// dispatches WM_PAINT while show_identify_overlays still holds the lock.
struct OverlayPayload {
    number: u32,
    device_name: String,
    dark: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    ((b as u32) << 16) | ((g as u32) << 8) | (r as u32)
}

fn ensure_class_registered() {
    CLASS_REGISTERED.get_or_init(|| {
        unsafe {
            let name = wide("KeyfireMonitorIdentify");
            let mut wc: WNDCLASSEXW = mem::zeroed();
            wc.cbSize = mem::size_of::<WNDCLASSEXW>() as u32;
            wc.lpfnWndProc = Some(wnd_proc);
            wc.hCursor = LoadCursorW(ptr::null_mut(), IDC_ARROW);
            wc.hbrBackground = ptr::null_mut();
            wc.lpszClassName = name.as_ptr();
            RegisterClassExW(&wc);
        }
    });
}

// ── Public API ──────────────────────────────────────────────────────────────

pub fn show_identify_overlays(dark: bool) {
    SHOULD_BE_SHOWN.store(true, Ordering::SeqCst);
    ensure_class_registered();
    // Idempotent — clear any lingering overlays first (dropdown can be
    // re-opened rapidly, and stale HWNDs from a previous show shouldn't stack).
    destroy_all_overlays();

    let monitors = crate::window_target::enum_monitors();
    let mut created: Vec<isize> = Vec::new();

    for m in monitors {
        let rc = match find_monitor_work_rect(&m.device_name) {
            Some(r) => r,
            None => continue,
        };
        let cx = rc.left + (rc.right - rc.left) / 2 - OVERLAY_W / 2;
        let cy = rc.top + (rc.bottom - rc.top) / 2 - OVERLAY_H / 2;

        let class = wide("KeyfireMonitorIdentify");
        let title = wide("");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE
                    | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                cx,
                cy,
                OVERLAY_W,
                OVERLAY_H,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if hwnd.is_null() {
            warn!("[MONITOR-ID] CreateWindowExW returned null for {}", m.device_name);
            continue;
        }

        // Attach the payload to the window via GWLP_USERDATA. wnd_proc reads
        // it during WM_PAINT and frees it during WM_NCDESTROY.
        let payload = Box::new(OverlayPayload {
            number: m.number,
            device_name: m.device_name.clone(),
            dark,
        });
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(payload) as isize);
            let rgn = CreateRoundRectRgn(0, 0, OVERLAY_W, OVERLAY_H, CORNER_RADIUS, CORNER_RADIUS);
            SetWindowRgn(hwnd, rgn, 0);
            SetLayeredWindowAttributes(hwnd, 0, BG_ALPHA, LWA_ALPHA);
            // Force synchronous WM_PAINT on the still-hidden window so the
            // backing store is fully painted before we composite to screen.
            // Prevents the one-frame flash of an unpainted layered rectangle.
            UpdateWindow(hwnd);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        created.push(hwnd as isize);
        info!(
            "[MONITOR-ID] shown Monitor {} on {} at ({}, {})",
            m.number, m.device_name, cx, cy
        );
    }

    // Publish the created HWNDs to the shared list in one shot.
    {
        let mut hwnds = ACTIVE_HWNDS.lock().unwrap();
        hwnds.extend(created);
    }

    // Race guard: if a hide arrived after we set the flag but before we
    // finished creating windows, tear them down now.
    if !SHOULD_BE_SHOWN.load(Ordering::SeqCst) {
        destroy_all_overlays();
    }
}

pub fn hide_identify_overlays() {
    SHOULD_BE_SHOWN.store(false, Ordering::SeqCst);
    destroy_all_overlays();
}

fn destroy_all_overlays() {
    // Snapshot + release the lock before calling DestroyWindow — DestroyWindow
    // sends WM_NCDESTROY synchronously which frees the payload via
    // GetWindowLongPtrW, no shared-state re-entry needed.
    let snapshot: Vec<isize> = {
        let mut hwnds = ACTIVE_HWNDS.lock().unwrap();
        hwnds.drain(..).collect()
    };
    for h in snapshot {
        unsafe { DestroyWindow(h as HWND); }
    }
}

// ── Monitor-rect lookup ─────────────────────────────────────────────────────

struct FindByName {
    target: String,
    found: Option<RECT>,
}

unsafe extern "system" fn find_by_name_cb(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let state = &mut *(lparam as *mut FindByName);
    let mut info: MONITORINFOEXW = mem::zeroed();
    info.monitorInfo.cbSize = mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(hmon, &mut info as *mut _ as *mut MONITORINFO) == 0 {
        return TRUE;
    }
    let device_w = &info.szDevice;
    let len = device_w.iter().position(|&c| c == 0).unwrap_or(device_w.len());
    let name = String::from_utf16_lossy(&device_w[..len]);
    if name == state.target {
        state.found = Some(info.monitorInfo.rcWork);
        return 0;
    }
    TRUE
}

fn find_monitor_work_rect(device_name: &str) -> Option<RECT> {
    let mut state = FindByName { target: device_name.to_string(), found: None };
    unsafe {
        EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(find_by_name_cb),
            &mut state as *mut _ as LPARAM,
        );
    }
    state.found
}

// ── Window proc + paint ─────────────────────────────────────────────────────

unsafe fn payload_ptr(hwnd: HWND) -> *mut OverlayPayload {
    GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut OverlayPayload
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            paint_overlay(hwnd);
            0
        }
        WM_NCDESTROY => {
            let p = payload_ptr(hwnd);
            if !p.is_null() {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(p));
            }
            DefWindowProcW(hwnd, msg, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn paint_overlay(hwnd: HWND) {
    let p = payload_ptr(hwnd);
    if p.is_null() { return; }
    let payload = &*p;

    let mut ps: PAINTSTRUCT = mem::zeroed();
    let hdc = BeginPaint(hwnd, &mut ps);
    let mut client: RECT = mem::zeroed();
    GetClientRect(hwnd, &mut client);

    // Theme-derived palette. Dark: elevated-surface bg + soft-gold number.
    // Light: off-white surface + brand-accent gold + darker caption for
    // readable contrast against light desktops.
    let (bg_color, number_color, caption_color) = if payload.dark {
        (rgb(0x1a, 0x1a, 0x24), rgb(0xf5, 0xb9, 0x5a), rgb(0xa0, 0xa0, 0xb4))
    } else {
        (rgb(0xf0, 0xf0, 0xf5), rgb(0xe8, 0xa0, 0x20), rgb(0x4a, 0x4a, 0x6a))
    };

    let bg = CreateSolidBrush(bg_color);
    FillRect(hdc, &client, bg);
    DeleteObject(bg as *mut _);

    SetBkMode(hdc, TRANSPARENT as i32);

    // Big number, upper 2/3 of client area.
    let face = wide("Segoe UI");
    let big_font = CreateFontW(
        160, 0, 0, 0, FW_BOLD as i32, 0, 0, 0,
        DEFAULT_CHARSET as u32, 0, 0, DEFAULT_QUALITY as u32, 0,
        face.as_ptr(),
    );
    let old_font = SelectObject(hdc, big_font as *mut _);
    SetTextColor(hdc, number_color);
    let mut top = client;
    top.bottom = client.top + (client.bottom - client.top) * 2 / 3;
    let number_text = wide(&payload.number.to_string());
    DrawTextW(
        hdc,
        number_text.as_ptr(),
        -1,
        &mut top,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(hdc, old_font);
    DeleteObject(big_font as *mut _);

    // Small device-name caption underneath.
    let small_font = CreateFontW(
        22, 0, 0, 0, FW_BOLD as i32, 0, 0, 0,
        DEFAULT_CHARSET as u32, 0, 0, DEFAULT_QUALITY as u32, 0,
        face.as_ptr(),
    );
    let old_font2 = SelectObject(hdc, small_font as *mut _);
    SetTextColor(hdc, caption_color);
    let mut bot = client;
    bot.top = client.top + (client.bottom - client.top) * 2 / 3;
    let short = payload.device_name.trim_start_matches(r"\\.\").to_string();
    let caption = wide(&format!("Keyfire · {}", short));
    DrawTextW(
        hdc,
        caption.as_ptr(),
        -1,
        &mut bot,
        DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
    );
    SelectObject(hdc, old_font2);
    DeleteObject(small_font as *mut _);

    EndPaint(hwnd, &ps);
}
