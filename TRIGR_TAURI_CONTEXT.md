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
