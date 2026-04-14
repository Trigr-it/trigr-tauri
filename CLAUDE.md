# Trigr — CLAUDE.md

> Read at the start of every session. For deep-dive context, see TRIGR_CONTEXT.md and TRIGR_TAURI_CONTEXT.md.
> For on-demand memory files, see ~/.claude/projects/-mnt-e-Development-Trigr-Tauri/memory/

---

## Quick Reference

| Field | Value |
|---|---|
| Product | Trigr — Windows desktop hotkey/macro/text expansion/clipboard manager |
| Stack | Tauri v2 + Rust backend + React 18 frontend |
| Version | v0.1.36 (check `tauri.conf.json` for current) |
| Dev command | `cargo tauri dev` |
| Build command | `cargo tauri build` |
| Frontend dev | `npm run dev` (Vite on localhost:5173) |
| Identifier | `com.nodescaffold.trigr` |
| GitHub | `Trigr-it/trigr-tauri` |
| Phase | Alpha (2 testers, targeting Beta May 2026) |
| Developer | Rory Brady — solo developer, London UK |

---

## Project Structure

```
src-tauri/
  src/
    main.rs          # Entry point (5 lines, delegates to lib.rs)
    lib.rs           # Tauri builder, 47 commands, window management (~1724 lines)
    hotkeys.rs       # LL keyboard/mouse hooks, modifier tracking, double-tap (~1785 lines)
    actions.rs       # Action execution: text, hotkey, macro, app/url/folder (~1438 lines)
    expansions.rs    # Text/image expansion, token resolution, fill-in fields (~1330 lines)
    clipboard.rs     # Clipboard history: listener, SQLite, auto-tag, images (~838 lines)
    config.rs        # Config load/save/backup, file watcher, shared path (~767 lines)
    tray.rs          # System tray, startup registry, window show/hide (~408 lines)
    foreground.rs    # Foreground watcher, app-profile auto-switching (~233 lines)
    analytics.rs     # SQLite action logging, time-saved stats (~262 lines)
  Cargo.toml         # Rust dependencies
  tauri.conf.json    # Tauri app config, bundler, updater
  capabilities/
    default.json     # Tauri v2 permissions
  icons/             # App icons (icon.png, icon.ico)

src/
  main.jsx           # React entry — routes to App, SearchOverlay, FillInWindow, or ClipboardOverlay
  tauriAPI.js        # window.electronAPI bridge — all invoke() and listen() wrappers
  App.jsx            # Root component (~1705 lines) — state owner, config persistence
  styles/
    global.css       # CSS variables, theme definitions
    app.css          # App-level layout
  components/
    TitleBar.jsx/css         # Custom titlebar, nav tabs, list view toggle, templates pill
    Sidebar.jsx/css          # Profile accordion, assignment list/grid views (~1015 lines)
    StatusBar.jsx/css        # Bottom bar: key info, engine status, version
    KeyboardCanvas.jsx/css   # Visual keyboard + ModifierBar (~431 lines)
    MouseCanvas.jsx/css      # Mouse button mapping UI (~399 lines)
    NumpadCanvas.jsx/css     # Slide-out numpad (~117 lines)
    MacroPanel.jsx/css       # Action editor: all 6 types + macro sequences (~1512 lines)
    TextExpansions.jsx/css   # Expansion editor with rich text, categories (~1618 lines)
    SettingsPanel.jsx/css    # All settings sections (~886 lines)
    SearchOverlay.jsx/css    # Ctrl+Space quick search (standalone window)
    ClipboardOverlay.jsx/css # Ctrl+Shift+V clipboard popup (standalone window)
    ClipboardPanel.jsx/css   # Clipboard history panel (main window tab)
    FillInWindow.jsx/css     # Fill-in field prompt (standalone window)
    FillInModal.jsx/css      # Simple fill-in modal
    AnalyticsPanel.jsx/css   # Usage stats dashboard
    TemplatesPanel.jsx/css   # Starter template packs
    OnboardingTour.jsx/css   # 5-step first-run tour
    WelcomeModal.jsx/css     # Welcome screen
    QuickTips.jsx/css        # Random tip display
    ZoomableImage.jsx/css    # Scroll-zoom + drag-pan image viewer
    keyboardLayout.jsx       # Keyboard key definitions and layout constants

assets/icons/        # Tray icon (tray-icon.png)
public/
  fonts/             # Bundled fonts (Rajdhani, DM Sans, Syne) — WOFF2
  fonts.css          # @font-face declarations (NEVER put in src/ CSS)
  app-icon*.png      # App icons in various sizes
docs/                # GitHub Pages: landing page, help guide, alpha tester guide
.github/workflows/
  release.yml        # Tag-triggered build + GitHub release (x64 + ARM64)
  cache-warm.yml     # Dependency cache warming on push to main
```

---

## Architecture Overview

### Threading Model (6 dedicated threads)
1. **Hook thread** — LL keyboard + mouse hooks, PeekMessageW polling loop, high priority
2. **Processor thread** — Event processing, modifier tracking, assignment matching, double-tap
3. **Clipboard listener thread** — Win32 message-only HWND, AddClipboardFormatListener
4. **Clipboard writer thread** — SQLite connection, processes ClipboardMsg via mpsc channel
5. **Analytics writer thread** — SQLite connection, processes AnalyticsMsg via mpsc channel
6. **Foreground watcher thread** — 1500ms poll, GetForegroundWindow, profile auto-switching

Additional threads spawned on demand: expansion injection, macro execution, fill-in flow, config file watcher.

### Multi-Window Architecture
| Window | Query Param | Purpose |
|---|---|---|
| main | (none) | Full UI |
| overlay | `?overlay=1` | Ctrl+Space quick search |
| fillin | `?fillin=1` | Fill-in field prompts |
| clipboardoverlay | `?clipboardoverlay=1` | Ctrl+Shift+V clipboard popup |

All secondary windows are pre-created hidden at startup and shown/hidden as needed.

### IPC Pattern
```
Frontend: invoke('command_name', { args })  →  Rust: #[tauri::command] fn command_name()
Frontend: listen('event-name', callback)    ←  Rust: app.emit("event-name", payload)
```
Command names use snake_case. See tauriAPI.js for the full list.

### State Ownership
- **Config**: Rust owns all disk I/O. Frontend reads via `load_config`, writes via `save_config`.
- **Engine state**: Rust `EngineState` mutex. Frontend syncs via `update_assignments`, `update_global_settings`.
- **UI state**: React owns via useState in App.jsx. Persisted to config on change.

---

## Storage & Config

### Files in AppData (`%LOCALAPPDATA%/com.nodescaffold.trigr/`)
| File | Purpose |
|---|---|
| `keyforge-config.json` | Main config (DO NOT rename) |
| `keyforge-config-last-known-good.json` | LKG backup |
| `backups/keyforge-config-*.json` | Timestamped backups (max 10) |
| `trigr-analytics.db` | Usage analytics SQLite |
| `trigr-clipboard.db` | Clipboard history SQLite |
| `trigr-local-settings.json` | Machine-specific settings (shared path) |
| `trigr-scratchpad.txt` | Clipboard overlay scratchpad |
| `logs/trigr.log` | Application log (5MB max, KeepOne rotation) |

### Storage Key Format
```
Single press:  ProfileName::Modifier::KeyCode
Double press:  ProfileName::Modifier::KeyCode::double
Bare key:      ProfileName::BARE::KeyCode
App-specific:  AppName::Modifier::KeyCode
Mouse button:  ProfileName::Modifier::MOUSE_LEFT
```

---

## Critical Rules

### DO NOT TOUCH
1. **Config filename** — `keyforge-config.json` must not change. Existing configs must load.
2. **Config writes** — Always owned by Rust backend. Frontend never writes directly to disk.
3. **Font @font-face** — Must go in `public/fonts.css`, never in `src/` CSS files.
4. **Theme colours** — All colours must use CSS variables. Never hardcode hex (except green dot #22c55e).
5. **Background threads** — Hook, foreground watcher, macro runner must never block main/UI thread.
6. **suppressNextClipboardWrite** — Must be set before every internal clipboard write.
7. **Electron reference repo** — `E:\Development\Trigr-Reference` is read-only. Never modify.

### Critical Implementation Rules
- **Hook callbacks**: No println!, file writes, or blocking I/O in `keyboard_hook_proc` / `mouse_hook_proc` (Windows removes hook if >300ms)
- **Atomic ordering**: ALL cross-thread atomics must use `Ordering::SeqCst` (Relaxed fails on ARM64)
- **Recording/Capture check order**: `IS_RECORDING_HOTKEY` and `IS_CAPTURING_KEY` checks must be ABOVE the `APP_INPUT_FOCUSED` guard in `handle_keydown`
- **SUPPRESS_SIMULATED**: Must be set `true` before any `SendInput` call
- **Modifier release**: `release_held_modifiers()` must be called before any clipboard paste or text injection
- **Hook reinstall atomic reset**: `spawn_hook_thread()` must reset ALL shared atomics after setting `HOOKS_RUNNING = true`
- **Double press clear**: `handleClearKey` must delete both `::` and `::double` entries
- **Hook heartbeat**: Must be incremented in BOTH keyboard AND mouse hook procs
- **Keystroke buffering**: `INJECTION_IN_PROGRESS` guards real keystrokes during injection, replays after

### Help Window
`open_help` uses `opener::open()` to the GitHub Pages URL. DO NOT create a WebviewWindow for help — a 3.2MB HTML file freezes WebView2.

---

## Release Process

1. Read current version from `src-tauri/tauri.conf.json`
2. Increment version (patch/minor/major)
3. Update version in BOTH `tauri.conf.json` AND `package.json`
4. Commit: `"Release vX.X.X"`
5. Push to main
6. Tag: `git tag vX.X.X`
7. Push tag: `git push origin vX.X.X`
8. CI builds x64 + ARM64, publishes to GitHub Releases automatically

Both files must match. Tag must be pushed to trigger the build.

---

## Key Dependencies

### Rust (Cargo.toml)
- `tauri` v2 (tray-icon, protocol-asset)
- `windows-sys` v0.59 (all Win32 API — hooks, SendInput, clipboard, process, GDI)
- `rusqlite` v0.31 (bundled SQLite — analytics + clipboard)
- `image` v0.25 (PNG/JPEG decode, ARM64 compatible)
- `notify` v8 (file watching for shared config)
- `chrono` v0.4 (date tokens)
- `tauri-plugin-updater`, `tauri-plugin-dialog`, `tauri-plugin-log`, `tauri-plugin-shell`, `tauri-plugin-process`

### Frontend (package.json)
- React 18.2.0 + React DOM
- @tauri-apps/api + plugins (dialog, shell, process, updater)
- @dnd-kit (core, sortable, utilities) — drag-and-drop
- lucide-react — icons
- Vite 5.4.0 — bundler

### ARM64 Compatibility
Machine is Surface Pro ARM64. Every native Rust crate must be verified for ARM64 before use. Known compatible: windows-sys, rusqlite (bundled), serde_json, image, notify v8.

---

## Common Tasks

### Adding a new Tauri command
1. Write the function in the appropriate Rust module with `#[tauri::command]`
2. Register it in `lib.rs` in the `.invoke_handler(tauri::generate_handler![...])` call
3. Add the JS wrapper in `src/tauriAPI.js`
4. Call from React via `window.electronAPI.commandName()`

### Adding a new action type
1. Add execution logic in `actions.rs` `execute_action()` or `execute_macro_step()`
2. Add UI form in `MacroPanel.jsx`
3. Update `MACRO_STEP_TYPES` array if it's a macro step

### Adding a new expansion token
1. Add replacement logic in `expansions.rs` `resolve_tokens()`
2. Add to `INSERT_MENU` array in `TextExpansions.jsx`

### Modifying the config schema
1. Update Rust read/write in `config.rs` or relevant module
2. Update React state in `App.jsx` (load + save paths)
3. Handle migration for existing configs (backwards compatible)
