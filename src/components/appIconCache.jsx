import React, { useEffect, useState } from 'react';

// Module-level cache mapping source_app (basename, e.g. "slack.exe") to a
// data-URL for the app's icon. First lookup fires the Rust `getAppIcon(path)`
// command; subsequent components rendering the same app read the cache
// synchronously. In-flight requests are deduped so ten cards for the same
// app fire ONE fetch.
//
// The `iconVersion` counter is bumped whenever a new icon lands so any
// AppIconBadge subscribing via useAppIcon re-renders once its icon is ready.

const iconByName = new Map();     // name → dataUrl | null (null = fetch failed)
const inflight = new Map();       // name → Promise<dataUrl|null>
const listeners = new Set();
let version = 0;

function notify() {
  version += 1;
  for (const cb of listeners) cb(version);
}

// Kick off a fetch for a given name+path if we don't have it yet. Returns
// the cached value synchronously if already resolved; otherwise undefined
// (caller renders fallback until useAppIcon re-runs).
//
// Resolution strategy:
// - Fresh rows carry both name + source_app_path. Path wins — fastest path.
// - Legacy rows carry only a name; we fall through to getAppIconByName which
//   resolves the exe via App Paths registry / running processes / System32
//   on the Rust side, then feeds the same icon-fetch pipeline.
function ensureIcon(name, path) {
  if (!name) return null;
  if (iconByName.has(name)) return iconByName.get(name);
  if (inflight.has(name)) return undefined;
  const p = (async () => {
    try {
      const dataUrl = path
        ? await window.electronAPI?.getAppIcon?.(path)
        : await window.electronAPI?.getAppIconByName?.(name);
      const val = dataUrl || null;
      iconByName.set(name, val);
      inflight.delete(name);
      notify();
      return val;
    } catch (_) {
      iconByName.set(name, null);
      inflight.delete(name);
      notify();
      return null;
    }
  })();
  inflight.set(name, p);
  return undefined;
}

// Public: subscribe to icon updates for a given app. Returns the current
// resolved value (data URL) or null (fetched, none) or undefined (pending).
export function useAppIcon(name, path) {
  const [, forceRender] = useState(0);
  useEffect(() => {
    const cb = (v) => forceRender(v);
    listeners.add(cb);
    return () => { listeners.delete(cb); };
  }, []);
  // Trigger the fetch if we don't have an entry yet.
  return ensureIcon(name, path);
}

// Public: shared badge that renders the app's icon when available, falling
// back to a compact text pill. Sized to sit inline with source_app metadata
// on the card / row. Alt text = the source_app name for accessibility.
export function AppIconBadge({ name, path, className = '', size = 14, showLabel = false }) {
  const icon = useAppIcon(name, path);
  if (!name) return null;
  if (icon) {
    return (
      <span className={`app-icon-badge ${className}`.trim()} title={name}>
        <img src={icon} width={size} height={size} alt="" draggable={false} />
        {showLabel && <span className="app-icon-badge-label">{name}</span>}
      </span>
    );
  }
  // Fallback: existing text-pill styling (caller controls the class). If
  // caller supplied showLabel-only mode the badge is invisible while the
  // fetch is pending, so it doesn't flash.
  return (
    <span className={`app-icon-badge app-icon-badge-fallback ${className}`.trim()} title={name}>
      {name}
    </span>
  );
}
