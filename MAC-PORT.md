# Keyfire Mac Port — working notes for development sessions

> Read this at the start of every Claude Code session on the Mac. It is the
> single source of truth for the port. The Windows dev machine holds richer
> project context; this file carries everything the Mac engine work needs.

## State (updated 2026-07-06)

- **Phase 0 (platform seam) + Phase 1 (CI .dmg) are DONE and merged to main.**
  The app compiles and runs on macOS as a UI shell: full React frontend,
  config persistence, all 155 Tauri commands present. The entire input engine
  is stubbed.
- **Phase 2 (this machine's job): the native macOS engine.**
- Dev machine: Apple Silicon (M4) iMac, repo at `~/Desktop/Keyfire`.
  A separate MacBook is the clean-machine artifact tester.

## Architecture you inherit

Tauri v2, Rust backend + React 18 frontend. On Windows the engine lives in 10
Win32-bound modules; the platform seam in `src-tauri/src/lib.rs` (top of file)
swaps each one at compile time:

```
#[cfg(windows)]           mod hotkeys;            // real Win32 module
#[cfg(not(windows))]
#[path = "stubs/hotkeys.rs"] mod hotkeys;         // no-op twin, same API
```

Modules and their macOS replacement targets, in build order:

| Stub | Windows original | macOS replacement | Order |
|---|---|---|---|
| stubs/hotkeys.rs | LL keyboard/mouse hooks, modifier tracking, double-tap/hold state machine | CGEventTap (needs Accessibility + Input Monitoring TCC grants) | 1 |
| stubs/actions.rs | SendInput injection, clipboard write, action execution | CGEventPost + NSPasteboard | 2 |
| stubs/expansions.rs | keystroke buffer, trigger match, token resolve, paste | reuse engine logic, mac injection path | 3-4 |
| stubs/clipboard.rs | clipboard listener + SQLite history | NSPasteboard polling (no change notification API; ~200ms changeCount poll) | 5 |
| stubs/foreground.rs | GetForegroundWindow poll, profile auto-switch | NSWorkspace.frontmostApplication | 6 |
| stubs/tray.rs | system tray + Run registry key | Tauri tray (works natively as menu bar item) + SMAppService login item | 7 |
| stubs/window_target.rs | GDI monitor enum | NSScreen | later |
| stubs/webview_mem.rs | WebView2 suspension | permanent no-op (WKWebView has no equivalent) | never |
| stubs/voice.rs, stubs/ocr.rs | WinRT | out of scope for beta (Speech/Vision frameworks someday) | never (for now) |

AHK Script Runner: Windows-only forever (closed decision). Hide in UI.

Strategy per module: replace the stub file's no-op bodies with real macOS
implementations, keeping the exact same public signatures — lib.rs and the
frontend then work unchanged. If a module needs mac-specific state, keep it
inside the stub file. Do NOT touch the `#[cfg(windows)]` originals.

## Hard rules (carry over from the main project)

1. `keyforge-config.json` filename and schema stay unchanged — configs are
   cross-platform. Config writes are owned by Rust (`config.rs`, shared).
2. All cross-thread atomics use `Ordering::SeqCst`.
3. Never block the event-tap callback thread (macOS disables taps that stall;
   same discipline as the Windows 300ms hook rule). No I/O or logging in the
   tap callback — post to the processor thread.
4. Simulated input must be marked and filtered: tag CGEvents posted by Keyfire
   (e.g. CGEventSetIntegerValueField with a magic user-data value) and ignore
   them in the tap — the equivalent of the Windows SUPPRESS_SIMULATED /
   LLKHF_INJECTED discipline.
5. Release held modifiers before any synthetic paste (mirror of the Windows
   `release_held_modifiers` invariant).
6. Storage keys keep the `'Win'` token for the Meta/Cmd modifier — stored
   format is cross-platform; translate only the DISPLAYED label (⌘/⌥) in the
   frontend. No config migration.
7. Use `log::info!/warn!/error!` (tauri-plugin-log), never println!.
8. Frontend: all colours via CSS variables; @font-face only in
   public/fonts.css; match existing component conventions.

## Workflow

- Branch from main as `port/<topic>` and push — `.github/workflows/port-check.yml`
  compile-checks macOS AND Windows on every push to `port/**`, and builds an
  unsigned .dmg artifact (`keyfire-macos-dmg`) for the test MacBook.
- Unsigned artifacts need `xattr -cr <app or dmg>` on the test machine.
- NEVER tag versions from a Mac session (tags trigger the Windows release
  pipeline). Merging to main is done after the owner signs off.
- Dev loop on this machine: `cargo tauri dev` from the repo root.
- The Windows build must never regress: don't edit code inside
  `#[cfg(windows)]` items; if a shared file must change, keep changes additive
  and platform-neutral.

## First milestone (start here)

CGEventTap listen-only spike inside `stubs/hotkeys.rs::start_hooks`:
1. Create a session event tap for keyDown/keyUp/flagsChanged on a dedicated
   thread with its own CFRunLoop.
2. On first run macOS will prompt for Accessibility / Input Monitoring — the
   human grants them (in dev, the grant attaches to the terminal app running
   `cargo tauri dev`).
3. Feed events into modifier-state tracking mirroring the Windows processor
   thread design (hook thread ingests, processor thread decides).
4. Success = log lines showing keys + modifiers globally, and
   `get_engine_status` reporting hooks running so the UI status dot goes live.
5. Handle tap-disabled callbacks (kCGEventTapDisabledByTimeout) by re-enabling.

Then: injection (CGEventPost with the simulated-event tag), then wire
`execute_action` for the simplest action type, then a first end-to-end hotkey.
