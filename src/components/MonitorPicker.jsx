import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Monitor, Check } from 'lucide-react';
import './MonitorPicker.css';

const VIRTUAL_OPTIONS = [
  { value: 'default',    label: 'Wherever Windows decides', hint: 'Default — no monitor targeting' },
  { value: 'primary',    label: 'Primary monitor',           hint: 'Windows’ primary display' },
  { value: 'cursor',     label: 'Monitor under cursor',      hint: 'Wherever the mouse is when the action fires' },
  { value: 'foreground', label: 'Same as foreground window', hint: 'Monitor of the app that was focused at fire time' },
];

let cachedMonitors = null;
let cachedAt = 0;
const CACHE_TTL_MS = 10_000;

async function loadMonitors() {
  const now = Date.now();
  if (cachedMonitors && (now - cachedAt) < CACHE_TTL_MS) return cachedMonitors;
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    const list = await invoke('enum_monitors');
    cachedMonitors = Array.isArray(list) ? list : [];
    cachedAt = now;
    return cachedMonitors;
  } catch (e) {
    console.error('[Keyfire] enum_monitors failed:', e);
    return [];
  }
}

async function showIdentifyOverlays() {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    // Read the effective theme from <html data-theme="…">. App.jsx resolves
    // 'auto' to 'light' or 'dark' and writes that attribute on init and on
    // OS theme changes, so this is always the currently-rendered theme.
    const dark = document.documentElement.getAttribute('data-theme') !== 'light';
    await invoke('show_monitor_identify', { dark });
  } catch (e) {
    console.error('[Keyfire] show_monitor_identify failed:', e);
  }
}

async function hideIdentifyOverlays() {
  try {
    const { invoke } = await import('@tauri-apps/api/core');
    await invoke('hide_monitor_identify');
  } catch (e) {
    console.error('[Keyfire] hide_monitor_identify failed:', e);
  }
}

export default function MonitorPicker({ value, onChange }) {
  const [open, setOpen] = useState(false);
  const [monitors, setMonitors] = useState(cachedMonitors);
  // Portal position (viewport coords). The dropdown used to be absolutely
  // positioned inside the step row, which has overflow:hidden (415px rule)
  // and is followed by sibling rows that paint over it — so it rendered
  // clipped and underneath the next macro step.
  const [pos, setPos] = useState({ top: 0, left: 0, width: 0 });
  const wrapRef = useRef(null);
  const dropdownRef = useRef(null);
  const current = value || 'default';

  useEffect(() => {
    if (!open) return;
    function onDown(e) {
      const inWrap = wrapRef.current && wrapRef.current.contains(e.target);
      const inDrop = dropdownRef.current && dropdownRef.current.contains(e.target);
      if (!inWrap && !inDrop) setOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  // Identify overlays follow the dropdown's open state. Any close path — item
  // click, outside click, unmount, prop-driven — fires the hide, so nothing
  // gets stranded on-screen.
  useEffect(() => {
    if (open) {
      showIdentifyOverlays();
      return () => { hideIdentifyOverlays(); };
    }
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !wrapRef.current) return;
    const place = () => {
      const anchor = wrapRef.current?.getBoundingClientRect();
      if (!anchor) return;
      const height = dropdownRef.current?.offsetHeight || 0;
      let top = anchor.bottom + 4;
      // Flip above the button when there's no room below.
      if (height && top + height > window.innerHeight - 8) {
        top = Math.max(8, anchor.top - height - 4);
      }
      setPos({ top, left: anchor.left, width: anchor.width });
    };
    place();
    window.addEventListener('resize', place);
    // Any scroll (panel or page) moves the anchor — close rather than drift.
    const onScroll = () => setOpen(false);
    document.addEventListener('scroll', onScroll, true);
    return () => {
      window.removeEventListener('resize', place);
      document.removeEventListener('scroll', onScroll, true);
    };
  }, [open, monitors]);

  const handleToggle = async () => {
    if (open) { setOpen(false); return; }
    setOpen(true);
    const list = await loadMonitors();
    setMonitors(list);
  };

  const handleSelect = (newValue) => {
    onChange(newValue);
    setOpen(false);
  };

  const labelFor = (v) => {
    const virt = VIRTUAL_OPTIONS.find(o => o.value === v);
    if (virt) return virt.label;
    if (monitors) {
      const m = monitors.find(x => x.deviceName === v);
      if (m) return m.friendlyName + (m.isPrimary ? ' (primary)' : '');
    }
    return v.replace(/^\\\\\.\\DISPLAY/, 'Monitor ');
  };

  return (
    <div ref={wrapRef} className="monitor-pick-wrap">
      <button
        type="button"
        className={`monitor-pick-btn${current !== 'default' ? ' monitor-pick-btn-picked' : ''}`}
        onClick={handleToggle}
        title={labelFor(current)}
      >
        <Monitor size={14} className="monitor-pick-icon" />
        <span className="monitor-pick-label">{labelFor(current)}</span>
        <span className="monitor-pick-caret" aria-hidden="true">▾</span>
      </button>
      {open && createPortal(
        <div
          className="monitor-pick-dropdown monitor-pick-dropdown--portal"
          ref={dropdownRef}
          style={{ top: pos.top, left: pos.left, width: pos.width }}
        >
          {VIRTUAL_OPTIONS.map(o => (
            <div
              key={o.value}
              className={`monitor-pick-item${o.value === current ? ' is-active' : ''}`}
              onClick={() => handleSelect(o.value)}
              title={o.hint}
            >
              <span className="monitor-pick-item-label">{o.label}</span>
              {o.value === current && <Check size={14} className="monitor-pick-check" />}
            </div>
          ))}
          {monitors === null ? (
            <div className="monitor-pick-loading">Loading monitors…</div>
          ) : monitors.length > 0 ? (
            <>
              <div className="monitor-pick-sep" />
              {monitors.map(m => (
                <div
                  key={m.deviceName}
                  className={`monitor-pick-item${m.deviceName === current ? ' is-active' : ''}`}
                  onClick={() => handleSelect(m.deviceName)}
                  title={m.deviceName}
                >
                  <span className="monitor-pick-item-label">
                    {m.friendlyName}
                    {m.isPrimary && <span className="monitor-pick-primary-tag"> primary</span>}
                  </span>
                  {m.deviceName === current && <Check size={14} className="monitor-pick-check" />}
                </div>
              ))}
            </>
          ) : null}
        </div>,
        document.body
      )}
    </div>
  );
}
