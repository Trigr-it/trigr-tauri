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
| `uiohook-napi` | `rdev` crate |
| `koffi` + Win32 API | `windows-rs` crate |
| koffi SendInput | `enigo` crate |
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

---

## 05 — Storage & Config Rules (CRITICAL)

**Config file:** `keyforge-config.json` in app data dir — filename must NOT change. Existing user configs from the Electron version must load without migration.

**Storage key formats — identical to Electron version:**
- Single press hotkey: `ProfileName::Modifier::KeyCode`
- Double press hotkey: `ProfileName::Modifier::KeyCode::double`
- Bare key: `ProfileName::Bare::KeyCode`
- App-specific: `AppName::Modifier::KeyCode`
- Mouse button: `ProfileName::Modifier::MOUSE_LEFT` (MOUSE_LEFT, MOUSE_RIGHT, MOUSE_MIDDLE, MOUSE_SIDE1, MOUSE_SIDE2)

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
| 0 | Codebase analysis + migration plan | ⬜ | Send first — plan only, no code |
| 1 | Project scaffold + React migration | ⬜ | App window opens, UI renders |
| 2 | Config read/write | ⬜ | load_config_safe, save_config, backups |
| 3 | Tray + window management | ⬜ | Close to tray, autolaunch, show/hide |
| 4 | Global hotkey capture (rdev) | ⬜ | ARM64 verify first — PLAN ONLY prompt |
| 5 | Text injection + Send Hotkey | ⬜ | MVP milestone — first real action fires |
| 6 | Foreground watcher + app profiles | ⬜ | Profile auto-switching |
| 7 | Text expansions | ⬜ | Trigger detection, replacement injection |
| 8 | Macro sequence + remaining actions | ⬜ | All action types complete |
| 9 | Quick Search overlay | ⬜ | Ctrl+Space floating window |
| 10 | Auto-updater + installer | ⬜ | Shippable build — share with testers |

---

## 10 — MVP Definition

**Dev MVP (Phase 5 complete):** App launches, tray icon shows, at least one hotkey fires a Type Text action. Test on device before continuing.

**Shippable MVP (Phase 10 complete):** All current Electron Alpha features working, NSIS installer produced, auto-updater configured. Share with existing 2 testers. Electron version stays live until Tauri confirmed stable.

---

## 11 — Tauri Config Reference

```json
{
  "productName": "Trigr",
  "identifier": "com.nodescaffold.trigr",
  "version": "0.1.0",
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
| — | — | — | — |
