// ── WebView2 idle memory management ──────────────────────────────────────────
//
// All secondary windows are pre-created hidden at startup (lib.rs setup) and
// only ever hide/show — never destroy. That keeps show latency instant but, by
// default, leaves every hidden window fully alive: Tauri hides the HWND but
// never tells WebView2, so `controller.IsVisible` stays TRUE, the page still
// reports `document.visibilityState === 'visible'`, and Chromium keeps its
// compositor layers, rasterised tiles, GPU surfaces and rAF loops for a
// window nobody can see. Measured 2026-08-28: eight hidden pages sharing one
// renderer cost ~350 MB private in the renderer + ~140 MB in the GPU process
// with only 73 MB of actual JS heap between them.
//
// Two tiers, both driven by a ticker that polls `is_visible()`:
//
//  1. PARK (within TICK_SECS of going hidden) — `SetIsVisible(false)` +
//     `MemoryUsageTargetLevel(LOW)`. Rendering stops and its resources are
//     released; JS keeps running (timers untouched thanks to
//     `--disable-background-timer-throttling` in WEBVIEW_BROWSER_ARGS, lib.rs),
//     so windows that must keep processing broadcast events while hidden
//     (main in the tray) are safe to park. Applies to PARK_ONLY_LABELS and
//     SUSPEND_LABELS.
//
//  2. SUSPEND (after IDLE_SECS parked) — WebView2 `TrySuspend`, overlays and
//     settings only. Safe because every Rust show path re-emits a complete
//     data payload before .show() (settings: "settings-shown" → App.jsx
//     re-broadcasts "settings-state"), so a suspended window misses nothing
//     it can't recover on show. The main window is never suspended.
//
// INVARIANT: resume_for_show() MUST be called at the top of every show path,
// BEFORE the first emit to that window. It restores IsVisible(true) + NORMAL
// target and resumes a suspended webview. All .show() call sites in
// src-tauri are covered; the frontend never calls window.show() itself.
// Parking makes this invariant load-bearing on EVERY show (a parked window
// shown without resume paints nothing), not just after a 5-minute idle.
//
// Frontend consequence: `document.visibilityState` is now truthful in every
// window, so `visibilitychange` fires hidden→visible on every show (it used to
// fire only after a TrySuspend resume). The overlays' wake handlers
// (SearchOverlay / RadialMenu / ClipboardOverlay selfHealPull) are idempotent
// re-pulls and tolerate that.
//
// Race safety: state lives behind a Mutex. resume_for_show() flips the state
// to Active BEFORE queueing any COM work, and every park/suspend closure
// re-checks the state when it actually executes on the main thread — so a
// show that lands between "queued" and "executed" aborts the park/suspend
// instead of blanking a window. with_webview, emit and show all dispatch
// through the same event-loop queue, so a resume queued at the top of a show
// path always executes before that path's emit/show.
//
// The ticker additionally refuses to park a window whose resume_for_show ran
// within RESUME_GRACE: a show path is "resume → emit → show", and a tick that
// reads is_visible()==false in that gap would otherwise see state Active and
// re-park the window just before it appears.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tauri::Manager;

/// Parked this long before an overlay is fully suspended. Frequent open/close
/// cycles (Ctrl+Space bursts, clipboard popups) never hit suspension — only
/// true idle does, which is exactly when snappiness doesn't matter.
const IDLE_SECS: u64 = 300;
/// Hide-detection latency: a hidden window is parked within this many seconds.
/// The check is one IsWindowVisible call per window per tick.
const TICK_SECS: u64 = 5;
/// A window resumed this recently is assumed to be mid-show; never park it.
const RESUME_GRACE: Duration = Duration::from_secs(3);

/// Refresh-on-show windows — parked when hidden, TrySuspend after IDLE_SECS.
/// "countdown" is intentionally NOT here — the countdown's React state is
/// driven by a reset event delivered at show time, and TrySuspend's
/// resume/IPC-reconnect race can lose that event (phase stays 'idle' →
/// no 3-2-1 → no recorder::start). The other overlays receive full data
/// payloads on show and recover cleanly from suspend; the countdown does
/// not. It is left entirely alone (not parked either): it is 14 DOM nodes.
const SUSPEND_LABELS: [&str; 6] =
    ["overlay", "fillin", "clipboardoverlay", "radialmenu", "settings", "snipoverlay"];
/// Parked when hidden, never suspended — must keep running JS while hidden
/// (see module docs).
const PARK_ONLY_LABELS: [&str; 1] = ["main"];

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    /// Visible, or resumed ahead of an imminent show.
    Active,
    /// Park closure queued to the main thread but not yet executed.
    Parking,
    /// Hidden: IsVisible(false) + LOW target applied; idle countdown running.
    Parked,
    /// Suspend closure queued to the main thread but not yet executed.
    Suspending,
    /// TrySuspend issued.
    Suspended,
}

struct WinMem {
    state: State,
    hidden_since: Option<Instant>,
    last_resume: Option<Instant>,
}

impl WinMem {
    fn fresh() -> Self {
        WinMem {
            state: State::Active,
            hidden_since: None,
            last_resume: None,
        }
    }
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
                "[MEM] Webview memory manager started (park within {}s of hide, suspend after {}s)",
                TICK_SECS,
                IDLE_SECS
            );
            loop {
                std::thread::sleep(Duration::from_secs(TICK_SECS));
                for label in SUSPEND_LABELS {
                    tick_window(&app, label, true);
                }
                for label in PARK_ONLY_LABELS {
                    tick_window(&app, label, false);
                }
            }
        });
}

/// Resume a window ahead of showing it (or emitting to it). Must be called at
/// the top of every show path, before the first emit to that window. Cheap
/// no-op when the window is already active.
pub fn resume_for_show(app: &tauri::AppHandle, label: &str) {
    let prev = {
        let mut reg = registry().lock().unwrap();
        let entry = reg.entry(label.to_string()).or_insert_with(WinMem::fresh);
        let prev = entry.state;
        entry.state = State::Active;
        entry.hidden_since = None;
        entry.last_resume = Some(Instant::now());
        prev
    };
    if prev == State::Active {
        return;
    }
    let Some(win) = app.get_webview_window(label) else {
        return;
    };
    let was_suspended = matches!(prev, State::Suspending | State::Suspended);
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
                if was_suspended {
                    if let Ok(wv3) = core.cast::<ICoreWebView2_3>() {
                        // No-op (Err) when the webview was never actually suspended.
                        let _ = wv3.Resume();
                    }
                }
                if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                    let _ = wv19
                        .SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL);
                }
            }
            // Restore controller visibility taken away by the park path.
            let _ = controller.SetIsVisible(true);
            if was_suspended {
                log::info!(
                    "[MEM] {} resumed from suspend in {}ms",
                    label_owned,
                    started.elapsed().as_millis()
                );
            } else {
                log::debug!("[MEM] {} unparked", label_owned);
            }
        }
    });
}

fn tick_window(app: &tauri::AppHandle, label: &str, may_suspend: bool) {
    let Some(win) = app.get_webview_window(label) else {
        return;
    };
    // Called only from the ticker thread — never the event loop — so the
    // blocking visibility getter is safe here.
    let visible = win.is_visible().unwrap_or(true);
    enum Action {
        None,
        Park,
        Suspend,
    }
    let action = {
        let mut reg = registry().lock().unwrap();
        let entry = reg.entry(label.to_string()).or_insert_with(WinMem::fresh);
        if visible {
            entry.state = State::Active;
            entry.hidden_since = None;
            Action::None
        } else {
            match entry.state {
                State::Active => {
                    let mid_show = entry
                        .last_resume
                        .map(|t| t.elapsed() < RESUME_GRACE)
                        .unwrap_or(false);
                    if mid_show {
                        Action::None
                    } else {
                        entry.state = State::Parking;
                        entry.hidden_since = Some(Instant::now());
                        Action::Park
                    }
                }
                State::Parked if may_suspend => {
                    let idle_elapsed = entry
                        .hidden_since
                        .map(|t| t.elapsed() >= Duration::from_secs(IDLE_SECS))
                        .unwrap_or(false);
                    if idle_elapsed {
                        entry.state = State::Suspending;
                        Action::Suspend
                    } else {
                        Action::None
                    }
                }
                State::Parking | State::Parked | State::Suspending | State::Suspended => {
                    Action::None
                }
            }
        }
    };
    match action {
        Action::None => {}
        Action::Park => queue_park(&win, label),
        Action::Suspend => queue_suspend(&win, label),
    }
}

/// Main-thread state check shared by the park/suspend closures: advance
/// `expect` → `next` and return true, or return false if a resume raced in.
fn advance_state(label: &str, expect: State, next: State) -> bool {
    let mut reg = registry().lock().unwrap();
    let Some(entry) = reg.get_mut(label) else {
        return false;
    };
    if entry.state != expect {
        return false;
    }
    entry.state = next;
    true
}

fn queue_park(win: &tauri::WebviewWindow, label: &str) {
    let label_owned = label.to_string();
    let _ = win.with_webview(move |webview| {
        // Re-check on the main thread: a show may have raced in after this
        // closure was queued. resume_for_show flips the state to Active before
        // queueing any of its own COM work, so this read is authoritative.
        if !advance_state(&label_owned, State::Parking, State::Parked) {
            return;
        }
        unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::{
                ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
            };
            use windows_core::Interface;
            let controller = webview.controller();
            if let Ok(core) = controller.CoreWebView2() {
                if let Ok(wv19) = core.cast::<ICoreWebView2_19>() {
                    let _ =
                        wv19.SetMemoryUsageTargetLevel(COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW);
                }
            }
            // The window HWND is already hidden, so this has no visual effect;
            // it lets Chromium drop the page's rendering resources.
            // resume_for_show restores it before any show.
            let _ = controller.SetIsVisible(false);
            log::debug!("[MEM] {} parked", label_owned);
        }
    });
}

fn queue_suspend(win: &tauri::WebviewWindow, label: &str) {
    let label_owned = label.to_string();
    let _ = win.with_webview(move |webview| {
        if !advance_state(&label_owned, State::Suspending, State::Suspended) {
            return;
        }
        unsafe {
            use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_3;
            use webview2_com::TrySuspendCompletedHandler;
            use windows_core::Interface;
            let controller = webview.controller();
            let Ok(core) = controller.CoreWebView2() else {
                return;
            };
            // TrySuspend requires the controller to report not-visible. The
            // park step already did this; repeat defensively.
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
                        "[MEM] {} ICoreWebView2_3 unavailable — parked only",
                        label_owned
                    );
                }
            }
        }
    });
}
