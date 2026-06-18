#!/usr/bin/env bash
# Netlify static-site build step.
# Runs on every Netlify deploy. Two jobs:
#  1. Sync softwareVersion in docs/ JSON-LD schemas to the live app version
#     in src-tauri/tauri.conf.json so the deployed site never serves a stale
#     version in rich-results.
#  2. Notify IndexNow (Bing + Yandex crawlers) that the canonical URL list
#     may have changed. Failure of either step never breaks the deploy.

set -u  # but not -e — we want a soft-warn flow

# ── 1. softwareVersion auto-sub ────────────────────────────────────────────
VER=$(grep '"version"' src-tauri/tauri.conf.json | head -1 | sed 's/.*"version": *"\([0-9.]*\)".*/\1/')
if [ -n "$VER" ]; then
  echo "Substituting softwareVersion → $VER across docs/*.html"
  sed -i "s/\"softwareVersion\": \"[0-9.]*\"/\"softwareVersion\": \"$VER\"/g" docs/*.html || true
else
  echo "WARN: could not extract version from src-tauri/tauri.conf.json"
fi

# ── 2. IndexNow ping ───────────────────────────────────────────────────────
echo "IndexNow ping → api.indexnow.org"
curl -sS -X POST -H 'Content-Type: application/json' --data-raw '{
  "host": "usetrigr.com",
  "key": "24db6ec83ef4488ba706bf6cd5fcb81b",
  "keyLocation": "https://usetrigr.com/24db6ec83ef4488ba706bf6cd5fcb81b.txt",
  "urlList": [
    "https://usetrigr.com/",
    "https://usetrigr.com/pricing.html",
    "https://usetrigr.com/text-expander.html",
    "https://usetrigr.com/autohotkey-alternative.html",
    "https://usetrigr.com/roadmap.html",
    "https://usetrigr.com/trigr-help.html",
    "https://usetrigr.com/blog/",
    "https://usetrigr.com/blog/custom-hotkeys-windows-no-coding.html",
    "https://usetrigr.com/blog/best-text-expander-windows-11-2026.html"
  ]
}' https://api.indexnow.org/indexnow || true

echo "Build script done."
