import React, { useState, useEffect, useLayoutEffect, useRef, useMemo, Fragment, useCallback } from 'react';
import { createPortal } from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import {
  Type, Keyboard, AppWindow, Globe, FolderOpen, Layers, FileCode,
  GripVertical, Copy,
} from 'lucide-react';
import './MacroPanel.css';
import { SearchBar } from './SearchBar';
import { friendlyKeyName, STATIC_BARE_ALLOWED } from './keyboardLayout';
import { readVoicePhrases, writeVoicePhrases } from '../voicePhrases';

const ACTION_TYPES = [
  {
    id: 'text',
    Icon: Type,
    label: 'Type Text',
    desc: 'Types a text snippet when key is pressed',
    color: '#64b4ff',
  },
  {
    id: 'hotkey',
    Icon: Keyboard,
    label: 'Send Hotkey',
    desc: 'Triggers a key combination like Ctrl+C',
    color: '#c864ff',
  },
  {
    id: 'app',
    Icon: AppWindow,
    label: 'Open App',
    desc: 'Launch an application or file',
    color: '#50c878',
  },
  {
    id: 'url',
    Icon: Globe,
    label: 'Open URL',
    desc: 'Open a website in your browser',
    color: '#ffc832',
  },
  {
    id: 'folder',
    Icon: FolderOpen,
    label: 'Open Folder',
    desc: 'Open a folder in File Explorer',
    color: '#40c8a0',
  },
  {
    id: 'macro',
    Icon: Layers,
    label: 'Macro Sequence',
    desc: 'Run a sequence of actions one after another',
    color: '#ff783c',
  },
  {
    id: 'ahk',
    Icon: FileCode,
    label: 'AHK Script',
    desc: 'Run an AutoHotkey v1 or v2 script',
    color: '#4ecdc4',
  },
];

const MODIFIER_KEYS = ['Ctrl', 'Alt', 'Shift', 'Win'];
const TRIGGER_KEYS = [
  'A','B','C','D','E','F','G','H','I','J','K','L','M',
  'N','O','P','Q','R','S','T','U','V','W','X','Y','Z',
  'F1','F2','F3','F4','F5','F6','F7','F8','F9','F10','F11','F12',
  '0','1','2','3','4','5','6','7','8','9',
  'Space','Tab','Enter','Escape','Delete','Home','End','PageUp','PageDown',
  'Up','Down','Left','Right',
];

const MACRO_STEP_TYPES = ['Type Text', 'Press Key', 'Click Mouse', 'Click at Position', 'Open App', 'Open URL', 'Open Folder', 'Wait (ms)', 'Wait for Input', 'Focus Window', 'Run AHK Script'];

const WFI_INPUT_OPTIONS = [
  { value: 'LButton',     label: 'Left Click'   },
  { value: 'RButton',     label: 'Right Click'  },
  { value: 'MButton',     label: 'Middle Click' },
  { value: 'AnyKey',      label: 'Any Key'      },
  { value: 'SpecificKey', label: 'Specific Key' },
];

const WFI_TRIGGER_OPTIONS = [
  { value: 'press',        label: 'Press (down)'          },
  { value: 'release',      label: 'Release (up)'          },
  { value: 'pressRelease', label: 'Press and Release'     },
];

const MOUSE_CLICK_OPTIONS = [
  { value: 'LButton', label: 'Left Click' },
  { value: 'RButton', label: 'Right Click' },
  { value: 'MButton', label: 'Middle Click' },
];

const INPUT_METHOD_OPTS = [
  { id: 'global',       label: 'Global default',  hint: 'Use the method set in Settings → Compatibility' },
  { id: 'direct',       label: 'Direct',           hint: 'Simulates real keypresses — works in CAD, games, any app' },
  { id: 'shift-insert', label: 'Clipboard',        hint: 'Fast for long text — pastes via clipboard' },
];

function TextForm({ value, onChange, globalInputMethod }) {
  // Read inputMethod; fall back to legacy pasteMethod for backward compat
  const inputMethod = value.inputMethod ||
    (value.pasteMethod && value.pasteMethod !== 'shift-insert' ? value.pasteMethod : 'global');
  const globalLabel = INPUT_METHOD_OPTS.find(o => o.id === globalInputMethod)?.label || globalInputMethod;
  return (
    <div className="form-section">
      <label className="form-label">Text to type</label>
      <textarea
        className="form-textarea"
        placeholder="Enter the text that will be typed when this key is pressed..."
        value={value.text || ''}
        onChange={e => onChange({ ...value, text: e.target.value })}
        rows={4}
      />
      <label className="form-label" style={{ marginTop: 12 }}>Input method</label>
      <div className="paste-method-group">
        {INPUT_METHOD_OPTS.map(opt => (
          <label
            key={opt.id}
            className={`paste-method-opt${inputMethod === opt.id ? ' active' : ''}`}
          >
            <input
              type="radio"
              name="inputMethod"
              value={opt.id}
              checked={inputMethod === opt.id}
              onChange={() => onChange({ ...value, inputMethod: opt.id, pasteMethod: undefined })}
            />
            <span className="paste-opt-label">
              {opt.label}{opt.id === 'global' ? <span className="input-method-global-val"> ({globalLabel})</span> : null}
            </span>
            <span className="paste-opt-hint">{opt.hint}</span>
          </label>
        ))}
      </div>
    </div>
  );
}

// Converts hotkey data object → display string e.g. { modifiers: ['Ctrl'], key: 'F4' } → "Ctrl+F4"
function hotkeyDataToString(data) {
  return [...(data.modifiers || []), data.key || ''].filter(Boolean).join('+');
}

// (Inline Win-key builder was removed in favour of the +Win toggle pill that
// pops out as an advisory panel below the capture field when the user presses
// the Windows key. See HotkeyCaptureInput / KeyCaptureInput.)

// Parses a captured combo string → hotkey data fields e.g. "Ctrl+Win+F4" → { modifiers: [...], key: 'F4' }
const HOTKEY_MODS = new Set(['Ctrl', 'Shift', 'Alt', 'Win']);
function parseHotkeyCapture(str) {
  const parts = str.split('+');
  return {
    modifiers: parts.filter(p => HOTKEY_MODS.has(p)),
    key:       parts.find(p => !HOTKEY_MODS.has(p)) || '',
  };
}

function HotkeyCaptureInput({ value, onChange }) {
  const [capturing, setCapturing] = useState(false);
  const [winPrompted, setWinPrompted] = useState(false);
  const divRef        = useRef(null);
  const onChangeRef   = useRef(onChange);
  const valueRef      = useRef(value);
  const capturingRef  = useRef(false);

  // Storage is { modifiers: [...], key }. The advisory sub-row owns the Win
  // representation via the +Win pill, so chips strip it.
  const hasWin = !!(value.modifiers || []).includes('Win');
  const hasWinRef = useRef(false);
  const showWinPanel = winPrompted || hasWin;

  useEffect(() => { onChangeRef.current = onChange; }, [onChange]);
  useEffect(() => { valueRef.current    = value;    }, [value]);
  useEffect(() => { capturingRef.current = capturing; }, [capturing]);
  useEffect(() => { hasWinRef.current    = hasWin;   }, [hasWin]);

  useEffect(() => {
    if (!window.electronAPI?.onKeyCaptured) return;
    const handler = (combo) => {
      if (!capturingRef.current) return;
      const parsed = parseHotkeyCapture(combo);
      // Preserve a previously-toggled +Win across re-captures.
      const mods = (parsed.modifiers || []).filter(m => m !== 'Win');
      if (hasWinRef.current) mods.unshift('Win');
      onChangeRef.current({ ...valueRef.current, modifiers: mods, key: parsed.key });
      setCapturing(false);
    };
    window.electronAPI.onKeyCaptured(handler);
    return () => window.electronAPI.removeAllListeners('key-captured');
  }, []);

  function startCapture() {
    setCapturing(true);
    divRef.current?.focus();
    window.electronAPI?.startKeyCapture();
  }

  function handleKeyDown(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      cancelCapture();
      return;
    }
    // Win opens the Start menu — we can't stop that (WebView2 limitation),
    // but we keep capture alive so the rest of the combo gets picked up by
    // the LL hook once Trigr loses focus. The advisory below explains the
    // +Win toggle for explicit Win-modifier composition.
    if (e.key === 'Meta') {
      e.preventDefault();
      e.stopPropagation();
      setWinPrompted(true);
    }
  }

  function cancelCapture() {
    window.electronAPI?.stopKeyCapture();
    setCapturing(false);
    divRef.current?.blur();
  }

  function handleBlur(e) {
    if (e.currentTarget.contains(e.relatedTarget)) return;
    if (e.relatedTarget?.dataset?.captureCancel) return;
    if (capturing) {
      window.electronAPI?.stopKeyCapture();
      setCapturing(false);
    }
  }

  function toggleWin() {
    const mods = (value.modifiers || []).filter(m => m !== 'Win');
    if (!hasWin) mods.unshift('Win');
    onChange({ ...value, modifiers: mods });
  }

  // Hide the Win modifier in the chip display since the advisory's pill owns
  // the visual representation.
  const baseValue = { ...value, modifiers: (value.modifiers || []).filter(m => m !== 'Win') };
  const currentCombo = hotkeyDataToString(baseValue);
  const isMouseValue = MOUSE_CLICK_OPTIONS.some(o => o.value === value.key && (!value.modifiers || value.modifiers.filter(m => m !== 'Win').length === 0));

  function toggleWin() {
    const mods = (value.modifiers || []).filter(m => m !== 'Win');
    if (!hasWin) mods.unshift('Win');
    onChange({ ...value, modifiers: mods });
  }

  return (
    <div className="form-section">
      <label className="form-label">Hotkey</label>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, minWidth: 0 }}>
        <div
          ref={divRef}
          className={`key-capture${capturing ? ' key-capture-active' : ''}`}
          tabIndex={0}
          onClick={!capturing ? startCapture : undefined}
          onKeyDown={capturing ? handleKeyDown : undefined}
          onBlur={handleBlur}
          role="button"
          aria-label={capturing ? 'Press your hotkey combination' : currentCombo || 'Click to capture hotkey'}
          style={{ flex: 1, minWidth: 0 }}
        >
          {capturing ? (
            <span className="key-capture-prompt">Press your hotkey combination…</span>
          ) : isMouseValue ? (
            <span className="key-capture-value"><kbd>{MOUSE_CLICK_OPTIONS.find(o => o.value === value.key)?.label}</kbd></span>
          ) : currentCombo ? (
            <span className="key-capture-value"><KeyChips combo={currentCombo} /></span>
          ) : (
            <span className="key-capture-placeholder">Click to capture hotkey…</span>
          )}
        </div>
        {capturing && (
          <button
            className="macro-advanced-toggle"
            type="button"
            data-capture-cancel="true"
            onMouseDown={e => { e.preventDefault(); cancelCapture(); }}
          >Cancel</button>
        )}
      </div>
      {showWinPanel && (
        <div className="step-advisory-row step-advisory-row--inline">
          <span className="step-advisory-icon" aria-hidden="true">ⓘ</span>
          <span className="step-advisory-text">
            Windows key can't be captured directly (it opens the Start menu). Toggle to add it as a modifier.
          </span>
          <button
            type="button"
            className={`win-toggle-pill${hasWin ? ' win-toggle-pill-on' : ''}`}
            onClick={e => { e.stopPropagation(); toggleWin(); }}
            title={hasWin ? 'Remove Windows key' : 'Add Windows key'}
          >
            {hasWin ? '✓ Win' : '+ Win'}
          </button>
          {winPrompted && !hasWin && (
            <button
              type="button"
              className="step-advisory-dismiss"
              onClick={() => setWinPrompted(false)}
              title="Dismiss"
              aria-label="Dismiss"
            >×</button>
          )}
        </div>
      )}
      <div className="mouse-click-pills">
        {MOUSE_CLICK_OPTIONS.map(opt => (
          <button
            key={opt.value}
            type="button"
            className={`mouse-click-pill${value.key === opt.value && (!value.modifiers || value.modifiers.length === 0) ? ' active' : ''}`}
            onClick={() => onChange({ ...value, modifiers: [], key: opt.value })}
          >{opt.label}</button>
        ))}
      </div>
    </div>
  );
}

// AppPickerModal — searchable list of apps from Windows AppsFolder, with a
// fallback to the file browser. Selecting an installed app stores an AUMID
// (portable across devices); browsing a file stores an absolute path (does
// not sync portably — same as the legacy behavior).
function AppPickerModal({ onSelect, onClose }) {
  const [apps, setApps] = useState([]);
  const [search, setSearch] = useState('');
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let mounted = true;
    (async () => {
      const result = await window.electronAPI?.listInstalledApps();
      if (!mounted) return;
      setApps(Array.isArray(result) ? result : []);
      setLoading(false);
    })();
    return () => { mounted = false; };
  }, []);

  useEffect(() => {
    const onKey = (e) => { if (e.key === 'Escape') onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return apps;
    return apps.filter(a => a.name.toLowerCase().includes(q));
  }, [apps, search]);

  async function handleBrowseFallback() {
    const path = await window.electronAPI?.browseForFile();
    if (path) {
      const baseName = path.split(/[\\/]/).pop().replace(/\.(exe|lnk|bat|cmd)$/i, '');
      onSelect({ kind: 'path', path, appId: '', name: baseName });
    }
  }

  return createPortal(
    <div className="app-picker-overlay" onClick={onClose}>
      <div className="app-picker-modal" onClick={e => e.stopPropagation()}>
        <div className="app-picker-header">
          <SearchBar
            className="app-picker-search-bar"
            placeholder="Search installed apps..."
            value={search}
            onChange={e => setSearch(e.target.value)}
            autoFocus
          />
          <button className="app-picker-close" type="button" onClick={onClose} title="Close">✕</button>
        </div>
        <div className="app-picker-list">
          {loading ? (
            <div className="app-picker-empty">Loading apps...</div>
          ) : filtered.length === 0 ? (
            <div className="app-picker-empty">
              {apps.length === 0 ? 'No apps found.' : 'No matches.'}
            </div>
          ) : (
            filtered.map(app => (
              <button
                key={app.appId}
                type="button"
                className="app-picker-item"
                onClick={() => onSelect({ kind: 'aumid', path: '', appId: app.appId, name: app.name })}
                title={app.appId}
              >
                <span className="app-picker-name">{app.name}</span>
              </button>
            ))
          )}
        </div>
        <div className="app-picker-footer">
          <span className="app-picker-hint">Can't find it?</span>
          <button className="app-picker-browse-link" type="button" onClick={handleBrowseFallback}>
            Browse for a file instead...
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}

export function AppForm({ value, onChange }) {
  const [pickerOpen, setPickerOpen] = useState(false);

  function handlePick(picked) {
    setPickerOpen(false);
    if (picked.kind === 'aumid') {
      onChange({
        ...value,
        kind: 'aumid',
        appId: picked.appId,
        path: '',
        appName: value.appName || picked.name || '',
      });
    } else {
      onChange({
        ...value,
        kind: 'path',
        appId: '',
        path: picked.path,
        appName: value.appName || picked.name || '',
      });
    }
  }

  // Display the chosen target — installed app name or absolute path.
  const isAumid = value.kind === 'aumid' && !!value.appId;
  const displayLabel = isAumid
    ? (value.appName || 'Installed app')
    : (value.path || '');

  return (
    <div className="form-section">
      <label className="form-label">Application</label>
      <div className="file-input-row">
        <input
          className="form-input"
          placeholder="Pick an installed app or browse for a file..."
          value={displayLabel}
          onChange={e => onChange({ ...value, kind: 'path', appId: '', path: e.target.value })}
        />
        <button className="browse-btn" type="button" onClick={() => setPickerOpen(true)}>Pick app...</button>
      </div>
      {isAumid && (
        <div className="form-hint">
          Syncs across devices — uses the Windows app identifier.
        </div>
      )}
      {!isAumid && value.path && (
        <div className="form-hint">
          Absolute path — won't work on other devices unless the app is installed at the same location.
        </div>
      )}
      <label className="form-label" style={{ marginTop: 12 }}>Display name (optional)</label>
      <input
        className="form-input"
        placeholder="My App"
        value={value.appName || ''}
        onChange={e => onChange({ ...value, appName: e.target.value })}
      />
      {pickerOpen && <AppPickerModal onSelect={handlePick} onClose={() => setPickerOpen(false)} />}
    </div>
  );
}

function FolderForm({ value, onChange }) {
  async function handleBrowse() {
    const path = await window.electronAPI?.browseForFolder();
    if (path) onChange({ ...value, path });
  }
  return (
    <div className="form-section">
      <label className="form-label">Folder path</label>
      <div className="file-input-row">
        <input
          className="form-input"
          placeholder="C:\Users\Me\Documents"
          value={value.path || ''}
          onChange={e => onChange({ ...value, path: e.target.value })}
        />
        <button className="browse-btn" type="button" onClick={handleBrowse}>Browse</button>
      </div>
      <label className="form-label" style={{ marginTop: 12 }}>Display name (optional)</label>
      <input
        className="form-input"
        placeholder="My Documents"
        value={value.folderName || ''}
        onChange={e => onChange({ ...value, folderName: e.target.value })}
      />
      <div className="form-hint">Opens the folder in File Explorer when the key is pressed.</div>
    </div>
  );
}

function UrlForm({ value, onChange }) {
  return (
    <div className="form-section">
      <label className="form-label">URL to open</label>
      <input
        className="form-input"
        placeholder="https://example.com"
        value={value.url || ''}
        onChange={e => onChange({ ...value, url: e.target.value })}
      />
      <label className="form-label" style={{ marginTop: 12 }}>Display name (optional)</label>
      <input
        className="form-input"
        placeholder="My Website"
        value={value.urlName || ''}
        onChange={e => onChange({ ...value, urlName: e.target.value })}
      />
    </div>
  );
}

function AhkForm({ value, onChange }) {
  const version = value.ahkVersion || 'v1';
  const isV2 = version === 'v2';
  return (
    <div className="form-section">
      <div className="ahk-version-row">
        <button
          type="button"
          className={`ahk-version-pill ${!isV2 ? 'active' : ''}`}
          onClick={() => onChange({ ...value, ahkVersion: 'v1' })}
        >v1</button>
        <button
          type="button"
          className={`ahk-version-pill ${isV2 ? 'active' : ''}`}
          onClick={() => onChange({ ...value, ahkVersion: 'v2' })}
        >v2</button>
      </div>
      <label className="form-label">AHK {version} Script</label>
      <textarea
        className="form-textarea"
        placeholder={isV2
          ? "; Write your AutoHotkey v2 script here\nMsgBox \"Hello from Trigr!\"\nSend \"{Enter}\""
          : "; Write your AutoHotkey v1 script here\nMsgBox, Hello from Trigr!\nSend, {Enter}"}
        value={value.script || ''}
        onChange={e => onChange({ ...value, script: e.target.value })}
        rows={8}
        onKeyDown={e => e.stopPropagation()}
      />
      <div className="form-hint">
        Write the script body only — no hotkey labels needed. Trigr handles the trigger.
      </div>
    </div>
  );
}

// Inline pick button for Click at Position (sits on the step row beside dropdown)
function ClickPositionPickBtn({ step, updateStep }) {
  let cp = { x: 0, y: 0, button: 'left', mode: 'absolute' };
  try { cp = { ...cp, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  const [picking, setPicking] = useState(false);
  const [countdown, setCountdown] = useState(0);

  const pickPosition = async () => {
    setPicking(true);
    for (let i = 3; i > 0; i--) {
      setCountdown(i);
      await new Promise(r => setTimeout(r, 1000));
    }
    setCountdown(0);
    const pos = await window.electronAPI?.getCursorPosition();
    setPicking(false);
    if (pos) {
      updateStep({ ...step, value: JSON.stringify({ ...cp, x: pos.x, y: pos.y }) });
    }
  };

  return (
    <button type="button" className="browse-btn" onClick={pickPosition} disabled={picking} style={{ flexShrink: 0 }}>
      {picking ? `${countdown}...` : 'Pick Position'}
    </button>
  );
}

// Sub-row fields for Click at Position (X, Y, button selector)
function ClickPositionFields({ step, updateStep }) {
  let cp = { x: 0, y: 0, button: 'left', mode: 'absolute' };
  try { cp = { ...cp, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  const update = (patch) => updateStep({ ...step, value: JSON.stringify({ ...cp, ...patch }) });

  return (
    <div className="wfi-config-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <label style={{ fontSize: 10, color: 'var(--text-muted)', fontWeight: 600 }}>X</label>
        <input className="form-input" type="number" style={{ width: 70 }} value={cp.x}
          onChange={e => update({ x: parseInt(e.target.value) || 0 })}
          onKeyDown={e => e.stopPropagation()} />
        <label style={{ fontSize: 10, color: 'var(--text-muted)', fontWeight: 600 }}>Y</label>
        <input className="form-input" type="number" style={{ width: 70 }} value={cp.y}
          onChange={e => update({ y: parseInt(e.target.value) || 0 })}
          onKeyDown={e => e.stopPropagation()} />
        <span style={{ fontSize: 10, color: 'var(--text-muted)' }}>
          {cp.x || cp.y ? `(${cp.x}, ${cp.y})` : ''}
        </span>
      </div>
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <select className="form-select" value={cp.button} style={{ flex: 1 }}
          onChange={e => update({ button: e.target.value })}>
          <option value="left">Left Click</option>
          <option value="right">Right Click</option>
          <option value="middle">Middle Click</option>
        </select>
      </div>
    </div>
  );
}

const KEY_DISPLAY_MAP = {
  ' ': 'Space', 'ArrowUp': 'Up', 'ArrowDown': 'Down',
  'ArrowLeft': 'Left', 'ArrowRight': 'Right',
  'Escape': 'Escape', 'Enter': 'Enter', 'Tab': 'Tab',
  'Backspace': 'Backspace', 'Delete': 'Delete',
  'Home': 'Home', 'End': 'End', 'PageUp': 'PageUp', 'PageDown': 'PageDown',
  'Insert': 'Insert', 'PrintScreen': 'PrintScreen',
};

// Shared helper: render captured key combo as <kbd> chips
function KeyChips({ combo }) {
  const keys = combo ? combo.split('+') : [];
  return (
    <>
      {keys.map((k, i) => (
        <Fragment key={i}>
          <kbd>{friendlyKeyName(k)}</kbd>
          {i < keys.length - 1 && <span className="key-capture-plus">+</span>}
        </Fragment>
      ))}
    </>
  );
}

function FocusWindowFields({ focusData, onChange }) {
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const [windowList, setWindowList] = useState(null); // null = not loaded, [] = empty
  const dropdownRef = useRef(null);

  // Close dropdown on outside click
  useEffect(() => {
    if (!dropdownOpen) return;
    function onDown(e) {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target)) {
        setDropdownOpen(false);
      }
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [dropdownOpen]);

  const handlePickClick = async () => {
    if (dropdownOpen) { setDropdownOpen(false); return; }
    setWindowList(null);
    setDropdownOpen(true);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const list = await invoke('list_open_windows');
      setWindowList(list || []);
    } catch (e) {
      console.error('[Trigr] list_open_windows failed:', e);
      setWindowList([]);
    }
  };

  const handleSelect = (win) => {
    onChange({ process: win.process, title: win.title });
    setDropdownOpen(false);
  };

  const handleClear = () => {
    onChange({ ...focusData, process: '' });
  };

  return (
    <div className="wfi-config-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
      <div className="pick-window-row" ref={dropdownRef}>
        {focusData.process ? (
          <span className="pick-window-badge">
            {focusData.process}
            <button className="pick-window-badge-clear" type="button" onClick={handleClear}>✕</button>
          </span>
        ) : null}
        <button className="browse-btn" type="button" onClick={handlePickClick}>
          ⊞ Pick Window
        </button>
        {dropdownOpen && (
          <div className="pick-window-dropdown">
            {windowList === null ? (
              <div className="pick-window-loading">Loading windows…</div>
            ) : windowList.length === 0 ? (
              <div className="pick-window-loading">No open windows found</div>
            ) : (
              windowList.map((win, i) => (
                <div key={i} className="pick-window-item" onClick={() => handleSelect(win)}>
                  <span className="pick-window-process">{win.process}</span>
                  <span className="pick-window-title">{win.title}</span>
                </div>
              ))
            )}
          </div>
        )}
      </div>
      <input
        className="form-input"
        placeholder="Window title match (optional)"
        value={focusData.title}
        onChange={e => onChange({ ...focusData, title: e.target.value })}
      />
    </div>
  );
}

function KeyCaptureInput({ value, onChange, onWinPressed }) {
  const [capturing, setCapturing] = useState(false);
  const divRef       = useRef(null);
  const onChangeRef  = useRef(onChange);
  const capturingRef = useRef(false);

  // Storage is always the full combo (e.g. "Win+Ctrl+F4"). The chips display
  // strips Win+ since the parent's advisory sub-row owns the visual Win
  // representation via the +Win toggle pill.
  const hasWin = value === 'Win' || value.startsWith('Win+');
  const baseValue = hasWin ? value.replace(/^Win\+?/, '') : value;
  const hasWinRef = useRef(false);

  useEffect(() => { onChangeRef.current  = onChange;   }, [onChange]);
  useEffect(() => { capturingRef.current = capturing;  }, [capturing]);
  useEffect(() => { hasWinRef.current    = hasWin;     }, [hasWin]);

  useEffect(() => {
    if (!window.electronAPI?.onKeyCaptured) return;
    const handler = (combo) => {
      if (!capturingRef.current) return;
      // Preserve a previously-toggled +Win across re-captures.
      const stripped = combo.replace(/^Win\+?/, '');
      const finalCombo = hasWinRef.current ? (stripped ? `Win+${stripped}` : 'Win') : stripped;
      onChangeRef.current(finalCombo);
      setCapturing(false);
    };
    window.electronAPI.onKeyCaptured(handler);
    return () => window.electronAPI.removeAllListeners('key-captured');
  }, []);

  function startCapture() {
    setCapturing(true);
    divRef.current?.focus();
    window.electronAPI?.startKeyCapture();
  }

  function handleKeyDown(e) {
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      cancelCapture();
      return;
    }
    // Win opens the Start menu — we can't stop that (WebView2 limitation),
    // but we keep capture alive so the rest of the combo (e.g. Shift+S in
    // Win+Shift+S) gets picked up by the LL hook once Trigr loses focus.
    // The advisory below the row tells the user how to add Win explicitly.
    if (e.key === 'Meta') {
      e.preventDefault();
      e.stopPropagation();
      onWinPressed?.();
    }
  }

  function cancelCapture() {
    window.electronAPI?.stopKeyCapture();
    setCapturing(false);
    divRef.current?.blur();
  }

  function handleBlur(e) {
    if (e.currentTarget.contains(e.relatedTarget)) return;
    if (e.relatedTarget?.dataset?.captureCancel) return;
    if (capturing) {
      window.electronAPI?.stopKeyCapture();
      setCapturing(false);
    }
  }

  const isMouseValue = MOUSE_CLICK_OPTIONS.some(o => o.value === value);
  const chipsValue = isMouseValue ? value : baseValue;

  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 4, flex: 1, minWidth: 0 }}>
      <div
        ref={divRef}
        className={`key-capture macro-step-value${capturing ? ' key-capture-active' : ''}`}
        tabIndex={0}
        onClick={!capturing ? startCapture : undefined}
        onKeyDown={capturing ? handleKeyDown : undefined}
        onBlur={handleBlur}
        role="button"
        aria-label={capturing ? 'Press a key combination' : value || 'Click to capture key'}
        style={{ flex: 1, minWidth: 0 }}
      >
        {capturing ? (
          <span className="key-capture-prompt">Press a key…</span>
        ) : isMouseValue ? (
          <span className="key-capture-value"><kbd>{MOUSE_CLICK_OPTIONS.find(o => o.value === value)?.label}</kbd></span>
        ) : chipsValue ? (
          <span className="key-capture-value"><KeyChips combo={chipsValue} /></span>
        ) : (
          <span className="key-capture-placeholder">Click to capture…</span>
        )}
      </div>
      {capturing && (
        <button
          className="macro-advanced-toggle"
          type="button"
          data-capture-cancel="true"
          onMouseDown={e => { e.preventDefault(); cancelCapture(); }}
        >Cancel</button>
      )}
    </div>
  );
}

// Open-App sub-row used inside SortableMacroStep — picker + optional args.
function MacroOpenAppRow({ appData, updateValue, advancedOpen, toggleAdvanced }) {
  const [pickerOpen, setPickerOpen] = useState(false);
  const isAumid = appData.kind === 'aumid' && !!appData.appId;
  const displayLabel = isAumid
    ? (appData.appName || 'Installed app')
    : (appData.path || '');

  function handlePick(picked) {
    setPickerOpen(false);
    if (picked.kind === 'aumid') {
      updateValue({ ...appData, kind: 'aumid', appId: picked.appId, appName: picked.name || appData.appName || '', path: '' });
    } else {
      updateValue({ ...appData, kind: 'path', appId: '', appName: appData.appName || picked.name || '', path: picked.path });
    }
  }

  return (
    <div className="wfi-config-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
      <div className="file-input-row">
        <input
          className="form-input"
          style={{ flex: 1 }}
          placeholder="Pick an installed app or browse for a file..."
          value={displayLabel}
          readOnly
        />
        <button className="browse-btn" type="button" onClick={() => setPickerOpen(true)}>Pick app...</button>
      </div>
      {(advancedOpen || appData.args) ? (
        <input
          className="form-input"
          placeholder="Arguments (optional)"
          value={appData.args}
          onChange={e => updateValue({ ...appData, args: e.target.value })}
        />
      ) : (
        <button className="macro-advanced-toggle" type="button" onClick={toggleAdvanced}>+ Advanced</button>
      )}
      {pickerOpen && <AppPickerModal onSelect={handlePick} onClose={() => setPickerOpen(false)} />}
    </div>
  );
}

// ── Sortable step row (extracted for @dnd-kit) ─────────────────────────────

let _nextStepId = 1;

function SortableMacroStep({ step, index, updateStep, removeStep, duplicateStep, advancedOpen, toggleAdvanced }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: step._id });
  const style = {
    transform: DndCSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
  };

  // Press Key Win-advisory — the LL hook can't intercept the Win key when
  // Trigr's own WebView2 has focus (documented Tauri/WebView2 limitation), so
  // direct Win+X capture isn't possible inside our own UI. The advisory
  // sub-row is the deliberate alternative: shown whenever Win is in the combo
  // OR the user just pressed Win during capture.
  const [winPrompted, setWinPrompted] = useState(false);
  const stepValue = step.value || '';
  const stepHasWin = step.type === 'Press Key' && (stepValue === 'Win' || stepValue.startsWith('Win+'));
  const showWinAdvisory = step.type === 'Press Key' && (winPrompted || stepHasWin);

  function toggleStepWin() {
    if (stepHasWin) {
      updateStep({ ...step, value: stepValue.replace(/^Win\+?/, '') });
    } else {
      updateStep({ ...step, value: stepValue ? `Win+${stepValue}` : 'Win' });
    }
  }

  const hasSubRow = ['Wait for Input', 'Open App', 'Open Folder', 'Focus Window', 'Run AHK Script', 'Click at Position'].includes(step.type) || showWinAdvisory;

  // Parse JSON values for structured step types
  let appData = { kind: 'path', appId: '', appName: '', path: '', args: '' };
  if (step.type === 'Open App') { try { appData = { ...appData, ...JSON.parse(step.value || '{}') }; } catch (_) {} }
  let focusData = { process: '', title: '' };
  if (step.type === 'Focus Window') { try { focusData = { ...focusData, ...JSON.parse(step.value || '{}') }; } catch (_) {} }

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={`macro-step${isDragging ? ' macro-step-dragging' : ''}${hasSubRow ? ' macro-step-wfi' : ''}`}
    >
      {/* Row 1: drag handle, step number, type dropdown, inline value, delete */}
      <div className="macro-step-row">
        <div className="step-drag-handle" {...attributes} {...listeners} title="Drag to reorder" aria-label="Drag to reorder">
          <GripVertical size={14} strokeWidth={1.75} />
        </div>
        <div className="macro-step-num">{index + 1}</div>
        <select
          className="form-select macro-step-type"
          value={step.type}
          onChange={e => updateStep({ ...step, type: e.target.value, value: '' })}
        >
          {MACRO_STEP_TYPES.map(t => <option key={t}>{t}</option>)}
        </select>

        {/* Inline value fields */}
        {step.type === 'Press Key' && (
          <KeyCaptureInput
            value={step.value || ''}
            onChange={v => updateStep({ ...step, value: v })}
            onWinPressed={() => setWinPrompted(true)}
          />
        )}
        {step.type === 'Click Mouse' && (
          <select
            className="form-select macro-step-value"
            value={step.value || 'LButton'}
            onChange={e => updateStep({ ...step, value: e.target.value })}
          >
            {MOUSE_CLICK_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
          </select>
        )}
        {step.type === 'Type Text' && (
          <input
            className="form-input macro-step-value"
            placeholder="Text to type..."
            value={step.value || ''}
            onChange={e => updateStep({ ...step, value: e.target.value })}
          />
        )}
        {step.type === 'Wait (ms)' && (
          <input
            className="form-input macro-step-value"
            placeholder="500"
            value={step.value || ''}
            onChange={e => updateStep({ ...step, value: e.target.value })}
          />
        )}
        {step.type === 'Open URL' && (
          <input
            className="form-input macro-step-value"
            placeholder="https://example.com"
            value={step.value || ''}
            onChange={e => updateStep({ ...step, value: e.target.value })}
          />
        )}
        {step.type === 'Wait for Input' && (() => {
          let wfi = { inputType: 'LButton', trigger: 'press', specificKey: '' };
          try { wfi = { ...wfi, ...JSON.parse(step.value || '{}') }; } catch (_) {}
          return (
            <select
              className="form-select macro-step-value"
              value={wfi.inputType}
              onChange={e => {
                const next = { ...wfi, inputType: e.target.value };
                if (e.target.value !== 'SpecificKey') next.specificKey = '';
                updateStep({ ...step, value: JSON.stringify(next) });
              }}
            >
              {WFI_INPUT_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
            </select>
          );
        })()}
        {step.type === 'Click at Position' && (
          <ClickPositionPickBtn step={step} updateStep={updateStep} />
        )}
        {(step.type === 'Press Key' || step.type === 'Click Mouse') && (
          <div className="step-repeat">
            <span className="step-repeat-label">×</span>
            <input
              className="form-input step-repeat-input"
              type="number"
              min="1"
              max="99"
              value={step.repeat ?? 1}
              onChange={e => {
                const raw = e.target.value;
                if (raw === '') { updateStep({ ...step, repeat: '' }); return; }
                const v = Math.min(99, parseInt(raw) || 1);
                updateStep({ ...step, repeat: v });
              }}
              onBlur={() => {
                const v = Math.max(1, Math.min(99, parseInt(step.repeat) || 1));
                updateStep({ ...step, repeat: v });
              }}
            />
          </div>
        )}
        <button className="step-duplicate" onClick={() => duplicateStep(step._id)} type="button" title="Duplicate step" aria-label="Duplicate step">
          <Copy size={13} strokeWidth={1.75} />
        </button>
        <button className="step-remove" onClick={() => removeStep(step._id)} type="button" aria-label="Remove step">✕</button>
      </div>

      {/* Sub-row: Press Key Win-advisory — pops out when user presses Win
          during capture, or when Win is already in the stored value. */}
      {showWinAdvisory && (
        <div className="step-advisory-row">
          <span className="step-advisory-icon" aria-hidden="true">ⓘ</span>
          <span className="step-advisory-text">
            Windows key can't be captured directly (it opens the Start menu). Toggle to add it as a modifier.
          </span>
          <button
            type="button"
            className={`win-toggle-pill${stepHasWin ? ' win-toggle-pill-on' : ''}`}
            onClick={toggleStepWin}
            title={stepHasWin ? 'Remove Windows key' : 'Add Windows key'}
          >
            {stepHasWin ? '✓ Win' : '+ Win'}
          </button>
          {winPrompted && !stepHasWin && (
            <button
              type="button"
              className="step-advisory-dismiss"
              onClick={() => setWinPrompted(false)}
              title="Dismiss"
              aria-label="Dismiss"
            >×</button>
          )}
        </div>
      )}

      {/* Sub-row: Open App — picker + optional args */}
      {step.type === 'Open App' && (
        <MacroOpenAppRow
          appData={appData}
          updateValue={(next) => updateStep({ ...step, value: JSON.stringify(next) })}
          advancedOpen={advancedOpen}
          toggleAdvanced={toggleAdvanced}
        />
      )}

      {/* Sub-row: Open Folder — path + browse */}
      {step.type === 'Open Folder' && (
        <div className="wfi-config-row">
          <div className="file-input-row" style={{ flex: 1 }}>
            <input
              className="form-input"
              style={{ flex: 1 }}
              placeholder="C:\Users\Me\Documents"
              value={step.value || ''}
              readOnly
            />
            <button className="browse-btn" type="button" onClick={async () => {
              const path = await window.electronAPI?.browseForFolder();
              if (path) updateStep({ ...step, value: path });
            }}>Browse</button>
          </div>
        </div>
      )}

      {/* Sub-row: Focus Window — pick process + title */}
      {step.type === 'Focus Window' && (
        <FocusWindowFields
          focusData={focusData}
          onChange={next => updateStep({ ...step, value: JSON.stringify(next) })}
        />
      )}

      {/* Sub-row: Wait for Input — trigger + optional specific key */}
      {step.type === 'Wait for Input' && (() => {
        let wfi = { inputType: 'LButton', trigger: 'press', specificKey: '' };
        try { wfi = { ...wfi, ...JSON.parse(step.value || '{}') }; } catch (_) {}
        const updateWfi = (patch) => {
          const next = { ...wfi, ...patch };
          if (patch.inputType && patch.inputType !== 'SpecificKey') next.specificKey = '';
          updateStep({ ...step, value: JSON.stringify(next) });
        };
        return (
          <div className="wfi-config-row">
            <div className="wfi-field">
              <span className="wfi-label">Trigger:</span>
              <select className="form-select wfi-select" value={wfi.trigger} onChange={e => updateWfi({ trigger: e.target.value })}>
                {WFI_TRIGGER_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
              </select>
            </div>
            {wfi.inputType === 'SpecificKey' && (
              <div className="wfi-field">
                <span className="wfi-label">Key:</span>
                <KeyCaptureInput value={wfi.specificKey || ''} onChange={v => updateWfi({ specificKey: v })} />
              </div>
            )}
          </div>
        );
      })()}
      {step.type === 'Run AHK Script' && (() => {
        let ahk = { script: '', scriptName: '', ahkVersion: 'v1' };
        try { ahk = { ...ahk, ...JSON.parse(step.value || '{}') }; } catch (_) {}
        const isV2 = ahk.ahkVersion === 'v2';
        return (
          <div className="wfi-config-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
            <div className="ahk-version-row">
              <button
                type="button"
                className={`ahk-version-pill ${!isV2 ? 'active' : ''}`}
                onClick={() => updateStep({ ...step, value: JSON.stringify({ ...ahk, ahkVersion: 'v1' }) })}
              >v1</button>
              <button
                type="button"
                className={`ahk-version-pill ${isV2 ? 'active' : ''}`}
                onClick={() => updateStep({ ...step, value: JSON.stringify({ ...ahk, ahkVersion: 'v2' }) })}
              >v2</button>
            </div>
            <textarea
              className="form-textarea"
              placeholder={isV2 ? "; AHK v2 script body..." : "; AHK v1 script body..."}
              value={ahk.script}
              onChange={e => updateStep({ ...step, value: JSON.stringify({ ...ahk, script: e.target.value }) })}
              rows={4}
              onKeyDown={e => e.stopPropagation()}
            />
          </div>
        );
      })()}
      {step.type === 'Click at Position' && (
        <ClickPositionFields step={step} updateStep={updateStep} />
      )}
    </div>
  );
}

export function MacroSequenceForm({ value, onChange, globalInputMethod }) {
  const seqMethod = value.inputMethod || 'global';
  const globalLabel = INPUT_METHOD_OPTS.find(o => o.id === globalInputMethod)?.label || globalInputMethod;
  const [advancedOpen, setAdvancedOpen] = useState({});
  const [activeId, setActiveId] = useState(null);

  // Assign stable runtime IDs to steps — never persisted to config
  const idMapRef = useRef(new Map());
  const stepsWithIds = (value.steps || []).map((step, i) => {
    const cached = idMapRef.current.get(i);
    if (!cached || cached.type !== step.type) {
      idMapRef.current.set(i, { type: step.type, id: 'step-' + (_nextStepId++) });
    }
    return { ...step, _id: idMapRef.current.get(i).id };
  });
  // Rebuild idMap after reorders so indices stay consistent
  useEffect(() => {
    const newMap = new Map();
    stepsWithIds.forEach((s, i) => newMap.set(i, { type: s.type, id: s._id }));
    idMapRef.current = newMap;
  }, [value.steps]);

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }));

  const stripIds = useCallback((steps) => steps.map(({ _id, ...rest }) => rest), []);

  const addStep = () => {
    onChange({ ...value, steps: [...(value.steps || []), { type: 'Type Text', value: '' }] });
  };

  const updateStep = useCallback((updated) => {
    const idx = stepsWithIds.findIndex(s => s._id === updated._id);
    if (idx === -1) return;
    const { _id, ...clean } = updated;
    const newSteps = [...(value.steps || [])];
    newSteps[idx] = clean;
    onChange({ ...value, steps: newSteps });
  }, [stepsWithIds, value, onChange]);

  const removeStep = useCallback((id) => {
    const idx = stepsWithIds.findIndex(s => s._id === id);
    if (idx === -1) return;
    onChange({ ...value, steps: (value.steps || []).filter((_, i) => i !== idx) });
    setAdvancedOpen(prev => { const n = { ...prev }; delete n[id]; return n; });
  }, [stepsWithIds, value, onChange]);

  const duplicateStep = useCallback((id) => {
    const idx = stepsWithIds.findIndex(s => s._id === id);
    if (idx === -1) return;
    const original = value.steps[idx];
    const clone = { ...original };
    const newSteps = [...(value.steps || [])];
    newSteps.splice(idx + 1, 0, clone);
    onChange({ ...value, steps: newSteps });
  }, [stepsWithIds, value, onChange]);

  function handleDragStart(event) {
    setActiveId(event.active.id);
  }

  function handleDragEnd(event) {
    setActiveId(null);
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const oldIndex = stepsWithIds.findIndex(s => s._id === active.id);
    const newIndex = stepsWithIds.findIndex(s => s._id === over.id);
    if (oldIndex === -1 || newIndex === -1) return;
    const reordered = arrayMove(value.steps || [], oldIndex, newIndex);
    onChange({ ...value, steps: reordered });
  }

  const activeStep = activeId ? stepsWithIds.find(s => s._id === activeId) : null;

  return (
    <div className="form-section">
      <div className="seq-method-row">
        <label className="form-label" style={{ marginBottom: 0 }}>Input method</label>
        <select
          className="form-select seq-method-select"
          value={seqMethod}
          onChange={e => onChange({ ...value, inputMethod: e.target.value })}
        >
          {INPUT_METHOD_OPTS.map(o => (
            <option key={o.id} value={o.id}>
              {o.label}{o.id === 'global' ? ` (${globalLabel})` : ''}
            </option>
          ))}
        </select>
      </div>
      <DndContext sensors={sensors} onDragStart={handleDragStart} onDragEnd={handleDragEnd}>
        <SortableContext items={stepsWithIds.map(s => s._id)} strategy={verticalListSortingStrategy}>
          <div className="macro-steps">
            {stepsWithIds.length === 0 && (
              <div className="macro-empty">No steps yet — add your first action below</div>
            )}
            {stepsWithIds.map((step, i) => (
              <SortableMacroStep
                key={step._id}
                step={step}
                index={i}
                updateStep={updateStep}
                removeStep={removeStep}
                duplicateStep={duplicateStep}
                advancedOpen={!!advancedOpen[step._id]}
                toggleAdvanced={() => setAdvancedOpen(prev => ({ ...prev, [step._id]: !prev[step._id] }))}
              />
            ))}
          </div>
        </SortableContext>
        <DragOverlay>
          {activeStep ? (
            <div className="macro-step macro-step-overlay">
              <div className="macro-step-row">
                <div className="step-drag-handle" aria-hidden="true">
                  <GripVertical size={14} strokeWidth={1.75} />
                </div>
                <div className="macro-step-num">{stepsWithIds.findIndex(s => s._id === activeId) + 1}</div>
                <span className="macro-step-type" style={{ fontSize: 11 }}>{activeStep.type}</span>
              </div>
            </div>
          ) : null}
        </DragOverlay>
      </DndContext>
      <button className="add-step-btn" onClick={addStep} type="button">
        + Add Step
      </button>
    </div>
  );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

const MOD_ORDER = ['Ctrl', 'Shift', 'Alt', 'Win'];

function keyIdToLabel(keyId) {
  return friendlyKeyName(keyId);
}

// ── Reassign hotkey overlay ────────────────────────────────────────────────────

function ReassignOverlay({ currentCombo, currentKeyId, assignments, activeProfile, profileLinked, onConfirm, onCancel, title = 'Reassign Hotkey', titleIcon = '⇄', instruction = 'Press new key or combo…', previewVerb = 'Move to' }) {
  const [captured, setCaptured] = useState(null);
  const captureRef = useRef(null);

  useLayoutEffect(() => {
    if (!captured) captureRef.current?.focus();
  }, [captured]);

  function handleKeyDown(e) {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') { onCancel(); return; }
    if (['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) return;

    const mods = [];
    if (e.ctrlKey)  mods.push('Ctrl');
    if (e.shiftKey) mods.push('Shift');
    if (e.altKey)   mods.push('Alt');
    if (e.metaKey)  mods.push('Win');

    // Bare key (no modifiers held) — app-linked: all keys; static: only non-character keys
    if (mods.length === 0 && !profileLinked && !STATIC_BARE_ALLOWED.has(e.code)) return;

    mods.sort((a, b) => MOD_ORDER.indexOf(a) - MOD_ORDER.indexOf(b));
    const newCombo = mods.length === 0 ? 'BARE' : mods.join('+');
    const newKeyId = e.code;

    // Same hotkey — dismiss silently
    if (newCombo === currentCombo && newKeyId === currentKeyId) { onCancel(); return; }

    const keyDisplay = e.key.length === 1 ? e.key.toUpperCase() : (KEY_DISPLAY_MAP[e.key] ?? e.key);
    const label = mods.length === 0 ? keyDisplay : [...mods, keyDisplay].join('+');
    // Check both single and double assignments at the target key for conflicts
    const existingSingle = assignments[`${activeProfile}::${newCombo}::${newKeyId}`] || null;
    const existingDouble = assignments[`${activeProfile}::${newCombo}::${newKeyId}::double`] || null;
    const existing = existingSingle || existingDouble;
    setCaptured({ combo: newCombo, keyId: newKeyId, label, conflict: existing });
  }

  const currentLabel = [
    ...(currentCombo === 'BARE' ? ['Bare'] : currentCombo ? currentCombo.split('+') : []),
    keyIdToLabel(currentKeyId),
  ].filter(Boolean);

  return (
    <div className="reassign-overlay">
      <div className="reassign-panel">
        <div className="reassign-header">
          <span className="reassign-icon">{titleIcon}</span>
          <span className="reassign-title">{title}</span>
        </div>

        <div className="reassign-current">
          Currently:&nbsp;
          {currentLabel.map((k, i) => (
            <React.Fragment key={i}>
              <kbd className="reassign-kbd">{k}</kbd>
              {i < currentLabel.length - 1 && <span className="reassign-plus">+</span>}
            </React.Fragment>
          ))}
        </div>

        {!captured ? (
          <>
            <div className="reassign-instruction">
              {instruction}
              {!profileLinked && (
                <span className="reassign-bare-note">Bare keys in static profiles: F-keys, numpad, and nav keys only</span>
              )}
            </div>
            <div
              ref={captureRef}
              className="reassign-capture-zone"
              tabIndex={0}
              onKeyDown={handleKeyDown}
            >
              <span className="reassign-waiting">Waiting for input…</span>
            </div>
            <div className="reassign-actions">
              <button className="reassign-cancel" onMouseDown={e => { e.preventDefault(); onCancel(); }}>
                Cancel
              </button>
            </div>
          </>
        ) : captured.conflict ? (
          <>
            <div className="reassign-conflict">
              <strong>{captured.label}</strong> is already assigned to
              <span className="reassign-conflict-label"> "{captured.conflict.label}"</span>. Replace it?
            </div>
            <div className="reassign-actions">
              <button className="reassign-cancel" onClick={onCancel}>Cancel</button>
              <button className="reassign-ok reassign-replace" onClick={() => onConfirm(captured.combo, captured.keyId)}>
                Replace
              </button>
            </div>
          </>
        ) : (
          <>
            <div className="reassign-preview">
              {previewVerb}&nbsp;
              {captured.label.split('+').map((k, i, arr) => (
                <React.Fragment key={i}>
                  <kbd className="reassign-kbd">{k}</kbd>
                  {i < arr.length - 1 && <span className="reassign-plus">+</span>}
                </React.Fragment>
              ))}
              ?
            </div>
            <div className="reassign-actions">
              <button className="reassign-cancel" onClick={onCancel}>Cancel</button>
              <button className="reassign-ok" onClick={() => onConfirm(captured.combo, captured.keyId)}>
                Confirm
              </button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}

export default function MacroPanel({
  selectedKey,
  activeModifiers,
  currentCombo,
  assignment,
  doubleAssignment,
  draftAssignment = null,
  draftDoubleAssignment = null,
  assignments,
  activeProfile,
  profiles,
  profileLinked,
  globalInputMethod = 'direct',
  onAssign,
  onClear,
  onAssignDouble,
  onClearDouble,
  onClose,
  onCancelDraft,
  onReassign,
  onDuplicate,
  isPro = false,
  voiceEnabled = false,
  onShowUpgrade,
}) {
  // When the user duplicates an assignment from the sidebar context menu, the
  // cloned action lives in draftAssignment until they pick a destination key.
  // Use it as a fallback so the editor shows the cloned data even before a key
  // is selected, AND once they pick an empty key slot (so the new key inherits
  // the duplicated action).
  const effectiveAssignment = assignment || draftAssignment;
  const effectiveDouble     = doubleAssignment || draftDoubleAssignment;
  const isDraftMode         = !selectedKey && !!draftAssignment;
  const [activeType, setActiveType] = useState('text');
  const [formValue, setFormValue] = useState({});
  const [label, setLabel] = useState('');
  const [voicePhrases, setVoicePhrases] = useState([]);
  const [pressMode, setPressMode] = useState('single'); // 'single' | 'double'
  const [reassigning, setReassigning] = useState(false);
  const [duplicating, setDuplicating] = useState(false);
  const [pendingMouseSave, setPendingMouseSave] = useState(null); // macro pending global-mouse confirmation

  useEffect(() => {
    setReassigning(false);
    setDuplicating(false);
    setPendingMouseSave(null);
    // Auto-switch to double mode when only a double assignment exists
    if (!effectiveAssignment && effectiveDouble) {
      setPressMode('double');
      setActiveType(effectiveDouble.type || 'text');
      setFormValue(effectiveDouble.data || {});
      setLabel(effectiveDouble.label || '');
      setVoicePhrases(readVoicePhrases(effectiveDouble.data));
    } else {
      setPressMode('single');
      if (effectiveAssignment) {
        setActiveType(effectiveAssignment.type || 'text');
        setFormValue(effectiveAssignment.data || {});
        setLabel(effectiveAssignment.label || '');
        setVoicePhrases(readVoicePhrases(effectiveAssignment.data));
      } else {
        setActiveType('text');
        setFormValue({});
        setLabel('');
        setVoicePhrases([]);
      }
    }
  }, [selectedKey, effectiveAssignment, effectiveDouble]);

  // When press mode switches, load the appropriate assignment's form values
  useEffect(() => {
    if (pressMode === 'double') {
      if (effectiveDouble) {
        setActiveType(effectiveDouble.type || 'text');
        setFormValue(effectiveDouble.data || {});
        setLabel(effectiveDouble.label || '');
        setVoicePhrases(readVoicePhrases(effectiveDouble.data));
      } else {
        setActiveType('text');
        setFormValue({});
        setLabel('');
        setVoicePhrases([]);
      }
    } else {
      if (effectiveAssignment) {
        setActiveType(effectiveAssignment.type || 'text');
        setFormValue(effectiveAssignment.data || {});
        setLabel(effectiveAssignment.label || '');
        setVoicePhrases(readVoicePhrases(effectiveAssignment.data));
      } else {
        setActiveType('text');
        setFormValue({});
        setLabel('');
        setVoicePhrases([]);
      }
    }
  // eslint-disable-next-line
  }, [pressMode]);

  const handleSave = () => {
    if (!selectedKey) return;

    const data = { ...formValue };
    // Empty list removes both new and legacy voice phrase fields.
    writeVoicePhrases(data, voicePhrases);

    const macro = {
      type: activeType,
      label: label || getAutoLabel(),
      data,
    };

    if (pressMode === 'double') {
      onAssignDouble?.(selectedKey, macro);
      return;
    }

    // Warn before saving a mouse macro on a global (non-app-linked) profile
    if (selectedKey.startsWith('MOUSE_') && !profileLinked) {
      setPendingMouseSave(macro);
      return;
    }

    onAssign(selectedKey, macro);
  };

  const confirmMouseSave = () => {
    if (!pendingMouseSave || !selectedKey) return;
    onAssign(selectedKey, pendingMouseSave);
    setPendingMouseSave(null);
  };

  const getAutoLabel = () => {
    switch (activeType) {
      case 'text':   return formValue.text?.substring(0, 30) || 'Text snippet';
      case 'hotkey': {
        const mouseOpt = MOUSE_CLICK_OPTIONS.find(o => o.value === formValue.key);
        if (mouseOpt && (!formValue.modifiers || formValue.modifiers.length === 0)) return mouseOpt.label;
        return [...(formValue.modifiers || []), formValue.key].filter(Boolean).join('+') || 'Key combo';
      }
      case 'app':    return formValue.appName || formValue.path?.split('\\').pop() || (formValue.appId ? 'Installed app' : 'Application');
      case 'folder': return formValue.folderName || formValue.path?.split('\\').pop() || 'Folder';
      case 'url':    return formValue.urlName || formValue.url || 'URL';
      case 'macro':  return `Macro (${(formValue.steps || []).length} steps)`;
      case 'ahk':    return 'AHK Script';
      default:       return 'Action';
    }
  };

  const isValid = () => {
    switch (activeType) {
      case 'text':   return !!formValue.text?.trim();
      case 'hotkey': return !!formValue.key;
      case 'app':    return !!(formValue.path?.trim() || formValue.appId?.trim());
      case 'folder': return !!formValue.path?.trim();
      case 'url':    return !!formValue.url?.trim();
      case 'macro':  return (formValue.steps || []).length > 0;
      case 'ahk':    return !!formValue.script?.trim();
      default:       return false;
    }
  };

  if (!selectedKey && !isDraftMode) {
    return (
      <div className="macro-panel macro-panel-empty">
        <div className="macro-panel-empty-content">
          <div className="macro-panel-empty-icon">
            <svg width="40" height="40" viewBox="0 0 40 40" fill="none">
              <rect x="4" y="8" width="12" height="8" rx="3" stroke="currentColor" strokeWidth="1.5" opacity="0.4"/>
              <rect x="18" y="8" width="8" height="8" rx="3" stroke="currentColor" strokeWidth="1.5" opacity="0.3"/>
              <rect x="28" y="8" width="8" height="8" rx="3" stroke="currentColor" strokeWidth="1.5" opacity="0.2"/>
              <rect x="4" y="20" width="8" height="8" rx="3" stroke="currentColor" strokeWidth="1.5" opacity="0.3"/>
              <rect x="14" y="20" width="22" height="8" rx="3" stroke="currentColor" strokeWidth="1.5" opacity="0.4"/>
            </svg>
          </div>
          <h3>Select a Key</h3>
          <p>Choose a modifier layer (Ctrl, Alt, etc.), then click a keyboard key or mouse button to assign a macro to that combination.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="macro-panel">
      {reassigning && (
        <ReassignOverlay
          currentCombo={currentCombo}
          currentKeyId={selectedKey}
          assignments={assignments}
          activeProfile={activeProfile}
          profileLinked={profileLinked}
          title={assignment && doubleAssignment ? 'Reassign Hotkey (single + double press)' : 'Reassign Hotkey'}
          onConfirm={(newCombo, newKeyId) => {
            setReassigning(false);
            onReassign?.(newCombo, newKeyId);
          }}
          onCancel={() => setReassigning(false)}
        />
      )}
      {duplicating && (
        <ReassignOverlay
          currentCombo={currentCombo}
          currentKeyId={selectedKey}
          assignments={assignments}
          activeProfile={activeProfile}
          profileLinked={profileLinked}
          title="Choose Hotkey for Duplicate"
          titleIcon="⊕"
          instruction="Press new key or combo for the duplicate…"
          previewVerb="Duplicate to"
          onConfirm={(newCombo, newKeyId) => {
            setDuplicating(false);
            onDuplicate?.(newCombo, newKeyId);
          }}
          onCancel={() => setDuplicating(false)}
        />
      )}
      <div className="macro-panel-header">
        <div className="macro-panel-title">
          {activeModifiers && activeModifiers.length > 0 && (
            <div className="combo-badge">
              {[...activeModifiers].sort().map((m, i) => (
                <React.Fragment key={m}>
                  <kbd className="selected-key-badge mod-badge">{m}</kbd>
                  {i < activeModifiers.length - 1 && <span className="badge-plus">+</span>}
                </React.Fragment>
              ))}
              <span className="badge-plus">+</span>
            </div>
          )}
          {selectedKey ? (
            <kbd className="selected-key-badge">
              {selectedKey.startsWith('MOUSE_')
                ? ({ MOUSE_LEFT: 'Left Click', MOUSE_RIGHT: 'Right Click', MOUSE_MIDDLE: 'Middle Click',
                     MOUSE_SCROLL_UP: 'Scroll ↑', MOUSE_SCROLL_DOWN: 'Scroll ↓',
                     MOUSE_SIDE1: 'Side 1', MOUSE_SIDE2: 'Side 2' })[selectedKey] ?? selectedKey
                : friendlyKeyName(selectedKey)}
            </kbd>
          ) : (
            <kbd className="selected-key-badge selected-key-badge-draft">Pick a key</kbd>
          )}
        </div>
        <div className="macro-panel-header-actions">
          {assignment && !selectedKey?.startsWith('MOUSE_') && (
            <button
              className="reassign-btn"
              onClick={() => setReassigning(true)}
              title="Move this macro to a different hotkey"
              type="button"
            >
              ⇄ Reassign
            </button>
          )}
          <button className="panel-close" onClick={onClose} title="Deselect key">✕</button>
        </div>
      </div>

      {/* Press mode toggle — keyboard keys and mouse buttons */}
      {selectedKey && (
        <div className="press-mode-bar">
          <button
            className={`press-mode-btn${pressMode === 'single' ? ' active' : ''}`}
            onClick={() => setPressMode('single')}
            type="button"
          >
            ×1 Single Press
          </button>
          <button
            className={`press-mode-btn${pressMode === 'double' ? ' active' : ''}`}
            onClick={() => {
              if (!isPro) { onShowUpgrade?.('Double press hotkeys'); return; }
              setPressMode('double');
            }}
            type="button"
          >
            ×2 Double Press <span className="pro-badge">PRO</span>
            {doubleAssignment && <span className="press-mode-dot" />}
          </button>
        </div>
      )}

      <div className="macro-panel-body">
        {isDraftMode && (
          <div className="draft-banner" role="note">
            <span className="draft-banner-icon">⊕</span>
            <span className="draft-banner-text">
              Editing a duplicate. Pick a key on the keyboard or record a new combination to save it.
            </span>
            <button
              className="draft-banner-cancel"
              type="button"
              onClick={onCancelDraft}
              title="Discard the duplicate"
            >Cancel</button>
          </div>
        )}
        {/* Action type selector */}
        <div className="type-selector">
          {ACTION_TYPES.map(type => {
            const TypeIcon = type.Icon;
            return (
              <button
                key={type.id}
                className={`type-btn ${activeType === type.id ? 'active' : ''}${type.id === 'ahk' ? ' type-btn-wide' : ''}`}
                style={{ '--type-color': type.color }}
                onClick={() => { setActiveType(type.id); setFormValue({}); }}
                type="button"
              >
                <span className="type-btn-icon"><TypeIcon size={18} strokeWidth={1.75} /></span>
                <span className="type-btn-label">{type.label}</span>
              </button>
            );
          })}
        </div>

        {/* Type description */}
        <div className="type-desc">
          {ACTION_TYPES.find(t => t.id === activeType)?.desc}
        </div>

        {/* Dynamic form */}
        <div className="form-body">
          {activeType === 'text'   && <TextForm value={formValue} onChange={setFormValue} globalInputMethod={globalInputMethod} />}
          {activeType === 'hotkey' && (
            <>
              <HotkeyCaptureInput value={formValue} onChange={setFormValue} />
              <div className="hold-mode-row">
                <span className="hold-mode-label">Hold mode</span>
                <button
                  type="button"
                  className={`hold-mode-toggle${formValue.holdMode ? ' on' : ''}`}
                  onClick={() => setFormValue(prev => ({ ...prev, holdMode: !prev.holdMode, repeatMode: false }))}
                  role="switch"
                  aria-checked={!!formValue.holdMode}
                />
              </div>
              {formValue.holdMode && (
                <p className="hold-mode-hint">Key stays held until hotkey is pressed again</p>
              )}
              <div className="hold-mode-row">
                <span className="hold-mode-label">Repeat mode</span>
                <button
                  type="button"
                  className={`hold-mode-toggle${formValue.repeatMode ? ' on' : ''}`}
                  onClick={() => setFormValue(prev => ({ ...prev, repeatMode: !prev.repeatMode, holdMode: false }))}
                  role="switch"
                  aria-checked={!!formValue.repeatMode}
                />
              </div>
              {formValue.repeatMode && (
                <>
                  <p className="hold-mode-hint">Fires repeatedly until hotkey is pressed again</p>
                  <div className="repeat-interval-row">
                    <label className="repeat-interval-label">Interval</label>
                    <input
                      className="form-input repeat-interval-input"
                      type="number"
                      min={50}
                      value={formValue.repeatInterval ?? 100}
                      onChange={e => {
                        const raw = e.target.value;
                        setFormValue(prev => ({ ...prev, repeatInterval: raw === '' ? '' : (parseInt(raw) || '') }));
                      }}
                      onBlur={() => {
                        setFormValue(prev => {
                          const val = parseInt(prev.repeatInterval);
                          return { ...prev, repeatInterval: (!val || val < 50) ? 50 : val };
                        });
                      }}
                      onKeyDown={e => e.stopPropagation()}
                    />
                    <span className="repeat-interval-suffix">ms</span>
                  </div>
                </>
              )}
            </>
          )}
          {activeType === 'app'    && <AppForm value={formValue} onChange={setFormValue} />}
          {activeType === 'folder' && <FolderForm value={formValue} onChange={setFormValue} />}
          {activeType === 'url'    && <UrlForm value={formValue} onChange={setFormValue} />}
          {activeType === 'macro'  && <MacroSequenceForm value={formValue} onChange={setFormValue} globalInputMethod={globalInputMethod} />}
          {activeType === 'ahk'   && <AhkForm value={formValue} onChange={setFormValue} />}
        </div>

        {/* Display label */}
        <div className="form-section" style={{ marginTop: 4 }}>
          <label className="form-label">Display label (optional)</label>
          <input
            className="form-input"
            placeholder={getAutoLabel() || 'Short label for this key...'}
            value={label}
            onChange={e => setLabel(e.target.value)}
            onKeyDown={e => e.stopPropagation()}
          />
        </div>

        {/* Voice command — only visible when voice activation is enabled in Settings */}
        {voiceEnabled && (
        <div className="form-section" style={{ marginTop: 4 }}>
          <label className="form-label">Voice commands <span className="experimental-badge">EXPERIMENTAL</span></label>
          <div className="voice-phrase-list">
            {voicePhrases.map((p, i) => (
              <div className="voice-phrase-row" key={i}>
                <input
                  className="form-input voice-phrase-input"
                  placeholder="e.g. open Revit"
                  value={p}
                  onChange={e => {
                    const next = [...voicePhrases];
                    next[i] = e.target.value;
                    setVoicePhrases(next);
                  }}
                  onKeyDown={e => e.stopPropagation()}
                />
                <button
                  type="button"
                  className="voice-phrase-remove"
                  title="Remove phrase"
                  onClick={() => setVoicePhrases(voicePhrases.filter((_, idx) => idx !== i))}
                >×</button>
              </div>
            ))}
            <button
              type="button"
              className="voice-phrase-add"
              onClick={() => setVoicePhrases([...voicePhrases, ''])}
            >+ Add voice phrase</button>
          </div>
          <span className="form-hint">Trigger any of these phrases by voice (all aliases fire the same action)</span>
        </div>
        )}
      </div>

      {/* Actions */}
      <div className="macro-panel-footer">
        {pressMode === 'double' ? (
          doubleAssignment && (
            <div className="footer-assignment-actions">
              <button className="btn-clear" onClick={() => onClearDouble?.(selectedKey)} type="button">
                Clear Double
              </button>
            </div>
          )
        ) : (
          assignment && (
            <div className="footer-assignment-actions">
              <button className="btn-clear" onClick={() => onClear(selectedKey)} type="button">
                Clear Key
              </button>
              <button className="btn-duplicate" onClick={() => setDuplicating(true)} type="button" title="Duplicate this macro to a different hotkey">
                Duplicate
              </button>
            </div>
          )
        )}
        {pendingMouseSave ? (
          <div className="mouse-save-confirm">
            <div className="mouse-save-confirm-text">
              <span className="mouse-save-confirm-icon">⚠</span>
              This combo is assigned globally and may conflict with browser or system shortcuts (e.g. Ctrl+Click opens new tab).
              Use an app-specific profile for safer assignments.
            </div>
            <div className="mouse-save-confirm-actions">
              <button
                className="btn-save-cancel"
                onClick={() => setPendingMouseSave(null)}
                type="button"
              >
                Cancel
              </button>
              <button
                className="btn-save-anyway"
                onClick={confirmMouseSave}
                type="button"
              >
                Assign Anyway
              </button>
            </div>
          </div>
        ) : (
          <button
            className="btn-save"
            onClick={handleSave}
            disabled={!isValid() || !selectedKey}
            type="button"
            title={!selectedKey ? 'Pick a key first — click Record or any keyboard key' : undefined}
          >
            {!selectedKey
              ? 'Pick a key to save'
              : pressMode === 'double'
                ? (doubleAssignment ? 'Update Double-Tap' : 'Assign Double-Tap')
                : (assignment ? 'Update' : 'Assign to Key')
            }
          </button>
        )}
      </div>
    </div>
  );
}
