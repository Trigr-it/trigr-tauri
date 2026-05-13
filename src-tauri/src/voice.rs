//! WinRT-based voice recognition for Trigr voice commands.
//!
//! Uses Windows.Media.SpeechRecognition with SpeechRecognitionListConstraint
//! for offline, grammar-constrained phrase matching.  100% local — no cloud.

use log::{error, info, warn};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use windows_core::Interface;

// ── State ──────────────────────────────────────────────────────────────────

static RECOGNIZING: AtomicBool = AtomicBool::new(false);

/// Shared recognizer handle — allows stop_recognition() to cancel from another thread.
static ACTIVE_RECOGNIZER: std::sync::OnceLock<Mutex<Option<windows::Media::SpeechRecognition::SpeechRecognizer>>> =
    std::sync::OnceLock::new();

fn active_recognizer() -> &'static Mutex<Option<windows::Media::SpeechRecognition::SpeechRecognizer>> {
    ACTIVE_RECOGNIZER.get_or_init(|| Mutex::new(None))
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
            let result = run_recognition(&phrases);
            RECOGNIZING.store(false, Ordering::SeqCst);

            // Clear the shared recognizer handle
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
        })
    {
        // Thread spawn failed — reset flag
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

pub fn is_recognizing() -> bool {
    RECOGNIZING.load(Ordering::SeqCst)
}

// ── Internal ───────────────────────────────────────────────────────────────

fn run_recognition(phrases: &[String]) -> Result<Option<String>, String> {
    use windows::core::HSTRING;
    use windows::Media::SpeechRecognition::*;

    if phrases.is_empty() {
        return Err("No voice phrases configured".to_string());
    }

    // Create recognizer
    let recognizer = SpeechRecognizer::new()
        .map_err(|e| format!("Failed to create SpeechRecognizer: {}", e))?;

    // Tune timeouts for better sensitivity:
    // - Longer initial silence: user has 8s to start speaking (default ~5s)
    // - Longer end silence: catches quieter trailing speech (default ~150ms)
    if let Ok(timeouts) = recognizer.Timeouts() {
        let _ = timeouts.SetInitialSilenceTimeout(windows::Foundation::TimeSpan { Duration: 80_000_000 }); // 8 seconds (100ns units)
        let _ = timeouts.SetEndSilenceTimeout(windows::Foundation::TimeSpan { Duration: 5_000_000 });     // 500ms
    }

    // Store in shared state so stop_recognition() can cancel
    if let Ok(mut guard) = active_recognizer().lock() {
        *guard = Some(recognizer.clone());
    }

    // Build the phrase list and convert to WinRT IIterable<HSTRING>
    let commands: Vec<HSTRING> = phrases
        .iter()
        .map(|s| HSTRING::from(s.as_str()))
        .collect();

    let iterable: windows_collections::IIterable<HSTRING> = commands.into();

    // Create list constraint from the phrases
    let constraint = SpeechRecognitionListConstraint::Create(&iterable)
        .map_err(|e| format!("Failed to create constraint: {}", e))?;

    // Add constraint to recognizer
    recognizer
        .Constraints()
        .map_err(|e| format!("Failed to get constraints: {}", e))?
        .Append(&constraint)
        .map_err(|e| format!("Failed to append constraint: {}", e))?;

    // Compile constraints
    let compile_result = recognizer
        .CompileConstraintsAsync()
        .map_err(|e| format!("CompileConstraintsAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("Compile await failed: {}", e))?;

    let status = compile_result
        .Status()
        .map_err(|e| format!("Failed to get compile status: {}", e))?;

    if status != SpeechRecognitionResultStatus::Success {
        return Err(format!("Constraint compilation failed: {:?}", status));
    }

    info!("[Voice] Constraints compiled, listening ({} phrases)...", phrases.len());

    // Start recognition (blocks until result, cancel, or timeout)
    let result = recognizer
        .RecognizeAsync()
        .map_err(|e| format!("RecognizeAsync failed: {}", e))?
        .get()
        .map_err(|e| format!("Recognize await failed: {}", e))?;

    // Explicitly close the recognizer to release audio resources
    let closable: Result<windows::Foundation::IClosable, _> = recognizer.cast();
    if let Ok(c) = closable {
        let _ = c.Close();
    }

    let result_status = result
        .Status()
        .map_err(|e| format!("Failed to get result status: {}", e))?;

    match result_status {
        SpeechRecognitionResultStatus::Success => {
            let text = result
                .Text()
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
