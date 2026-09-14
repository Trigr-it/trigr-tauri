use log::{error, info};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use tauri::{
    image::Image,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::TrayIconBuilder,
    AppHandle, Emitter, Manager,
};

static HAS_SHOWN_BALLOON: AtomicBool = AtomicBool::new(false);

/// Pre-generated tray icons stored at startup — normal, paused, and held variants.
static TRAY_ICON_NORMAL: OnceLock<Image<'static>> = OnceLock::new();
static TRAY_ICON_PAUSED: OnceLock<Image<'static>> = OnceLock::new();
static TRAY_ICON_HELD: OnceLock<Image<'static>> = OnceLock::new();

// ── Autolaunch detection ────────────────────────────────────────────────────

pub fn is_autolaunch() -> bool {
    std::env::args().any(|a| a == "--autolaunch")
}

// ── Tray setup ──────────────────────────────────────────────────────────────

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let (rgba, width, height) = load_tray_icon_raw(app)?;

    // Store normal icon
    let normal = Image::new_owned(rgba.clone(), width, height);
    let _ = TRAY_ICON_NORMAL.set(normal);

    // Generate paused icon by dimming alpha to 1/3
    let mut paused_rgba = rgba.clone();
    for i in (3..paused_rgba.len()).step_by(4) {
        paused_rgba[i] = paused_rgba[i] / 3;
    }
    let paused = Image::new_owned(paused_rgba, width, height);
    let _ = TRAY_ICON_PAUSED.set(paused);

    // Generate held icon with red tint (boost R, halve G and B)
    let mut held_rgba = rgba.clone();
    for i in (0..held_rgba.len()).step_by(4) {
        held_rgba[i] = held_rgba[i].saturating_add(80).min(255); // R
        held_rgba[i + 1] = held_rgba[i + 1] / 2;                 // G
        held_rgba[i + 2] = held_rgba[i + 2] / 2;                 // B
        // Alpha unchanged
    }
    let held = Image::new_owned(held_rgba, width, height);
    let _ = TRAY_ICON_HELD.set(held);

    // Build tray with normal icon
    let tray_icon = Image::new_owned(rgba, width, height);
    build_tray(app.handle(), tray_icon)?;
    info!("[Keyfire] System tray created");
    Ok(())
}

/// Decode a PNG to raw RGBA bytes + dimensions (for icon generation).
fn decode_png_raw(path: &std::path::Path) -> Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
    let file = std::fs::File::open(path)?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity((info.width * info.height * 4) as usize);
            for chunk in buf.chunks(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        _ => buf,
    };
    Ok((rgba, info.width, info.height))
}

fn load_tray_icon_raw(app: &tauri::App) -> Result<(Vec<u8>, u32, u32), Box<dyn std::error::Error>> {
    let resource_path = app
        .path()
        .resource_dir()
        .map(|d| d.join("icons").join("tray-icon.png"))
        .unwrap_or_default();
    if resource_path.exists() {
        return decode_png_raw(&resource_path);
    }
    let dev_path = std::env::current_dir()?
        .join("assets")
        .join("icons")
        .join("tray-icon.png");
    if dev_path.exists() {
        return decode_png_raw(&dev_path);
    }
    let fallback = std::env::current_dir()?.join("icons").join("icon.png");
    decode_png_raw(&fallback)
}

/// Swap the tray icon between active (normal) and paused (alpha-dimmed) states.
/// Reads from pre-generated static images — no disk I/O on toggle.
pub fn update_tray_icon(app: &AppHandle, macros_enabled: bool) {
    let icon = if macros_enabled {
        TRAY_ICON_NORMAL.get()
    } else {
        TRAY_ICON_PAUSED.get()
    };
    if let Some(img) = icon {
        if let Some(tray) = app.tray_by_id("trigr-tray") {
            let _ = tray.set_icon(Some(img.clone()));
        }
    }
}

/// Switch tray to held state — red-tinted icon + custom tooltip.
pub fn update_tray_icon_held(app: &AppHandle, held_label: &str) {
    if let Some(img) = TRAY_ICON_HELD.get() {
        if let Some(tray) = app.tray_by_id("trigr-tray") {
            let _ = tray.set_icon(Some(img.clone()));
            let tip = format!("Keyfire — Holding: {} — press again to release", held_label);
            let _ = tray.set_tooltip(Some(&tip));
        }
    }
}

/// Update tray icon to indicate a key is being repeated.
pub fn update_tray_icon_repeating(app: &AppHandle, label: &str, interval_ms: u64) {
    if let Some(img) = TRAY_ICON_HELD.get() {
        if let Some(tray) = app.tray_by_id("trigr-tray") {
            let _ = tray.set_icon(Some(img.clone()));
            let tip = format!("Keyfire — Repeating: {} ({}ms) — press again to stop", label, interval_ms);
            let _ = tray.set_tooltip(Some(&tip));
        }
    }
}

/// Restore tray to the correct non-held state (active or paused).
pub fn update_tray_icon_normal(app: &AppHandle) {
    let enabled = crate::hotkeys::engine_active();
    update_tray_icon(app, enabled);
    let tooltip = if enabled {
        "Keyfire — Active".to_string()
    } else if let Some(exe) = crate::foreground::excluded_app_in_foreground() {
        format!("Keyfire — Paused in {} (excluded app)", exe)
    } else {
        "Keyfire — Paused".to_string()
    };
    if let Some(tray) = app.tray_by_id("trigr-tray") {
        let _ = tray.set_tooltip(Some(tooltip.as_str()));
    }
}

fn build_tray(
    app: &AppHandle,
    icon: Image<'static>,
) -> Result<(), Box<dyn std::error::Error>> {
    let enabled = crate::hotkeys::MACROS_ENABLED.load(Ordering::Relaxed);

    // Menu items
    let open_item = MenuItem::with_id(app, "open", "Open Keyfire", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;

    let pause_label = if enabled {
        "Pause Keyfire"
    } else {
        "Resume Keyfire"
    };
    let pause_item = MenuItem::with_id(app, "pause", pause_label, true, None::<&str>)?;

    let sep2 = PredefinedMenuItem::separator(app)?;

    let startup_on = get_startup_enabled_sync();
    let startup_item =
        CheckMenuItem::with_id(app, "startup", "Start with Windows", true, startup_on, None::<&str>)?;

    let sep3 = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(app, "quit", "Quit Keyfire", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &open_item,
            &sep1,
            &pause_item,
            &sep2,
            &startup_item,
            &sep3,
            &quit_item,
        ],
    )?;

    let tooltip = if enabled {
        "Keyfire — Active"
    } else {
        "Keyfire — Paused"
    };

    // Remove existing tray icon if any (for rebuilds)
    if let Some(existing) = app.tray_by_id("trigr-tray") {
        let _ = existing.set_menu(Some(menu));
        let _ = existing.set_tooltip(Some(tooltip));
        return Ok(());
    }

    TrayIconBuilder::with_id("trigr-tray")
        .icon(icon)
        .tooltip(tooltip)
        .menu(&menu)
        // Tauri's default pops the context menu on LEFT click too, so a single
        // left click both toggled the window (our handler below) and opened
        // the menu as if right-clicked. Left = show/hide only; right = menu.
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            match event.id().as_ref() {
                "open" => show_window(app),
                "pause" => toggle_pause(app),
                "startup" => {
                    let currently_on = get_startup_enabled_sync();
                    set_startup_enabled_impl(!currently_on);
                }
                "quit" => {
                    info!("[Keyfire] Quit requested from tray");
                    // Same cleanup as the UI quit_app command (RunEvent::Exit
                    // repeats it; both are idempotent).
                    crate::actions::release_held_key();
                    crate::actions::stop_repeating_key();
                    crate::actions::release_all_bare_remaps();
                    crate::actions::kill_all_ahk_processes();
                    app.exit(0);
                }
                _ => {}
            }
        })
        .on_tray_icon_event(move |tray, event| {
            if let tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            } = event
            {
                // A double-click arrives as UP, DBLCLK, UP — two toggles, so the
                // window flashed open and vanished for anyone who double-clicks
                // tray icons by habit. Ignore an Up inside the double-click window.
                static LAST_TRAY_CLICK_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let last = LAST_TRAY_CLICK_MS.swap(now_ms, std::sync::atomic::Ordering::SeqCst);
                if now_ms.saturating_sub(last) < 400 {
                    return;
                }
                let app = tray.app_handle();
                toggle_window_visibility(app);
            }
        })
        .build(app)?;

    Ok(())
}

/// Rebuild the tray menu (e.g. after pause/resume state change).
pub fn rebuild_tray_menu(app: &AppHandle) {
    if let Some(tray) = app.tray_by_id("trigr-tray") {
        let enabled = crate::hotkeys::MACROS_ENABLED.load(Ordering::Relaxed);

        let tooltip = if enabled {
            "Keyfire — Active"
        } else {
            "Keyfire — Paused"
        };
        let _ = tray.set_tooltip(Some(tooltip));

        // Rebuild menu items
        let pause_label = if enabled {
            "Pause Keyfire"
        } else {
            "Resume Keyfire"
        };

        if let Ok(open_item) = MenuItem::with_id(app, "open", "Open Keyfire", true, None::<&str>) {
            if let Ok(sep1) = PredefinedMenuItem::separator(app) {
                if let Ok(pause_item) = MenuItem::with_id(app, "pause", pause_label, true, None::<&str>) {
                    if let Ok(sep2) = PredefinedMenuItem::separator(app) {
                        let startup_on = get_startup_enabled_sync();
                        if let Ok(startup_item) = CheckMenuItem::with_id(app, "startup", "Start with Windows", true, startup_on, None::<&str>) {
                            if let Ok(sep3) = PredefinedMenuItem::separator(app) {
                                if let Ok(quit_item) = MenuItem::with_id(app, "quit", "Quit Keyfire", true, None::<&str>) {
                                    if let Ok(menu) = Menu::with_items(app, &[&open_item, &sep1, &pause_item, &sep2, &startup_item, &sep3, &quit_item]) {
                                        let _ = tray.set_menu(Some(menu));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── Window management ───────────────────────────────────────────────────────

pub fn show_window(app: &AppHandle) {
    // Restore the webview memory target trimmed while hidden in the tray.
    crate::webview_mem::resume_for_show(app, "main");
    if let Some(window) = app.get_webview_window("main") {
        // Off-screen guard: Windows relocates VISIBLE windows when a monitor
        // disappears, not hidden ones. A user who parked Keyfire on an
        // external monitor, closed to tray and undocked got an invisible
        // window (and is_visible() then said true, so the next click hid it
        // again). Centre it if it no longer intersects any monitor.
        #[cfg(windows)]
        if let Ok(hwnd) = window.hwnd() {
            let hmon = unsafe {
                windows_sys::Win32::Graphics::Gdi::MonitorFromWindow(
                    hwnd.0 as _,
                    windows_sys::Win32::Graphics::Gdi::MONITOR_DEFAULTTONULL,
                )
            };
            if hmon.is_null() {
                info!("[Keyfire] Main window is off every monitor — centring before show");
                let _ = window.center();
            }
        }
        let _ = window.show();
        // If the window is minimized, restore it before focusing — set_focus
        // on a minimized window leaves it in the taskbar with no actual focus.
        if window.is_minimized().unwrap_or(false) {
            let _ = window.unminimize();
        }
        let _ = window.set_focus();

        // Windows focus-stealing prevention causes SetForegroundWindow (which
        // set_focus() calls underneath) to silently fail when our process
        // isn't already foreground — which it isn't when explorer.exe handled
        // the tray click. The AttachThreadInput trick temporarily attaches
        // our input queue to the current foreground thread's, lifting the
        // restriction. Standard Windows workaround, used by most apps that
        // surface from the tray.
        if let Ok(hwnd) = window.hwnd() {
            unsafe {
                use windows_sys::Win32::UI::WindowsAndMessaging::{
                    BringWindowToTop, GetForegroundWindow,
                    GetWindowThreadProcessId, SetForegroundWindow,
                };
                use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};

                let target_hwnd = hwnd.0 as isize;
                let foreground_hwnd = GetForegroundWindow() as isize;
                if foreground_hwnd != 0 && foreground_hwnd != target_hwnd {
                    let mut foreground_pid: u32 = 0;
                    let foreground_tid = GetWindowThreadProcessId(
                        foreground_hwnd as _,
                        &mut foreground_pid,
                    );
                    let current_tid = GetCurrentThreadId();
                    if foreground_tid != 0 && foreground_tid != current_tid {
                        let attached = AttachThreadInput(current_tid, foreground_tid, 1);
                        let _ = SetForegroundWindow(target_hwnd as _);
                        let _ = BringWindowToTop(target_hwnd as _);
                        if attached != 0 {
                            AttachThreadInput(current_tid, foreground_tid, 0);
                        }
                    } else {
                        let _ = SetForegroundWindow(target_hwnd as _);
                        let _ = BringWindowToTop(target_hwnd as _);
                    }
                }
            }
        }
    }
}

pub fn hide_window_to_tray(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();

        // Hiding to the tray (X button or tray toggle) means "I'm done" — unlike
        // minimising or navigating to another app, which keep the editing lock so
        // the user can test their work. Drop the editing lock so the foreground
        // watcher resumes auto-switching, and tell the renderer to clear its
        // selection so the window reopens to a blank slate (no key/action open).
        // Minimise / focus-loss never call this path, so the test-in-another-app
        // flow keeps its profile lock as designed.
        crate::foreground::set_editing_active(false);
        let _ = window.emit("reset-editing-on-hide", ());

        // One-time log on first hide
        if !HAS_SHOWN_BALLOON.swap(true, Ordering::Relaxed) {
            info!("[Keyfire] Window hidden to tray");
        }
    }
}

fn toggle_window_visibility(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        // Windows reports a MINIMISED window as visible (IsWindowVisible returns
        // true for minimised windows — they're technically visible, just as a
        // taskbar icon). Using is_visible alone would treat "minimised via (-)"
        // as "shown normally" and hide the window fully on tray click — the
        // user sees NOTHING happen (both taskbar entry and window vanish) and
        // thinks the tray is broken. Treat minimised as "needs restore" so a
        // single tray click surfaces the window regardless of how it was hidden.
        let visible = window.is_visible().unwrap_or(false);
        let minimized = window.is_minimized().unwrap_or(false);
        if visible && !minimized {
            hide_window_to_tray(app);
        } else {
            show_window(app);
        }
    }
}

// ── Pause toggle ────────────────────────────────────────────────────────────

fn toggle_pause(app: &AppHandle) {
    let was_enabled = crate::hotkeys::MACROS_ENABLED.load(Ordering::SeqCst);
    // Release any held/repeating key before pausing
    if was_enabled {
        crate::actions::release_held_key();
        crate::actions::stop_repeating_key();
    }
    crate::hotkeys::MACROS_ENABLED.store(!was_enabled, Ordering::SeqCst);
    let now_enabled = !was_enabled;

    info!(
        "[Keyfire] Global {} — macros {}",
        if now_enabled { "resume" } else { "pause" },
        if now_enabled { "active" } else { "paused" }
    );

    rebuild_tray_menu(app);
    update_tray_icon(app, now_enabled);

    // Notify the renderer of the state change
    if let Some(window) = app.get_webview_window("main") {
        let state = crate::hotkeys::engine_state_lock();
        let _ = window.emit(
            "engine-status",
            serde_json::json!({
                "uiohookAvailable": crate::hotkeys::hooks_running(),
                "nutjsAvailable": false,
                "macrosEnabled": now_enabled,
                "activeProfile": state.active_profile,
                "globalPauseToggleKey": state.pause_hotkey_str,
                "isDemoMode": false
            }),
        );
    }
}

// ── Start with Windows (registry) ───────────────────────────────────────────
//
// Direct Win32 registry calls. Until v0.8.14 this shelled out to `reg.exe`
// three times per boot (tray menu state, the heal, and again per toggle); at
// logon each process spawn costs hundreds of milliseconds under AV and disk
// contention and ran on the main thread before the tray icon existed. The
// Win32 calls take microseconds and cannot fail on a broken PATH.
//
// Two per-user keys are involved:
//   Run             — the autostart entry Explorer launches at logon.
//   StartupApproved — where Task Manager and Settings > Apps > Startup record
//                     the user's enable/disable choice for a Run entry (first
//                     byte 0x02 = enabled, 0x03 = disabled; the Run entry
//                     stays either way). Reading only Run made Keyfire's
//                     toggle say ON while Windows had the entry disabled, so
//                     "Start with Windows" looked on but never started.
//
// The machine-local intent (`config::get_start_with_windows`) is the third
// leg: `heal_startup_registration` restores a Run entry that has gone missing
// while the intent is still ON, and never adds one the user did not ask for.

const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_SUBKEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
// Registry value name stays "Trigr" across the rebrand to preserve existing
// users' "Start with Windows" setting (the registry value name is invisible
// to users; renaming it would orphan their existing entry and make the
// setting appear OFF after update).
const REG_NAME: &str = "Trigr";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Read one HKCU value. `None` = key or value absent (or unreadable).
fn reg_read(subkey: &str, name: &str) -> Option<(u32, Vec<u8>)> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE,
    };
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(subkey).as_ptr(), 0, KEY_QUERY_VALUE, &mut hkey) != 0 {
            return None;
        }
        let name_w = wide(name);
        let mut ty: u32 = 0;
        let mut size: u32 = 0;
        let s1 = RegQueryValueExW(
            hkey,
            name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut ty,
            std::ptr::null_mut(),
            &mut size,
        );
        if s1 != 0 {
            RegCloseKey(hkey);
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let s2 = RegQueryValueExW(
            hkey,
            name_w.as_ptr(),
            std::ptr::null_mut(),
            &mut ty,
            buf.as_mut_ptr(),
            &mut size,
        );
        RegCloseKey(hkey);
        if s2 != 0 {
            return None;
        }
        buf.truncate(size as usize);
        Some((ty, buf))
    }
}

/// Write one HKCU value, creating the key if needed.
fn reg_write(subkey: &str, name: &str, ty: u32, data: &[u8]) -> bool {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
        REG_OPTION_NON_VOLATILE,
    };
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let status = RegCreateKeyExW(
            HKEY_CURRENT_USER,
            wide(subkey).as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut hkey,
            std::ptr::null_mut(),
        );
        if status != 0 {
            return false;
        }
        let ok = RegSetValueExW(hkey, wide(name).as_ptr(), 0, ty, data.as_ptr(), data.len() as u32) == 0;
        RegCloseKey(hkey);
        ok
    }
}

/// Delete one HKCU value. Absent counts as success.
fn reg_delete(subkey: &str, name: &str) -> bool {
    use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegDeleteValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    };
    unsafe {
        let mut hkey: HKEY = std::ptr::null_mut();
        let open = RegOpenKeyExW(HKEY_CURRENT_USER, wide(subkey).as_ptr(), 0, KEY_SET_VALUE, &mut hkey);
        if open == ERROR_FILE_NOT_FOUND {
            return true;
        }
        if open != 0 {
            return false;
        }
        let status = RegDeleteValueW(hkey, wide(name).as_ptr());
        RegCloseKey(hkey);
        status == 0 || status == ERROR_FILE_NOT_FOUND
    }
}

/// Current data of the Run entry, trimmed. `None` = no entry.
fn read_run_value() -> Option<String> {
    use windows_sys::Win32::System::Registry::{REG_EXPAND_SZ, REG_SZ};
    let (ty, data) = reg_read(RUN_SUBKEY, REG_NAME)?;
    if ty != REG_SZ && ty != REG_EXPAND_SZ {
        return None;
    }
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    Some(String::from_utf16_lossy(&units).trim_end_matches('\0').trim().to_string())
}

/// True when Task Manager / Settings > Apps > Startup has the entry disabled.
/// Absent value = enabled (Windows only writes it once the user has toggled).
fn windows_disabled_at_startup() -> bool {
    use windows_sys::Win32::System::Registry::REG_BINARY;
    match reg_read(APPROVED_SUBKEY, REG_NAME) {
        Some((ty, data)) if ty == REG_BINARY && !data.is_empty() => data[0] & 1 == 1,
        _ => false,
    }
}

/// `"C:\...\keyfire.exe" --autolaunch` for the running binary.
fn expected_run_value() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    Some(format!("\"{}\" --autolaunch", exe.to_string_lossy()))
}

fn write_run_entry(value: &str) -> bool {
    use windows_sys::Win32::System::Registry::REG_SZ;
    let data: Vec<u8> = wide(value).iter().flat_map(|u| u.to_le_bytes()).collect();
    reg_write(RUN_SUBKEY, REG_NAME, REG_SZ, &data)
}

/// Mark the entry enabled in StartupApproved so a "Disabled" left over from an
/// earlier Task Manager toggle cannot silently veto the Run entry just written.
fn approve_startup_entry() -> bool {
    use windows_sys::Win32::System::Registry::REG_BINARY;
    reg_write(APPROVED_SUBKEY, REG_NAME, REG_BINARY, &[0x02, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
}

/// A debug build must never write the target\debug exe path into HKCU Run:
/// every boot would then launch a stale dev build instead of the installed
/// app (whichever starts first wins the single-instance mutex). Found live on
/// the dev machine 2026-06-04. Debug builds also share AppData with the
/// installed app, so they leave the intent flag alone too: a toggle in a dev
/// session must not switch the installed app's autostart off.
fn registry_writes_allowed() -> bool {
    !cfg!(debug_assertions)
}

fn get_startup_enabled_sync() -> bool {
    read_run_value().is_some() && !windows_disabled_at_startup()
}

pub fn get_startup_enabled() -> bool {
    get_startup_enabled_sync()
}

fn set_startup_enabled_impl(enable: bool) {
    if !registry_writes_allowed() {
        info!(
            "[Keyfire] Startup registration change ignored (debug build — would pin the dev exe path): enable={}",
            enable
        );
        return;
    }
    if enable {
        let Some(value) = expected_run_value() else {
            error!("[Keyfire] Startup enable failed: current_exe() unavailable");
            return;
        };
        let wrote = write_run_entry(&value);
        let approved = approve_startup_entry();
        crate::config::set_start_with_windows(true);
        if wrote {
            info!("[Keyfire] Startup enabled: {} (approved={})", value, approved);
        } else {
            error!("[Keyfire] Startup enable failed: could not write the Run entry");
        }
    } else {
        let removed = reg_delete(RUN_SUBKEY, REG_NAME);
        let _ = reg_delete(APPROVED_SUBKEY, REG_NAME);
        crate::config::set_start_with_windows(false);
        info!("[Keyfire] Startup disabled (removed={})", removed);
    }
}

pub fn set_startup_enabled(enable: bool) {
    set_startup_enabled_impl(enable);
}

/// Make the "Start with Windows" registration match reality on every boot.
///
/// 1. Entry present but pointing at another exe (pre-rebrand trigr.exe, a
///    moved install): rewrite it. Installs that enabled startup before v0.6.0
///    still pointed at trigr.exe, which the Keyfire installer never removes,
///    so every boot launched stale Trigr and re-prompted the update forever.
/// 2. Entry present and no intent recorded yet: learn ON, so a later loss of
///    the entry can be repaired.
/// 3. Entry missing while the intent is ON and Windows has not disabled it:
///    put it back. This is the "sometimes it just doesn't start" case where
///    an uninstaller, registry cleaner or a dev-session toggle removed the
///    value; the user never asked for it to go.
/// Idempotent; never touches the registry in a debug build or in demo /
/// profile mode (whose local settings are not the real machine's).
pub fn heal_startup_registration() {
    if !registry_writes_allowed() || crate::is_demo_mode() || crate::profile_mode().is_some() {
        return;
    }
    let Some(expected) = expected_run_value() else {
        return;
    };
    let intent = crate::config::get_start_with_windows();
    match read_run_value() {
        Some(current) => {
            // Windows paths are case-insensitive; don't churn the value over case.
            if !current.eq_ignore_ascii_case(&expected) {
                if write_run_entry(&expected) {
                    info!("[Keyfire] Startup Run entry re-pointed from stale path: {} -> {}", current, expected);
                } else {
                    error!("[Keyfire] Startup Run entry is stale ({}) and could not be rewritten", current);
                }
            }
            if intent.is_none() {
                crate::config::set_start_with_windows(true);
            }
        }
        None => {
            if intent != Some(true) {
                return; // never enabled here, or the user switched it off
            }
            if windows_disabled_at_startup() {
                info!("[Keyfire] Startup Run entry missing and disabled in Windows Startup apps — leaving it off");
                return;
            }
            if write_run_entry(&expected) {
                approve_startup_entry();
                info!("[Keyfire] Startup Run entry restored (was missing): {}", expected);
            } else {
                error!("[Keyfire] Startup Run entry missing and could not be restored");
            }
        }
    }
}

// ── Close-to-tray event handler ─────────────────────────────────────────────

/// Call this in the Tauri builder's `on_window_event` to intercept close.
pub fn handle_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
        // Prevent the window from being destroyed — hide to tray instead
        api.prevent_close();
        hide_window_to_tray(window.app_handle());
    }
}
