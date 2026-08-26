//! WinRT-based voice recognition for Keyfire voice commands.
//!
//! Uses Windows.Media.SpeechRecognition with SpeechRecognitionListConstraint
//! for offline, grammar-constrained phrase matching.  100% local — no cloud.
//!
//! Two recognition modes share ONE cached SpeechRecognizer instance:
//!   - Single-shot via RecognizeAsync (tap voice hotkey)
//!   - Continuous via SpeechContinuousRecognitionSession (pill click in overlay)
//!
//! The cached recognizer is rebuilt only when the phrase list hash changes
//! (CachedRecognizer.phrase_hash). Invalidation happens in lib.rs at the end
//! of update_assignments, set_active_global_profile, and save_config.

use log::{error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager};
use windows_core::Interface;

// ── State ──────────────────────────────────────────────────────────────────

static RECOGNIZING: AtomicBool = AtomicBool::new(false);

/// Gate for continuous session start/stop — prevents double-start.
static CONTINUOUS_RUNNING: AtomicBool = AtomicBool::new(false);

/// Set when prewarm is requested while recognition is in flight. Consumed by
/// the recognition-completion paths (run_recognition's thread and the
/// SpeechContinuousRecognitionSession Completed handler) to re-fire prewarm
/// after the active session ends. Prevents the WinRT-level cancellation that
/// happens when CompileConstraintsAsync runs concurrently with RecognizeAsync.
static REWARM_PENDING: AtomicBool = AtomicBool::new(false);

/// Shared recognizer handle — allows stop_recognition() to cancel from another thread.
static ACTIVE_RECOGNIZER: OnceLock<Mutex<Option<windows::Media::SpeechRecognition::SpeechRecognizer>>> =
    OnceLock::new();

fn active_recognizer() -> &'static Mutex<Option<windows::Media::SpeechRecognition::SpeechRecognizer>> {
    ACTIVE_RECOGNIZER.get_or_init(|| Mutex::new(None))
}

/// Active SpeechContinuousRecognitionSession (when continuous mode is running).
/// Held separately so stop_continuous_recognition() can cancel without touching
/// the cached recognizer.
static ACTIVE_CONTINUOUS: OnceLock<Mutex<Option<windows::Media::SpeechRecognition::SpeechContinuousRecognitionSession>>> =
    OnceLock::new();

fn active_continuous() -> &'static Mutex<Option<windows::Media::SpeechRecognition::SpeechContinuousRecognitionSession>> {
    ACTIVE_CONTINUOUS.get_or_init(|| Mutex::new(None))
}

/// Cached compiled recognizer + the hash of the phrase list it was built from.
/// Shared by single-shot and continuous paths so we never re-compile the constraint
/// unless the phrase list actually changed.
///
/// `state_subscribed`: whether SpeechRecognizer::StateChanged has been wired up
/// for this recognizer instance. Subscribed lazily on first use that has an
/// AppHandle (prewarm builds with None so the flag starts false; first real
/// recognition will subscribe). Reset to false on cache rebuild so the new
/// instance gets its own subscription.
struct CachedRecognizer {
    recognizer: windows::Media::SpeechRecognition::SpeechRecognizer,
    phrase_hash: u64,
    state_subscribed: bool,
}

static CACHED_RECOGNIZER: OnceLock<Mutex<Option<CachedRecognizer>>> = OnceLock::new();

fn cached_recognizer() -> &'static Mutex<Option<CachedRecognizer>> {
    CACHED_RECOGNIZER.get_or_init(|| Mutex::new(None))
}

fn phrase_hash(phrases: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut sorted: Vec<&str> = phrases.iter().map(|s| s.as_str()).collect();
    sorted.sort();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in &sorted {
        p.hash(&mut h);
    }
    h.finish()
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Start voice recognition with a list of phrases to match against.
/// Runs on a background thread. Emits "voice-result" or "voice-error" to the overlay.
pub fn start_recognition(phrases: Vec<String>, app: AppHandle) {
    if RECOGNIZING.swap(true, Ordering::SeqCst) {
        warn!("[Voice] Recognition already running, ignoring start");
        return;
    }

    if let Err(e) = std::thread::Builder::new()
        .name("trigr-voice".to_string())
        .spawn(move || {
            let result = run_recognition(&phrases, &app);
            RECOGNIZING.store(false, Ordering::SeqCst);

            // Clear the in-flight handle (cache survives)
            if let Ok(mut guard) = active_recognizer().lock() {
                *guard = None;
            }

            match result {
                Ok(Some(text)) => {
                    info!("[Voice] Recognised: \"{}\"", text);
                    if let Some(overlay) = app.get_webview_window("overlay") {
                        let _ = overlay.emit("voice-result", serde_json::json!({
                            "text": text,
                        }));
                    }
                }
                Ok(None) => {
                    info!("[Voice] No speech / cancelled");
                    if let Some(overlay) = app.get_webview_window("overlay") {
                        let _ = overlay.emit("voice-error", serde_json::json!({
                            "error": "no-speech",
                        }));
                    }
                }
                Err(e) => {
                    error!("[Voice] Recognition failed: {}", e);
                    if let Some(overlay) = app.get_webview_window("overlay") {
                        let _ = overlay.emit("voice-error", serde_json::json!({
                            "error": e,
                        }));
                    }
                }
            }

            // Consume deferred prewarm if a config/profile change came in during
            // recognition. Safe now because RECOGNIZING was cleared above.
            if REWARM_PENDING.swap(false, Ordering::SeqCst) {
                prewarm_from_state();
            }
        })
    {
        RECOGNIZING.store(false, Ordering::SeqCst);
        error!("[Voice] Failed to spawn recognition thread: {}", e);
    }
}

/// Stop any ongoing recognition by cancelling the WinRT recognizer.
pub fn stop_recognition() {
    if let Ok(guard) = active_recognizer().lock() {
        if let Some(ref recognizer) = *guard {
            info!("[Voice] Stopping recognition...");
            let _ = recognizer.StopRecognitionAsync();
        }
    }
}

/// Start continuous voice recognition. The cached SpeechRecognizer (shared with
/// single-shot RecognizeAsync) is reused — no separate compile. ResultGenerated
/// events fire on a WinRT thread and emit "voice-result" to the overlay window.
/// Errors (mic in use, permission denied, device unavailable, Completed with
/// non-Success status) emit "voice-error" and clear CONTINUOUS_RUNNING.
pub fn start_continuous_recognition(phrases: Vec<String>, app: AppHandle) {
    if CONTINUOUS_RUNNING.swap(true, Ordering::SeqCst) {
        warn!("[Voice] Continuous session already running, ignoring start");
        return;
    }

    let _ = std::thread::Builder::new()
        .name("trigr-voice-continuous".to_string())
        .spawn(move || {
            if let Err(e) = start_continuous_inner(phrases, app.clone()) {
                error!("[Voice] Continuous start failed: {}", e);
                CONTINUOUS_RUNNING.store(false, Ordering::SeqCst);
                if let Some(overlay) = app.get_webview_window("overlay") {
                    let _ = overlay.emit("voice-error", serde_json::json!({ "error": e }));
                }
            }
        });
}

/// Stop the active continuous session. Safe to call when none is running.
pub fn stop_continuous_recognition() {
    if let Ok(guard) = active_continuous().lock() {
        if let Some(ref session) = *guard {
            info!("[Voice] Stopping continuous session...");
            let _ = session.StopAsync();
        }
    }
    CONTINUOUS_RUNNING.store(false, Ordering::SeqCst);
}

/// Pre-warm the cached recognizer with the given phrase list. Returns immediately;
/// the constraint compile happens on a background thread. If the cached hash already
/// matches, this is a no-op.
///
/// CRITICAL: skipped when recognition is in flight. Running CompileConstraintsAsync
/// on a fresh SpeechRecognizer while another RecognizeAsync is active causes the
/// Windows speech subsystem to cancel the active session with UserCanceled (status 5).
/// When that happens, REWARM_PENDING is set; the recognition-completion paths
/// (run_recognition thread tail + continuous Completed handler) re-fire prewarm.
pub fn prewarm(phrases: Vec<String>) {
    if phrases.is_empty() {
        return;
    }
    if RECOGNIZING.load(Ordering::SeqCst) || CONTINUOUS_RUNNING.load(Ordering::SeqCst) {
        REWARM_PENDING.store(true, Ordering::SeqCst);
        return;
    }
    let h = phrase_hash(&phrases);
    // Quick check — already cached at this hash?
    if let Ok(guard) = cached_recognizer().lock() {
        if let Some(ref cached) = *guard {
            if cached.phrase_hash == h {
                return;
            }
        }
    }
    let _ = std::thread::Builder::new()
        .name("trigr-voice-prewarm".to_string())
        .spawn(move || match build_recognizer(&phrases) {
            Ok(rec) => {
                if let Ok(mut guard) = cached_recognizer().lock() {
                    // Race: another thread may have populated the cache. Only overwrite
                    // if our hash is different from what's currently cached.
                    let should_write = match &*guard {
                        Some(c) => c.phrase_hash != h,
                        None => true,
                    };
                    if should_write {
                        *guard = Some(CachedRecognizer {
                            recognizer: rec,
                            phrase_hash: h,
                            state_subscribed: false,
                        });
                        info!("[Voice] Pre-warmed recognizer: {} phrases", phrases.len());
                    }
                }
            }
            Err(e) => warn!("[Voice] Pre-warm failed: {}", e),
        });
}

/// Collect voice phrases from the current engine state. Mirrors the frontend
/// buildItems filter: active profile assignments + GLOBAL expansions + GLOBAL
/// quick actions. Reads both voicePhrases (array) and legacy voicePhrase (string).
pub fn collect_voice_phrases_from_state() -> Vec<String> {
    let state = crate::hotkeys::engine_state_lock();
    let active_profile = state.active_profile.clone();
    let profile_prefix = format!("{}::", active_profile);
    let mut phrases = Vec::new();

    for (key, value) in state.assignments.iter() {
        let include = key.starts_with(&profile_prefix)
            || key.starts_with("GLOBAL::EXPANSION::")
            || key.starts_with("GLOBAL::QUICKACTION::");
        if !include {
            continue;
        }
        // Unassigned library entries ("{Profile}::UNASSIGNED::{uuid}") have no
        // trigger and must not fire by voice either — deliberate exclusion.
        if key.contains("::UNASSIGNED::") {
            continue;
        }

        let data = value.get("data");
        // voicePhrases array takes precedence
        if let Some(arr) = data.and_then(|d| d.get("voicePhrases")).and_then(|v| v.as_array()) {
            let mut had_any = false;
            for p in arr {
                if let Some(s) = p.as_str() {
                    let t = s.trim();
                    if !t.is_empty() {
                        phrases.push(t.to_string());
                        had_any = true;
                    }
                }
            }
            if had_any {
                continue;
            }
        }
        // Legacy single string fallback
        if let Some(s) = data.and_then(|d| d.get("voicePhrase")).and_then(|v| v.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                phrases.push(t.to_string());
            }
        }
    }

    phrases
}

/// Pre-warm from the current engine state. Safe to call from any thread.
pub fn prewarm_from_state() {
    let phrases = collect_voice_phrases_from_state();
    prewarm(phrases);
}

// ── Internal ───────────────────────────────────────────────────────────────

/// Build a fresh SpeechRecognizer with the given phrase list and compile the
/// constraint. Does NOT install in any cache — caller decides.
fn build_recognizer(phrases: &[String]) -> Result<windows::Media::SpeechRecognition::SpeechRecognizer, String> {
    use windows::core::HSTRING;
    use windows::Media::SpeechRecognition::*;

    if phrases.is_empty() {
        return Err("No voice phrases configured".to_string());
    }

    let recognizer = SpeechRecognizer::new()
        .map_err(|e| format!("Failed to create SpeechRecognizer: {}", e))?;

    // Dual-layer timeout strategy: WinRT InitialSilenceTimeout fires when audio
    // frames arrive but the user stays silent. It does NOT fire if audio frames
    // never arrive (Bluetooth mid-session dropout, exclusive mic capture by another
    // app, OS-level permission revocation during recognition, USB driver hang).
    // For those edge cases a JS-side backstop timer in SearchOverlay.jsx force-stops
    // RecognizeAsync — do not delete it thinking it's redundant with this timer.
    if let Ok(timeouts) = recognizer.Timeouts() {
        let _ = timeouts.SetInitialSilenceTimeout(windows::Foundation::TimeSpan { Duration: 80_000_000 }); // 8 seconds (100ns units)
        let _ = timeouts.SetEndSilenceTimeout(windows::Foundation::TimeSpan { Duration: 5_000_000 });     // 500ms
    }

    let commands: Vec<HSTRING> = phrases.iter().map(|s| HSTRING::from(s.as_str())).collect();
    let iterable: windows_collections::IIterable<HSTRING> = commands.into();
    let constraint = SpeechRecognitionListConstraint::Create(&iterable)
        .map_err(|e| format!("Failed to create constraint: {}", e))?;

    recognizer
        .Constraints()
        .map_err(|e| format!("Failed to get constraints: {}", e))?
        .Append(&constraint)
        .map_err(|e| format!("Failed to append constraint: {}", e))?;

    let compile_result = recognizer
        .CompileConstraintsAsync()
        .map_err(|e| format!("CompileConstraintsAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("Compile await failed: {}", e))?;

    let status = compile_result.Status()
        .map_err(|e| format!("Failed to get compile status: {}", e))?;

    if status != SpeechRecognitionResultStatus::Success {
        return Err(format!("Constraint compilation failed: {:?}", status));
    }

    Ok(recognizer)
}

/// Subscribe a StateChanged handler that emits voice-sound-started /
/// voice-sound-ended to the overlay window. These power the voice pill's
/// waveform animation: bars dance between SoundStarted and SoundEnded.
///
/// Subscribed once per recognizer instance (tracked via CachedRecognizer
/// .state_subscribed). The closure captures an owned AppHandle clone so it
/// outlives the recognition session. The same handler-reference-lifetime
/// caveat that applies to ResultGenerated applies here — see
/// [[winrt-speechrecognizer-shared-instance-caveats]].
fn subscribe_state_changed(
    recognizer: &windows::Media::SpeechRecognition::SpeechRecognizer,
    app: AppHandle,
) -> Result<(), String> {
    use windows::Foundation::TypedEventHandler;
    use windows::Media::SpeechRecognition::*;

    let handler = TypedEventHandler::<
        SpeechRecognizer,
        SpeechRecognizerStateChangedEventArgs,
    >::new(move |_sender, args| {
        if let Some(args) = args.as_ref() {
            if let Ok(state) = args.State() {
                let event_name = match state {
                    SpeechRecognizerState::SoundStarted => Some("voice-sound-started"),
                    SpeechRecognizerState::SoundEnded => Some("voice-sound-ended"),
                    _ => None,
                };
                if let Some(name) = event_name {
                    if let Some(overlay) = app.get_webview_window("overlay") {
                        let _ = overlay.emit(name, serde_json::json!({}));
                    }
                }
            }
        }
        Ok(())
    });

    recognizer
        .StateChanged(&handler)
        .map_err(|e| format!("Failed to subscribe StateChanged: {}", e))?;
    Ok(())
}

/// Get-or-build a recognizer for the given phrase list. Holds the cache mutex
/// across the build to serialize concurrent rebuilds (mutex held briefly during
/// the .get() compile — small race window in practice; concurrent .get() calls
/// on different recognizer instances are not legal).
///
/// When `app` is Some, also lazily subscribes the StateChanged handler if not
/// already subscribed on this recognizer instance. Prewarm passes None (no
/// subscription needed before real use); start_recognition and
/// start_continuous_recognition pass Some(&app).
fn get_or_build_cached(
    phrases: &[String],
    app: Option<&AppHandle>,
) -> Result<windows::Media::SpeechRecognition::SpeechRecognizer, String> {
    let h = phrase_hash(phrases);
    let mut guard = cached_recognizer()
        .lock()
        .map_err(|_| "Cache mutex poisoned".to_string())?;

    if let Some(ref mut cached) = *guard {
        if cached.phrase_hash == h {
            info!("[Voice] Cache hit: reusing recognizer ({} phrases)", phrases.len());
            if let Some(app) = app {
                if !cached.state_subscribed
                    && subscribe_state_changed(&cached.recognizer, app.clone()).is_ok()
                {
                    cached.state_subscribed = true;
                }
            }
            return Ok(cached.recognizer.clone());
        }
        // Hash mismatch — drop old recognizer (best-effort Close).
        let closable: Result<windows::Foundation::IClosable, _> = cached.recognizer.cast();
        if let Ok(c) = closable {
            let _ = c.Close();
        }
    }

    info!("[Voice] Cache miss: building recognizer ({} phrases)", phrases.len());
    let rec = build_recognizer(phrases)?;
    let mut state_subscribed = false;
    if let Some(app) = app {
        if subscribe_state_changed(&rec, app.clone()).is_ok() {
            state_subscribed = true;
        }
    }
    *guard = Some(CachedRecognizer {
        recognizer: rec.clone(),
        phrase_hash: h,
        state_subscribed,
    });
    Ok(rec)
}

fn start_continuous_inner(phrases: Vec<String>, app: AppHandle) -> Result<(), String> {
    use windows::Foundation::TypedEventHandler;
    use windows::Media::SpeechRecognition::*;

    let recognizer = get_or_build_cached(&phrases, Some(&app))?;
    let session = recognizer
        .ContinuousRecognitionSession()
        .map_err(|e| format!("Failed to get ContinuousRecognitionSession: {}", e))?;

    // ResultGenerated — fires for each utterance match. Captured AppHandle is
    // cloned per-event to avoid holding the handler closure's reference across emits.
    let app_for_result = app.clone();
    let result_handler = TypedEventHandler::<
        SpeechContinuousRecognitionSession,
        SpeechContinuousRecognitionResultGeneratedEventArgs,
    >::new(move |_session, args| {
        if let Some(args) = args.as_ref() {
            if let Ok(result) = args.Result() {
                if let Ok(text_h) = result.Text() {
                    let text = text_h.to_string();
                    if !text.is_empty() {
                        info!("[Voice] Continuous recognised: \"{}\"", text);
                        if let Some(overlay) = app_for_result.get_webview_window("overlay") {
                            let _ = overlay.emit(
                                "voice-result",
                                serde_json::json!({ "text": text }),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    });
    session
        .ResultGenerated(&result_handler)
        .map_err(|e| format!("Failed to subscribe ResultGenerated: {}", e))?;

    // Completed — fires when the session ends. Non-Success status maps to voice-error.
    let app_for_done = app.clone();
    let done_handler = TypedEventHandler::<
        SpeechContinuousRecognitionSession,
        SpeechContinuousRecognitionCompletedEventArgs,
    >::new(move |_session, args| {
        CONTINUOUS_RUNNING.store(false, Ordering::SeqCst);
        if let Some(args) = args.as_ref() {
            if let Ok(status) = args.Status() {
                if status != SpeechRecognitionResultStatus::Success {
                    let msg = format!("{:?}", status);
                    info!("[Voice] Continuous session ended: {}", msg);
                    if let Some(overlay) = app_for_done.get_webview_window("overlay") {
                        let _ = overlay.emit(
                            "voice-error",
                            serde_json::json!({ "error": msg }),
                        );
                    }
                }
            }
        }
        // Clear the active session handle
        if let Ok(mut guard) = active_continuous().lock() {
            *guard = None;
        }
        // Consume deferred prewarm if config/profile changed during the session.
        if REWARM_PENDING.swap(false, Ordering::SeqCst) {
            prewarm_from_state();
        }
        Ok(())
    });
    session
        .Completed(&done_handler)
        .map_err(|e| format!("Failed to subscribe Completed: {}", e))?;

    // Store session BEFORE StartAsync so stop_continuous_recognition can find it.
    if let Ok(mut guard) = active_continuous().lock() {
        *guard = Some(session.clone());
    }

    session
        .StartAsync()
        .map_err(|e| format!("StartAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("StartAsync await failed: {}", e))?;

    info!("[Voice] Continuous session started ({} phrases)", phrases.len());
    Ok(())
}

fn run_recognition(phrases: &[String], app: &AppHandle) -> Result<Option<String>, String> {
    use windows::Media::SpeechRecognition::*;

    let recognizer = get_or_build_cached(phrases, Some(app))?;

    // Store the in-flight recognizer so stop_recognition() can cancel.
    if let Ok(mut guard) = active_recognizer().lock() {
        *guard = Some(recognizer.clone());
    }

    info!("[Voice] Listening...");

    let result = recognizer
        .RecognizeAsync()
        .map_err(|e| format!("RecognizeAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("Recognize await failed: {}", e))?;

    let result_status = result.Status()
        .map_err(|e| format!("Failed to get result status: {}", e))?;

    match result_status {
        SpeechRecognitionResultStatus::Success => {
            let text = result.Text()
                .map_err(|e| format!("Failed to get result text: {}", e))?
                .to_string();
            if text.is_empty() {
                Ok(None)
            } else {
                Ok(Some(text))
            }
        }
        SpeechRecognitionResultStatus::UserCanceled => Ok(None),
        SpeechRecognitionResultStatus::TimeoutExceeded => Ok(None),
        other => Err(format!("Recognition status: {:?}", other)),
    }
}
