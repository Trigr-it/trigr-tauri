#!/usr/bin/env bash
# Kills any leftover Trigr dev processes after Ctrl+C, so `cargo tauri dev`
# can rebind port 5173 and re-spawn trigr.exe cleanly.
#
# Usage: ./scripts/dev-clean.sh

# Kill the compiled Trigr app if still running.
taskkill //F //IM trigr.exe 2>/dev/null

# Kill whatever process is bound to the Vite dev server port (5173).
netstat -ano | grep 'LISTENING' | grep ':5173' | awk '{print $5}' | sort -u \
  | xargs -r -I {} taskkill //F //PID {} 2>/dev/null

echo "Dev environment cleaned. Run: cargo tauri dev"
