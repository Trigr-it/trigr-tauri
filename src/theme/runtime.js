// Theme runtime: the ONE mechanism that paints a theme into a window.
//
// Imported by src/main.jsx so it runs in every themed window (main, settings,
// Quick Search, clipboard popup, radial, fill-in, snip) BEFORE React mounts.
// The main window is the single resolver (App.jsx): it computes a snapshot
// { v, half, presetId, tokens|null } and publish()es it, which
//   1. applies it to its own <html>,
//   2. writes it to localStorage (all windows share one origin), and
//   3. emits 'theme-changed' so parked-but-alive windows update live.
// Every other window applies the cached snapshot at module load and again
// on visibilitychange (webview_mem parks hidden windows, so that fires on
// EVERY show), which covers the case where a TrySuspended window missed the
// emit. Inline custom properties on <html> beat both global.css and each
// overlay's local :root mirror block, so nothing else needs to change per
// window. Keyfire (tokens === null) removes every property, restoring the
// stylesheet defaults byte for byte.
import { listen, emit } from '@tauri-apps/api/event';
import { THEME_TOKENS } from './engine.js';

const SNAPSHOT_KEY = 'trigr_theme_snapshot';
// Legacy per-overlay cache from before the runtime existed. Still written
// (half only) for one release so nothing reading it regresses mid-update.
const LEGACY_KEY = 'trigr_overlay_theme';
export const THEME_EVENT = 'theme-changed';

function normaliseSnapshot(snap) {
  if (!snap || typeof snap !== 'object') return null;
  const half = snap.half === 'light' ? 'light' : 'dark';
  const tokens = (snap.tokens && typeof snap.tokens === 'object') ? snap.tokens : null;
  return { v: 1, half, presetId: snap.presetId || 'keyfire', tokens };
}

export function applySnapshot(rawSnap) {
  const snap = normaliseSnapshot(rawSnap);
  if (!snap) return;
  const root = document.documentElement;
  root.setAttribute('data-theme', snap.half);
  root.setAttribute('data-theme-preset', snap.presetId);
  const style = root.style;
  for (const key of THEME_TOKENS) {
    const v = snap.tokens ? snap.tokens[key] : null;
    if (typeof v === 'string' && v) style.setProperty(key, v);
    else style.removeProperty(key);
  }
  // Popup transparency below 100 % turns on the backdrop blur in the two
  // overlay stylesheets via this attribute (blur is a compositor cost, so it
  // is never on for opaque popups).
  if (snap.tokens && snap.tokens['--overlay-alpha']) root.setAttribute('data-translucent', '1');
  else root.removeAttribute('data-translucent');
  try { localStorage.setItem(LEGACY_KEY, snap.half); } catch { /* storage unavailable */ }
}

export function readCached() {
  try {
    const raw = localStorage.getItem(SNAPSHOT_KEY);
    if (!raw) return null;
    const snap = JSON.parse(raw);
    return snap && snap.v === 1 ? normaliseSnapshot(snap) : null;
  } catch {
    return null;
  }
}

/**
 * Apply the last published snapshot. Returns true when one existed. With no
 * snapshot yet (first launch before App.jsx has loaded config) it paints the
 * stylesheet defaults for the OS half so nothing renders with a mismatched
 * data-theme.
 */
export function applyCached() {
  const snap = readCached();
  if (snap) {
    applySnapshot(snap);
    return true;
  }
  let dark = true;
  try { dark = window.matchMedia('(prefers-color-scheme: dark)').matches; } catch { /* no matchMedia */ }
  applySnapshot({ v: 1, half: dark ? 'dark' : 'light', presetId: 'keyfire', tokens: null });
  return false;
}

/** Main window only: apply, cache, broadcast. */
export function publish(rawSnap) {
  const snap = normaliseSnapshot(rawSnap);
  if (!snap) return;
  applySnapshot(snap);
  try { localStorage.setItem(SNAPSHOT_KEY, JSON.stringify(snap)); } catch { /* storage unavailable */ }
  emit(THEME_EVENT, snap).catch(() => {});
}

let listening = false;
/** Subscribe this window to live theme changes. Idempotent. */
export function startListening() {
  if (listening) return;
  listening = true;
  listen(THEME_EVENT, (e) => applySnapshot(e.payload)).catch(() => { listening = false; });
}
