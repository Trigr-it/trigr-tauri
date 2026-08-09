// ── WebView2 idle memory management ──────────────────────────────────────────
//
// All five windows are pre-created hidden at startup (lib.rs setup) and only
// ever hide/show — never destroy. That keeps show latency instant but leaves
// every hidden window holding its full renderer process and GPU surfaces.
// This module reclaims that memory once a window has been hidden for a while:
//
//  - Overlay windows (overlay / fillin / clipboardoverlay / radialmenu) and
//    the settings window get a full WebView2 TrySuspend. Safe because every
//    Rust show path re-emits a complete data payload before .show() (settings:
//    "settings-shown" → App.jsx re-broadcasts "settings-state") — a suspended
//    window misses nothing
//    it can't recover on show. INVARIANT: resume_for_show() MUST be called at
//    the top of every show path, BEFORE the first emit to that window. All
//    .show() call sites in src-tauri are covered; the frontend never calls
//    window.show() itself.
//
//  - The main window only gets MemoryUsageTargetLevel(LOW) (cache trim — JS
//    keeps running). It must keep processing broadcast events while hidden in
//    the tray (engine-status, profile-switched, config-reloaded-from-sync,
//    clipboard history updates), so it is never suspended.
//
// Race safety: state lives behind a Mutex. resume_for_show() flips the state
// to Active BEFORE queueing any COM work, and the suspend closure re-checks
// the state when it actually executes on the main thread — so a show that
// lands between "suspend queued" and "suspend executed" aborts the suspend
// instead of blanking a window. with_webview, emit and show all dispatch
// through the same event-loop queue, so a resume queued at the top of a show
// path always executes before that path's emit/show.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::Manager;

/// Hidden this long before a window is trimmed/suspended. Frequent open/close
/// cycles (Ctrl+Space bursts, clipboard popups) never hit suspension — only
/// true idle does, which is exactly when snappiness doesn't matter.
const IDLE_SECS: u64 = 300;
const TICK_SECS: u64 = 30;

/// Refresh-on-show windows — full TrySuspend when idle-hidden.
/// "countdown" is intentionally NOT here — the countdown's React state is
/// driven by a reset event delivered at show time, and TrySuspend's
/// resume/IPC-reconnect race can lose that event (phase stays 'idle' →
/// no 3-2-1 → no recorder::start). The other overlays receive full data
/// payloads on show and recover cleanly from suspend; the countdown does
/// not. Memory cost is ~10MB shared via the WebView2 process — worth it
/// for reliability.
const SUSPEND_LABELS: [&str; 5] =
    ["overlay", "fillin", "clipboardoverlay", "radialmenu", "settings"];
/// Cache-trim only — must keep running JS while hidden (see module docs).
const TRIM_ONLY_LABELS: [&str; 1] = ["main"];

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    /// Visible, or resumed ahead of an imminent show.
    Active,
    /// Hidden, idle countdown running.
    HiddenPending,
    /// Trim/suspend closure queued to the main thread but not yet executed.
    Trimming,
    /// Suspended (overlays) or LOW memory target applied (main).
    Trimmed,
}

struct WinMem {
    state: State,
    hidden_since: Option<Instant>,
}

fn registry() -> &'static Mutex<HashMap<String, WinMem>> {
    static REG: OnceLock<Mutex<HashMap<String, WinMem>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Spawn the idle ticker thread. Call once from setup, after all windows exist.
pub fn start(app: tauri::AppHandle) {
    let _ = std::thread::Builder::new()
        .name("trigr-webview-mem".into())
        .spawn(move || {
            log::info!(
                "[MEM] Idle webview memory manager started (idle={}s, tick={}s)",
                IDLE_SECS,
                TICK_SECS
            );
            loop {
                std::thread::sleep(Duration::from_secs(TICK_SECS));
                for label in SUSPEND_LABELS {
                    tick_window(&app, label, true);
                }
                for label in TRIM_ONLY_LABELS {
                    tick_window(&app, label, false);
                }
            }
        });
}

/// Resume a window ahead of showing it (or emitting to it). Must be called at
/// the top of every show path, before the first emit to that window. Cheap
/// no-op when the window was never trimmed.
pub fn resume_for_show(app: &tauri::AppHandle, label: &str) {
    let needs_com = {
        let mut reg = registry().lock().unwrap();
        let entry = reg.entry(label.to_string()).or_insert(WinMem {
            state: State::Active,
            hidden_since: None,
        });
        let needs = matches!(entry.state, State::Trimming | State::Trimmed);
        entry.state = State::Active;
        entry.hidden_since = None;
        needs
    };
    if !needs_com {
        return;
    }
    let Some(win) = app.get_webview_window(label) else {
        return;
    };
    let label_owned = label.to_string();
    let started = Instant::now();
    let _ = win.with_webview(move |webview| {
        unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::{
                ICoreWebView2_19, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
            };
            use windows_core::Interface;
            let controller = webview.controller();
            if let Ok(core) = controller.CoreWebView2() {
                if let Ok(wv3) = core.cast::<ICoreWebView2_3>() {
                    // No-op (Err) when the webview was never actually suspended.
                    let _ = wv3.Resume();
                }
                if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                    let _ = wv19
                        .SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL);
                }
            }
            // Restore controller visibility taken away by the suspend path.
            let _ = controller.SetIsVisible(true);
            log::info!(
                "[MEM] {} resumed in {}ms",
                label_owned,
                started.elapsed().as_millis()
            );
        }
    });
}

fn tick_window(app: &tauri::AppHandle, label: &str, full_suspend: bool) {
    let Some(win) = app.get_webview_window(label) else {
        return;
    };
    // Called only from the ticker thread — never the event loop — so the
    // blocking visibility getter is safe here.
    let visible = win.is_visible().unwrap_or(true);
    let should_trim = {
        let mut reg = registry().lock().unwrap();
        let entry = reg.entry(label.to_string()).or_insert(WinMem {
            state: State::Active,
            hidden_since: None,
        });
        if visible {
            entry.state = State::Active;
            entry.hidden_since = None;
            false
        } else {
            match entry.state {
                State::Active => {
                    entry.state = State::HiddenPending;
                    entry.hidden_since = Some(Instant::now());
                    false
                }
                State::HiddenPending => {
                    let idle_elapsed = entry
                        .hidden_since
                        .map(|t| t.elapsed() >= Duration::from_secs(IDLE_SECS))
                        .unwrap_or(false);
                    if idle_elapsed {
                        entry.state = State::Trimming;
                    }
                    idle_elapsed
                }
                State::Trimming | State::Trimmed => false,
            }
        }
    };
    if should_trim {
        queue_trim(&win, label, full_suspend);
    }
}

fn queue_trim(win: &tauri::WebviewWindow, label: &str, full_suspend: bool) {
    let label_owned = label.to_string();
    let _ = win.with_webview(move |webview| {
        // Re-check on the main thread: a show may have raced in after this
        // closure was queued. resume_for_show flips the state to Active before
        // queueing any of its own COM work, so this read is authoritative.
        {
            let mut reg = registry().lock().unwrap();
            let Some(entry) = reg.get_mut(&label_owned) else {
                return;
            };
            if entry.state != State::Trimming {
                return;
            }
            entry.state = State::Trimmed;
        }
        unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::{
                ICoreWebView2_19, ICoreWebView2_3, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
            };
            use webview2_com::TrySuspendCompletedHandler;
            use windows_core::Interface;
            let controller = webview.controller();
            let Ok(core) = controller.CoreWebView2() else {
                return;
            };
            if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                let _ =
                    wv19.SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
            }
            if !full_suspend {
                log::info!("[MEM] {} memory target set LOW (trim only)", label_owned);
                return;
            }
            // TrySuspend requires the controller to report not-visible. The
            // window HWND is already hidden, so this has no visual effect;
            // resume_for_show restores it before any show.
            let _ = controller.SetIsVisible(false);
            match core.cast::<ICoreWebView2_3>() {
                Ok(wv3) => {
                    let lbl = label_owned.clone();
                    let handler = TrySuspendCompletedHandler::create(Box::new(
                        move |hr, is_successful| {
                            match (hr, is_successful) {
                                (Ok(()), true) => log::info!("[MEM] {} suspended", lbl),
                                (Ok(()), false) => {
                                    log::warn!("[MEM] {} suspend declined by WebView2", lbl)
                                }
                                (Err(e), _) => log::warn!("[MEM] {} suspend failed: {}", lbl, e),
                            }
                            Ok(())
                        },
                    ));
                    if let Err(e) = wv3.TrySuspend(&handler) {
                        log::warn!("[MEM] {} TrySuspend call failed: {}", label_owned, e);
                    }
                }
                Err(_) => {
                    log::info!(
                        "[MEM] {} ICoreWebView2_3 unavailable — trim only",
                        label_owned
                    );
                }
            }
        }
    });
}
