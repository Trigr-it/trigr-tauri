# TRIGR TAURI — Historical Migration Context
> **ARCHIVE** — Do NOT read at session start. Only consult if debugging migration-era code or needing Electron comparison.
> Migration complete as of v0.1.34 (2026-04-12). All 10 build phases done. Post-MVP features actively shipping.
> Current rules and architecture live in **CLAUDE.md** (the single source of truth).

---

## 01 — Migration Summary

**Migration:** Electron 28 + React 18 -> Tauri v2 + Rust + React 18
**Reason:** Installer size (77MB -> ~10MB), RAM (150-250MB -> 20-50MB), battery drain, performance at scale
**Approach:** Single codebase, same React UI, Rust replaces main.js entirely

**Repos:**
- Reference (Electron, read-only spec): `E:\Development\Trigr-Reference` / `github.com/Trigr-it/trigr`
- Active development (Tauri): `E:\Development\Trigr-Tauri` / `github.com/Trigr-it/trigr-tauri`

---

## 02 — What Changed (Electron -> Tauri)

| Was (Electron/Node) | Now (Tauri/Rust) |
|---|---|
| `electron/main.js` | Rust modules in `src-tauri/src/` |
| `ipcMain.handle()` | `#[tauri::command]` functions |
| `ipcRenderer.invoke()` | `invoke()` from `@tauri-apps/api/core` |
| `uiohook-napi` | `windows-sys` SetWindowsHookExW (WH_KEYBOARD_LL + WH_MOUSE_LL) |
| `koffi` + Win32 API | `windows-sys` crate |
| `better-sqlite3` | `rusqlite` crate |
| `electron-store` / JSON | `serde_json` + `std::fs` |
| Custom HTTPS auto-updater | `tauri-plugin-updater` |
| `app.getPath('userData')` | `tauri::api::path::app_data_dir()` |
| Electron tray | `tauri::tray` |
| Electron BrowserWindow | `tauri::WebviewWindow` |
| NSIS config in package.json | Tauri bundler in `tauri.conf.json` |

---

## 03 — IPC Pattern Reference

**Old (Electron):**
```javascript
window.electron.invoke('get-config')     // Renderer
ipcMain.handle('get-config', () => ...)  // Main
```

**New (Tauri):**
```javascript
invoke('get_config')                     // Frontend (snake_case)
```
```rust
#[tauri::command]
fn get_config() -> Value { ... }         // Rust
```

---

## 04 — ARM64 Rules

Machine: Surface Pro, Windows ARM64. Every native Rust crate must be verified for ARM64 before use.

Known compatible: `windows-sys`, `rusqlite` (bundled), `serde_json`, `tauri-plugin-updater`, `notify` v8, `image` v0.25.

Skipped: `rdev`, `enigo` — `windows-sys` SendInput/SetWindowsHookExW handles everything directly.

---

## 05 — Build Phases (all complete)

All 10 migration phases completed by v0.1.34 (2026-04-12): scaffold, config, tray, hotkey capture, text injection, foreground watcher, expansions, macros, quick search overlay, auto-updater + installer.

---

## 06 — Session Log (v0.1.9 to v0.1.34)

Detailed session-by-session changelog preserved for historical reference. Covers: onboarding tour, fill-in fields, analytics, macro steps, profile accordion, list view, templates, context menus, hold/repeat mode, image expansions, clipboard manager, bare mouse remapping, scratchpad, and all bugfixes through April 2026.

For specific historical details, use `git log` to find the relevant commits.

---

## 07 — App Icon / Brand Mark

**Canonical mark:** `src-tauri/icons/trigr-logo.svg` (gradient version, for UI use)
**Flat favicon version:** `src-tauri/icons/trigr-favicon.svg` (flat fills, for ICO export)

**Design:** Chamfered keycap. Gold gradient base (`#f0b942` → `#c8860a`), white-to-cream keytop inset, gold-deep T legend stamped on keytop.
- Base: `<rect x="0" y="0" width="64" height="64" rx="9"/>` — 9px chamfer
- Keytop: `<rect x="7.68" y="6.4" width="48.64" height="43.52" rx="6.5"/>` — inset with inner shadow strip at `y="46.5"`
- T crossbar: `<rect x="19" y="20" width="26" height="8" rx="1.5" fill="#c8860a"/>`
- T stem: `<rect x="28" y="24" width="8" height="11" rx="1.5" fill="#c8860a"/>`

**Usage:**
- App titlebar (TitleBar.jsx): inline SVG in `<span class="trigr-mark">`, 22×22px
- Marketing nav + footer (docs/index.html): inline SVG in `.logo-mark` div, 32×32px; unique gradient IDs per instance (`-nav`, `-ft`)
- Marketing mockup titlebar (docs/index.html): inline SVG in `.t-logo-mark` div, 22×22px; gradient ID suffix `-mock`
- Tauri app icon (src-tauri/icons/): regenerate with `npx @tauri-apps/cli icon src-tauri/icons/trigr-favicon.svg` (run from Windows Git Bash/PowerShell, not WSL)
- Marketing site favicon: `docs/favicon.svg` (copy of trigr-favicon.svg)

---

## 08 — Clipboard Panel Architecture (post-migration additions)

Notes on non-obvious wiring for the main UI clipboard manager (`ClipboardPanel.jsx`). The popup overlay (`ClipboardOverlay.jsx`) is a separate component with its own state and IPC paths — these notes do not apply to it.

- **Preview pane is collapsible and resizable.** Width persisted in `keyforge-config.json` as `clipboardPreviewWidth` (int, clamped 320–1200, default 480). Loaded in `App.jsx` and passed as a prop to `ClipboardPanel` with a save callback. Drag handle on the left edge of `.cbg-detail` updates an internal `dragWidth` during drag and only calls the persist callback on mouseup. Layout: toolbar is a sibling of `.cbg-main` (always full width); `.cbg-main` flips to `flex-direction: row` when an item is selected (`.cbg-main-split`), so opening the preview never affects the toolbar.
- **`paste_count` column on `clipboard_history`.** Schema migration via `ALTER TABLE … ADD COLUMN paste_count INTEGER NOT NULL DEFAULT 0` in `clipboard.rs`. Old rows default to 0 — backwards compatible. Surfaced in `handle_get_history` JSON; UI optimistically increments on paste.
- **`paste_text(text, source_id)` Tauri command** (`lib.rs`). Pastes arbitrary text via the standard `release_held_modifiers` + `write_clipboard_pub` + Ctrl+V pipeline; does NOT modify the source clip. Used by Stage C transform pills (lowercase / UPPERCASE / Trimmed / Plain) and "Paste edited". Both paste paths — existing `paste_clipboard_item` AND new `paste_text` — increment `paste_count` for the source clip via `clipboard::increment_paste_count(id)` (fire-and-forget, no reply channel).
- **OCR via Windows.Media.Ocr.** Implementation in `src-tauri/src/ocr.rs` (new module). Uses `OcrEngine::TryCreateFromUserProfileLanguages` — works on systems with at least one OCR language pack installed (English ships by default on en-* Windows). Fully offline, no extra runtime. Async: `ocr_clipboard_image` Tauri command runs the blocking WinRT calls via `tauri::async_runtime::spawn_blocking`. Failure path returns `Result<String, String>` with a user-friendly message — UI shows "OCR not available on this system" and never panics. Required Cargo features: `Media_Ocr` and `Graphics_Imaging` on the `windows` crate (in addition to the existing `Storage_Streams` etc.).
- **Dominant colour extraction.** `color-thief = "0.2"` (pure Rust, ARM64 fine). `clipboard::dominant_colors(blob, n)` decodes PNG via existing `image` crate, runs `color_thief::get_palette` on the RGBA buffer. Surfaced via `get_clipboard_image_colors` Tauri command — returns up to 5 RGB triplets. UI shows clickable colour swatches; click copies hex via `navigator.clipboard.writeText`.
- **`src/utils/presetIcons.js` shared util.** Houses `findPresetIconForUrl(url)`. Imports `PRESET_ICONS_BY_DOMAIN` from `SearchTemplatesPanel.jsx` (which now exports it — built from the `PRESETS` array at module load). Both `SearchTemplatesPanel` (URL-typed Quick Actions) and `ClipboardPanel` (LinkPane in the preview) import from the util. Uses exact-match-then-suffix-match against the bundled icon set — handles `gist.github.com` → GitHub icon, etc.

---

## 09 — Voice Command System (post-migration overhaul)

Major overhaul shipped after the icon-reorg / rich-text work. Five distinct improvements implemented as one staged build. See `src-tauri/src/voice.rs`, `src-tauri/src/hotkeys.rs` voice block, `src-tauri/src/lib.rs` voice commands, `src/components/SearchOverlay.jsx`, and `src/voicePhrases.js`.

- **Pre-warmed recognizer cache.** `CachedRecognizer { recognizer, phrase_hash }` in `voice.rs` holds one compiled `SpeechRecognizer` reused across single-shot and continuous paths. `phrase_hash` is a sorted-then-DefaultHasher digest of the phrase list — when it matches a fresh `start_recognition` call skips the WinRT constraint compile (~150-300ms saved per recognition). Invalidated and rebuilt in a background thread at the end of `update_assignments`, `set_active_global_profile`, and `save_config` (lib.rs). The initial cache warm happens automatically on the first `update_assignments` from frontend startup. Phrase list assembled in Rust by `collect_voice_phrases_from_state()` which mirrors the frontend `buildItems` filter (active profile assignments + GLOBAL expansions + GLOBAL quick actions) and reads BOTH `voicePhrases` (array) and legacy `voicePhrase` (single string).
- **ContinuousRecognitionSession for continuous mode.** Continuous mode uses `recognizer.ContinuousRecognitionSession()` instead of chained `RecognizeAsync` calls. ONE long-running session emits `ResultGenerated` events as utterances match, with no per-utterance restart gap. Reuses the cached `SpeechRecognizer` (no separate compile). State: `CONTINUOUS_RUNNING: AtomicBool` (SeqCst) gates double-start; `ACTIVE_CONTINUOUS: OnceLock<Mutex<Option<SpeechContinuousRecognitionSession>>>` holds the live session so `stop_continuous_recognition` can cancel from any thread. Event handlers are `TypedEventHandler` closures that capture cloned `AppHandle` and emit `voice-result` (per match) or `voice-error` (Completed with non-Success status). Single-shot `RecognizeAsync` path is unchanged.
- **Voice trigger model.** Tap voice hotkey with `VOICE_ACTIVE=false` → emit `voice-open` (overlay opens, single-shot recognition fires). Press while `VOICE_ACTIVE=true` (overlay open) → emit `voice-keydown` (overlay closes). Simple toggle, no timing checks. **The Stage 4 double-tap-to-continuous mechanic was reverted on 2026-05-14** after multiple debug rounds showed it didn't reliably detect the user's natural gesture (hold Ctrl+Alt, tap E twice) — `sync_modifier_state_from_os` on the second tap re-asserted modifiers in some cases, and `clear_voice_active()` resetting `VOICE_LAST_PRESS_MS=0` on overlay close meant any test with a recognition cycle between presses fused into two single-fresh events. Pill click in the voice overlay is now the ONE entry point to continuous mode. `VOICE_LAST_PRESS_MS`, `voice-continuous-toggle` event, and `pub(crate)` on `VOICE_CONTINUOUS` were all removed.
- **Phrase aliases.** One action / expansion / quick action can have multiple voice phrases. Canonical storage: `data.voicePhrases: string[]`. Legacy single-string `data.voicePhrase` is kept as a read-time fallback for one release cycle to migrate old configs cleanly. Shared helper at `src/voicePhrases.js` exports `readVoicePhrases(data)` (returns array, prefers new field, falls back to legacy) and `writeVoicePhrases(data, phrases)` (writes new array, deletes legacy field, deletes BOTH if list is empty so no orphan keys). UI list editor in MacroPanel, TextExpansions, and SearchTemplatesPanel (the latter previously had no voice phrase UI — added in this overhaul). Empty state shows "+ Add voice phrase" button only, no empty input row. The Rust `collect_voice_phrases_from_state()` reads both fields for cache assembly.
- **No-match feedback.** On rejection, the overlay flashes 3 random example phrases from the current grammar via `pickExamplePhrases(count)` (Fisher–Yates partial shuffle). Single mode: examples visible for 3 seconds then overlay closes. Continuous mode: examples shown as a non-blocking banner ABOVE the listening pill — the session keeps running and the user can speak again immediately. Banner clears on next successful match or continuous exit. Window resize via new `voice_overlay_examples_expand` Tauri command (340x168 logical), separate from `voice_overlay_error_expand` (340x72) since the examples need more height for 3 rows.
- **Dual-layer voice timeout (DO NOT REMOVE).** WinRT `InitialSilenceTimeout` is set to 8s in `build_recognizer` (voice.rs). It fires when audio frames arrive but the user stays silent. It does NOT fire if audio frames never arrive at all — Bluetooth mic mid-session dropout, exclusive mic capture by another app (Teams call starting), OS-level permission revocation during recognition, USB driver hang. In those cases `RecognizeAsync.get()` blocks indefinitely. JS-side backstop timer at 11s in SearchOverlay.jsx (3s past WinRT's 8s) is the only escape from that hung state — force-stops via `stopVoiceRecognition()`. Comment blocks in both files explicitly warn future maintainers not to delete the JS timer as redundant.
- **Open bug — pill-click continuous mode doesn't recognise audio.** When the user clicks the pill in the voice overlay, the UI flips correctly (∞ badge shows, `VOICE_CONTINUOUS=true`, session.StartAsync returns Ok), but `ResultGenerated` never fires during the session. Spoken phrases that work in single-shot mode produce no recognition. Single-shot voice is unaffected. Hypothesis under investigation: cached `SpeechRecognizer` instance state corruption from the cancel-then-start race — pill click invokes `stopVoiceRecognition` (fire-and-forget `StopRecognitionAsync`) then immediately `startVoiceContinuous`, which calls `ContinuousRecognitionSession.StartAsync` on the same recognizer while it's still mid-cancel. WinRT reports the session as started but the audio capture pipeline never engages. Temporary `[VOICE-DIAG]` instrumentation in `voice.rs::start_continuous_inner` (handler registration + per-event logs with confidence + raw_confidence) and in `SearchOverlay.jsx` voice-result handler is in place pending a clean trace. **Fix paths under consideration** (not yet approved): wait-for-RECOGNIZING-false synchronisation, fresh-recognizer-per-session (would violate shared-cache design), or moving the cancel before the React state flip.
