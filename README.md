# Trigr

**Visual hotkeys, macros, text expansions, and clipboard history for Windows. No scripting. Local-only.**

[Website](https://usetrigr.com) · [Latest release](https://github.com/Trigr-it/trigr-tauri/releases/latest) · [Help guide](https://usetrigr.com/trigr-help.html) · [Roadmap](https://usetrigr.com/roadmap.html)

---

## What Trigr is

Trigr is a Windows desktop app that lets non-technical users create keyboard hotkeys, macros, and text expansions through a visual interface. You see a keyboard on screen, click a key, pick what it does. No scripting language to learn.

It was built to replace AutoHotkey for teams where the original script author is not always around to maintain things, starting with the civil engineering firm where the maintainer works.

## Why the source is public

To prove the privacy claims on the website. Trigr runs fully locally, with no accounts, no cloud, no telemetry, and no data leaves your machine. You should not have to take that on faith. The source is here to be read and audited.

**This is not an invitation to copy or redistribute.** Trigr is a commercial product. Read [LICENSE.md](LICENSE.md) before using anything from this repository beyond reading and auditing it.

## Install

Download the latest installer for Windows 10 or 11:

- [Trigr_x64-setup.exe](https://github.com/Trigr-it/trigr-tauri/releases/latest/download/Trigr_x64-setup.exe) (x64)
- [Trigr_arm64-setup.exe](https://github.com/Trigr-it/trigr-tauri/releases/latest/download/Trigr_arm64-setup.exe) (ARM64, Surface Pro and Snapdragon devices)

Around 10MB installer, 20 to 50MB RAM at runtime. Auto-updates from this repository's Releases.

The app is free during alpha and beta. Paid tiers (Personal £29/yr, Pro £49/yr, Lifetime £99) launch with v1.0. Beta testers receive a minimum of one year free access on the paid tier they end up on.

## Features

- **Visual keyboard mapping.** Click a key, assign an action. No config files, no .ahk files.
- **Six action types.** Type Text, Send Hotkey (with hold and repeat modes), Macro Sequence (10 step types, including AHK Script), Open App / URL / Folder, Focus Window, Run AHK Script.
- **Text expansions.** Trigger snippets, fill-in fields, global variables, categories, image expansion, autocorrect, smart case matching.
- **App-specific profiles.** Foreground watcher auto-switches profiles when you change app. Your AutoCAD bindings only fire in AutoCAD.
- **Clipboard manager.** Ctrl+Shift+V overlay with history, search, pin, edit, auto-tagging, source app capture, scratchpad.
- **Quick Search overlay.** Ctrl+Space, search every assignment and expansion, with trigger+Space query mode for search-template packs.
- **Radial menu launcher.** Hotkey-triggered wheel of actions, 8 segments per profile, folder nesting, icon library.
- **Analytics dashboard.** Activity chart, heatmap, leaderboards, time-saved breakdown, CSV export (Pro).
- **AHK Script Runner.** Run AutoHotkey v1 and v2 scripts directly from a hotkey, no separate AHK install needed. Useful for migrating existing scripts without rewriting them.
- **Voice triggers** (experimental, Pro). Offline WinRT speech recognition for hands-busy workflows.

## Stack

- **Backend**: Rust + Tauri v2
- **Frontend**: React 18 + Vite
- **Storage**: SQLite (rusqlite, bundled), JSON config, local-only
- **Licence verification**: Offline Ed25519 signed keys, no phone-home
- **Hooks**: Win32 low-level keyboard and mouse hooks
- **Build**: GitHub Actions, x64 + ARM64 in parallel, NSIS installer
- **Auto-update**: tauri-plugin-updater with GitHub Releases backend

## Project structure (brief)

```
src-tauri/        Rust backend
  src/
    lib.rs          Tauri builder, commands, window management
    hotkeys.rs      Low-level keyboard and mouse hooks
    actions.rs      Action execution (text, hotkey, macro, app, AHK)
    expansions.rs   Text and image expansion, token resolution
    clipboard.rs    Clipboard history, SQLite, auto-tag
    config.rs       Config load, save, backup, file watcher
    tray.rs         System tray
    foreground.rs   Foreground watcher, app-profile switching
    analytics.rs    Usage analytics, time saved
    licence.rs      Offline Ed25519 licence verification
    voice.rs        WinRT speech recognition (experimental)

  trigr-keygen/     Standalone CLI crate to sign beta licence keys (private to maintainer)

src/              React frontend
  App.jsx           Root component, state, persistence
  components/       UI panels (Sidebar, MacroPanel, TextExpansions, etc.)
  styles/           CSS variables and global styles

docs/             GitHub Pages site (usetrigr.com)
```

## Building from source

The build instructions are included for transparency. Distribution of binaries you build is governed by [LICENSE.md](LICENSE.md).

Requirements:

- Rust 1.80 or later
- Node.js 20 or later
- Windows 10 / 11 (cross-compilation not supported)

```
npm install
cargo tauri dev      # development build, hot-reload frontend
cargo tauri build    # release build, produces NSIS installer
```

The output installer lives in `src-tauri/target/release/bundle/nsis/`.

## Reporting bugs

Inside the app: **Settings → Feedback**.
By email: **admin@usetrigr.com**.

For security issues, please email directly rather than opening a public issue.

## Contributing

Pull requests are accepted at the maintainer's discretion. By submitting a contribution you agree to the contribution terms in [LICENSE.md](LICENSE.md#contributions).

Before opening a PR for anything non-trivial, please email admin@usetrigr.com first to check it aligns with the roadmap.

## Licence

Source-available proprietary. See [LICENSE.md](LICENSE.md) for the full text. In short:

- Read, audit, run locally for evaluation: allowed
- Redistribute the source, fork as a competing product, sell derivatives: not allowed
- Compiled binaries are governed by a separate End User Licence shown inside the app

For commercial licensing, OEM, or anything else: **admin@usetrigr.com**.

## About

Trigr is built by a small studio in London. the founder is the maintainer.

*Set it. Trigr it.*
