# TRIGR TAURI — Migration Context
> Read this file at the start of every CC session before touching any code.
> Update the Completed Phases section after every session.
> Last updated: 2026-04-12 (post v0.1.34)

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
| Actions | `src-tauri/src/actions.rs` | Execute all action types (Type Text, Send Hotkey, Macro, Open App/URL/Folder, Focus Window) |
| Expansions | `src-tauri/src/expansions.rs` | Keystroke buffer, trigger detection, text + image injection |
| Foreground | `src-tauri/src/foreground.rs` | Foreground watcher, process name detection, profile auto-switching |
| Tray | `src-tauri/src/tray.rs` | System tray icon, window show/hide, autolaunch, close-to-tray |
| Analytics | `src-tauri/src/analytics.rs` | SQLite usage tracking — action counts, time saved, best day/week records |
| Clipboard | `src-tauri/src/clipboard.rs` | Clipboard history — listener, SQLite storage, dedup, retention pruning |
| Main | `src-tauri/src/main.rs` | App entry point, Tauri builder, module wiring |

**React components added post-MVP:**

| Component | File | Responsibility |
|---|---|---|
| OnboardingTour | `src/components/OnboardingTour.jsx` | 5-step first-run tour with progressive coach marks |
| OnboardingTour CSS | `src/components/OnboardingTour.css` | Tour overlay, tooltip, coach mark styling (CSS variables only) |
| FillInWindow | `src/components/FillInWindow.jsx` | Fill-in field prompt window for {fillIn:Label} tokens |
| FillInWindow CSS | `src/components/FillInWindow.css` | Fill-in window styling (transparent bg, content-based auto-resize) |
| AnalyticsPanel | `src/components/AnalyticsPanel.jsx` | Local usage analytics — today, last 7 days, records, breakdown |
| AnalyticsPanel CSS | `src/components/AnalyticsPanel.css` | Analytics panel styling (CSS variables only) |
| List View | `src/components/Sidebar.jsx` | Assignment list view — multi-column card grid in expanded sidebar (state owned by App.jsx, toggle in TitleBar) |
| TemplatesPanel | `src/components/TemplatesPanel.jsx` | Starter template packs — shared component used by TitleBar dropdown and Settings accordion |
| ClipboardPanel | `src/components/ClipboardPanel.jsx` | Clipboard history panel — list, search, paste, pin, context menu |
| ClipboardOverlay | `src/components/ClipboardOverlay.jsx` | Ctrl+Shift+V quick-paste popup — arrow key nav, Enter to paste |
| ZoomableImage | `src/components/ZoomableImage.jsx` | Scroll-to-zoom + drag-to-pan image viewer — used in ClipboardPanel and ClipboardOverlay detail panes |

---

## 05 — Storage & Config Rules (CRITICAL)

**Config file:** `keyforge-config.json` in app data dir — filename must NOT change. Existing user configs from the Electron version must load without migration.

**Storage key formats — identical to Electron version:**
- Single press hotkey: `ProfileName::Modifier::KeyCode`
- Double press hotkey: `ProfileName::Modifier::KeyCode::double`
- Bare key: `ProfileName::Bare::KeyCode`
- App-specific: `AppName::Modifier::KeyCode`
- Mouse button: `ProfileName::Modifier::MOUSE_LEFT` (MOUSE_LEFT, MOUSE_RIGHT, MOUSE_MIDDLE, MOUSE_SIDE1, MOUSE_SIDE2)

**Analytics DB:** `trigr-analytics.db` in app data dir (alongside `keyforge-config.json`). SQLite via `rusqlite` with `bundled` feature. Single table `action_log` with columns: `id`, `timestamp` (ISO-8601 UTC), `action_type` (expansion/hotkey/macro), `char_count`, `time_saved` (seconds). Completely separate from the config system — never modify config.rs for analytics. WAL journal mode. All access via a dedicated writer thread (see Critical Implementation Rules).

**`onboarding_complete`:** Bool field in config. Default `false` for new users (triggers onboarding tour). Set to `true` when tour finishes or is skipped. Migration: auto-set to `true` on first load if `hasSeenWelcome` is already `true` (prevents existing alpha testers from seeing the tour). Reset via `reset_onboarding` Rust command (Settings > Restart Onboarding Tour).

**suppressNextClipboardWrite:** Module-level bool in `actions.rs` or `expansions.rs`. Set to `true` immediately before any internal clipboard write (text expansion fire, image expansion fire, any Trigr-initiated clipboard write). The future clipboard manager will check this flag and skip logging if set, then clear it.

**Local settings file:** `trigr-local-settings.json` in AppData dir. JSON format: `{ "shared_config_path": "C:\\path" }`. Machine-specific — never syncs. Read at startup by `config::init()` → `load_local_settings()`. Written by `save_local_settings()`. Controls where `config_path()` resolves to.

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
- `get_analytics` — returns aggregate usage stats (total, today, last 7 days, best day, best 7 days, breakdown)
- `reset_analytics` — deletes all analytics data, returns `bool`
- `list_open_windows` — EnumWindows to list visible non-minimized windows, returns `Vec<{ process, title }>`, filters system processes
- `export_profile` — takes `filename_hint: String` + `content: String`, opens native save dialog (Desktop default, .json filter), writes content to chosen path, returns `{ ok: bool, error? }`
- `import_profile` — opens native file picker (.json filter), reads file, returns `{ ok: bool, content?: String, error? }`. Frontend handles all validation and merging.
- `get_shared_config_path` — returns `Option<String>`, the current shared config directory path (or null if local)
- `set_shared_config_path` — takes `path: String` + `mode: Option<String>` ("use_existing" | "replace" | null). Validates folder, handles migration, writes local settings, starts file watcher. Returns `{ ok, existed }` or `{ ok: false, needs_choice: true }` if file exists and no mode specified.
- `clear_shared_config_path` — stops file watcher, clears override, writes local settings, returns `bool`
- `browse_for_image` — file picker filtered to PNG/JPG/JPEG, returns `Value::String(path)` or `Value::Null`
- `get_clipboard_history` — paginated: `{ page, per_page }` → `{ items, total }`
- `paste_clipboard_item` — reads item by ID, writes to clipboard, fires Ctrl+V with full suppress
- `delete_clipboard_item` — deletes single item by ID, returns `bool`
- `clear_clipboard_history` — deletes all items, returns `bool`
- `pin_clipboard_item` — toggles pin state `{ id, pinned }`, returns `bool`
- `get_clipboard_settings` / `set_clipboard_settings` — retention days from config
- `get_clipboard_image` — returns base64-encoded PNG blob for a single image item by ID
- `get_distinct_source_apps` — returns `Vec<String>` of distinct source_app values from clipboard history
- `close_clipboard_overlay` — hides clipboard overlay, restores focus
- `clipboard_overlay_resize` — resizes overlay window: `{ width, height }` (width 500–1200, height 60–600)
- `update_clipboard_item` — updates text content + auto-retags, returns new `content_tag`
- `get_clipboard_storage_size` — returns total bytes of clipboard DB (main + WAL + SHM files)
- `get_scratchpad` — reads `trigr-scratchpad.txt` from AppData, returns `String`
- `save_scratchpad` — writes text to `trigr-scratchpad.txt` in AppData, returns `bool`

---

## 07 — ARM64 Rules (CRITICAL)

Machine: Surface Pro, Windows ARM64. Every native Rust crate must be verified for ARM64 compatibility before implementation.

Known compatible:
- `windows-sys` — ARM64 compatible (Microsoft maintained) — used for all Win32 API
- `rusqlite` — ARM64 compatible (bundled feature)
- `serde_json` — ARM64 compatible (pure Rust)
- `tauri-plugin-updater` — ARM64 compatible
- `notify` v8 — ARM64 compatible (uses ReadDirectoryChangesW on Windows, platform-agnostic)
- `image` v0.25 — ARM64 compatible (pure Rust PNG/JPEG decoders, no native deps)

Note: `rdev` and `enigo` were evaluated and skipped — `windows-sys` SendInput/SetWindowsHookExW handles everything directly.

If any new crate is added, verify ARM64 compatibility before proceeding. Do not assume — test on device.

---

## 08 — Do Not Touch Rules

1. **E:\Development\Trigr-Reference** — read-only reference only. Never modify this directory.
2. **E:\Development\Trigr** (Electron production) — never touch during Tauri migration. Testers stay on Electron until Tauri is proven.
3. **keyforge-config.json filename** — must not change. Existing configs must load.
4. **React UI components** — CSS variables only for colours. Never hardcode hex values in CSS (except green dot #22c55e and status colours).
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

### Analytics Writer Thread
`analytics.rs` owns a dedicated background thread that holds the only `rusqlite::Connection`. All DB access goes through `AnalyticsMsg` enum sent via `mpsc::Sender`. `log_action()` is non-blocking (channel send only) — safe to call from any thread including the action executor and expansion threads. `get_stats()` and `reset_stats()` send a message and block on a reply channel (5s timeout). Never open a second connection or access the DB from any other thread. Time saved calculation: expansion = `char_count * 0.3s`, hotkey = `3.0s`, macro = `5.0s`. Character count excludes `\r` (CRLF correction).

### Vite Dev Server
`tauri.conf.json` has `"devUrl": "http://localhost:5173"`. Vite binds to default host (localhost). Vite watch config excludes `**/src-tauri/target/**` to prevent scanning Rust build artifacts.

### Atomic Ordering — SeqCst Only
All `AtomicBool` and `AtomicIsize` shared across threads in `hotkeys.rs`, `expansions.rs`, and `actions.rs` must use `Ordering::SeqCst`. `Ordering::Relaxed` causes silent failures on ARM64. This was bulk-upgraded in v0.1.14 — do not reintroduce `Relaxed` on any cross-thread atomic.

### Hook Heartbeat — Mouse Events
`HOOK_HEARTBEAT` must be incremented in BOTH `keyboard_hook_proc` AND `mouse_hook_proc`. Missing the mouse increment caused false-positive watchdog reinstalls every 30s during mouse-only activity (browsing, scrolling, clicking) because the heartbeat flatlined.

### Hook Reinstall — Atomic Reset
When `spawn_hook_thread()` runs (initial install or watchdog reinstall), it must reset ALL shared atomics to safe defaults after setting `HOOKS_RUNNING = true`: `INJECTION_IN_PROGRESS → false`, `SUPPRESS_SIMULATED → false`, `FILL_IN_ACTIVE → false`, `FILLIN_HWND → 0`, `MOD_CTRL/ALT/SHIFT/META → false`. Without this, stale values from a dead hook session corrupt the new hook (e.g. `INJECTION_IN_PROGRESS` stuck true buffers all keystrokes forever).

### Double Press Clear — Delete Both Entries
`handleClearKey` in App.jsx must delete both the single-press key (`Profile::Combo::KeyId`) AND the double-press key (`Profile::Combo::KeyId::double`). Failing to delete the `::double` entry leaves an orphaned assignment that still fires, appears in Quick Search, and persists in config.

### Autocorrect — Disabled for Alpha
Both custom (`GLOBAL::AUTOCORRECT::`) and built-in (`builtin_autocorrect()`) autocorrect checks are commented out in `check_space_trigger()`. The Autocorrect tab is hidden in TextExpansions.jsx. Space-triggered text expansions continue to work normally.

### Trailing Space — Synthetic Keystroke
Autocorrect/expansion trailing space is sent as a synthetic `VK_SPACE` keystroke via SendInput, NOT included in the clipboard paste string. Some apps (browsers, web inputs) strip trailing whitespace from clipboard paste.

### Structured Logging (tauri-plugin-log)
`tauri-plugin-log` is registered in the builder chain in lib.rs. Log file: `AppData\Local\com.nodescaffold.trigr\logs\trigr.log`. Targets: LogDir (file) + Stdout. 5MB max file size, KeepOne rotation, Info level. **CRITICAL:** Call `.clear_targets()` after `Builder::new()` before adding targets — the plugin ships with 2 default targets (Stdout + LogDir) and `.target()` appends, not replaces. Without `.clear_targets()`, every log entry is duplicated. All Rust modules use `log::info!()` / `log::error!()` / `log::warn!()` — never `println!()`. Settings panel has "Open logs folder" button.

### Config Factory Defaults
If `load_config_safe()` returns `(None, None)` (all config sources failed), the `load_config` Tauri command writes a factory-default config `{ "profiles": ["Default"], "assignments": {}, "activeProfile": "Default" }` via the atomic write path and returns it to the frontend. This ensures a valid config file always exists after the first load attempt.

### Bare Key Suppression
Bare key assignments (e.g. `Profile::BARE::KeyQ`) are added to the `SUPPRESS_KEYS` set with `modifier_bits = 0` so the LL hook callback suppresses the original keystroke via `return 1` (skipping `CallNextHookEx`). This only happens when the active profile is app-linked (`linkedApp` present in profile settings). `rebuild_suppress_keys` receives `profile_settings` as its third parameter and checks `is_linked` before inserting bare entries.

### ESC Key — Mappable, Cancel via Button
ESC is a mappable key in both hotkey recording and key capture. The 4 `key_id == "Escape"` → emit Null cancel branches were removed from hotkeys.rs (handle_keydown and handle_js_key_event). Cancel is handled by dedicated Cancel buttons in HotkeyCaptureInput, KeyCaptureInput. SettingsPanel pause/quick-search hotkey capture uses existing ✕ buttons. App.jsx document-level ESC handler has a `__trigr_capturing || __trigr_recording` guard to avoid swallowing ESC during active capture.

### Key Capture — No Blur Clearing
HotkeyCaptureInput and KeyCaptureInput do NOT use onBlur to clear capture state when Win builder mode is active. The Win key causes WebView2 to lose focus, and blur-based clearing dismisses the builder. Instead, when `winBuilder` is true, `handleBlur` calls `e.currentTarget.focus()` to immediately refocus.

### Win Key Capture — Known Limitation
Win key combinations (Win+Left, Win+D, etc.) cannot be reliably captured as hotkeys. The Win key is processed by DWM/Shell at the OS level before the LL hook can suppress it. Win key manual builder in MacroPanel provides a dropdown alternative for assigning Win+key combos.

### Macro Step Types
`execute_macro_step` in actions.rs handles 8 step types: Type Text, Press Key, Open App, Open URL, Open Folder, Focus Window, Wait (ms), Wait for Input. The `MACRO_STEP_TYPES` array in MacroPanel.jsx must match this exact order. Open App uses `ShellExecuteW` (not `Command::new`). Focus Window uses `EnumWindows` + `SetForegroundWindow` and writes `*target_hwnd` so subsequent steps fire into the focused window. Open Folder and Open URL use `opener::open()`. Step drag-and-drop uses `@dnd-kit/sortable` with stable runtime IDs (stripped before config save).

### Macro Step DnD — ID Stability
`@dnd-kit/sortable` step IDs (idMapRef in MacroSequenceForm) must be keyed by step TYPE only, not value. Including value in the stability check causes a new ID on every keystroke → React remounts the component → input focus lost. The ID regenerates only when the step type changes or steps are added/removed/reordered.

### List View Architecture
`listViewActive` state is owned by `App.jsx`, persisted in `localStorage` key `trigr_list_view`. Toggle button lives in `TitleBar.jsx` (`.tb-list-toggle`), only visible in mapping area. When active: `main-area` gets `.main-area--hidden` (flex: 0, width: 0), KeyboardCanvas is not rendered, and Sidebar gets `.sidebar--expanded` (flex: 1) filling all space left of MacroPanel (300px). Sidebar renders a modifier pill bar (Ctrl/Alt/Shift/Win/Bare + Record button) as the filter — no tabs in list view. Assignments display as a CSS grid of cards (`repeat(auto-fill, minmax(200px, 1fr))`). Gold header bars (`var(--accent)` background, `var(--bg-base)` text) render as group separators: multiple bars in unfiltered "All" view, single bar with modifier label + count when a specific modifier tab/pill is selected. Bare keys group sorts first. Cards show: key combo, label, type pill, and preview line. Classic sidebar (non-list-view) is unchanged at 200px with its own tabs. KeyboardCanvas has no list view code — it was fully removed in v0.1.15. Auto-switches to list view below 800px via `window.resize` listener with `wasInKeyboardModeRef` state memory — auto-restores keyboard view when width goes back above 800px unless user manually toggled.

### Focus Window — Pick Window
Focus Window macro step uses `FocusWindowFields` component in MacroPanel.jsx. Process name field replaced with a Pick Window button that calls `list_open_windows` (Rust command in lib.rs using `EnumWindows` + `IsWindowVisible` + `IsIconic` + `QueryFullProcessImageNameW`). Inline dropdown shows running windows, clicking one sets both process and title fields. Window title input remains editable. Value format unchanged: `{"process":"...","title":"..."}`.

### Starter Templates
Three packs defined in `TemplatesPanel.jsx`: General/Office (7 expansions + 1 hotkey), CAD/Engineering (5 expansions + 8 bare keys, requires Pick App flow), Sales/BD (7 expansions + 2 hotkeys). Import is additive only — `handleImportTemplate` in App.jsx skips existing keys. CAD pack uses `handleImportCadTemplate` which creates an app-specific profile (`CAD — exeName`) with `linkedApp` set, imports bare keys into that profile and expansions globally. TemplatesPanel is shared between TitleBar dropdown and SettingsPanel accordion. TitleBar pill button (`◈ Templates`) visible on mapping area, dismissible via right-click context menu ("Don't show this again") which sets `localStorage` key `trigr_templates_dismissed`. Settings accordion is permanent home (collapsed by default).

### Assignment Context Menu
Right-click on assigned cards (list view) or assigned keys (keyboard view) shows a context menu with Rename, Duplicate, Clear. Handlers in App.jsx: `handleRenameAssignment(combo, keyId, newLabel)` updates label in-place; `handleClearAssignment(combo, keyId)` deletes single + double entries; `handleDuplicateFromContext(combo, keyId)` deep clones single + double press into `pendingDuplicateRef` (`{ single, double }`) and auto-triggers Record. On key recorded: MacroPanel opens with `assignment={pendingDuplicateRef.current?.single}` and `doubleAssignment={pendingDuplicateRef.current?.double}`. On assign: `handleAssign` saves both single and double (if present) to the new key. On cancel/close: ref cleared, nothing saved. `handleDuplicateAssignment` (MacroPanel's built-in duplicate) also copies double press. Sidebar.jsx owns context menu state + inline rename input + inline clear confirmation for both renderItem and renderCard. KeyboardCanvas.jsx has its own context menu state with fixed-position popovers for rename and clear. CSS classes (`.assign-ctx-menu`, `.assign-ctx-item`, `.sidebar-rename-input`, `.sidebar-confirm-*`) defined in Sidebar.css, shared by both views. Empty keys on keyboard canvas: right-click does nothing (onContextMenu only attached when `isAssigned && !isSystem && !noLayer`).

### Send Hotkey Hold Mode
`holdMode` boolean field on Send Hotkey data: `{ modifiers, key, holdMode }`. Default false. When true: first press sends keydown only (no keyup), stores in `HELD_KEY` (static `Mutex<Option<HeldKeyState>>` in actions.rs with `target_vk`, `mod_vks`, `mouse_button`, `label`). Second press on same key sends keyup (or mouse up) + clears. Different hold-mode key: releases previous, holds new. `release_held_key()` public function checks `mouse_button` field — if `Some`, sends `MOUSEEVENTF_*UP` via `send_mouse_event()`; otherwise sends keyboard keyup. Called from: `toggle_macros` (lib.rs), `toggle_pause` (tray.rs), `quit_app` (lib.rs), and `handle_keydown` (hotkeys.rs — any non-modifier physical keypress releases held key). Works for both keyboard keys AND mouse buttons (LButton/RButton/MButton). Mouse hold sends `MOUSEEVENTF_*DOWN` only, release sends `MOUSEEVENTF_*UP`. Three tray icon states: `TRAY_ICON_NORMAL`, `TRAY_ICON_PAUSED` (alpha/3), `TRAY_ICON_HELD` (R+80, G/2, B/2 red tint). Held tooltip: "Trigr — Holding: [key] — press again to release". `execute_action` signature includes `trigger_key: Option<&str>` and `&AppHandle`. MacroPanel UI: hold mode toggle visible for ALL targets including mouse pills, mutually exclusive with repeat mode.

### Send Hotkey Repeat Mode
`repeatMode` boolean + `repeatInterval` number (ms) fields on Send Hotkey data. Default false / 100ms. Minimum interval clamped to 50ms in Rust. When active: first press of trigger hotkey starts a background thread that fires the target key/click repeatedly at the configured interval. Second press of the SAME trigger hotkey stops it (toggle behaviour — does NOT stop on any other keypress, unlike hold mode). Works for both keyboard keys and mouse buttons. **State:** `RepeatingKeyState` struct with `trigger_storage_key`, `label`, `interval_ms`, `stop: Arc<AtomicBool>` in `REPEATING_KEY` static `Mutex`. **Public functions:** `stop_repeating_key()`, `is_repeating()`, `get_repeating_trigger()`. **Second-press detection:** In `handle_keydown` (hotkeys.rs), after the storage key is built and before action dispatch, checks `is_repeating()` + `get_repeating_trigger()` — if match, stops repeat and returns (both bare and modified key paths). **Auto-stop:** `toggle_pause` (tray.rs), `toggle_macros` (lib.rs), `quit_app` (lib.rs), and pause hotkey handler (hotkeys.rs) all call `stop_repeating_key()`. Loop thread also checks `MACROS_ENABLED` as safety net. **Thread cleanup:** On exit, thread clears `REPEATING_KEY` only if its `Arc<AtomicBool>` still matches the active state (prevents clearing a newer repeat). **Tray:** `update_tray_icon_repeating(app, label, interval_ms)` reuses `TRAY_ICON_HELD` (red tint), tooltip: "Trigr — Repeating: [label] ([interval]ms) — press again to stop". **Trigger key threading:** `pending_trigger_key: Option<String>` field in `EngineState`, set alongside `pending_macro` in all paths (bare, modified, double-tap). Threaded through `handle_keyup` → `dispatch_with_double_tap` → `fire_macro` → `execute_action` → `execute_send_hotkey`. **UI:** Mutually exclusive with hold mode — enabling one disables the other. Repeat mode shows hint text + interval input (min 50ms, default 100ms). Both toggles visible for all targets including mouse.

### Mouse Click in Press Key / Send Hotkey
`send_mouse_click(button)` in actions.rs fires `INPUT_MOUSE` SendInput (down + up) at current cursor position (dx: 0, dy: 0, no MOUSEEVENTF_ABSOLUTE). `send_mouse_event(button, is_up)` sends a single mouse event (down or up only) — used by hold mode. Both accept "LButton", "RButton", "MButton". Set `SUPPRESS_SIMULATED` before/after. Wired into: (1) `execute_macro_step` Press Key — if value is a mouse button name, calls `send_mouse_click()` instead of keyboard path. (2) `execute_send_hotkey` — mouse buttons supported in all three modes: normal (full click), hold (down only / up on release), repeat (full click per iteration). UI: `MOUSE_CLICK_OPTIONS` array in MacroPanel.jsx. Three pill buttons rendered in `.press-key-pills` grid (3-column `minmax(0, 1fr)`) as a sub-row below the Press Key dropdown+capture field row. Send Hotkey form has its own `.mouse-click-pills` flex row below `HotkeyCaptureInput`. All pill styling uses CSS variables only.

### Press Key Macro Step — Two-Row Layout
Press Key capture field is inline in `.macro-step-row` (same row as dropdown + remove button). Mouse pills are in a `.press-key-sub-row` below with `padding: 4px 30px 6px 48px` (aligns left with dropdown, right with capture field). No `border-top` on sub-rows — compact layout. `grid-template-columns: repeat(3, minmax(0, 1fr))` prevents pill overflow.

### Macro Sequence Clipboard Batching
The macro loop in `execute_action` saves the clipboard ONCE before all steps, uses `clipboard_paste_core()` for consecutive Type Text steps (no per-step save/restore), and restores the clipboard ONCE after the sequence ends (or before a non-Type-Text step). This eliminates the 50ms `CLIPBOARD_RESTORE_DELAY` between consecutive Type Text steps. A 5-step text macro went from ~360ms to ~160ms overhead.

### Macro Speed Presets
`macro_speed` field in `EngineState` (hotkeys.rs): `"safe"` | `"fast"` | `"instant"` | `"custom"`. `speed_delays()` in actions.rs returns `(initial_delay, step_settle, foreground_settle, clipboard_restore)` tuple based on preset:
- **Safe** (default): 10/10/10/50ms
- **Fast**: 5/5/5/25ms
- **Instant**: 0/0/5/25ms
- **Custom**: derives from `custom_pre_execution_delay` and `custom_keystroke_delay` fields in EngineState (set from frontend slider values via `updateGlobalSettings`)

All hardcoded delay constants (`INITIAL_DELAY_MS`, `MACRO_TRIGGER_DELAY_MS`, `HOTKEY_SETTLE_MS`, `CLIPBOARD_RESTORE_DELAY_MS`) were removed and replaced with `speed_delays()` calls. Presets are radio buttons in SettingsPanel Compatibility section (`MACRO_SPEED_PRESETS` array). Selecting a preset updates slider values (Keystroke delay, Pre-execution delay, Double-tap window). Manually adjusting any slider auto-switches to "Custom". Config field: `macroSpeed`, persisted via `saveConfig`.

### Profile Link/Unlink via Context Menu
Profile right-click context menu includes "Link to App" (for static non-Default profiles) and "Unlink App" (for app-linked profiles). Link opens an inline Pick App picker inside the accordion with "⊞ Pick App" (calls `list_open_windows`) and "Browse…" (calls `browseForFile`, extracts filename). Uses `handleUpdateProfileSettings(profileName, { linkedApp: exeName })` — existing handler, no new Rust code. Unlink calls same handler with `{ linkedApp: null }`. Profile automatically moves between STATIC and APP-SPECIFIC groups on re-render.

### Category Tab Reorder — dnd-kit
Category tabs in TextExpansions.jsx use `@dnd-kit/sortable` with `horizontalListSortingStrategy`. `SortableCatTab` wrapper component. "All" and "Uncategorised" tabs are fixed (outside DndContext). `DragOverlay` shows a ghost tab. `handleCatDragEnd` uses `arrayMove` and calls `onReorderCategories`. Native HTML drag-and-drop was removed — all old `dragCat`/`dragOverCat`/`dragOverSide` state and handlers deleted.

### Date Tokens
Seven tokens in expansions.rs: `{date:DD/MM/YYYY}` (%d/%m/%Y), `{date:DD/MM/YY}` (%d/%m/%y), `{date:MM/DD/YYYY}` (%m/%d/%Y), `{date:YYYY-MM-DD}` (%Y-%m-%d), `{time:HH:MM:SS}` (%H:%M:%S), `{time:HH:MM}` (%H:%M), `{dayofweek}` (%A). All use `chrono::Local::now()` + `str::replace`. INSERT_MENU in TextExpansions.jsx lists all tokens for the editor dropdown.

### Expansion Injection Architecture
All text expansions use `inject_via_clipboard()` (clipboard paste). `should_use_clipboard()` unconditionally returns `true`. A `inject_via_sendinput()` function exists in expansions.rs (KEYEVENTF_UNICODE batched SendInput, surrogate pair support via `encode_utf16()`, single `SendInput` call) but is **dead code** — unreachable from both `fire_expansion()` and `fire_expansion_with_fillin()`. It was built and tested in v0.1.24 but disabled because clipboard injection is more reliable across all app types (terminals, admin apps, RDP, games). The `SUPPRESS_NEXT_CLIPBOARD_WRITE = false` cleanup after injection is conditional on the `used_clipboard` flag in both fire functions. Terminal process detection (`is_terminal_process()` with cmd/powershell/pwsh/windowsterminal/wt/mintty/conhost) also exists but is currently unused since clipboard is always chosen.

### Image Expansion Type
New expansion type: `expansionType: "image"`. Config data: `{ expansionType: "image", imagePath: "C:\\path\\to\\file.png", imageScale: 100, triggerMode, category, displayName }`. No `html`/`text` fields. Supported formats: PNG and JPG only. `image` crate (v0.25, `png`+`jpeg` features only, pure Rust decoders, ARM64 compatible) decodes the file. `imageScale` integer 10–100 (default 100) — if < 100, `image::resize_exact` with `Lanczos3` filter applied before clipboard write.

**Clipboard format:** `write_clipboard_image` in expansions.rs writes two formats in a single `OpenClipboard` → `EmptyClipboard` → `CloseClipboard` session: (1) `CF_DIB` (format 8) — `BITMAPINFOHEADER` (40 bytes) + BGRA bottom-up pixel data; (2) PNG stream — `RegisterClipboardFormatW("PNG")` custom format with the raw PNG file bytes (Word, Outlook, and other Office apps prefer this over CF_DIB). No `CF_UNICODETEXT` — including a text format causes rich text editors (Outlook, Google Docs) to show a "Paste Options" dialog. For PNG source files at 100% scale, the original file bytes are used directly for the PNG stream. For JPEG sources or resized images, the final image is re-encoded as PNG via `img.write_to(..., ImageFormat::Png)`. `SUPPRESS_NEXT_CLIPBOARD_WRITE` set `true` before clipboard write (SeqCst).

**Execution:** `fire_image_expansion` follows the same pattern as `fire_expansion`: `InjectionGuard`, `SUPPRESS_SIMULATED`, modifier release, `Ctrl+V` or `Shift+Insert` paste. No trailing space after image paste. No clipboard restore — image left on clipboard. Silent failure: if file not found or decode fails, `log::warn!()` and return — no user-facing error. Dispatch fork in both `check_space_trigger()` and `check_immediate_triggers()`: checks `data.expansionType`, if `"image"` calls `fire_image_expansion()` instead of `fire_expansion()`. Analytics: `log_action("expansion", 0)` — char_count 0 for images.

**UI:** `browse_for_image` Tauri command in lib.rs (PNG/JPG filter). Text/Image type selector pills in edit panel, "Choose Image…" button, thumbnail preview via `convertFileSrc()` (asset protocol enabled), scale input (10–100%), "File not found" state via `<img>` onError. IMG badge in expansion list. Quick Search shows `[IMG] filename` preview for image expansions. `protocol-asset` feature added to tauri Cargo.toml, CSP updated with `asset:` and `https://asset.localhost` for img-src. Type filter pills (All/Text/Image) above expansion list — client-side filter, works alongside category tabs. `typeFiltered` derived array applies type filter before category/sort logic.

### Clipboard Manager Architecture
**Listener:** Message-only HWND (`HWND_MESSAGE` parent) created via `CreateWindowExW` on a dedicated thread. Registered with `AddClipboardFormatListener`. Receives `WM_CLIPBOARDUPDATE` via `GetMessageW` loop. Checks `SUPPRESS_NEXT_CLIPBOARD_WRITE.load(SeqCst)` — if true, skips capture entirely (prevents capturing Trigr's own expansion/paste clipboard writes). Skips `CF_HDROP` (file copies). Captures `CF_UNICODETEXT` (text) and `CF_DIB` (images — supports BI_RGB compression=0 and BI_BITFIELDS compression=3, decoded to PNG via `image` crate). Deduplication via `DefaultHasher` hash of content.

**Source app capture:** Before `OpenClipboard`, calls `GetForegroundWindow()` → `GetWindowThreadProcessId()` → `OpenProcess()` → `QueryFullProcessImageNameW()` to get the exe name (e.g. "chrome.exe"). Stored in `source_app` column. Implemented directly in clipboard.rs — does not use foreground.rs.

**Auto-tagging:** At capture time, `auto_tag()` analyses text content and assigns `content_tag`. Priority order: `content_type == "image"` → "Image"; starts with `http://`/`https://` → "Link"; contains `@` with dot after → "Email"; matches `#hex`/`rgb(`/`rgba(` → "Colour"; purely numeric with optional `£$€%` → "Number"; everything else → "Text".

**Storage:** `trigr-clipboard.db` in AppData, SQLite via `rusqlite` (same writer thread + `mpsc` channel pattern as `analytics.rs`). Table `clipboard_history`: id, timestamp, content_type, text_content, image_blob (BLOB, PNG bytes), image_width, image_height, preview, pinned, source_app, content_tag. Schema migration: `ALTER TABLE ADD COLUMN` for source_app and content_tag — silently ignored if columns already exist. Images stored as inline PNG BLOBs. Auto-prune on each insert: deletes unpinned entries older than retention_days. **Query ordering:** `ORDER BY pinned DESC, id DESC` — pinned items always returned first regardless of age, then most recent. **Preview truncation:** Uses `char_indices().nth(200)` for char-boundary-safe slicing — raw byte slice `&text[..200]` panics on multi-byte characters (e.g. box-drawing `─`).

**Paste:** `paste_clipboard_item` uses `CLIPBOARD_OVERLAY_TARGET` (stored when overlay opened) for target HWND. Sets `SUPPRESS_NEXT_CLIPBOARD_WRITE = true` before writing item to clipboard, fires Ctrl+V, then clears. Text items restore previous clipboard; image items use CF_DIB + PNG stream only (no text formats — prevents "Paste Options" dialog in rich text editors). **Hotkey:** Ctrl+Shift+V (mod_bits=3, vk=0x56) in `EngineState.clipboard_paste_hotkey`. Intercept in `handle_keydown` after overlay hotkey, before pause. Emits `"toggle-clipboard-overlay"` event. Added to `SUPPRESS_KEYS` via `add_clipboard_paste_to_suppress()`. **Overlay:** Pre-created hidden window `"clipboardoverlay"` (`index.html?clipboardoverlay=1`), transparent, always-on-top, master-detail two-pane layout. Left: search + compact row list with source_app labels. Right: full detail view with metadata + Pin/Delete/Paste buttons. Arrow key nav, Enter to paste, Escape to close. Auto-hide on blur. Hidden scrollbar on left pane. **Panel:** Masonry 3-column grid (`ClipboardPanel.jsx`). Tag filter pills (All/Text/Link/Email/Colour/Number/Image) + source app dropdown. Cards show tag pill, source_app badge, pin indicator. Colour cards show swatch. Image cards show lazy-loaded thumbnails via `get_clipboard_image` (base64 encoded in Rust without external crate). **Settings:** Retention days in SettingsPanel (free: 1–7, Pro: up to 30). `clipboardRetentionDays` field in config.

**Timeline grouping:** Both ClipboardPanel and ClipboardOverlay group items by age: Pinned (always first), Today, Yesterday, This Week, This Month, Older. `groupByTimeline()` function in each component. Panel renders `.cbg-timeline-header` per group above each grid section. Overlay uses `groupedFlat` memo (array of `{ type: 'header', label }` and `{ type: 'item', item, flatIndex }` entries) to interleave `.co-timeline-header` divs with rows while preserving flat index for keyboard navigation.

**Storage size indicator:** `get_clipboard_storage_size` Tauri command returns total bytes of `trigr-clipboard.db` + WAL + SHM files. `DB_PATH: OnceLock<PathBuf>` static stored in `clipboard::init()`. `formatStorageSize()` in ClipboardPanel formats as B/KB/MB/GB. Displayed in `.cbg-storage-size` span in toolbar.

**Overlay selection:** `selected` in ClipboardOverlay resolves through `groupedFlat.find(e => e.flatIndex === selectedIndex)`, NOT `filtered[selectedIndex]`. `groupByTimeline` reorders items (Pinned first, then Today, etc.) so `filtered` indices don't match visual positions. Using `filtered[selectedIndex]` caused the detail pane, Enter key, and all detail buttons to target the wrong item — offset by the number of pinned items.

**Zoomable image preview:** `ZoomableImage.jsx` component used in both ClipboardPanel and ClipboardOverlay detail panes. Scroll to zoom (1x–5x), drag to pan when zoomed. Zoom badge shows percentage. Resets on image change. `ImageThumb` accepts `zoomable` prop — when true, renders `ZoomableImage` instead of plain `<img>`.

**Inline editing (both views):** Text-only items (content_tag Text or Number) can be edited inline. `editing` + `editText` state, `<textarea>` replaces `<pre>` when editing. Calls `update_clipboard_item` IPC → auto-retags → updates preview in local state. ClipboardPanel: Edit/Save/Cancel buttons in detail actions. ClipboardOverlay: same pattern, keyboard nav suppressed during editing (Escape cancels edit instead of closing overlay).

**Gold accent styling:** Both CSS files match Quick Search overlay design language. Search bars use `var(--border-accent)` (gold tint) border + `var(--bg-elevated)` background + `caret-color: var(--accent)`. No drop shadows on panels/cards/context menus. Tag pills, timeline headers, button hovers all use `var(--accent)` gold. Overlay panel border: `var(--border-accent)`. **Overlay list rows:** Flat full-width style (no card borders/border-radius), `border-bottom: 1px solid var(--border)` separators, `10px 12px` padding. Selected row: `rgba(232, 160, 32, 0.18)` gold background fill + `.co-row-text` turns `var(--accent)`. Hover: `rgba(232, 160, 32, 0.06)`. Light theme selection: `0.16` opacity via `[data-theme="light"]` override.

### Bare Mouse Button Remapping
All mouse buttons (Left, Right, Middle, Side1, Side2, Scroll Up/Down) can be assigned as bare (no modifier) in app-linked profiles. Bare mouse assignments act as **full button replacements**: the original click is suppressed at the LL hook level and only the assigned action fires. Modifiers pass through naturally — e.g., if bare Right Click → Middle Click (hold), then Shift+Right Click → Shift+Middle Click because Shift is physically held.

**Hook suppression:** `SUPPRESS_BARE_MOUSE` static `RwLock<HashSet<u8>>` (same pattern as `SUPPRESS_KEYS`). Populated by `rebuild_suppress_keys` for bare `MOUSE_*` entries in app-linked profiles. Checked in `mouse_hook_proc` regardless of modifier state — if the button is in the set and `MACROS_ENABLED`, the hook returns 1 (swallows the click) after `send_event()`. Constants: `SUPPRESS_MOUSE_LEFT=1` through `SUPPRESS_MOUSE_SCROLL_DOWN=7`.

**Modified fallthrough:** In `handle_mouse_down`, when modifiers are held and no explicit modifier assignment exists (e.g. no `Profile::Shift::MOUSE_RIGHT`), falls through to check for a bare assignment in the same app-linked profile. This enables modifier passthrough for button remaps.

**Press-hold mirroring:** When a bare mouse button triggers a Send Hotkey with hold mode, the hold mirrors the physical button state: button DOWN → target DOWN, button UP → target UP. `HeldKeyState` has `trigger_mouse_id: Option<String>` tracking which mouse button triggered the hold. `handle_mouse_up` calls `release_held_if_mouse_trigger(mouse_id)` to release on button-up. Keyboard-triggered holds retain toggle behavior (press to hold, press again to release).

**Frontend:** `BARE_MOUSE_ALLOWED` in MouseCanvas.jsx includes all 7 zones. All zones are clickable in Bare mode. Advisory text says "all mouse buttons".

### Clipboard Overlay Scratchpad
Slide-out scratchpad pane on the right side of the clipboard overlay (Ctrl+Shift+V). Toggle via slim arrow button (numpad-style `◂`/`▸`) attached to the right edge of `.co-panel`. Arrow + scratchpad sit OUTSIDE `.co-panel` in the `.co-root` flex container so the main panel (list + detail) never changes size.

**Persistence:** `trigr-scratchpad.txt` plain text file in AppData dir. Read by `get_scratchpad` command and sent in `show_clipboard_overlay` payload alongside clipboard data. Written by `save_scratchpad` command. Auto-saves 400ms after typing stops (debounced in React). Persists through restarts/reboots.

**Layout:** `.co-panel` has `flex: 0 0 auto; width: 730px` (fixed). `.co-pad-slide` uses `max-width: 0` → `max-width: 300px` CSS transition (200ms ease). `.co-pad` inside is 280px wide with gold accent border, rounded right corners. Window resizes from 750px (collapsed) to 1050px (open) via `clipboard_overlay_resize(width, height)`.

**State:** `padOpen` boolean persisted in `localStorage` key `trigr_scratchpad_open`. `padText` string loaded from overlay data event. Keyboard nav pauses when textarea focused (`.co-pad-textarea` class check in keydown handler), Escape refocuses main search input.

### Uncategorised Expansion Category
The "Uncategorised" tab uses internal value `'__uncategorised__'` for `activeCategory` state. The `openAdd()` function in TextExpansions.jsx must treat this the same as `'All'` — set `category: null`, NOT `category: '__uncategorised__'`. The filter at render time checks `e.category == null` for uncategorised items. Writing `"__uncategorised__"` to config makes the expansion invisible in the UI.

### Input Method — Simplified
UI shows 3 options: Global default (`"global"`), Direct (`"direct"`), Clipboard (`"shift-insert"`). "SendInput API" and "Clipboard (Ctrl+V)" removed from UI — both were identical to existing options at the Rust level. Existing configs with `"ctrl-v"` or `"send-input"` still work at the Rust level.

### Profile Accordion — Sidebar
Profiles live in the sidebar (Sidebar.jsx ProfileAccordion), NOT the titlebar. TitleBar.jsx has no profile code. Profiles split into STATIC and APP-SPECIFIC groups with separate SortableContext instances. Cross-group drag is blocked. Default profile is always first in STATIC, not draggable. Green dot indicates activeGlobalProfile (fallback). `.profile-accordion` is a flex column with `flex-shrink: 1` and `min-height: 0` — no max-height cap. It grows to fit all profiles and only scrolls when window height forces it. `.profile-accordion-list` has `overflow-y: auto` and `min-height: 0`. The assignments list below (`.sidebar-list` / `.sidebar-grid-wrap`, `flex: 1`) fills remaining space.

### Profile Export/Import
**Export:** Right-click context menu "Export Profile" on any profile row (including Default). `handleExportProfile` in App.jsx collects all assignments with `profileName::` prefix, builds `{ trigr_profile: "1.0", name, assignments, linkedApp: null }`. `linkedApp` is always `null` (machine-specific). Calls `export_profile` Rust command (save dialog, Desktop default, `<ProfileName>-trigr-profile.json`). **Import:** "↓ Import Profile" button in ProfileAccordion footer (always visible, below Add Profile). `handleImportProfile` in App.jsx calls `import_profile` Rust command (file picker), validates `trigr_profile` field. On name collision: shows inline Copy/Overwrite prompt in accordion footer (`importPrompt` state in App.jsx, `profile-import-prompt` UI in Sidebar.jsx). **Copy** deduplicates name with ` (1)` / ` (2)` suffix, creates new profile. **Overwrite** deletes all existing `profileName::` assignments, writes imported assignments with keys rewritten via split-on-`::`-replace-index-0-rejoin, preserves existing `linkedApp` and profile position unchanged. No collision: imports directly. Both paths call `saveConfig` + `syncEngine`, switch active profile, show toast. Props: `importPrompt`, `onImportProfileResolve(choice)`, `onImportPromptDismiss` passed from App.jsx through Sidebar to ProfileAccordion. Prompt dismisses on outside click or Escape.

### ResizeObserver Safety
Any ResizeObserver that calls setState must guard against infinite loops. Store last measured width in a ref; skip callback if `Math.abs(newWidth - lastWidth) < 1`. The profile tab overflow attempt (now removed) proved this causes system freezes without the guard.

### Help Window — External Browser
`open_help` in lib.rs uses `opener::open("https://usetrigr.com/trigr-help.html")` to open the user guide in the default browser. DO NOT create a Tauri WebviewWindow for help — a 3.2MB HTML file with inline base64 images freezes WebView2 and makes the entire app unresponsive (P0 bug in v0.1.20). The help page is hosted on Netlify (usetrigr.com) and no longer bundled in the app (`public/help.html` was deleted in v0.1.21).

### Shared Config Path (Cloud Sync)
**Local settings file:** `trigr-local-settings.json` in AppData dir (alongside `keyforge-config.json`). JSON format: `{ "shared_config_path": "C:\\path\\to\\folder" }`. This file is machine-specific — it MUST NOT be placed in the shared folder and MUST NOT sync between machines.

**Config path resolution:** `config_path()` in config.rs checks `SHARED_CONFIG_DIR: RwLock<Option<PathBuf>>` first. If set and the directory exists, returns `shared_dir.join("keyforge-config.json")`. Otherwise falls back to `APP_DATA_DIR.join("keyforge-config.json")`. `backup_dir()` ALWAYS uses `APP_DATA_DIR` — backups never follow the shared path.

**Load order at startup:** (1) `config::init()` reads `trigr-local-settings.json` from AppData. (2) If `shared_config_path` exists and is non-empty, sets `SHARED_CONFIG_DIR`. (3) lib.rs setup starts file watcher if shared dir exists, or spawns a 30-second reconnection poller if the dir doesn't exist yet (drive disconnected).

**File watcher:** `notify` v8 crate (`ReadDirectoryChangesW` on Windows, ARM64 verified). Watches the shared directory (non-recursive). **Debounce:** 2-second quiet window — timer resets on each qualifying event, only fires after 2 seconds of no activity. **Filename filter:** Only reacts to `keyforge-config.json` events. Ignores temp files: `~$*`, `.~*`, `*.tmp`, `*.gstmp` (sync client artifacts). **Self-write suppression:** `SELF_WRITE_IN_PROGRESS` AtomicBool (SeqCst) set before `save_config` writes, cleared after. `LAST_WRITTEN_HASH` stores FNV-1a hash of last written content — skips reload if hash matches. **Lock retry:** 3 attempts × 500ms delay for sync-client-locked files. **On valid change:** emits `config-reloaded-from-sync` Tauri event with the parsed config JSON. Frontend re-applies all state and shows "Config updated from sync" info toast.

**Migration:** When user sets a shared folder: (1) If `keyforge-config.json` doesn't exist there, copies current config to new location. (2) If file already exists, prompts "Use Existing" (loads the shared file) or "Replace with Mine" (overwrites with current config). Old AppData config preserved as implicit backup.

**Revert:** "Use Local Config" button in Settings clears `SHARED_CONFIG_DIR`, stops file watcher, clears `trigr-local-settings.json`. Shared file is NOT deleted. Trigr returns to reading from AppData.

**UI location:** Inside PRIVACY & SECURITY section of SettingsPanel.jsx, below the "Open logs folder" button. Shows "Shared" badge + path when active, "Set Shared Folder…" button when local.

---

## 11 — Tauri Config Reference

```json
{
  "productName": "Trigr",
  "identifier": "com.nodescaffold.trigr",
  "version": "0.1.34",
  "build": { "devUrl": "http://localhost:5173" },
  "app": {
    "windows": [{
      "title": "Trigr",
      "width": 1200, "height": 800,
      "minWidth": 800, "minHeight": 500,
      "resizable": true, "decorations": false, "visible": false
    }],
    "security": {
      "assetProtocol": { "enable": true, "scope": ["**"] }
    }
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
| 2026-04-03 | Post-MVP | Local analytics feature | analytics.rs: SQLite `trigr-analytics.db` in AppData, dedicated writer thread via mpsc channel, `action_log` table. Instrumented all fire points: `fire_macro()` in hotkeys.rs, `fire_expansion()`/`fire_expansion_with_fillin()` in expansions.rs, `execute_search_result()` overlay path in lib.rs. Time saved: expansion=chars×0.3s (excluding \r), hotkey=3s, macro=5s. Stats: total, today, last 7 days, best day (MAX daily SUM), best 7 days (rolling window self-join). AnalyticsPanel.jsx: compound Today/Last 7 Days cards, 4-column records row, breakdown bars, reset with confirmation. Third nav tab in TitleBar. `rusqlite` 0.31 with bundled feature (ARM64 compatible). Privacy text updated in SettingsPanel. v0.1.11 released. |
| 2026-04-04 | Post-MVP | Macro step types + UI overhaul | **New macro steps:** Open App (ShellExecuteW + args), Open Folder (opener::open), Focus Window (EnumWindows + process/title match + SetForegroundWindow with mutable target_hwnd), Open URL (already existed). `MACRO_STEP_TYPES` = 8 types. All sub-row step UIs (Type Text, Open App, Open Folder, Focus Window, Open URL) moved to full-width rows below the step type dropdown. **@dnd-kit/sortable** replaces HTML5 drag-and-drop: stable runtime IDs via idMapRef keyed by step type only (not value — prevents focus loss on keystroke), PointerSensor with 8px distance, DragOverlay ghost. **Win key manual builder:** Meta key during capture switches to dropdown builder (Win+key selection). Win builder blur guard refocuses field when OS steals focus. **ESC mappable:** Removed 4 `key_id == "Escape"` cancel branches from hotkeys.rs, cancel via UI buttons only. App.jsx ESC guard for capture mode. **Input method simplified:** 5 options → 3 (Global default, Direct, Clipboard). **Cargo.toml:** Added `Win32_UI_Shell` feature for ShellExecuteW. |
| 2026-04-04 | Post-MVP | Profile accordion in sidebar | Profiles moved from titlebar to sidebar accordion. TitleBar stripped to logo + nav tabs + right controls only. ProfileAccordion: collapsed header shows fallback profile (green dot) + editing profile name. Expanded: two groups (STATIC / APP-SPECIFIC) with separate SortableContext instances, cross-group drag blocked. Right-click context menu (Rename, Duplicate, Set as default fallback, Delete). Green dot fallback indicator on activeGlobalProfile. @dnd-kit/sortable for profile reordering. Default profile always first, not draggable. minWidth increased to 800px. Vite watch excludes src-tauri/target. |
| 2026-04-04 | Release | v0.1.13 released | Patch release. All post-MVP work from 2026-04-04 sessions included (macro step types, UI overhaul, profile accordion). |
| 2026-04-04 | Post-MVP | Bugfixes + list view + hook hardening | **Vite IPv4:** `host: '127.0.0.1'` in vite.config.js + `devUrl` in tauri.conf.json — fixes ARM64 WebView2 dev mode. **Ordering::SeqCst:** All cross-thread atomics in hotkeys.rs/expansions.rs/actions.rs upgraded from Relaxed. **Double-press clear:** `handleClearKey` now deletes `::double` entry too. **Hook heartbeat:** `mouse_hook_proc` now increments `HOOK_HEARTBEAT` — fixes false-positive 30s watchdog reinstalls during mouse-only activity. **Hook reinstall atomic reset:** `spawn_hook_thread()` resets INJECTION_IN_PROGRESS, SUPPRESS_SIMULATED, FILL_IN_ACTIVE, FILLIN_HWND, modifier state after reinstall. **List view:** Toggle in KeyboardCanvas.jsx — flat assignment table, modifier-layer filtered, localStorage persisted, auto-switch at 850px, narrow-responsive (hides TYPE column). Toggle button absolutely positioned against keyboard-canvas-wrap to avoid shrinking keyboard. **Autocorrect log:** Clarified message (engine disabled for Alpha). |
| 2026-04-04 | Release | v0.1.14 released | Patch release. All bugfixes + list view from this session. |
| 2026-04-05 | Post-MVP | List view refactor | **State lifted:** `listViewActive` moved from KeyboardCanvas local state to App.jsx with localStorage persistence. Toggle button moved to TitleBar (`.tb-list-toggle`). **Layout:** When active, `main-area` collapses (`main-area--hidden`), Sidebar expands to `flex: 1` (`sidebar--expanded`), KeyboardCanvas not rendered. **Grid view in Sidebar:** Modifier pill buttons (Ctrl/Alt/Shift/Win/Bare) + Record button as filter bar. CSS grid cards (`repeat(auto-fill, minmax(200px, 1fr))`) with combo, label, type pill, preview. Tabs removed from list view — pills are sole filter. **Gold group headers:** `var(--accent)` background, `var(--bg-base)` text, uppercase, full-width bars in both grid and classic sidebar views. **Bare keys first:** Group sort comparator updated in both views. **Bare suffix stripped:** Cards show "Q" not "Q (bare)". **KeyboardCanvas cleanup:** Removed AssignmentList, buildAssignmentList, formatKeyId, userPref/narrow/wrapRef state, ResizeObserver, list-toggle-btn, all list-view CSS (~232 lines). Removed `assignments`/`activeProfile` props. ModifierBar `narrow` prop removed. |
| 2026-04-05 | Release | v0.1.15 released | Patch release. List view refactor from this session. |
| 2026-04-05 | Post-MVP | Pick Window + auto list view + templates | **Pick Window:** New `list_open_windows` Rust command (EnumWindows + IsIconic + QueryFullProcessImageNameW). FocusWindowFields component replaces manual process input with Pick Window button + inline dropdown. **Auto list view:** 800px breakpoint via window resize listener, `wasInKeyboardModeRef` for state memory — auto-restores keyboard mode when widened unless user manually toggled. **Starter Templates:** TemplatesPanel.jsx — 3 packs (General/Office, CAD/Engineering, Sales/BD). Additive import via `handleImportTemplate`/`handleImportCadTemplate` in App.jsx. CAD pack: 8 bare key Type Text commands (FILLET, EXPLODE, etc.), Pick App flow creates app-specific profile. Templates moved from TextExpansions to shared TemplatesPanel component. TitleBar pill button with right-click dismiss context menu + localStorage persistence. SettingsPanel accordion (collapsed by default) as permanent home. All template code removed from TextExpansions.jsx/css. Onboarding Step 5 hint updated to "Settings → Templates". |
| 2026-04-05 | Release | v0.1.16 released | Patch release. Pick Window, auto list view, starter templates from this session. |
| 2026-04-05 | Post-MVP | Assignment context menu + P0 fix | **Right-click context menu:** Rename (inline input), Duplicate (auto-triggers Record), Clear (inline Yes/No confirmation) on both list view cards (Sidebar.jsx) and keyboard canvas keys (KeyboardCanvas.jsx). Three new handlers in App.jsx: `handleRenameAssignment`, `handleClearAssignment`, `handleDuplicateFromContext`. Key component gains `onContextMenu` prop, only attached to assigned non-system keys. **P0 crash fix:** Restored `panelMode` useState declaration accidentally removed from TextExpansions.jsx during templates cleanup — clicking Text Expansion tab crashed the app. |
| 2026-04-05 | Release | v0.1.17 released | Patch release. Assignment context menu + P0 crash fix. |
| 2026-04-05 | Post-MVP | Hold mode + duplicate fix + profile linking + tokens + category dnd | **Send Hotkey hold mode:** `holdMode` bool on hotkey data, `HELD_KEY` Mutex in actions.rs, three tray icon states (normal/paused/red-held), `release_held_key()` auto-release on keypress/pause/exit, `execute_action` now takes `&AppHandle`. MacroPanel toggle switch UI. **Duplicate deep clone fix:** `handleDuplicateFromContext` now deep clones single + double press into `pendingDuplicateRef { single, double }`. `handleAssign` saves both. `handleDuplicateAssignment` also copies double press. MacroPanel receives pending duplicate via `assignment`/`doubleAssignment` props. **Profile link/unlink:** Context menu "Link to App" (Pick App picker + Browse button) and "Unlink App" in ProfileAccordion. Uses existing `handleUpdateProfileSettings`. **{date:DD/MM/YY}** short date token added to expansions.rs + INSERT_MENU. **Category tab dnd-kit:** Replaced broken native HTML drag with `@dnd-kit/sortable` + `horizontalListSortingStrategy`. SortableCatTab component, DragOverlay ghost. Old drag state removed. |
| 2026-04-05 | Release | v0.1.18 released | Patch release. Hold mode, duplicate fix, profile linking, tokens, category dnd from this session. |
| 2026-04-05 | Post-MVP | Profile export/import + accordion fix | **Profile export:** Right-click "Export Profile" on any profile row. `export_profile` Rust command (save dialog + file write). Payload: `{ trigr_profile: "1.0", name, assignments, linkedApp: null }`. **Profile import:** "↓ Import Profile" button in accordion footer. `import_profile` Rust command (file picker + read). Validates `trigr_profile` field. Name collision shows inline Copy/Overwrite prompt (`importPrompt` state in App.jsx, `profile-import-prompt` UI in Sidebar.jsx). Copy = dedup with ` (1)` suffix. Overwrite = delete existing `profileName::` assignments, write imported, preserve linkedApp. Key rewriting via split-on-`::`-replace-index-0-rejoin. **Accordion height fix:** Removed `max-height: 280px` cap on `.profile-accordion-list`. `.profile-accordion` now `flex-shrink: 1` + `display: flex` + `flex-direction: column` + `min-height: 0`. Accordion grows to fit content, scrolls only when window forces it. |
| 2026-04-05 | Release | v0.1.19 released | Patch release. Profile export/import with Copy/Overwrite, accordion height fix. |
| 2026-04-06 | Post-MVP | Gold group headers on modifier tabs + help window fix | **Gold headers:** Added `sidebar-grid-group-header` to both classic sidebar (individual `activeTab`) and expanded list view (individual modifier pill) branches in Sidebar.jsx — previously only rendered in "All" tab. **P0 help window fix:** `open_help` changed from `WebviewWindowBuilder` (froze WebView2 on 3.2MB HTML) to `opener::open()` with GitHub Pages URL. `public/help.html` deleted — no longer bundled. Google Fonts `<link>` in help.html replaced with local `@font-face` (applies to hosted version). |
| 2026-04-06 | Release | v0.1.20 released | Patch release. Gold group headers on modifier tabs. |
| 2026-04-06 | Release | v0.1.21 released | Patch release. P0 help window fix (opener::open to GitHub Pages), public/help.html deleted. |
| 2026-04-07 | Post-MVP | Mouse click in Press Key + Send Hotkey | `send_mouse_click()` in actions.rs: INPUT_MOUSE SendInput (down+up) at cursor, SUPPRESS_SIMULATED guarded. Wired into Press Key macro step (mouse button names bypass keyboard path) and Send Hotkey (mouse buttons fire directly, skip hold mode). UI: MOUSE_CLICK_OPTIONS pill buttons in both KeyCaptureInput and HotkeyCaptureInput. Hold mode toggle hidden when mouse button selected. CSS variables only. |
| 2026-04-07 | Post-MVP | Repeat mode + mouse hold mode | **Repeat mode:** `RepeatingKeyState` with `trigger_storage_key`, `stop: Arc<AtomicBool>` in `REPEATING_KEY` Mutex. Toggle: first press starts loop thread, second press of SAME trigger stops. Does NOT stop on other keypresses. `pending_trigger_key` field added to EngineState, threaded through `handle_keyup` → `dispatch_with_double_tap` → `fire_macro` → `execute_action` → `execute_send_hotkey`. Stop checks in both bare and modified key paths of `handle_keydown`. Auto-stop: `toggle_pause`, `toggle_macros`, `quit_app`, pause hotkey handler all call `stop_repeating_key()`. Loop thread also checks `MACROS_ENABLED`. Tray: `update_tray_icon_repeating()` reuses held red icon, custom tooltip. UI: mutually exclusive toggle with hold mode, interval input (min 50ms, default 100ms). **Mouse hold mode:** `HeldKeyState` now has `mouse_button: Option<String>`. `send_mouse_event(button, is_up)` helper for single down/up. Hold sends `MOUSEEVENTF_*DOWN` only, release sends `MOUSEEVENTF_*UP`. `release_held_key()` checks `mouse_button` field. Hold mode toggle now visible for all targets including mouse pills. |
| 2026-04-07 | Post-MVP | Expansion injection refactor | Added `inject_via_sendinput()` (KEYEVENTF_UNICODE batched SendInput with surrogate pair support) and `should_use_clipboard()` branching in both `fire_expansion` and `fire_expansion_with_fillin`. Terminal process detection via `is_terminal_process()`. After testing, `should_use_clipboard()` set to always return `true` — clipboard injection more reliable across all app types. `inject_via_sendinput()` is dead code but retained for future use. `SUPPRESS_NEXT_CLIPBOARD_WRITE` cleanup now conditional on `used_clipboard` flag. |
| 2026-04-07 | Release | v0.1.24 released | Patch release. Expansion injection refactor (clipboard-only for now). |
| 2026-04-07 | Post-MVP | Image expansion type | New expansion type: `expansionType: "image"` with `imagePath` and `imageScale` (10–100, default 100) config fields. `image` crate (v0.25, png+jpeg features, pure Rust, ARM64 compatible) for decoding. `write_clipboard_image` in expansions.rs: CF_DIB + PNG stream (no text formats). `fire_image_expansion`: InjectionGuard, SUPPRESS_SIMULATED, Ctrl+V/Shift+Insert paste, no trailing space, no clipboard restore. Lanczos3 resize when scale < 100. Silent failure (log::warn + return) on missing/unreadable files. Dispatch fork in check_space_trigger + check_immediate_triggers. `browse_for_image` command (PNG/JPG filter). UI: Text/Image type pills, image picker, convertFileSrc thumbnail preview, scale input (allows empty during typing, validates min 10 on blur), IMG badge in list, [IMG] prefix in Quick Search. `protocol-asset` Tauri feature + CSP asset: protocol enabled. |
| 2026-04-07 | Post-MVP | PNG stream clipboard fix | `write_clipboard_image` now writes PNG stream alongside CF_DIB — `RegisterClipboardFormatW("PNG")` custom format with raw PNG file bytes. Fixes Word/Outlook not pasting images (they prefer PNG over CF_DIB). JPEG sources or resized images re-encoded as PNG via `img.write_to()`. Function signature updated to accept `raw_png_bytes: &[u8]`. |
| 2026-04-07 | Post-MVP | Expansion type filter pills | All/Text/Image filter pills in TextExpansions.jsx above expansion list. `typeFilter` state (`'all'`/`'text'`/`'image'`). `typeFiltered` derived array applied before category/sort. Category tab counts updated to reflect type filter. Empty state shows "No text/image expansions" when filter active with zero results. CSS: `.te-type-filter` + `.te-type-filter-pill` styled consistently with category tabs. |
| 2026-04-07 | Release | v0.1.26 released | Image expansion type, PNG stream clipboard, type filter pills. |
| 2026-04-07 | Post-MVP | Clipboard Manager | Full clipboard history feature. `clipboard.rs`: message-only HWND listener (`AddClipboardFormatListener`), SQLite writer thread (same pattern as analytics.rs), `clipboard_history` table with text/image BLOB storage, dedup via hash, auto-prune by retention days. `SUPPRESS_NEXT_CLIPBOARD_WRITE` integration prevents capturing Trigr's own writes. CF_HDROP explicitly skipped. Ctrl+Shift+V hotkey intercept in hotkeys.rs (after overlay, before pause). Pre-created `clipboardoverlay` window (transparent, always-on-top). IPC: 9 new commands. Frontend: ClipboardPanel.jsx (4th nav tab — Clipboard), ClipboardOverlay.jsx (quick-paste popup with arrow nav), retention settings in SettingsPanel (free: 1–7 days, Pro label for 30). `clipboardoverlay` added to capabilities/default.json. |
| 2026-04-07 | Post-MVP | Clipboard: source app + auto-tag + gallery | Schema migration: `source_app TEXT` and `content_tag TEXT` columns added via ALTER TABLE (backwards compatible). Source app capture: `GetForegroundWindow` → `QueryFullProcessImageNameW` in clipboard.rs. Auto-tagging: Image/Link/Email/Colour/Number/Text priority rules. `get_distinct_source_apps` IPC command. ClipboardPanel redesigned as masonry 3-column grid with tag filter pills + source app dropdown + colour swatches + lazy image thumbnails. ClipboardOverlay: hidden scrollbar, 2-line text preview, source_app labels on rows and detail meta. |
| 2026-04-07 | Post-MVP | Shared config path (cloud sync) | See "Shared Config Path" section in 10a for full details. |
| 2026-04-08 | Post-MVP | Bugfixes + MacroPanel layout | **Uncategorised expansion fix:** `openAdd()` in TextExpansions.jsx now sets `category: null` when `activeCategory === '__uncategorised__'` (was writing literal string, making expansions invisible). **Press Key layout:** Capture field moved inline with dropdown in `.macro-step-row`, mouse pills in `.press-key-sub-row` 3-column grid below. `minmax(0, 1fr)` prevents pill overflow. Removed `border-top` from all sub-rows for compact look. |
| 2026-04-08 | Release | v0.1.28–v0.1.29 released | Bugfixes + MacroPanel layout. |
| 2026-04-08 | Post-MVP | Macro speed optimization + presets | **Clipboard batching:** Macro loop saves clipboard once, uses `clipboard_paste_core()` for consecutive Type Text steps (no per-step save/restore), restores once at end. 5-step macro: 360ms→160ms. **Speed presets:** `macro_speed` field in EngineState, `speed_delays()` returns tuples: Safe (10/10/10/50), Fast (5/5/5/25), Instant (0/0/5/25), Custom (derived from slider values). All hardcoded delay constants removed. Radio buttons in Settings Compatibility section. Selecting preset updates sliders; manual slider change auto-switches to Custom. |
| 2026-04-08 | Post-MVP | Clipboard timeline + storage + editing + styling | **Timeline grouping:** `groupByTimeline()` in both ClipboardPanel (grouped grid sections) and ClipboardOverlay (inline headers via `groupedFlat` memo preserving keyboard nav indices). Groups: Pinned/Today/Yesterday/This Week/This Month/Older. **Storage size:** `get_clipboard_storage_size` Rust command (DB+WAL+SHM), displayed in panel toolbar. **Inline editing:** ClipboardOverlay now supports text editing (textarea + Edit/Save/Cancel buttons, keyboard nav suppressed during edit). **Gold styling:** Both CSS files updated to match Quick Search design language — gold border-accent search bars, no drop shadows, gold hover states throughout. |
| 2026-04-08 | Release | v0.1.30 released | All clipboard improvements + macro speed presets from this session. |
| 2026-04-08 | Post-MVP | Image paste fix + overlay polish + UX | **Image paste "Paste Options" fix:** Removed `CF_UNICODETEXT` from both `write_image_to_clipboard` (lib.rs, clipboard paste path) and `write_clipboard_image` (expansions.rs, image expansion path). Rich text editors (Outlook, Google Docs) showed "Paste Options" dialog when they saw both text and image formats — now only CF_DIB + PNG stream written. **Clipboard overlay history limit:** `show_clipboard_overlay` in lib.rs changed from `get_history(1, 15)` to `get_history(1, 500)` — overlay now shows full retention period. **Clipboard overlay selection UI:** Rows changed from card-style (individual borders, border-radius) to flat list with `border-bottom` separators, increased padding (10px 12px), and prominent full-width gold background selection (`rgba(232,160,32,0.18)`) with selected text turned gold. Light theme overrides via `[data-theme="light"]`. **Image scale input UX:** TextExpansions.jsx scale input `onChange` allows empty string during typing (no immediate clamp), `onBlur` validates and defaults to 10 if empty or below minimum. |
| 2026-04-08 | Release | v0.1.31 released | Image paste fix, overlay history limit, overlay selection polish, scale input UX. |
| 2026-04-09 | Post-MVP | Clipboard overlay selection fix | `selected` in ClipboardOverlay changed from `filtered[selectedIndex]` to `groupedFlat.find(flatIndex === selectedIndex)`. `groupByTimeline` reorders items (Pinned first) so `filtered` indices didn't match visual positions — detail pane, Enter key, Pin/Delete/Edit buttons all targeted wrong item, offset by pinned count. Row click was correct (used `item.id` from closure). |
| 2026-04-09 | Release | v0.1.32 released | Clipboard overlay selection fix. |
| 2026-04-09 | Post-MVP | Pinned items query + preview crash + zoomable images | **Pinned query:** `handle_get_history` SQL changed from `ORDER BY id DESC` to `ORDER BY pinned DESC, id DESC` — pinned items always appear on page 1 regardless of age. **Preview crash fix:** `&text[..200]` byte slice replaced with `char_indices().nth(200)` in both capture and update paths — multi-byte characters (box-drawing `─`, emoji) caused panic in clipboard listener thread. **Zoomable image preview:** New `ZoomableImage.jsx` component — scroll to zoom (1x–5x), drag to pan when zoomed, percentage badge, resets on image change. Used in both ClipboardPanel and ClipboardOverlay detail panes via `zoomable` prop on `ImageThumb`. |
| 2026-04-09 | Release | v0.1.33 released | Pinned items query fix, preview crash fix. |
| 2026-04-11 | Post-MVP | Bare mouse button remapping | All mouse buttons (Left/Right/Middle/Side1/Side2/Scroll) now assignable as bare in app-linked profiles. `SUPPRESS_BARE_MOUSE` RwLock HashSet in hotkeys.rs — populated by `rebuild_suppress_keys`, checked in `mouse_hook_proc` regardless of modifier state. Hook swallows original click, fires action only. Modified fallthrough: if no `Profile::Shift::MOUSE_RIGHT` exists, falls through to `Profile::BARE::MOUSE_RIGHT` — modifiers pass through naturally (Shift+RightClick→Shift+MiddleClick). Press-hold mirroring: `trigger_mouse_id` field on `HeldKeyState`, `release_held_if_mouse_trigger()` in actions.rs called from `handle_mouse_up` — button DOWN→target DOWN, button UP→target UP (not toggle). Bare scroll wheel support added to `handle_mouse_wheel`. Frontend: `BARE_MOUSE_ALLOWED` expanded to all 7 zones, advisory text updated. |
| 2026-04-11 | Post-MVP | Clipboard overlay scratchpad | Slide-out scratchpad pane on clipboard overlay. Numpad-style arrow toggle (`◂`/`▸`) on right edge, `max-width` CSS transition. Panel sits outside `.co-panel` (fixed 730px) so main layout untouched. `trigr-scratchpad.txt` in AppData, `get_scratchpad`/`save_scratchpad` IPC commands. Debounced auto-save (400ms). `clipboard_overlay_resize` updated to accept `(width, height)`. `show_clipboard_overlay` sends scratchpad text in payload. Window: 750px collapsed, 1050px expanded. |
| 2026-04-11 | Release | v0.1.34 released | Bare mouse remapping, press-hold mirroring, modifier passthrough, clipboard scratchpad. |
