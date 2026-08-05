#!/usr/bin/env bash
# Kills any leftover Keyfire dev processes after Ctrl+C, so `cargo tauri dev`
# can rebind port 5173 and re-spawn keyfire.exe cleanly.
#
# NOTE: this also kills the DETACHED background Vite that dev-server.mjs
# starts (it lives on 5173). That's fine as a full reset - the next
# `cargo tauri dev` detaches a fresh one - just don't run this mid-session
# if you're about to use a demo/profile relaunch, which needs Vite alive.
#
# Usage: ./scripts/dev-clean.sh

# Kill the compiled Keyfire app if still running.
taskkill //F //IM keyfire.exe 2>/dev/null

# Kill whatever process is bound to the Vite dev server port (5173).
netstat -ano | grep 'LISTENING' | grep ':5173' | awk '{print $5}' | sort -u \
  | xargs -r -I {} taskkill //F //PID {} 2>/dev/null

echo "Dev environment cleaned. Run: cargo tauri dev"
