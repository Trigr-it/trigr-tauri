//! Non-Windows twin of foreground.rs. The real foreground.rs polls
//! GetForegroundWindow for app-profile auto-switching.
//!
//! Mac port Phase 2, milestone "foreground" (`port/mac-hooks`): on macOS this
//! is now the real watcher — a poll thread reads
//! NSWorkspace.frontmostApplication every 1.5s (same cadence as Windows) and
//! runs the same auto-switch decision chain: self-focus guard → editor guard
//! → recorder guards → linked-profile match → Pro gating → snap-back to the
//! manually selected global profile. Profile switches go through
//! hotkeys::set_active_profile, which also rebuilds the tap's suppress set.
//!
//! Differences from Windows, deliberate for this milestone:
//!   * `linkedWindowTitle` filters are ignored — reading other apps' window
//!     titles on macOS needs the Screen Recording permission
//!     (CGWindowListCopyWindowInfo). Profiles with a title filter match on
//!     the app alone; a warn is logged once so the behaviour is visible.
//!   * No fullscreen-game mouse-hook pause (no mouse hook on mac yet) and no
//!     cursor-over-linked-app PID cache (backs a hook feature that doesn't
//!     exist here yet).
//!
//! Matching currency: Windows configs store `linkedApp` as a path and match
//! on the lowercase file stem ("photoshop.exe" → "photoshop"). The mac
//! watcher matches that same stem against the frontmost app's executable
//! stem, bundle stem ("Safari.app" → "safari") and localizedName, so configs
//! written on either OS behave sensibly.
#![allow(dead_code, unused_variables)]

use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::AppHandle;

static WATCHER_RUNNING: AtomicBool = AtomicBool::new(false);

/// True while the user has an editor surface open (mapping right-panel,
/// expansion add/edit form, quick action edit form, radial segment edit).
/// When set, the watcher does not auto-switch profiles. Pushed from the
/// frontend via `set_editing_active`.
static EDITING_ACTIVE: AtomicBool = AtomicBool::new(false);

struct FgState {
    current_fg_proc: String,
    active_global_profile: String,
    profile_settings: HashMap<String, Value>,
    self_proc_names: Vec<String>,
}

impl Default for FgState {
    fn default() -> Self {
        let mut self_names = vec!["trigr".to_string(), "keyfire".to_string()];
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

static FG_STATE: OnceLock<Mutex<FgState>> = OnceLock::new();

fn fg_state() -> &'static Mutex<FgState> {
    FG_STATE.get_or_init(|| Mutex::new(FgState::default()))
}

pub fn force_check(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    {
        macos::poll_once(app, true);
    }
}

pub fn get_current_fg_proc() -> String {
    fg_state()
        .lock()
        .map(|s| s.current_fg_proc.clone())
        .unwrap_or_default()
}

pub fn set_active_global_profile(profile: String) {
    if let Ok(mut s) = fg_state().lock() {
        s.active_global_profile = profile;
    }
}

pub fn set_editing_active(active: bool) {
    EDITING_ACTIVE.store(active, Ordering::SeqCst);
}

pub fn start_watcher(app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        macos::start_watcher(app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        log::warn!("[stub] foreground watcher is not available on this platform yet");
    }
}

pub fn update_profile_settings(settings: HashMap<String, Value>) {
    if let Ok(mut s) = fg_state().lock() {
        s.profile_settings = settings;
    }
}

/// PID of the frontmost app, for overlay focus hand-back (lib.rs captures it
/// before showing a focus-stealing overlay and re-activates it on hide —
/// the mac analogue of the Windows OVERLAY_TARGET_HWND round-trip). 0 = none.
#[cfg(target_os = "macos")]
pub(crate) fn capture_frontmost_pid() -> i32 {
    macos::frontmost_pid()
}

/// Re-activate the app with the given PID (no-op for 0 / vanished apps).
#[cfg(target_os = "macos")]
pub(crate) fn activate_pid(pid: i32) {
    macos::activate_pid(pid);
}

// ── macOS NSWorkspace watcher ────────────────────────────────────────────────
#[cfg(target_os = "macos")]
mod macos {
    use super::{fg_state, EDITING_ACTIVE, WATCHER_RUNNING};
    use log::{info, warn};
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSWorkspace;
    use std::path::Path;
    use std::sync::atomic::Ordering;
    use std::sync::OnceLock;
    use std::thread;
    use std::time::Duration;
    use tauri::{AppHandle, Emitter};

    const POLL_INTERVAL_MS: u64 = 1500;

    pub(super) fn start_watcher(app: AppHandle) {
        if WATCHER_RUNNING.load(Ordering::SeqCst) {
            return;
        }
        WATCHER_RUNNING.store(true, Ordering::SeqCst);

        thread::Builder::new()
            .name("keyfire-fg-watcher".to_string())
            .spawn(move || {
                info!(
                    "[Keyfire] Foreground watcher started (NSWorkspace, {}ms poll)",
                    POLL_INTERVAL_MS
                );
                while WATCHER_RUNNING.load(Ordering::SeqCst) {
                    poll_once(&app, false);
                    thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
                }
                info!("[Keyfire] Foreground watcher stopped");
            })
            .expect("Failed to spawn foreground watcher thread");
    }

    /// One poll tick. `force` skips the change-detection short-circuit (used
    /// by force_check after profile edits). Wrapped in an autorelease pool —
    /// the ObjC calls autorelease intermediates and this thread has no pool
    /// of its own.
    pub(super) fn poll_once(app: &AppHandle, force: bool) {
        let Some(names) = frontmost_app_names() else { return };

        // Change detection: primary name (executable stem) vs last seen.
        static LAST_FG: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
        let last = LAST_FG.get_or_init(|| std::sync::Mutex::new(String::new()));
        if let Ok(mut l) = last.lock() {
            if !force && *l == names[0] {
                return;
            }
            *l = names[0].clone();
        }

        handle_foreground_change(&names, app);
    }

    /// Lowercase name candidates of the frontmost app that we match linkedApp
    /// stems against (executable stem, bundle stem, display name). Wrapped in
    /// an autorelease pool — the ObjC calls autorelease intermediates and the
    /// watcher thread has no pool of its own.
    pub(super) fn frontmost_app_names() -> Option<Vec<String>> {
        autoreleasepool(|_| {
            let ws = NSWorkspace::sharedWorkspace();
            let app = ws.frontmostApplication()?;
            let mut names = Vec::new();
            if let Some(url) = app.executableURL() {
                if let Some(path) = url.path() {
                    if let Some(stem) = Path::new(&path.to_string()).file_stem() {
                        names.push(stem.to_string_lossy().to_lowercase());
                    }
                }
            }
            if let Some(url) = app.bundleURL() {
                if let Some(path) = url.path() {
                    if let Some(stem) = Path::new(&path.to_string()).file_stem() {
                        names.push(stem.to_string_lossy().to_lowercase());
                    }
                }
            }
            if let Some(name) = app.localizedName() {
                names.push(name.to_string().to_lowercase());
            }
            names.dedup();
            if names.is_empty() {
                None
            } else {
                Some(names)
            }
        })
    }

    /// PID of the frontmost app (0 if none). See capture_frontmost_pid.
    pub(super) fn frontmost_pid() -> i32 {
        autoreleasepool(|_| {
            NSWorkspace::sharedWorkspace()
                .frontmostApplication()
                .map(|a| a.processIdentifier())
                .unwrap_or(0)
        })
    }

    /// Bring the app with `pid` back to front. Used after hiding a
    /// focus-stealing overlay so the user's target app regains key focus
    /// before any synthetic paste lands.
    pub(super) fn activate_pid(pid: i32) {
        if pid == 0 {
            return;
        }
        autoreleasepool(|_| {
            use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
            if let Some(app) = NSRunningApplication::runningApplicationWithProcessIdentifier(pid) {
                app.activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
            }
        });
    }

    /// Twin of the Windows handle_foreground_change decision chain.
    fn handle_foreground_change(names: &[String], app: &AppHandle) {
        let primary = names[0].clone();

        let mut state = fg_state().lock().unwrap();
        state.current_fg_proc = primary.clone();

        // Never auto-switch when Keyfire itself is focused.
        if state
            .self_proc_names
            .iter()
            .any(|s| names.iter().any(|n| n == s))
        {
            return;
        }

        // Editor open → user may be testing in another app; don't snap away.
        if EDITING_ACTIVE.load(Ordering::SeqCst) {
            return;
        }

        // Recorder flow / active recording → a switch would tear down the
        // recording UI (same gates as Windows; statics live in shared
        // recorder.rs).
        if crate::recorder::RECORDER_FLOW_ACTIVE.load(Ordering::SeqCst)
            || crate::recorder::IS_RECORDING_MACRO.load(Ordering::SeqCst)
        {
            return;
        }

        // Linked profiles: (profile, app stem, has_title_filter).
        let linked: Vec<(String, String, bool)> = state
            .profile_settings
            .iter()
            .filter_map(|(profile, settings)| {
                settings.get("linkedApp").and_then(|v| v.as_str()).map(|app_path| {
                    let app_name = Path::new(app_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_lowercase())
                        .unwrap_or_default();
                    let has_title = settings
                        .get("linkedWindowTitle")
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.is_empty());
                    (profile.clone(), app_name, has_title)
                })
            })
            .collect();

        let matched = linked
            .iter()
            .find(|(_, app_name, has_title)| {
                let hit = names.iter().any(|n| n == app_name);
                if hit && *has_title {
                    // Title filters need the Screen Recording permission on
                    // macOS — not wired yet; match on app alone.
                    static WARNED: std::sync::atomic::AtomicBool =
                        std::sync::atomic::AtomicBool::new(false);
                    if !WARNED.swap(true, Ordering::SeqCst) {
                        warn!(
                            "[Keyfire] linkedWindowTitle filters are ignored on macOS for \
                             now — matching on the app alone"
                        );
                    }
                }
                hit
            })
            .map(|(profile, _, _)| profile.clone());

        // Pro users: matched linked profile or snap back to the manual global
        // profile. Free users: always the global profile (preserves Pro
        // gating; snap-back still works).
        let target = if crate::licence::is_pro() {
            matched.unwrap_or_else(|| state.active_global_profile.clone())
        } else {
            state.active_global_profile.clone()
        };

        let current_profile = crate::hotkeys::engine_state()
            .lock()
            .map(|s| s.active_profile.clone())
            .unwrap_or_default();

        if target != current_profile {
            info!(
                "[Keyfire] Auto-switched to profile \"{}\" (foreground: {})",
                target, primary
            );
            // Release any held/repeating key before switching (no-ops until
            // the hold/repeat machinery lands on mac).
            crate::actions::release_held_key();
            crate::actions::stop_repeating_key();

            // Updates the matcher AND rebuilds the tap suppress set.
            crate::hotkeys::set_active_profile(target.clone());

            drop(state); // release lock before emit
            let _ = app.emit("profile-switched", serde_json::json!({ "profile": target }));
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    /// Exercises the real NSWorkspace bridge: whatever app is frontmost while
    /// the test runs (the terminal, an IDE) must yield at least one lowercase
    /// name candidate.
    #[test]
    fn frontmost_app_names_resolves() {
        let names = super::macos::frontmost_app_names()
            .expect("frontmost app should resolve to name candidates");
        assert!(!names.is_empty());
        for n in &names {
            assert_eq!(n, &n.to_lowercase(), "candidates must be lowercase");
        }
    }
}
