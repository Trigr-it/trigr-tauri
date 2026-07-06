# Keyfire Mac Port — working notes for development sessions

> Read this at the start of every Claude Code session on the Mac. It is the
> single source of truth for the port. The Windows dev machine holds richer
> project context; this file carries everything the Mac engine work needs.

## State (updated 2026-07-07, overnight autonomous session)

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
    full auto-switch decision chain (b9b886d); also pins the app theme to
    the OS appearance every poll (WKWebView prefers-color-scheme misreports).
  - **Overlays working** (086675c/6a25c3f/7faa4f3): quick search + clipboard
    popup show/position/focus with PID-based focus hand-back; theme "auto"
    resolved Rust-side; titlebar drags via window_start_dragging (WKWebView
    has no -webkit-app-region).
  - **M5 clipboard history** (9b9e765): changeCount poll listener (200ms),
    NSPasteboard string/HTML/PNG/TIFF reads, SQLite + AES-256-GCM identical
    to Windows (raw 0600 key file instead of DPAPI). Verified live: text +
    image rows captured encrypted, popup shows history.
  - **M7 tray** (258fa52): menu bar item with pause/login-item/quit,
    close-to-tray; login item = LaunchAgent plist.
  - **END-TO-END VERIFIED 2026-07-06 (after Accessibility grant)**: tap
    ACTIVE; synthesized untagged chords (CGEvent swift helper) fired real
    assignments — text-via-clipboard AND text-via-direct-typing both landed
    in TextEdit with NO leaked trigger characters (suppression works); pause
    hotkey toggled on/off twice; clipboard restored after the paste.
    Still untested by a human: overlay/clipboard-overlay UI appearing on
    hotkey, profile auto-switch UX, and everything on the clean-machine
    .dmg (TCC grants attach differently to a bundled app).
  - **M3-4 EXPANSIONS ENGINE (2c2b119 + 1ecbf46)**: stubs/expansions.rs is
    the real engine — keystroke buffer fed from the tap processor
    (layout-aware chars via UCKeyTranslate over 'uchr' bytes cached on the
    MAIN thread at start_hooks; TIS calls dispatch-assert the main queue,
    verified SIGTRAP otherwise), Space pre-swallow in the tap callback,
    space/immediate triggers, smart case, full token resolver ({date},
    {clipboard}, {selection} via synthetic ⌘C capture, {set}/{if}/{=},
    {cursor}, {key:} chords, {{globals}} Pro gate), fill-in + variant picker
    on the fillin window with PID focus hand-back, image expansions
    (PNG+TIFF). NSPasteboard dual writes (plain + public.html — raw
    fragment, no CF_HTML container) + multi-flavor snapshot/restore +
    org.nspasteboard.ConcealedType marker. NO SUPPRESS_SIMULATED replay
    buffer on mac — tagged events replace it; mid-injection keystrokes pass
    through live. RUNTIME-VERIFIED in TextEdit via untagged synthetic
    events: space + immediate + date-token + smart-case (UPPER/Capitalized)
    fires all landed clean; clipboard restored after every fire.
  - **MACRO ACTION + SEND HOTKEY HOLD/REPEAT (12702d2)**: full step runner
    (loops with re-press/Esc/pause cancel, clipboard batching, all step
    types except AHK; Wait for Window / Focus Window match app NAME only —
    titles need Screen Recording; Wait for Input keyboard-only; Click at
    Position absolute-only), Send Hotkey hold + repeat modes, mouse
    click/move/scroll synthesis (core-graphics "highsierra"), "app"/"folder"
    actions via opener (monitor targeting still deferred). RUNTIME-VERIFIED:
    4-step macro, repeat toggle at 100ms, forever-loop + Esc cancel.
  - **BARE KEYS + ::double/::hold MATCHER (28ec212)**: full Windows dispatch
    semantics — linked-profile vs static-bare gating, AHK-style bare remap
    passthrough, keydown double-tap + cancelable single timers, HOLD_TIMERS
    + 16ms watcher, early-release re-injection, hold passthrough taps with
    live modifier state, suppress-set now includes bare + ::hold (Pro).
    RUNTIME-VERIFIED: bare F5, Ctrl+Shift+D single/double, F6 tap vs 600ms
    hold.
  - **Known parity quirk (upstream, both OSes)**: {cursor} lands one char
    right of ideal because the bundled trailing space isn't counted in
    cursor_back — Windows math is identical; fix upstream first if at all.
  - **Deferred (deliberately unsuppressed so keys stay alive)**: voice,
    radial, Quick Record capture (replay of Windows-recorded streams works),
    mouse hooks/triggers, monitor-targeted launches, AHK (forever).
- **Next**: UI-level human passes (fill-in/variant picker appearing,
  overlay UX, clean-machine .dmg on the test MacBook); then mouse hooks or
  Quick Record capture as the next engine milestone.
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

The engine is at near-parity with Windows: hooks, injection, matcher (bare /
double / hold / remap), expansions (buffer, triggers, tokens, fill-ins,
images), macros (all step types, loops, cancel), Send Hotkey (all modes),
clipboard history, tray, overlays, foreground watcher — all CI-green and
runtime-verified on this machine via synthetic untagged events.

Remaining human checks are UI-level: fill-in / variant picker appearing and
usable on a live expansion, overlay UX, profile auto-switch feel, and a
clean-machine .dmg pass on the test MacBook (TCC grants attach differently
to a bundled app).

Next engine work in rough order: mouse hooks (mouse triggers, Wait for
Input mouse types, hold release-on-mouse-up); Quick Record capture (replay
already works); monitor-targeted launches (window_target); voice / radial
(post-beta).
