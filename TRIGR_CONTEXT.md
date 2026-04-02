# TRIGR — Project Context
> Read this file at the start of every CC session before touching any code.
> Last updated: March 2026 (session 3)

---

## 01 — Project Identity

**Product:** Trigr  
**Type:** Windows desktop productivity app  
**Stack:** Electron 28 + React 18  
**GitHub:** `Trigr-it/trigr` (public repo)  
**Working directory:** `E:\Development\Trigr`
**Dev command:** `npm run electron-dev`  
**Landing page:** `https://trigr-it.github.io/trigr`  
**Company:** Node Scaffold Design Ltd (trading as Trigr)  
**Developer:** Rory Brady — solo developer and civil engineer, London UK  
**Business partner:** Has a partner with a sales background — may formalise as co-founder  

---

## 02 — What Trigr Does

Trigr sits in the Windows system tray and lets users create keyboard hotkeys, macro sequences, text expansions, and manage clipboard history through a visual UI — no scripting, no config files.

**Origin story:** Built for Rory's civil engineering firm as a replacement for AutoHotkey — his team could not learn the scripting language.

---

## 03 — Current Version

**v0.1.44** (as of March 2026)

Alpha is live with 2 testers. Google Form feedback pipeline is active.

---

## 04 — Features: COMPLETE (do not rebuild or modify without explicit instruction)

### Key Mapping
- Visual keyboard UI — full keyboard, modifier bar, numpad slide-out panel
- Modifier layers: Ctrl, Alt, Shift, Win, Bare Keys
- Click any key to open action assignment panel
- Record Hotkey button — press the combo, Trigr records it
- App-specific profiles with automatic foreground watcher switching
- Multiple global profiles — user selects active base profile
- Bare key assignments (no modifier required)
- Double press hotkeys — single press and double press have separate assignments, for both keyboard keys and mouse buttons
  - Timer-based detection (default 300ms window, configurable 150-500ms)
  - Storage format: `Profile::Modifier::Key` (single) and `Profile::Modifier::Key::double` (double)
  - Both assignments move together on reassign
  - If no double assignment exists, single press fires immediately with no delay
  - Mouse modifier+mouse: Path-B inline detection at mousedown. Bare mouse: `dispatchHotkeyWithDoubleTap` directly
- Mouse button assignments (multiple versions implemented — DO NOT assume this is missing)
- Numpad support via slide-out panel
- x2 badge on keyboard keys that have double press assigned
- Single Press / Double Press toggle bar in action panel (mirrors Keyboard / Mouse toggle)

### Action Types
- **Type Text** — fires pre-written text at cursor
- **Send Hotkey** — remaps combo to different combo
- **Macro Sequence** — chain keystrokes, text, waits, app launches. Drag-drop reorder. Wait for Input support.
- **Open App** — launch application or file
- **Open URL** — open in default browser
- **Open Folder** — open in File Explorer

### Text Expansions
- Trigger word + Space fires replacement text anywhere in Windows
- Rich editor with categories
- Fill-in fields — prompt user for input mid-expansion
- Global Variables — {clipboard}, {cursor} in expansions
- My Details — stored personal info variables
- Autocorrect with built-in library
- Category colours, rename, reorder

### App-Specific Profiles
- Profiles linked to a specific app (process name)
- Foreground watcher auto-switches when app gains focus
- App profile indicator (🖥 emoji prefix on tab)
- App-linked profile indicator in title bar

### Quick Search (Ctrl+Space)
- Floating search overlay fires from anywhere
- Searches all assignments and expansions
- Configurable hotkey (default Ctrl+Space)

### System & UI
- System tray — active/pause toggle
- Tray icon: gold T (active), different state (paused)
- Global pause toggle — modifier-combo only (not bare keys to avoid accidental pause)
- Windows taskbar overlay icon via setOverlayIcon
- Light and dark mode
- Start with Windows toggle
- Import/Export config backup via file dialog
- Automatic rolling config backups
- Config corruption protection — loadConfigSafe() tries main, then last-known-good, then timestamped backups
- Settings panel: Help, About, Privacy & Security, General, Global Pause, Quick Search, Compatibility, Backup & Restore
- In-app feedback button — opens Google Form URL
- Password warning in expansion editor (shown once, dismissible)

### Auto-updater (CRITICAL — DO NOT MODIFY)
- Direct HTTPS download to `os.tmpdir()` — does NOT use electron-updater's built-in download
- Download URL uses consistent filename: `Trigr-Setup.exe` (no version in URL)
- Spawn with `/VERYSILENT /RESTARTAPPLICATIONS` flags
- Fire-and-forget — no await after spawn
- `app.quit()` immediately after spawn
- Only runs in production (`if (!isDev) { initAutoUpdater() }`)
- Never shows in `npm run electron-dev` console — test only in installed version

### Start with Windows (auto-launch)
- Registry entry (`HKCU\...\Run`) now written as `"<execPath>" --autolaunch`
- `isAutoLaunch = process.argv.includes('--autolaunch')` detected at startup
- When auto-launched: `BrowserWindow` created with `show: false` — tray appears, window stays hidden
- Normal launch: window shows as usual
- Existing users who had the old registry entry (no arg) must re-toggle the setting once

### Fonts (bundled — no CDN)
- All fonts are bundled locally in `public/fonts/` — app works fully offline
- Loaded via `<link rel="stylesheet" href="%PUBLIC_URL%/fonts.css">` in `public/index.html`
- `@font-face` declarations live in `public/fonts.css` — NOT in any `src/` CSS file
- `keyforge-help.html` has its own inline `<style>` block with `@font-face` declarations
- Fonts: Rajdhani 400/500/600/700, DM Sans 300/400/500/600 normal + 300 italic, Syne 800

### Logo / Header
- Header wordmark uses Syne 800 (`font-family: 'Syne'`, `font-weight: 800`)
- T icon/monogram SVG has been removed — wordmark text only
- Colour: `var(--text-primary)` — works in both light and dark theme

### Foreground watcher profile switching
- Auto-switching is suppressed while the main window is visible (`mainWindow.isVisible() && !mainWindow.isMinimized()`)
- Switching only resumes when the window is hidden to tray or minimised to taskbar
- This allows users to leave Trigr open while testing macros in other apps without losing their selected profile

### Installer & Build
- NSIS installer
- ~77MB (reduced from 119MB)
- `artifactName: "Trigr-Setup.${ext}"` in electron-builder nsis config — output is always `Trigr-Setup.exe` (no version in filename)
- Permanent download URL: `https://github.com/Trigr-it/trigr/releases/latest/download/Trigr-Setup.exe`
- Excluded: koffi cross-platform binaries, unused locales
- `react-scripts` moved to devDependencies
- Source maps removed
- Icons live in `assets/icons/` — NOT `build/` (React wipes build/ on every build)

---

## 05 — Publish Sequence (ALWAYS in this exact order)

```
git add .
git commit -m "your message"
npm run build
npm version patch
npm run publish
```

**Important:** Installed version is always one behind the latest GitHub release. Fixes in dev do not appear in the installed version until the next publish cycle.

---

## 06 — Architecture Notes

### IPC Patterns
- All config read/write owned by `main.js` — renderer never writes directly
- `loadConfigSafe()` — resilient loader: main config → last-known-good → timestamped backup
- Import config: main.js writes to disk immediately after validation (not via renderer round-trip)
- Export config: uses `loadConfigSafe()` not `loadConfig()` to prevent empty export on error

### Storage Key Format
- Single press hotkey: `ProfileName::Modifier::KeyCode`
- Double press hotkey: `ProfileName::Modifier::KeyCode::double`
- Bare key: `ProfileName::Bare::KeyCode`
- App-specific: `AppName::Modifier::KeyCode`

### Keyboard Scaling
- Width-only ResizeObserver scaling — does NOT divide by `devicePixelRatio`
- Fixes visual scaling bug on ARM64 (150% DPI, 2304×1536) vs 1080p (100% DPI, 1920×1080)

### Config File
- Path: `app.getPath('userData') + '/keyforge-config.json'`
- Note: file is still named `keyforge-config.json` internally

---

## 07 — Code Signing

**Provider:** Microsoft Azure Trusted Signing  
**Account:** `nodescaffold-signing`  
**Region:** West Europe  
**Tier:** Basic (~£9/month)  
**Company:** Node Scaffold Design Ltd  
**Status:** Identity validation submitted, awaiting Microsoft verification (1-3 weeks)  
**Decision:** Keep under Node Scaffold Design Ltd — no need to switch to Trigr Ltd

---

## 08 — Pricing Model (confirmed)

| Tier | Price | Key features |
|---|---|---|
| Free | Free forever | Unlimited hotkeys, expansions, profiles, AHK Script Runner, basic clipboard (30 days), basic analytics, quick search, starter templates, profile export/import, macro recorder |
| Pro | £49/year or £99 lifetime | Clipboard Manager (full), Save Snippet from Highlight, full analytics dashboard, AHK importer, TextExpander importer, espanso importer, conditional expansions, scheduled macros, mouse button assignments, dynamic variables, regex triggers, priority support |
| Teams | £12-15/user/month | Everything in Pro + cloud sync, shared snippet libraries, admin dashboard, team analytics, SSO |

**LemonSqueezy** handles all payments, licence keys, VAT compliance.  
**Beta strategy:** LemonSqueezy integrated with free £0 keys — validates the licence flow before money changes hands.

---

## 09 — Roadmap

### Alpha (now — April 2026) — CURRENT PHASE
- 2-5 testers
- Google Form feedback
- Fix bugs
- No licence system yet

### Beta (May 2026)
- LemonSqueezy integration (free beta keys)
- Onboarding flow (2-3 CC sessions)
- Starter template library (1-2 CC sessions)
- List view toggle — keyboard on by default, togglable in settings (1-2 CC sessions)
- Basic analytics — total actions fired + time saved counter (2-3 CC sessions)
- AHK Script Runner v1 syntax (4-6 CC sessions)
- Code signing (Microsoft-dependent)

### v1.0 (July 2026)
- AHK v2 syntax support (2-3 CC sessions)
- AHK importer .ahk file parser (3-4 CC sessions)
- Clipboard Manager basic — history, search, pin, 30-day retention (4-5 CC sessions) — FREE
- Clipboard Manager advanced — smart type detection, source app tagging, sensitive item detection, unlimited retention (2-3 CC sessions) — PRO
- Save Snippet from Highlight via Ctrl+Space contextual cards (4-6 CC sessions) — PRO
- Full analytics dashboard — 14-day chart, per-assignment breakdown, CSV export (2-3 CC sessions) — PRO
- Shared profile export/import (1-2 CC sessions)
- Macro recorder prominent (2-3 CC sessions)
- UI polish — eliminate generic vibe-coded elements, action panel in particular (2-3 CC sessions)
- LemonSqueezy paid tiers activated (1 session)
- Privacy policy + terms of service (1-2 CC sessions)

### v1.1 (August 2026)
- Post-launch bug fixes
- TextExpander importer (Pro)
- espanso importer (Pro)
- Run Macro on selection — Ctrl+Space quick action (Pro)
- Expand Here without hotkey — Ctrl+Space quick action (Pro)

### v1.2 (Sept-Oct 2026)
- Conditional expansions (Pro)
- Scheduled macros / time triggers (Pro)
- Mouse button assignments advanced (Pro)
- Dynamic variables {date} {time} {computername} (Pro)
- Regex triggers (Pro)

### v2.0 (Q4 2026)
- Cloud sync (Pro)
- Shared team snippet libraries (Teams)
- Admin dashboard (Teams)
- Team analytics (Teams)
- SSO (Teams)
- Browser extension (Pro)
- AHK script export .ahk output (Pro)

---

## 10 — Planned Features Detail

### AHK Script Runner
- New action type: AHK Custom Script (button in action panel below Open Folder and Macro Sequence)
- Textarea for script body — user pastes body only, no hotkey label, no Return
- Trigr auto-wraps: prepends `^key::` label and appends `Return`
- Bundles AutoHotkey.exe (~2MB) via electron-builder extraResources
- No AHK installation needed on user machine
- Execution: write temp .ahk file → spawn AHK process → fire-and-forget
- Error handling: surface AHK errors to UI
- Edge cases: kill on re-trigger, long-running script cleanup on app quit
- AHK is GPL v2 — credit in About screen required
- v1 syntax first in Beta. v2 syntax support added at v1.0.

### Clipboard Manager
**Free (basic):**
- Clipboard history, search, pin
- 30-day retention
- Promote-to-snippet (→Trigr button)

**Pro (advanced):**
- Smart type detection (URL, code, email, phone — regex)
- Source app tagging (Chrome, Outlook, VS Code icon)
- Sensitive item detection (auto-clear from password managers)
- Unlimited retention with configurable policies

**Build sequence:**
1. Windows clipboard listener (WM_CLIPBOARDUPDATE) — TEST FIRST for uiohook-napi conflict
2. SQLite storage in main process + IPC to renderer
3. History UI — list, search, pin, source app icon
4. Smart type detection
5. →Trigr promote button (hooks into existing snippet engine)
6. Retention policies UI
7. Sensitive item detection

### Save Snippet from Highlight
- User highlights text anywhere in Windows
- Opens Ctrl+Space overlay
- Trigr detects clipboard content and surfaces contextual card ABOVE search input
- Card: "Save Snippet" — opens new expansion dialog with highlighted text pre-filled
- RISK: clipboard timing. Must simulate Ctrl+C and wait ms delay before reading clipboard
- Future quick actions in same pattern: Run Macro on selection, Expand Here

### List View Toggle
- Visual keyboard is the default (on)
- Toggle in settings — not a prominent main UI element
- List view: flat searchable table of all assignments (trigger, action type, profile)
- Faster to scan with 50+ assignments
- For TextExpander migrants and text-expansion-only users

### Usage Analytics
**Free (basic):**
- Total actions fired counter
- Total time saved counter (configurable seconds-per-use per action type)

**Pro (full dashboard):**
- 14-day usage bar chart (recharts — already in stack)
- Per-assignment breakdown
- CSV export

100% local — SQLite in AppData. Nothing leaves device.

---

## 11 — CC Session Definition

**1 CC session** = one continuous Claude Code conversation  
**Rory's time per session:** 20-40 minutes (Claude Chat writes all prompts, Rory pastes into CC, pastes result back for review)  
**Rory writes no code**  
**Calendar time:** roughly 1-2 days per 4-6 sessions  

---

## 12 — Tooling Stack

| Tool | Purpose |
|---|---|
| Claude Chat | Strategy, planning, documents, writing CC prompts |
| Claude Code (CC) | All implementation — run from project root |
| Cowork | Scheduled tasks, form automation |
| ittybitty | Multi-agent CC orchestration — installed at `~/tools/ittybitty` |
| LemonSqueezy | Payments, licence keys, VAT compliance |
| Microsoft Azure Trusted Signing | Code signing |
| GitHub Pages | Landing page + help guide hosting |
| Google Form + Google Sheet | Alpha feedback pipeline |

---

## 13 — Alpha Feedback Pipeline

1. Tester finds bug → fills Google Form (2 min)
2. Response appears in Google Sheet (Trigr Alpha Bug Reports)
3. Rory downloads sheet as .xlsx
4. Uploads to Claude Chat with "Review alpha bugs"
5. Claude Chat triages, prioritises, writes CC prompts
6. Rory pastes prompts into CC
7. Fix pushed via publish sequence

---

## 14 — Document Set

All 6 core documents are stored in Google Drive (Trigr folder) as Google Docs AND .docx:

1. `Trigr_Business_Plan_v4.docx`
2. `Trigr_Competitor_Analysis.docx`
3. `Trigr_Use_Cases.docx`
4. `Trigr_Use_Cases_Additional.docx`
5. `Trigr_Alpha_Feedback_Setup.docx`
6. `Trigr_Reddit_Research.docx`

**Rule:** When any single document is updated, ALL six must be rebuilt and presented together.

**Build system:** `brand.js` at `/home/claude/brand.js` with gold #E8A020 accents, navy #1A1A2E headings.

---

## 15 — Known Do-Not-Touch Rules

1. **Auto-updater** — the direct HTTPS download mechanism is confirmed working. Never modify this pattern.
2. **Icons** — must live in `assets/icons/` not `build/` — React wipes `build/` on every build
3. **Keyboard scaling** — width-only ResizeObserver, no devicePixelRatio division
4. **Publish sequence** — always in the exact order: `git add` → `git commit` → `npm run build` → `npm version patch` → `npm run publish`
5. **Config writes** — always owned by `main.js`, never via renderer round-trip
6. **Import config** — write directly to disk in main.js immediately after validation (not renderer saveConfig)
7. **Font @font-face declarations** — must go in `public/fonts.css`, never in `src/` CSS files. Webpack treats `url()` in src CSS as module imports and fails the build if the file isn't in the JS module graph.
8. **Font files** — live in `public/fonts/`. Files in `public/` are copied to `build/` by CRA — they survive builds. Never put font files in `build/` directly.
9. **Start with Windows registry entry** — written as `"<execPath>" --autolaunch`. If you ever modify `setStartupEnabled`, preserve the `--autolaunch` arg — removing it breaks silent tray launch.

---

## 16 — Branding

**Tagline:** Set it. Trigr it.  
**Website:** trigr.it  
**Colours:**
- Gold (primary): #E8A020
- Gold dark: #C8860A
- Navy: #1A1A2E
- Body: #4A4A6A

**Font:** Arial throughout  
**Footer pattern:** `Trigr • [Document Title] • Set it. Trigr it. • trigr.it`

---

## 17 — Marketing & Reddit

- **Target subs:** r/AutoHotkey, r/productivity, r/windows, r/WindowsApps, r/SideProject, r/selfhosted, r/privacy, r/CAD, r/betatests
- **Key positioning:** Free for all core features. Pro for advanced automation. Teams for businesses.
- **Primary hook for Reddit:** Free alternative to cobbled-together tools. Visual keyboard. AHK Script Runner for existing AHK users.
- **Always:** Direct download link, no DM friction. Lead with visual keyboard in GIFs.
- **GIF tool:** ScreenToGif (free, Windows)
- **Best post time:** Tuesday-Thursday 9-11am EST

---

## 18 — Competitive Context

| Competitor | Why Trigr wins |
|---|---|
| AutoHotkey | Trigr is free, visual, no scripting. AHK Script Runner lets AHK users keep their scripts. |
| TextExpander | Trigr Free does everything TextExpander personal does plus hotkeys, macros, AHK. Free. |
| FastKeys | Trigr is modern, has AHK runner, Teams tier. FastKeys is stuck in 2010. |
| Keyboard Maestro | Mac only. Trigr is the Windows equivalent. |
| Logitech G Hub | Hardware-locked. Trigr works with any keyboard. |
| espanso | Config file only. Trigr Free has full visual UI. |
| Macronyx | No text expansion, no teams, no business model. Same Electron/React stack. |
| Ditto / WIN+V | Clipboard only. Trigr includes clipboard as one pillar of a complete tool. |

**Trigr's moat:** No competitor offers visual UI + hotkeys + macros + text expansions + AHK Script Runner + clipboard + analytics + Teams on Windows in a single tool.

---

## 19 — Help Guide

`trigr-help.html` — pixel-perfect HTML guide with live interactive UI demos  
Hosted at GitHub Pages  
Screenshot status (as of March 2026): screenshots 10, 12, and 16 still placeholder

---

## 20 — Release Process

To release a new version, tell CC: "release patch", "release minor", or "release major".

CC should then:
1. Read current version from `src-tauri/tauri.conf.json`
2. Increment appropriately (patch = x.x.+1, minor = x.+1.0, major = +1.0.0)
3. Update version in BOTH `tauri.conf.json` AND `package.json`
4. Commit: `"Release vX.X.X"`
5. Push to main
6. Tag: `git tag vX.X.X`
7. Push tag: `git push origin vX.X.X`
8. Confirm the GitHub Actions URL to monitor the build

Never skip steps 3 or 7. Both files must match and tag must be pushed to trigger the build.

The release is published automatically when the build completes — no manual draft publishing step. Release notes are auto-generated from commit messages since the last tag.
