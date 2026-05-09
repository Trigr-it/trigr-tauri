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
