# Keyfire Mac Port — working notes for development sessions

> Read this at the start of every Claude Code session on the Mac. It is the
> single source of truth for the port. The Windows dev machine holds richer
> project context; this file carries everything the Mac engine work needs.

## State (updated 2026-07-06, evening)

- **Phase 0 (platform seam) + Phase 1 (CI .dmg) are DONE and merged to main.**
- **Phase 2 in progress on `port/mac-hooks`** (all compile-checked green on
  macOS + Windows CI, unit-tested locally):
  - **M1 hooks** — CGEventTap + processor thread split (commit 24bb8df).
  - **M2 injection** — stubs/actions.rs is the real engine: CGEventPost with
    the INJECTED_EVENT_MAGIC tag (filtered in the tap), NSPasteboard
    clipboard, release/restore held modifiers via CGEventSourceKeyState,
    VK→mac-keycode translation (Ctrl→⌘ accelerator mapping for shared lib.rs
    paste sequences), execute_action for "text"/"url"/"expansion" (d19cc00).
  - **M3 matcher** — tap is ACTIVE (suppressing; listen-only fallback until
    Accessibility granted), suppress-set consulted in the callback, processor
    matches profile::Combo::KeyId and fires at keyup; overlay/pause/clipboard
    specials; hotkey capture + recording flows (414ced7).
  - **Foreground watcher** — NSWorkspace.frontmostApplication poll with the
    full auto-switch decision chain (b9b886d).
  - **END-TO-END VERIFIED 2026-07-06 (after Accessibility grant)**: tap
    ACTIVE; synthesized untagged chords (CGEvent swift helper) fired real
    assignments — text-via-clipboard AND text-via-direct-typing both landed
    in TextEdit with NO leaked trigger characters (suppression works); pause
    hotkey toggled on/off twice; clipboard restored after the paste.
    Still untested by a human: overlay/clipboard-overlay UI appearing on
    hotkey, profile auto-switch UX, and everything on the clean-machine
    .dmg (TCC grants attach differently to a bundled app).
  - **Deferred (deliberately unsuppressed so keys stay alive)**: bare keys,
    ::double/::hold variants, expansion keystroke buffer (module 3-4), voice,
    radial, Quick Record, mouse hooks, "hotkey"/"macro" action types.
- **Next modules**: expansions engine (keystroke buffer + trigger match) or
  clipboard history (NSPasteboard changeCount poll + SQLite); actions.rs
  self-change-count queue (`is_self_clipboard_change`) is already in place
  for the clipboard listener.
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

## Current milestone (see State above for what's done)

Hooks M1–M3 + injection + Send Hotkey + foreground watcher are implemented,
CI-green, and END-TO-END VERIFIED on this machine (Accessibility granted
2026-07-06; see State). Remaining human checks are UI-level only: overlay /
clipboard-overlay appearing on their hotkeys, profile auto-switch UX, and a
clean-machine .dmg pass on the test MacBook.

Next engine work in rough order: expansions keystroke buffer +
trigger matching (reuse the tap's KeyDown stream); "hotkey" + "macro" action
types in stubs/actions.rs (VK map + modifier mask already exist); clipboard
history listener (NSPasteboard changeCount poll — actions.rs already queues
self-write changeCounts via `is_self_clipboard_change`); then bare keys and
::double/::hold variants in the matcher.
