import { useEffect, useLayoutEffect, useRef, useState } from 'react';
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

export default function MonitorPicker({ value, onChange }) {
  const [open, setOpen] = useState(false);
  const [monitors, setMonitors] = useState(cachedMonitors);
  const wrapRef = useRef(null);
  const dropdownRef = useRef(null);
  const current = value || 'default';

  useEffect(() => {
    if (!open) return;
    function onDown(e) {
      if (wrapRef.current && !wrapRef.current.contains(e.target)) setOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !dropdownRef.current) return;
    const el = dropdownRef.current;
    el.style.top = '';
    el.style.bottom = '';
    el.style.marginTop = '';
    el.style.marginBottom = '';
    const rect = el.getBoundingClientRect();
    if (rect.bottom > window.innerHeight - 8) {
      el.style.top = 'auto';
      el.style.bottom = '100%';
      el.style.marginTop = '0';
      el.style.marginBottom = '4px';
    }
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
      {open && (
        <div className="monitor-pick-dropdown" ref={dropdownRef}>
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
        </div>
      )}
    </div>
  );
}
