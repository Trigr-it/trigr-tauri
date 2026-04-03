# TRIGR TAURI — Migration Context
> Read this file at the start of every CC session before touching any code.
> Update the Completed Phases section after every session.
> Last updated: April 2026

---

## 01 — What We Are Building

**Project:** Trigr — Windows desktop hotkey/macro/text expansion app
**Migration:** Electron 28 + React 18 → Tauri v2 + Rust + React 18
**Reason:** Installer size (77MB → ~10MB), RAM (150-250MB → 20-50MB), battery drain, performance at scale
**Approach:** Single codebase, same React UI, Rust replaces main.js entirely

**Repos:**
- Reference (Electron, read-only spec): `E:\Development\Trigr-Reference` / `github.com/Trigr-it/trigr`
- Active development (Tauri): `E:\Development\Trigr-Tauri` / `github.com/Trigr-it/trigr-tauri`

**Dev command:** `cargo tauri dev`
**Build command:** `cargo tauri build`
**Working directory:** `E:\Development\Trigr-Tauri`

---

## 02 — What Stays the Same

- All React UI components — keyboard, sidebar, settings, expansion editor, mouse canvas
- All CSS and theming — light/dark, CSS variables, all colours
- Config JSON structure and all storage key formats (see Section 05)
- All product decisions, UX patterns, feature logic
- GitHub Pages landing page (`docs/index.html`)
- TRIGR_CONTEXT.md rules still apply — all architecture decisions carry over

---

## 03 — What Changes

| Was (Electron/Node) | Now (Tauri/Rust) |
|---|---|
| `electron/main.js` | Rust modules in `src-tauri/src/` |
| `ipcMain.handle()` | `#[tauri::command]` functions |
| `ipcRenderer.invoke()` | `invoke()` from `@tauri-apps/api/core` |
| `uiohook-napi` | `windows-sys` SetWindowsHookExW (WH_KEYBOARD_LL + WH_MOUSE_LL) |
| `koffi` + Win32 API | `windows-sys` crate |
| koffi SendInput | `windows-sys` SendInput (Phase 5) |
| `better-sqlite3` | `rusqlite` crate |
| `electron-store` / JSON | `serde_json` + `std::fs` |
| Custom HTTPS auto-updater | `tauri-plugin-updater` |
| `app.getPath('userData')` | `tauri::api::path::app_data_dir()` |
| Electron tray | `tauri::tray` |
| Electron BrowserWindow | `tauri::WebviewWindow` |
| NSIS config in package.json | Tauri bundler in `tauri.conf.json` |

---

## 04 — Rust Module Map

Each module owns a specific responsibility. CC must not duplicate logic across modules.

| Module | File | Responsibility |
|---|---|---|
| Config | `src-tauri/src/config.rs` | Load, save, backup keyforge-config.json |
| Hotkeys | `src-tauri/src/hotkeys.rs` | Global input hook, modifier tracking, double-tap, mouse buttons |
| Actions | `src-tauri/src/actions.rs` | Execute all action types (Type Text, Send Hotkey, Macro, Open App/URL/Folder) |
| Expansions | `src-tauri/src/expansions.rs` | Keystroke buffer, trigger detection, text injection |
| Foreground | `src-tauri/src/foreground.rs` | Foreground watcher, process name detection, profile auto-switching |
| Tray | `src-tauri/src/tray.rs` | System tray icon, window show/hide, autolaunch, close-to-tray |
| Main | `src-tauri/src/main.rs` | App entry point, Tauri builder, module wiring |

**React components added post-MVP:**

| Component | File | Responsibility |
|---|---|---|
| OnboardingTour | `src/components/OnboardingTour.jsx` | 5-step first-run tour with progressive coach marks |
| OnboardingTour CSS | `src/components/OnboardingTour.css` | Tour overlay, tooltip, coach mark styling (CSS variables only) |
| FillInWindow | `src/components/FillInWindow.jsx` | Fill-in field prompt window for {fillIn:Label} tokens |
| FillInWindow CSS | `src/components/FillInWindow.css` | Fill-in window styling (transparent bg, content-based auto-resize) |

---

## 05 — Storage & Config Rules (CRITICAL)

**Config file:** `keyforge-config.json` in app data dir — filename must NOT change. Existing user configs from the Electron version must load without migration.

**Storage key formats — identical to Electron version:**
- Single press hotkey: `ProfileName::Modifier::KeyCode`
- Double press hotkey: `ProfileName::Modifier::KeyCode::double`
- Bare key: `ProfileName::Bare::KeyCode`
- App-specific: `AppName::Modifier::KeyCode`
- Mouse button: `ProfileName::Modifier::MOUSE_LEFT` (MOUSE_LEFT, MOUSE_RIGHT, MOUSE_MIDDLE, MOUSE_SIDE1, MOUSE_SIDE2)

**`onboarding_complete`:** Bool field in config. Default `false` for new users (triggers onboarding tour). Set to `true` when tour finishes or is skipped. Migration: auto-set to `true` on first load if `hasSeenWelcome` is already `true` (prevents existing alpha testers from seeing the tour). Reset via `reset_onboarding` Rust command (Settings > Restart Onboarding Tour).

**suppressNextClipboardWrite:** Module-level bool in `actions.rs` or `expansions.rs`. Set to `true` immediately before any internal clipboard write (text expansion fire, image expansion fire, any Trigr-initiated clipboard write). The future clipboard manager checks this flag and skips logging if set, then clears it. Establish this pattern in Phase 7 even though the clipboard manager is not built yet.

---

## 06 — IPC Pattern

**Old (Electron):**
```javascript
// Renderer
window.electron.invoke('get-config')
// Main
ipcMain.handle('get-config', () => loadConfigSafe())
```

**New (Tauri):**
```javascript
// Renderer
import { invoke } from '@tauri-apps/api/core'
invoke('get_config')
```
```rust
// Rust
#[tauri::command]
fn get_config() -> Value { load_config_safe() }
```

Note: Tauri command names use snake_case. Channel names map as: `get-config` → `get_config`, `save-config` → `save_config` etc.

**Commands added post-MVP:**
- `reset_onboarding` — sets `onboarding_complete: false` in config, returns `bool`
- `set_window_resizable` — calls `window.set_resizable(bool)` on the main window
- `update_global_variables` — pushes global variables HashMap to expansion engine
- `fill_in_ready` — renderer ready handshake for fill-in window
- `fillin_resize` — JS-driven content-based window resize (same pattern as overlay_resize)
- `fill_in_submit` — receives fill-in field values (or null for cancel) from renderer
- `open_logs_folder` — opens the log directory in File Explorer

---

## 07 — ARM64 Rules (CRITICAL)

Machine: Surface Pro, Windows ARM64. Every native Rust crate must be verified for ARM64 compatibility before implementation.

Known status:
- `rdev` — verify ARM64 before Phase 4
- `enigo` — verify ARM64 before Phase 5
- `windows-rs` — ARM64 compatible (Microsoft maintained)
- `rusqlite` — ARM64 compatible
- `serde_json` — ARM64 compatible (pure Rust)
- `tauri-plugin-updater` — ARM64 compatible

If any crate fails on ARM64, find an alternative before proceeding. Do not assume compatibility — test on device.

---

## 08 — Do Not Touch Rules

1. **E:\Development\Trigr-Reference** — read-only reference only. Never modify this directory.
2. **E:\Development\Trigr** (Electron production) — never touch during Tauri migration. Testers stay on Electron until Tauri is proven.
3. **keyforge-config.json filename** — must not change. Existing configs must load.
4. **React UI components** — only change invoke() call targets. Never change component logic, CSS variables, or theming.
5. **suppressNextClipboardWrite** — must be set before every internal clipboard write, no exceptions.
6. **Theme colours** — all colours must use CSS variables. Never hardcode hex values.
7. **Config writes** — always owned by Rust backend. Frontend never writes directly to disk.
8. **Background threads** — hotkey hook, foreground watcher, macro runner must never block the main/UI thread.

---

## 09 — Build Phases

### Phase status key
- ⬜ Not started
- 🔄 In progress
- ✅ Complete and tested

### Phases

| # | Phase | Status | Notes |
|---|---|---|---|
| 0 | Codebase analysis + migration plan | ✅ | Complete |
| 1 | Project scaffold + React migration | ✅ | App window opens, UI renders |
| 2 | Config read/write | ✅ | load_config_safe, save_config, backups, import/export, file dialogs |
| 3 | Tray + window management | ✅ | Close to tray, autolaunch, show/hide, registry startup, tray menu |
| 4 | Global hotkey capture (windows-sys) | ✅ | ARM64 verified, SetWindowsHookExW LL hooks, full matching + double-tap |
| 5 | Text injection + Send Hotkey | ✅ | SendInput Unicode text + VK-based hotkey sim, macro sequences |
| 6 | Foreground watcher + app profiles | ✅ | Win32 API process detection, 1500ms poll, visibility guard |
| 7 | Text expansions | ✅ | Buffer tracking, space + immediate triggers, clipboard paste injection |
| 8 | Macro sequence + remaining actions | ✅ | Wait for Input step added, all action types complete |
| 9 | Quick Search overlay | ✅ | Pre-created hidden window, cursor-following position, Ctrl+Space toggle |
| 10 | Auto-updater + installer | ✅ | Shippable build — share with testers |

---

## 10 — MVP Definition

**Dev MVP (Phase 5 complete):** App launches, tray icon shows, at least one hotkey fires a Type Text action. Test on device before continuing.

**Shippable MVP (Phase 10 complete):** All current Electron Alpha features working, NSIS installer produced, auto-updater configured. Share with existing 2 testers. Electron version stays live until Tauri confirmed stable.

---

## 10a — Critical Implementation Rules

These rules were discovered during development and must not be violated. Each caused a serious bug when broken.

### Hook Callback — No I/O
`keyboard_hook_proc` and `mouse_hook_proc` in `hotkeys.rs` must NEVER contain `println!`, file writes, or any blocking operation. Windows silently removes the LL hook if the callback takes >300ms to return. All diagnostic logging must happen on the processor thread via `send_event()`.

### Recording/Capture Check Order
In `handle_keydown()`, the `IS_RECORDING_HOTKEY` and `IS_CAPTURING_KEY` checks must remain ABOVE the `APP_INPUT_FOCUSED` guard. Moving them below causes capture to silently fail when Trigr has focus.

### Two-Path Capture
Keystroke capture uses two paths: the LL hook (when other apps have focus) and a JS `keydown` listener in `tauriAPI.js` (when Trigr's WebView2 has focus). The JS `window.__trigr_recording` / `__trigr_capturing` flags MUST stay in sync with the Rust `IS_RECORDING_HOTKEY` / `IS_CAPTURING_KEY` atomics. Both flags must be cleared when the capture event is received (in `onHotkeyRecorded` / `onKeyCaptured`).

### Suppress Flags Before SendInput
`SUPPRESS_SIMULATED` must be set `true` before any `SendInput` call in `actions.rs` or `expansions.rs`. `SUPPRESS_NEXT_CLIPBOARD_WRITE` must be set `true` before any internal clipboard write. Omitting either causes Trigr to intercept its own simulated keystrokes or log its own clipboard writes.

### Startup Assignment Sync
`updateAssignments(assignments, profile)` must be called from the frontend after config loads on startup (`App.jsx`). The Tauri command parameter name must match the Rust function signature exactly (e.g. `config` not `incoming`). Missing this call causes `assignments=0` on all hotkeys.

### Modifier Release Before Injection
`release_held_modifiers()` (using `GetAsyncKeyState`) must be called before any clipboard paste or text injection. Without this, physically held modifiers (e.g. Ctrl+Shift from the trigger) combine with the injected Shift+Insert or Ctrl+V, sending the wrong key combination to the target app.

### Hook Thread Architecture
The LL hook runs on a dedicated high-priority thread with a `PeekMessageW` polling loop (not `GetMessageW`). This is required because the Tauri main thread's message pump is monopolised by WebView2's COM dispatch when the app has focus, preventing LL hook callbacks from being serviced. A health monitor thread reinstalls hooks if the heartbeat stalls for 30s.

### Keystroke Buffering During Injection
`INJECTION_IN_PROGRESS` (AtomicBool in hotkeys.rs) is set via `InjectionGuard` before injection starts. While true, `keyboard_hook_proc` swallows real user keystrokes and buffers them in `INJECTION_BUFFER` for replay after injection completes. Exception: if `FILLIN_HWND` matches the foreground window, keystrokes pass through (user is typing in fill-in input). Replayed keystrokes are fed into the expansion buffer to support sequential triggers.

### Fill-In Window Architecture
- Pre-created hidden at startup in lib.rs setup() with `transparent(true)` + WebView2 COM `SetDefaultBackgroundColor(0,0,0,0)` — DO NOT remove either
- Window sizing is content-based via JS auto-resize (`fillin_resize` command) — DO NOT set fixed heights in Rust
- `FILL_IN_ACTIVE` (AtomicBool) prevents concurrent fill-in invocations
- `FILLIN_HWND` (AtomicIsize, set via `win.hwnd()`) lets the hook pass keystrokes to fill-in input
- `FILLIN_HWND` guard in `handle_keydown` skips expansion buffer while fill-in is visible
- Ready handshake: Rust emits `fill-in-request-ready`, JS calls `fillInReady()`, Rust waits on mpsc channel (5s timeout)
- Fill-in flow runs on dedicated spawned thread — never blocks the processor thread
- `on_window_event` CloseRequested handler prevents destruction, hides window, sends cancel via channel

### Autocorrect — Disabled for Alpha
Both custom (`GLOBAL::AUTOCORRECT::`) and built-in (`builtin_autocorrect()`) autocorrect checks are commented out in `check_space_trigger()`. The Autocorrect tab is hidden in TextExpansions.jsx. Space-triggered text expansions continue to work normally.

### Trailing Space — Synthetic Keystroke
Autocorrect/expansion trailing space is sent as a synthetic `VK_SPACE` keystroke via SendInput, NOT included in the clipboard paste string. Some apps (browsers, web inputs) strip trailing whitespace from clipboard paste.

### Structured Logging (tauri-plugin-log)
`tauri-plugin-log` is registered in the builder chain in lib.rs. Log file: `AppData\Local\com.nodescaffold.trigr\logs\trigr.log`. Targets: LogDir (file) + Stdout. 5MB max file size, KeepOne rotation, Info level. **CRITICAL:** Call `.clear_targets()` after `Builder::new()` before adding targets — the plugin ships with 2 default targets (Stdout + LogDir) and `.target()` appends, not replaces. Without `.clear_targets()`, every log entry is duplicated. All Rust modules use `log::info!()` / `log::error!()` / `log::warn!()` — never `println!()`. Settings panel has "Open logs folder" button.

### Config Factory Defaults
If `load_config_safe()` returns `(None, None)` (all config sources failed), the `load_config` Tauri command writes a factory-default config `{ "profiles": ["Default"], "assignments": {}, "activeProfile": "Default" }` via the atomic write path and returns it to the frontend. This ensures a valid config file always exists after the first load attempt.

---

## 11 — Tauri Config Reference

```json
{
  "productName": "Trigr",
  "identifier": "com.nodescaffold.trigr",
  "version": "0.1.9",
  "bundle": {
    "active": true,
    "targets": ["nsis"],
    "icon": ["assets/icons/icon.png"],
    "windows": {
      "nsis": {
        "artifactName": "Trigr-Setup.exe"
      }
    }
  },
  "app": {
    "windows": [{
      "title": "Trigr",
      "width": 1200,
      "height": 800,
      "resizable": true,
      "visible": false
    }]
  }
}
```

Permanent download URL (same pattern as Electron version):
`https://github.com/Trigr-it/trigr-tauri/releases/latest/download/Trigr-Setup.exe`

---

## 12 — Session Log

Record key decisions and findings here after each session.

| Date | Phase | What was done | Key findings / decisions |
|---|---|---|---|
| 2026-04-01 | Phase 2 | Config read/write complete | config.rs module with full fallback chain, atomic save, backup management. Added tauri-plugin-dialog for import/export/browse file dialogs. opener crate for open_config_folder/open_external. Capabilities file created for Tauri v2 permissions. |
| 2026-04-01 | Phase 3 | Tray + window management complete | tray.rs module: tray icon with PNG decode, menu (Open/Pause/Start with Windows/Quit), left-click toggle, close-to-tray via on_window_event, autolaunch --autolaunch flag, registry read/write for startup. png crate used for icon loading. |
| 2026-04-01 | Phase 4 | Global hotkey capture complete | Skipped rdev — used windows-sys SetWindowsHookExW directly (ARM64 verified). WH_KEYBOARD_LL + WH_MOUSE_LL on background thread, event channel to processor thread. Full VK→keyId mapping, modifier tracking, storage key matching, double-tap with timer-based detection, hotkey recording, key capture, bare key support, mouse buttons + scroll wheel. |
| 2026-04-01 | Phase 5 | Text injection + Send Hotkey complete | actions.rs: SendInput KEYEVENTF_UNICODE for Type Text (with surrogate pair support for emoji), VK-based key simulation for Send Hotkey, modifier release before action, bare key Backspace erase, macro sequence executor (Type Text, Press Key, Wait, Open URL steps). No enigo needed — windows-sys SendInput handles everything. suppressNextClipboardWrite flag established for future clipboard manager. |
| 2026-04-01 | Phase 6 | Foreground watcher + app profiles complete | foreground.rs: GetForegroundWindow + GetWindowThreadProcessId + OpenProcess + QueryFullProcessImageNameW via windows-sys. 1500ms poll on background thread, HWND cache optimization. Visibility guard (is_visible && !is_minimized). Self-detection via exe stem. Profile auto-switching with global fallback. get_foreground_process command exposed. |
| 2026-04-02 | Phase 7 | Text expansions complete | expansions.rs: keystroke buffer (50 char rolling), space-triggered + immediate-mode expansion matching, backspace trigger deletion + Shift+Insert clipboard paste injection. Win32 clipboard API (OpenClipboard/GetClipboardData/SetClipboardData) for {clipboard} token and paste. Global variable tokens ({{var}}, {date}, {time}, {dayofweek}, {cursor}). Built-in autocorrect dictionary (~50 common typos). suppressNextClipboardWrite pattern established. Buffer integrated into hotkeys.rs event processor. |
| 2026-04-02 | Phase 8 | Macro sequence + Wait for Input complete | Added "Wait for Input" step to execute_macro_step in actions.rs. WaitEvent enum + one-shot mpsc channel in hotkeys.rs. Event processor forwards key/mouse events to waiter before normal handling. Supports LButton/RButton/MButton/AnyKey/SpecificKey input types, press/release/pressRelease triggers. Two-phase pressRelease state machine is per-waiter. 30s timeout. Clears waiter on timeout/cancel/macro-disable. Waited-for keystrokes pass through to target app (no suppression). Keystroke capture modes (recording + key capture) already working from Phase 4. |
| 2026-04-03 | Post-MVP | Onboarding tour built | 5-step first-run tour (OnboardingTour.jsx + CSS). Progressive coach marks with SVG mask cutouts, deferred tooltip positioning, active detection for hotkey creation flow. New config field `onboarding_complete` (bool). New Rust commands: `reset_onboarding`, `set_window_resizable`. Window resize locked during tour. Restart button in SettingsPanel HELP section. Existing-user migration: if `hasSeenWelcome` is true and `onboarding_complete` undefined, auto-sets `onboarding_complete: true` to skip tour for alpha testers. |
| 2026-04-03 | Post-MVP | Expansion fixes + fill-in fields + global vars | Autocorrect disabled for Alpha. Trailing space sent as synthetic keystroke. Initial injection delay 30ms. Keystroke buffer-and-replay (INJECTION_IN_PROGRESS + InjectionGuard). Blank clipboard entry fix. Fill-in field tokens ({fillIn:Label}) fully implemented: pre-created hidden window, Electron-style ready handshake, FILL_IN_ACTIVE concurrency guard, FILLIN_HWND hook passthrough, content-based auto-resize via JS→Rust IPC. Global variables wired to Rust (update_global_variables command + frontend sync). Insert dropdown scroll fix + dynamic maxHeight. htmlToPlainText double-newline fix. Diagnostic logs cleaned. v0.1.9 released. |
| 2026-04-03 | Post-MVP | Structured logging + config hardening | Added tauri-plugin-log: file + stdout targets, 5MB rotation, Info level. .clear_targets() required before .target() to avoid duplicate entries. "Open logs folder" button in Settings. Config hardening: factory-default write on total config failure (all sources return None). Converted remaining println! in config.rs to log::info!/error!. |
