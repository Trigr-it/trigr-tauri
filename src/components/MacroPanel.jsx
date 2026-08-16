import React, { useState, useEffect, useLayoutEffect, useRef, useMemo, Fragment, useCallback } from 'react';
import { createPortal } from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import {
  Type, Keyboard, AppWindow, Globe, FolderOpen, Layers, FileCode,
  GripVertical, Copy, Sparkles, Circle, CircleDot, Square, Trash2,
} from 'lucide-react';
import './MacroPanel.css';
import MonitorPicker from './MonitorPicker';
import NumberField from './NumberField';
import { SearchBar } from './SearchBar';
import { friendlyKeyName, STATIC_BARE_ALLOWED } from './keyboardLayout';
import { readVoicePhrases, writeVoicePhrases } from '../voicePhrases';

// `desc` is the copy shown in the dismissible TIP box under the selector —
// a sentence or two of real explanation, not just a label restatement.
const ACTION_TYPES = [
  {
    id: 'macro',
    Icon: Layers,
    label: 'Macro',
    desc: 'Build a sequence of steps that run in order: type text, press keys, open apps, control windows, manage files and more. Drag steps to reorder them.',
    color: '#ff783c',
  },
  {
    id: 'recordmacro',
    Icon: CircleDot,
    label: 'Record Macro',
    desc: 'Record your actual keyboard and mouse actions once, then replay them exactly with a single trigger. Press Ctrl+Shift+R to stop recording.',
    color: '#ff5c5c',
  },
  {
    id: 'text',
    Icon: Type,
    label: 'Text',
    desc: 'Types a text snippet wherever your cursor is, like a signature, address or greeting. Supports multiple lines.',
    color: '#64b4ff',
  },
  {
    id: 'expansion',
    Icon: Sparkles,
    label: 'Expansion',
    desc: 'Fires one of your saved text expansions, so a snippet with fill-ins and formatting is one key press away.',
    color: '#a070ff',
  },
  {
    id: 'hotkey',
    Icon: Keyboard,
    label: 'Hotkey',
    desc: 'Sends a key combination like Ctrl+C to the app you are using. Handy for remapping awkward shortcuts onto comfortable keys.',
    color: '#c864ff',
  },
  {
    id: 'app',
    Icon: AppWindow,
    label: 'App',
    desc: 'Launches an application or file. If the app is already running, its window is brought to the front instead.',
    color: '#50c878',
  },
  {
    id: 'url',
    Icon: Globe,
    label: 'URL',
    desc: 'Opens a website in your default browser. Any address works, including deep links into web apps.',
    color: '#ffc832',
  },
  {
    id: 'folder',
    Icon: FolderOpen,
    label: 'Folder',
    desc: 'Opens a folder in File Explorer. Point it at the folders you dig through every day.',
    color: '#40c8a0',
  },
  {
    id: 'ahk',
    Icon: FileCode,
    label: 'AHK Script',
    desc: 'Runs an AutoHotkey v1 or v2 script, so existing AHK scripts can move across without being rebuilt as macros.',
    color: '#4ecdc4',
  },
];

// The three Open types (app/url/folder) collapse into a single "Open" button
// in the type selector; the active sub-type is picked via the segmented bar
// rendered below it. Underlying type ids are unchanged — saved assignments,
// drafts and the Rust side are unaffected.
const OPEN_TYPE_IDS = ['app', 'url', 'folder'];

// Text + Expansion collapse into a single "Text" button under the same
// sub-pill pattern. Type ids stay distinct; saved assignments unchanged.
const TEXT_TYPE_IDS = ['text', 'expansion'];

// "Record Macro" is a UI-only pseudo-type: a dedicated recorder view whose
// saved form is a normal macro assignment with a single "Record Macro" step.
// The stored type is ALWAYS 'macro' (config schema + Rust untouched) —
// 'recordmacro' must never be written to an assignment's `type`. A saved
// macro whose steps are exactly one recording lights this button on load;
// a recording mixed with other steps still displays as Macro.
const RECORD_TYPE_ID = 'recordmacro';
const isPureRecording = (data) => {
  const steps = data?.steps || [];
  return steps.length === 1 && steps[0]?.type === 'Record Macro';
};
const displayTypeOf = (entry) => {
  const t = entry?.type || 'text';
  if (t === 'macro' && isPureRecording(entry.data)) return RECORD_TYPE_ID;
  return t;
};

const MODIFIER_KEYS = ['Ctrl', 'Alt', 'Shift', 'Win'];
const TRIGGER_KEYS = [
  'A','B','C','D','E','F','G','H','I','J','K','L','M',
  'N','O','P','Q','R','S','T','U','V','W','X','Y','Z',
  'F1','F2','F3','F4','F5','F6','F7','F8','F9','F10','F11','F12',
  '0','1','2','3','4','5','6','7','8','9',
  'Space','Tab','Enter','Escape','Delete','Home','End','PageUp','PageDown',
  'Up','Down','Left','Right',
];

// Macro step types grouped for the flyout menu. Record Macro leads with a
// divider so it reads as its own affordance, seven categories in the middle,
// then Run AHK Script under a trailing divider. Add new step types into the
// matching group; the dropdown reads from this structure directly.
const MACRO_STEP_CATEGORIES = [
  // "Record Macro" is the literal-replay recorder (Phase 1). Pinned at the
  // top with a divider below. Backend step.type is "Record Macro" —
  // actions.rs match arm + execute_macro_step focus-recapture list both use
  // that string.
  { kind: 'leaf',  label: 'Record Macro' },
  { kind: 'divider' },
  // "Existing Action" — fires an existing assignment or text expansion from
  // inside a macro. Sub-items display as "Trigger" / "Text Expansion" via the
  // displayLabel map below, but their step.type values are explicit "Fire …"
  // strings to avoid collisions with how "trigger" and "text expansion" are
  // used elsewhere in the codebase. See actions.rs execute_macro_step arms.
  { kind: 'group', label: 'Existing Action',  items: ['Fire Trigger', 'Fire Text Expansion'] },
  { kind: 'group', label: 'Type & Keys',      items: ['Type Text', 'Dynamic Text', 'Press Key', 'Copy to Clipboard', 'Paste Clipboard', 'Select All'] },
  { kind: 'group', label: 'Mouse',            items: ['Click Mouse', 'Click at Position', 'Mouse Scroll'] },
  { kind: 'group', label: 'Open & Play',      items: ['Open App', 'Open Folder', 'Open URL', 'Play Audio File', 'Play Video File'] },
  { kind: 'group', label: 'Files',            items: ['Create Folder', 'Copy Files', 'Move Files', 'Sort Files'] },
  { kind: 'group', label: 'Timing',           items: ['Wait (ms)', 'Wait for Input', 'Wait for Window'] },
  { kind: 'group', label: 'Window',           items: ['Focus Window', 'Minimise Window', 'Maximise Window', 'Resize Window', 'Minimise All', 'Restore All'] },
  { kind: 'group', label: 'System',           items: ['Change Volume', 'Change Audio Output', 'Control Panel', 'Sleep Computer', 'Lock Computer', 'Log Off', 'Shut Down Computer'] },
  { kind: 'divider' },
  { kind: 'leaf',  label: 'Run AHK Script' },
];

// Override for menu/submenu display. Step.type stays canonical ("Fire Trigger")
// but the dropdown shows the friendlier label the user picked in the design.
const MACRO_STEP_DISPLAY_LABEL = {
  'Fire Trigger': 'Trigger',
  'Fire Text Expansion': 'Text Expansion',
};
function macroStepLabel(stepType) {
  return MACRO_STEP_DISPLAY_LABEL[stepType] || stepType;
}

// Steps that are Pro-gated in the backend (licence::is_pro() in their
// actions.rs arm). Shown with a PRO badge in the step menu; selecting one
// without a licence opens the upgrade prompt (SortableMacroStep type-change
// handler). Keep in sync with the backend gates.
const PRO_MACRO_STEPS = new Set(['Sort Files']);

// Sort Files (Pro) — full default config, shared by the type-change seed and
// the parse block so a fresh step and a legacy/partial value agree. Shape
// documented in actions.rs "Sort Files" arm.
const SORT_FILES_DEFAULTS = {
  sourceMode: 'selected', sourcePath: '', pattern: '*',
  rootPath: '', searchDepth: 3,
  keyMode: 'prefix', keyLength: 6, keySegment: 1, keySeparator: '-',
  routeEnabled: false, codeSegment: 3, codeSeparator: '-', mappings: [],
  confirm: true, collision: 'prompt',
};

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

// MOUSE_CLICK_OPTIONS drives the hotkey-capture picker (binds a mouse button
// as the trigger for an assignment). Full clicks only — users can't hold
// "LButton Down" as a distinct trigger key.
const MOUSE_CLICK_OPTIONS = [
  { value: 'LButton', label: 'Left Click' },
  { value: 'RButton', label: 'Right Click' },
  { value: 'MButton', label: 'Middle Click' },
];

// PRESS_KEY_PHASE_OPTIONS drives the Press Key step's phase sub-row.
// Down = hold the key (or chord) — no release, a later Release step is
// expected to close it. Up = release only. "full" (default) is the classic
// down+up pulse.
const PRESS_KEY_PHASE_OPTIONS = [
  { value: 'full', label: 'Press'   },
  { value: 'down', label: 'Hold'    },
  { value: 'up',   label: 'Release' },
];

// MOUSE_CLICK_PHASE_OPTIONS drives the Click Mouse step's phase sub-row.
// Same shape as Press Key phases but the default is labelled "Click" instead
// of "Press" because mouse buttons click.
const MOUSE_CLICK_PHASE_OPTIONS = [
  { value: 'full', label: 'Click'   },
  { value: 'down', label: 'Hold'    },
  { value: 'up',   label: 'Release' },
];

// Legacy Click Mouse values used to bake the phase into the value string
// ("LButtonDown"/"LButtonUp"). Kept purely as a decode table so existing
// saved macros still open correctly; new saves write the clean split shape
// (value = "LButton", phase = "down") via migrateClickMouseValue.
function migrateClickMouseValue(rawValue, rawPhase) {
  const v = rawValue || 'LButton';
  if (v.endsWith('Down')) return { button: v.slice(0, -4), phase: 'down' };
  if (v.endsWith('Up'))   return { button: v.slice(0, -2), phase: 'up'   };
  return { button: v, phase: rawPhase || 'full' };
}

const VOLUME_MODE_OPTIONS = [
  { value: 'set',      label: 'Set'      },
  { value: 'increase', label: 'Increase' },
  { value: 'decrease', label: 'Decrease' },
  { value: 'mute',     label: 'Mute'     },
];

const SCROLL_DIRECTION_OPTIONS = [
  { value: 'down',  label: 'Down'  },
  { value: 'up',    label: 'Up'    },
  { value: 'right', label: 'Right' },
  { value: 'left',  label: 'Left'  },
];

const INPUT_METHOD_OPTS = [
  { id: 'global',       label: 'Global default',  hint: 'Use the method set in Settings → Compatibility' },
  { id: 'direct',       label: 'Direct',           hint: 'Simulates real keypresses — works in CAD, games, any app' },
  { id: 'shift-insert', label: 'Clipboard',        hint: 'Fast for long text — pastes via clipboard' },
];

// Dynamic tokens offered by the Dynamic Text macro step. Mirrors the Insert
// menu in TextExpansions.jsx (kept in sync by convention — these are the
// runtime tokens resolve_tokens() in expansions.rs can substitute). {cursor}
// and {{var}} are omitted: cursor positioning isn't honoured by output_text
// in macro context, and global variables live in their own Pro-gated picker.
const DYNAMIC_TEXT_TOKEN_GROUPS = [
  { label: 'Date', items: [
    { token: '{date}',             label: 'Date (your default)' },
    { token: '{date:DD/MM/YYYY}',  label: 'DD/MM/YYYY'          },
    { token: '{date:DD/MM/YY}',    label: 'DD/MM/YY'            },
    { token: '{date:MM/DD/YYYY}',  label: 'MM/DD/YYYY'          },
    { token: '{date:YYYY-MM-DD}',  label: 'YYYY-MM-DD'          },
    { token: '{date:D MMMM YYYY}', label: 'D MMMM YYYY (1 May 2026)' },
    { token: '{dayofweek}',        label: 'Day of Week'         },
    { token: '{month}',            label: 'Month Name'          },
    { token: '{year}',             label: 'Year (YYYY)'         },
    { token: '{day}',              label: 'Day of Month'        },
  ]},
  { label: 'Time', items: [
    { token: '{time:HH:MM}',    label: 'HH:MM'             },
    { token: '{time:HH:MM:SS}', label: 'HH:MM:SS'          },
    { token: '{isodate}',       label: 'ISO 8601 Date+Time' },
  ]},
  { label: 'Date Math', items: [
    { token: '{date:+1d}', label: 'Tomorrow (+1 day)' },
    { token: '{date:-1d}', label: 'Yesterday (-1 day)' },
    { token: '{date:-1b}', label: 'Last Business Day (skips weekend)' },
    { token: '{date:+1b}', label: 'Next Business Day (skips weekend)' },
    { token: '{date:+7d}', label: 'Next Week (+7 days)' },
    { token: '{date:+1m}', label: 'Next Month (+1 month)' },
  ]},
  { label: 'Clipboard', items: [
    { token: '{clipboard}',           label: 'Clipboard Contents'    },
    { token: '{clipboard:uppercase}', label: 'Clipboard (UPPERCASE)' },
    { token: '{clipboard:lowercase}', label: 'Clipboard (lowercase)' },
    { token: '{clipboard:trim}',      label: 'Clipboard (trimmed)'   },
    { token: '{clipboard:urlencode}', label: 'Clipboard (URL encode)' },
  ]},
];

function TextForm({ value, onChange, globalInputMethod }) {
  // Read inputMethod; fall back to legacy pasteMethod for backward compat
  const inputMethod = value.inputMethod ||
    (value.pasteMethod && value.pasteMethod !== 'shift-insert' ? value.pasteMethod : 'global');
  const globalLabel = INPUT_METHOD_OPTS.find(o => o.id === globalInputMethod)?.label || globalInputMethod;
  const selectedHint = INPUT_METHOD_OPTS.find(o => o.id === inputMethod)?.hint || '';
  return (
    <div className="form-section">
      <label className="form-label">Input method</label>
      <select
        className="form-select"
        value={inputMethod}
        onChange={e => onChange({ ...value, inputMethod: e.target.value, pasteMethod: undefined })}
      >
        {INPUT_METHOD_OPTS.map(opt => (
          <option key={opt.id} value={opt.id}>
            {opt.label}{opt.id === 'global' ? ` (${globalLabel})` : ''}
          </option>
        ))}
      </select>
      {selectedHint && <div className="form-hint">{selectedHint}</div>}
      <label className="form-label" style={{ marginTop: 12 }}>Text to type</label>
      <textarea
        className="form-textarea"
        placeholder="Enter the text that will be typed when this key is pressed..."
        value={value.text || ''}
        onChange={e => onChange({ ...value, text: e.target.value })}
        rows={4}
      />
    </div>
  );
}

// Single-action "fire an existing text expansion" — reuses FireTargetPicker
// (mode='expansion') and stores the chosen trigger word in `value.trigger`.
// The Rust side dispatches via `fire_expansion_by_trigger`, same path as the
// "Fire Text Expansion" macro step.
function ExpansionForm({ value, onChange, assignments }) {
  const [pickerOpen, setPickerOpen] = useState(false);
  const trigger = value.trigger || '';
  const entry = trigger ? assignments?.[`GLOBAL::EXPANSION::${trigger}`] : null;
  const isMissing = !!trigger && !entry;
  const friendly = entry
    ? (entry.data?.displayName ? `${entry.data.displayName} (:${trigger})` : `:${trigger}`)
    : null;
  const placeholder = 'Choose a text expansion…';
  return (
    <div className="form-section">
      <label className="form-label">Text expansion</label>
      <button
        type="button"
        className={`fire-target-chip fire-target-chip-block${trigger ? ' fire-target-chip-set' : ''}${isMissing ? ' fire-target-chip-missing' : ''}`}
        onClick={() => setPickerOpen(true)}
        title={isMissing ? `Missing: :${trigger}` : (friendly || placeholder)}
      >
        <span className="fire-target-chip-label">
          {isMissing ? `Missing: :${trigger}` : (friendly || placeholder)}
        </span>
        <span className="fire-target-chip-caret" aria-hidden="true">▾</span>
      </button>
      <div className="form-hint">
        Pressing this key fires the expansion as if you typed its trigger.
      </div>
      {pickerOpen && (
        <FireTargetPicker
          mode="expansion"
          assignments={assignments || {}}
          currentValue={trigger}
          onSelect={(t) => { onChange({ ...value, trigger: t }); setPickerOpen(false); }}
          onClose={() => setPickerOpen(false)}
        />
      )}
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
    // the LL hook once Keyfire loses focus. The advisory below explains the
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
                onClick={() => onSelect({ kind: 'aumid', path: '', appId: app.appId, name: app.name, iconSource: app.iconSource || '' })}
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
        iconSource: picked.iconSource || '',
      });
    } else {
      onChange({
        ...value,
        kind: 'path',
        appId: '',
        path: picked.path,
        appName: value.appName || picked.name || '',
        iconSource: '',
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
      <div className="form-row-monitor">
        <label className="form-sublabel">Open on monitor</label>
        <MonitorPicker
          value={value.monitor || 'default'}
          onChange={(m) => onChange({ ...value, monitor: m })}
        />
      </div>
      {/* Display name is set via the single top-level "Display label" field
          below this sub-form. Auto-populated `appName` (from the picker) still
          feeds the Display label placeholder and the Sidebar/SearchOverlay
          fallback display, so existing assignments are unaffected. */}
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
      <div className="form-hint">Opens the folder in File Explorer when the key is pressed.</div>
      <div className="form-row-monitor">
        <label className="form-sublabel">Open on monitor</label>
        <MonitorPicker
          value={value.monitor || 'default'}
          onChange={(m) => onChange({ ...value, monitor: m })}
        />
      </div>
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
    </div>
  );
}

function AhkForm({ value, onChange }) {
  const version = value.ahkVersion || 'v1';
  const isV2 = version === 'v2';
  return (
    <div className="form-section">
      <label className="form-label">AHK {version} Script</label>
      <textarea
        className="form-textarea"
        placeholder={isV2
          ? "; Write your AutoHotkey v2 script here\nMsgBox \"Hello from Keyfire!\"\nSend \"{Enter}\""
          : "; Write your AutoHotkey v1 script here\nMsgBox, Hello from Keyfire!\nSend, {Enter}"}
        value={value.script || ''}
        onChange={e => onChange({ ...value, script: e.target.value })}
        rows={8}
        onKeyDown={e => e.stopPropagation()}
      />
      <div className="form-hint">
        Paste your script as-is. Keyfire is the trigger, so hotkey labels like <code>^!j::</code> are stripped automatically.
      </div>
    </div>
  );
}

// Dynamic Text macro-step value — single-select of all available dynamic
// tokens (date, time, clipboard, ...). The stored value is the token string
// itself (e.g. "{date}"), resolved at runtime by resolve_type_text_tokens
// in actions.rs — same path Type Text takes for literal text.
function MacroDynamicTextValue({ value, onChange }) {
  return (
    <select
      className="form-select macro-step-value"
      value={value}
      onChange={e => onChange(e.target.value)}
      aria-label="Dynamic value"
    >
      <option value="">Pick a dynamic value…</option>
      {DYNAMIC_TEXT_TOKEN_GROUPS.map(group => (
        <optgroup key={group.label} label={group.label}>
          {group.items.map(item => (
            <option key={item.token} value={item.token}>{item.label}</option>
          ))}
        </optgroup>
      ))}
    </select>
  );
}

// Inline value for the "Record Macro" macro step. Hosts the Record / Stop
// / Re-record / Discard buttons + a live duration+event-count readout.
// While recording the LL hooks observe input and push to recorder.rs; pressing
// Ctrl+Shift+R from anywhere on the system stops the recording (the hook
// detects it and emits a Tauri event we listen for here).
function formatRecordingDuration(ms) {
  if (!ms || ms < 0) return '0:00';
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${m}:${String(s).padStart(2, '0')}`;
}

// Phase 2 Record Macro step value can be either:
//   (a) legacy raw events array: [ {kind:'keydown',...}, ... ]
//   (b) wrapper object: { events, distilled, playbackMode, targetApp }
// parseRecordingValue normalises to (b). Save always writes (b).
function parseRecordingValue(rawValue) {
  if (!rawValue) return null;
  try {
    const parsed = JSON.parse(rawValue);
    if (Array.isArray(parsed)) {
      if (parsed.length === 0) return null;
      return { events: parsed, distilled: null, playbackMode: 'raw', targetApp: null };
    }
    if (parsed && typeof parsed === 'object' && Array.isArray(parsed.events)) {
      if (parsed.events.length === 0) return null;
      return {
        events: parsed.events,
        distilled: Array.isArray(parsed.distilled) ? parsed.distilled : null,
        playbackMode: parsed.playbackMode || 'raw',
        targetApp: parsed.targetApp || null,
      };
    }
    return null;
  } catch (_) {
    return null;
  }
}

function summariseRecording(parsed) {
  if (!parsed || !parsed.events?.length) return null;
  const lastT = parsed.events[parsed.events.length - 1]?.t ?? 0;
  return { duration: formatRecordingDuration(lastT), count: parsed.events.length };
}


function ReplayRecordingValue({ value, onChange, isPro = false, onShowUpgrade, assignments = {}, profiles = [], globalInputMethod = 'global' }) {
  const [isRecording, setIsRecording] = useState(false);
  const [liveStatus, setLiveStatus] = useState({ count: 0, durationMs: 0 });
  const [isDistilling, setIsDistilling] = useState(false);

  const parsed = useMemo(() => parseRecordingValue(value), [value]);
  const summary = useMemo(() => summariseRecording(parsed), [parsed]);
  const hasDistilled = !!(parsed?.distilled && parsed.distilled.length > 0);
  const mode = parsed?.playbackMode || 'raw';

  const finishRecording = useCallback(async () => {
    try {
      const events = await window.electronAPI.stopMacroRecording();
      // Rust returns the raw event array. Wrap in the Phase 2 shape at save
      // time so downstream code always sees the object form.
      if (Array.isArray(events) && events.length > 0) {
        onChange(JSON.stringify({
          events,
          distilled: null,
          playbackMode: 'raw',
          targetApp: null,
        }));
      }
    } catch (e) {
      console.error('[recorder] stop failed', e);
    } finally {
      setIsRecording(false);
      await window.electronAPI.removeAllListeners('recorder-stop-requested');
      await window.electronAPI.removeAllListeners('recorder-countdown-cancelled');
      // Tidy up the countdown overlay and restore the main window.
      try { await window.electronAPI.hideRecorderCountdown(); } catch (_) {}
      try { await window.electronAPI.recorderRestoreMain(); } catch (_) {}
    }
  }, [onChange]);

  const cancelRecording = useCallback(async () => {
    // Esc / Cancel hit during the 3-2-1 — countdown was emitting cancelled,
    // recorder::start() was never called by Rust, so just unwind UI state.
    setIsRecording(false);
    await window.electronAPI.removeAllListeners('recorder-stop-requested');
    await window.electronAPI.removeAllListeners('recorder-countdown-cancelled');
    try { await window.electronAPI.hideRecorderCountdown(); } catch (_) {}
    try { await window.electronAPI.restoreMainWindow(); } catch (_) {}
  }, []);

  // Safety cleanup: if the component unmounts mid-recording (user navigated
  // away, changed step type), discard the in-flight recording so the LL
  // hooks stop pushing into a buffer no one will retrieve. Discarding only
  // touches Rust state when IS_RECORDING_MACRO is still true.
  const isRecordingRef = useRef(false);
  useEffect(() => { isRecordingRef.current = isRecording; }, [isRecording]);
  useEffect(() => {
    return () => {
      if (isRecordingRef.current) {
        window.electronAPI.removeAllListeners('recorder-stop-requested');
        window.electronAPI.removeAllListeners('recorder-countdown-cancelled');
        window.electronAPI.discardMacroRecording().catch(() => {});
        window.electronAPI.hideRecorderCountdown().catch(() => {});
        window.electronAPI.recorderRestoreMain().catch(() => {});
      }
    };
  }, []);

  // Poll status while recording — counter-update ONLY. Stop detection comes
  // exclusively from the `recorder-stop-requested` event, never from
  // `!status.recording` here. During the 3-second countdown isRecording is
  // true but recorder::start() hasn't run yet, so the backend correctly
  // reports recording=false — treating that as a stop signal caused the
  // recorder to "finish" in the same millisecond it was requested.
  useEffect(() => {
    if (!isRecording) return undefined;
    let cancelled = false;
    const tick = async () => {
      if (cancelled) return;
      try {
        const status = await window.electronAPI.getRecordingStatus();
        if (cancelled || !status?.recording) return;
        setLiveStatus({ count: status.count || 0, durationMs: status.durationMs || 0 });
      } catch (_) { /* ignore transient errors */ }
    };
    const interval = setInterval(tick, 250);
    tick();
    return () => { cancelled = true; clearInterval(interval); };
  }, [isRecording]);

  async function startRecording() {
    setLiveStatus({ count: 0, durationMs: 0 });
    // Defensive clean-slate: discard any leftover recorder state from a
    // previous attempt (in dev, HMR reloads can desync React/Rust; in prod,
    // a cleanup path could have silently failed).
    try { await window.electronAPI.discardMacroRecording(); } catch (_) {}
    window.electronAPI.onRecorderStopRequested(() => { finishRecording(); });
    window.electronAPI.onRecorderCountdownCancelled(() => { cancelRecording(); });
    setIsRecording(true);
    // CRITICAL ORDER: show the countdown overlay BEFORE hiding main.
    // Chromium throttles JS in hidden webviews, which left the next invoke
    // (`showRecorderCountdown`) hanging forever — the main window's JS was
    // paused awaiting an IPC response that couldn't be delivered. Showing
    // the overlay first keeps main alive long enough to issue the hide call,
    // and the overlay is always-on-top so the user sees the countdown card
    // immediately rather than the brief flash of main.
    await window.electronAPI.showRecorderCountdown();
    try { await window.electronAPI.recorderHideMain(); } catch (_) {}
  }

  async function stopRecording() {
    await finishRecording();
  }

  async function discardRecording() {
    onChange('');
  }

  if (isRecording) {
    return (
      <div className="macro-step-value replay-rec replay-rec--recording" role="status">
        <span className="replay-rec-dot" aria-hidden="true" />
        <span className="replay-rec-label">
          Recording {formatRecordingDuration(liveStatus.durationMs)} · {liveStatus.count} events
          <span className="replay-rec-hint"> · Ctrl+Shift+R to stop</span>
        </span>
        <button type="button" className="replay-rec-btn replay-rec-btn--stop" onClick={stopRecording}>
          <Square size={11} fill="currentColor" strokeWidth={0} /> Stop
        </button>
      </div>
    );
  }

  async function handleDistil() {
    if (!parsed?.events?.length) return;
    if (!isPro) {
      onShowUpgrade?.('Macro distillation — turns raw recordings into editable steps and makes clicks window-aware so the macro survives moves and resizes.');
      return;
    }
    setIsDistilling(true);
    try {
      // Backend returns { steps, targetApp }. targetApp is auto-extracted
      // from the first ForegroundChanged event so the macro is bound to the
      // app it was recorded against — replay checks the app is running and
      // aborts via a modal if not.
      const result = await window.electronAPI.distillEvents(parsed.events);
      const steps = Array.isArray(result?.steps) ? result.steps : [];
      const targetApp = result?.targetApp || parsed.targetApp || null;
      if (steps.length > 0) {
        onChange(JSON.stringify({
          ...parsed,
          distilled: steps,
          playbackMode: 'distilled',
          targetApp,
          // Re-distil provides a fresh binding, so unset the user-cleared flag.
          // If the user cleared once, distilled again, then wants it clear
          // again, they click the Clear button after the re-distil.
          disableTargetBinding: false,
        }));
      } else {
        console.warn('[distill] engine returned empty step list');
      }
    } catch (e) {
      console.error('[distill] failed', e);
    } finally {
      setIsDistilling(false);
    }
  }

  function handleSetMode(nextMode) {
    if (!parsed) return;
    if (nextMode === 'distilled' && !isPro) {
      onShowUpgrade?.('Macro distillation');
      return;
    }
    onChange(JSON.stringify({ ...parsed, playbackMode: nextMode }));
  }

  return (
    <div className="macro-step-value replay-rec replay-rec--column">
      {summary ? (
        <>
          <div className="replay-rec-toprow">
            <span className="replay-rec-label">{summary.duration} · {summary.count} events captured</span>
            {hasDistilled && (
              <div className="replay-rec-mode-toggle" role="radiogroup" aria-label="Playback mode">
                <button
                  type="button"
                  className={`replay-rec-mode ${mode === 'raw' ? 'is-active' : ''}`}
                  onClick={() => handleSetMode('raw')}
                >Raw</button>
                <button
                  type="button"
                  className={`replay-rec-mode ${mode === 'distilled' ? 'is-active' : ''}`}
                  onClick={() => handleSetMode('distilled')}
                >Distilled</button>
              </div>
            )}
            <div className="replay-rec-btnrow">
              {!hasDistilled && (
                <button
                  type="button"
                  className={`replay-rec-btn ${!isPro ? 'replay-rec-btn--pro' : ''}`}
                  onClick={handleDistil}
                  disabled={isDistilling}
                  title={isPro ? 'Convert raw events into readable steps' : 'Pro feature — turns raw recordings into editable, window-aware steps'}
                >
                  {isDistilling ? 'Distilling…' : (<>Distil {!isPro && <span className="pro-badge" style={{ marginLeft: 4 }}>Pro</span>}</>)}
                </button>
              )}
              {hasDistilled && (
                <button type="button" className="replay-rec-btn replay-rec-btn--ghost" onClick={handleDistil} disabled={isDistilling} title="Re-run distillation from raw events">
                  {isDistilling ? 'Re-distilling…' : 'Re-distil'}
                </button>
              )}
              <button type="button" className="replay-rec-btn" onClick={startRecording}>
                <Circle size={11} fill="currentColor" strokeWidth={0} /> Re-record
              </button>
              <button type="button" className="replay-rec-btn replay-rec-btn--ghost" onClick={discardRecording} title="Discard recording">
                <Trash2 size={12} />
              </button>
            </div>
          </div>
          {hasDistilled && mode === 'distilled' && (
            <>
              {/* Target-app binding — visible so users understand which app
                  their distilled macro is locked to. ✕ clears the binding so
                  the macro runs against whatever's foreground (no precheck,
                  no "app not running" modal). Re-distil re-detects from the
                  first ForegroundChanged event in the raw stream. */}
              {parsed.targetApp ? (
                <div className="distilled-target-app is-set" role="status">
                  <span className="distilled-target-app-icon" aria-hidden="true">◎</span>
                  <span className="distilled-target-app-label">
                    Bound to <strong>{parsed.targetApp.windowTitleHint || parsed.targetApp.exe}</strong>
                  </span>
                  <button
                    type="button"
                    className="distilled-target-app-clear"
                    onClick={() => onChange(JSON.stringify({
                      ...parsed,
                      targetApp: null,
                      // Explicit user intent, so the Rust fire path knows to
                      // skip the event-stream fallback that would otherwise
                      // re-derive the binding from the first ForegroundChanged
                      // event. Reset to false on re-distil.
                      disableTargetBinding: true,
                    }))}
                    title="Clear binding, macro will run against whatever's focused"
                    aria-label="Clear target app binding"
                  >
                    <svg width="8" height="8" viewBox="0 0 10 10" aria-hidden="true">
                      <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
                      <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
                    </svg>
                  </button>
                </div>
              ) : (
                <div className="distilled-target-app is-unset" role="status">
                  <span className="distilled-target-app-label">
                    Not bound to a specific app — runs against whatever's focused
                  </span>
                </div>
              )}
              <div className="distilled-macro-embed">
                <MacroSequenceForm
                  value={{ steps: parsed.distilled }}
                  onChange={next => onChange(JSON.stringify({
                    ...parsed,
                    distilled: (next.steps || []),
                  }))}
                  globalInputMethod={globalInputMethod}
                  assignments={assignments}
                  profiles={profiles}
                  isPro={isPro}
                  onShowUpgrade={onShowUpgrade}
                />
              </div>
            </>
          )}
        </>
      ) : (
        <>
          <span className="replay-rec-label replay-rec-empty">No recording yet</span>
          <button type="button" className="replay-rec-btn" onClick={startRecording}>
            <Circle size={11} fill="currentColor" strokeWidth={0} /> Record
          </button>
        </>
      )}
    </div>
  );
}

// Standalone form for the Record Macro action type. Wraps the same
// ReplayRecordingValue recorder UI the macro step uses, but as the whole
// editor pane — the form value keeps the macro data shape ({ steps: [one
// Record Macro step] }) so saving/loading needs no conversion beyond the
// type remap in handleSave / displayTypeOf.
function RecordMacroForm({ value, onChange, isPro = false, onShowUpgrade, assignments = {}, profiles = [], globalInputMethod = 'global' }) {
  const step = (value.steps && value.steps[0]) || { type: 'Record Macro', value: '' };
  return (
    <div className="record-macro-form">
      <label className="form-label">Recording</label>
      <ReplayRecordingValue
        value={step.value || ''}
        onChange={v => onChange({ ...value, steps: [{ type: 'Record Macro', value: v }] })}
        isPro={isPro}
        onShowUpgrade={onShowUpgrade}
        assignments={assignments}
        profiles={profiles}
        globalInputMethod={globalInputMethod}
      />
      <p className="record-macro-hint">
        Press Record, perform your actions anywhere on screen, then press Ctrl+Shift+R to stop.
        Playback repeats them exactly, with the same timing.
      </p>
    </div>
  );
}

// Click-at-Position button selector — sits on the main step row as the inline
// value. User picks which mouse button fires at the chosen coordinates.
function ClickPositionButtonSelect({ step, updateStep }) {
  let cp = { x: 0, y: 0, button: 'left', mode: 'absolute' };
  try { cp = { ...cp, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  return (
    <select
      className="form-select macro-step-value"
      value={cp.button}
      onChange={e => updateStep({ ...step, value: JSON.stringify({ ...cp, button: e.target.value }) })}
    >
      <option value="left">Left Click</option>
      <option value="right">Right Click</option>
      <option value="middle">Middle Click</option>
    </select>
  );
}

// Click-at-Position sub-row — single row containing Pick Position button +
// x label + x input + y label + y input. Aligned padding so the row's left
// edge sits under Left Click's left edge and the y input's right edge sits
// under Left Click's right edge. Pick Position is intrinsic-width; the x/y
// inputs flex:1 to share the remaining space, ensuring everything end-to-end
// matches the button above.
function ClickPositionFields({ step, updateStep }) {
  let cp = { x: 0, y: 0, button: 'left', mode: 'absolute' };
  try { cp = { ...cp, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  const update = (patch) => updateStep({ ...step, value: JSON.stringify({ ...cp, ...patch }) });
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
    if (pos) update({ x: pos.x, y: pos.y });
  };

  return (
    // Sub-row mirrors the main row's column structure: drag+num padding (48px)
    // then a 130px-wide button matching the step-type dropdown's column, then
    // the x/y inputs filling the inline-value column. Right padding (60px)
    // matches the main row's dup+del area, so the y input's right edge sits
    // flush under Left Click's right edge.
    <div className="wfi-config-row wfi-config-row-columns click-pos-row">
      <button
        type="button"
        className="browse-btn click-pos-pick-btn"
        onClick={pickPosition}
        disabled={picking}
      >
        {picking ? `${countdown}...` : 'Pick Position'}
      </button>
      <label className="click-pos-axis-label">x</label>
      <NumberField
        value={cp.x}
        min={-32768}
        max={32767}
        defaultOnEmpty={0}
        onCommit={n => update({ x: n })}
        className="form-input click-pos-coord-input"
      />
      <label className="click-pos-axis-label">y</label>
      <NumberField
        value={cp.y}
        min={-32768}
        max={32767}
        defaultOnEmpty={0}
        onCommit={n => update({ y: n })}
        className="form-input click-pos-coord-input"
      />
      {/* Distilled-recording extras, read-only: held modifiers, press-hold
          duration and drag end point captured by the recorder. Shown so
          users can verify what got extracted; cleared via ✕ for a plain
          click instead. */}
      {(() => {
        const mods = Array.isArray(cp.modifiers) && cp.modifiers.length ? cp.modifiers.join('+') : '';
        const isDrag = cp.dragToX !== undefined;
        const isHold = (cp.holdMs || 0) > 150;
        if (!isDrag && !isHold && !mods) return null;
        const core = isDrag
          ? `drag → (${cp.dragToX}, ${cp.dragToY})${isHold ? ` · ${(cp.holdMs / 1000).toFixed(1)}s` : ''}`
          : isHold
            ? `hold ${(cp.holdMs / 1000).toFixed(1)}s`
            : 'click';
        return (
          <span className="click-pos-extra" title="Captured from the recording">
            {mods ? `${mods}+${core}` : core}
            <button
              type="button"
              className="click-pos-extra-clear"
              title="Remove modifiers/hold/drag — make this a plain click"
              onClick={() => {
                const { holdMs, dragToX, dragToY, modifiers, ...rest } = cp;
                updateStep({ ...step, value: JSON.stringify(rest) });
              }}
            >×</button>
          </span>
        );
      })()}
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
    // Win+Shift+S) gets picked up by the LL hook once Keyfire loses focus.
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

// Window picker — sits in the macro-step-row's .macro-step-value slot for both
// Focus Window and Wait for Window steps. Renders the "Pick Window" button
// (flex-fills the row, end-to-end with where Type Text's input would end) +
// a clear X + the currently-open-windows dropdown below. The button label
// reflects what's picked when present, so the user can see at a glance which
// window will be matched. The title sub-row underneath handles substring tweaks.
function WindowPicker({ value, onChange }) {
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const [windowList, setWindowList] = useState(null);
  const wrapRef = useRef(null);
  const dropdownRef = useRef(null);

  useEffect(() => {
    if (!dropdownOpen) return;
    function onDown(e) {
      if (wrapRef.current && !wrapRef.current.contains(e.target)) setDropdownOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [dropdownOpen]);

  // Flip the dropdown upward when its default top:100% position would clip the
  // viewport bottom. The picker often sits in a sub-row near the bottom of the
  // macro editor, so the default position frequently overflows. Remeasures
  // when windowList loads (placeholder → real rows changes the height).
  useLayoutEffect(() => {
    if (!dropdownOpen || !dropdownRef.current) return;
    const el = dropdownRef.current;
    // Clear any prior inline overrides so measurement reflects the default.
    el.style.top = '';
    el.style.bottom = '';
    el.style.marginTop = '';
    el.style.marginBottom = '';
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = 'auto';
      el.style.bottom = '100%';
      el.style.marginTop = '0';
      el.style.marginBottom = '4px';
    }
  }, [dropdownOpen, windowList]);

  const handlePickClick = async () => {
    if (dropdownOpen) { setDropdownOpen(false); return; }
    setWindowList(null);
    setDropdownOpen(true);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const list = await invoke('list_open_windows');
      setWindowList(list || []);
    } catch (e) {
      console.error('[Keyfire] list_open_windows failed:', e);
      setWindowList([]);
    }
  };

  const handleSelect = (win) => {
    // Preserve any non-process/title fields on value (e.g. timeoutMs for
    // Wait for Window). Focus Window value has only process+title; the spread
    // is a no-op there.
    onChange({ ...value, process: win.process, title: win.title });
    setDropdownOpen(false);
  };

  const handleClear = (e) => {
    e.stopPropagation();
    onChange({ ...value, process: '', title: '' });
  };

  const hasPick = !!(value.process || value.title);
  const pickedLabel = hasPick
    ? (value.process && value.title ? `${value.process} — ${value.title}` : (value.process || value.title))
    : 'Pick Window';

  return (
    <div ref={wrapRef} className="window-pick-wrap">
      <button
        type="button"
        className={`window-pick-btn${hasPick ? ' window-pick-btn-picked' : ''}`}
        onClick={handlePickClick}
        title={hasPick ? pickedLabel : 'Pick an open window'}
      >
        <span className="window-pick-btn-icon" aria-hidden="true">⊞</span>
        <span className="window-pick-btn-label">{pickedLabel}</span>
        <span className="window-pick-btn-caret" aria-hidden="true">▾</span>
      </button>
      {hasPick && (
        <button
          type="button"
          className="window-pick-clear"
          onClick={handleClear}
          aria-label="Clear picked window"
          title="Clear picked window"
        >✕</button>
      )}
      {dropdownOpen && (
        <div className="pick-window-dropdown" ref={dropdownRef}>
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
  );
}

// Open-App sub-row used inside SortableMacroStep — picker + optional args.
// Main-row inline picker + monitor picker for Open App. Renders as a fragment
// so the parent's `.macro-step-value-slot` (main row) holds both alongside
// the step-type dropdown. Modal state (pickerOpen) is local; the modal is
// portaled to document.body so it doesn't get clipped by the 415 row.
function MacroOpenAppInlinePicker({ appData, updateValue }) {
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

  // Just the picker button now — MonitorPicker moved to a dedicated sub-row.
  return (
    <>
      <button
        type="button"
        className="picker-field"
        style={{ flex: '1 1 60px', minWidth: 60 }}
        onClick={() => setPickerOpen(true)}
        title={displayLabel || 'Pick an installed app'}
      >
        <span className={`picker-field-value${displayLabel ? '' : ' picker-field-placeholder'}`}>
          {displayLabel || 'Pick an app…'}
        </span>
        <span className="picker-field-caret" aria-hidden="true">▾</span>
      </button>
      {pickerOpen && <AppPickerModal onSelect={handlePick} onClose={() => setPickerOpen(false)} />}
    </>
  );
}

// Sub-rows for Open App: monitor picker on its own row, then args / advanced.
// Picker button lives inline on the main row.
function MacroOpenAppArgsRow({ appData, updateValue, advancedOpen, toggleAdvanced }) {
  return (
    <>
      <div className="wfi-config-row" style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
        <MonitorPicker
          value={appData.monitor || 'default'}
          onChange={(m) => updateValue({ ...appData, monitor: m })}
        />
      </div>
      {(advancedOpen || appData.args) ? (
        <div className="wfi-config-row" style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <input
            className="form-input"
            style={{ flex: 1 }}
            placeholder="Arguments (optional)"
            value={appData.args}
            onChange={e => updateValue({ ...appData, args: e.target.value })}
          />
        </div>
      ) : (
        <div className="wfi-config-row" style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <button className="macro-advanced-toggle" type="button" onClick={toggleAdvanced}>+ Advanced</button>
        </div>
      )}
    </>
  );
}

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
    <>
      {/* Primary horizontal row — app picker, monitor picker sit on the same
          line so they read as a single control group, not stacked. Args /
          Advanced is on a second sub-row because the args input is wider than
          what fits horizontally alongside the picker + monitor. */}
      <div className="wfi-config-row" style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
        {/* Single clickable field — shows the picked app's display name (or a
            placeholder when empty). Click anywhere on the field to reopen the
            picker. Replaces the separate input + Pick app button. */}
        <button
          type="button"
          className="picker-field"
          style={{ flex: '1 1 60px', minWidth: 60 }}
          onClick={() => setPickerOpen(true)}
          title={displayLabel || 'Pick an installed app'}
        >
          <span className={`picker-field-value${displayLabel ? '' : ' picker-field-placeholder'}`}>
            {displayLabel || 'Pick an installed app...'}
          </span>
          <span className="picker-field-caret" aria-hidden="true">▾</span>
        </button>
        <MonitorPicker
          value={appData.monitor || 'default'}
          onChange={(m) => updateValue({ ...appData, monitor: m })}
        />
      </div>
      {(advancedOpen || appData.args) ? (
        <div className="wfi-config-row" style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <input
            className="form-input"
            style={{ flex: 1 }}
            placeholder="Arguments (optional)"
            value={appData.args}
            onChange={e => updateValue({ ...appData, args: e.target.value })}
          />
        </div>
      ) : (
        <div className="wfi-config-row" style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <button className="macro-advanced-toggle" type="button" onClick={toggleAdvanced}>+ Advanced</button>
        </div>
      )}
      {pickerOpen && <AppPickerModal onSelect={handlePick} onClose={() => setPickerOpen(false)} />}
    </>
  );
}

// ── Step-type dropdown with category flyouts ───────────────────────────────
// Replaces a flat <select> that had grown to 11 options. Categories from
// MACRO_STEP_CATEGORIES open a submenu to the right on hover, mirroring the
// .assign-ctx-sub pattern in Sidebar. Portal'd to <body> with fixed positioning
// so the menu and its flyouts don't clip on any ancestor's overflow.

function MacroStepTypeMenu({ value, onChange }) {
  const [open, setOpen] = useState(false);
  const [hovered, setHovered] = useState(null);
  const [pos, setPos] = useState({ top: 0, left: 0, btnTop: 0 });
  const btnRef = useRef(null);
  const menuRef = useRef(null);
  // Captures whichever submenu is currently rendered (only one at a time, since
  // hovered === entry.label gates the JSX render below).
  const submenuRef = useRef(null);

  const handleToggle = () => {
    if (open) { setOpen(false); return; }
    if (btnRef.current) {
      const r = btnRef.current.getBoundingClientRect();
      setPos({ top: r.bottom + 4, left: r.left, btnTop: r.top });
    }
    setOpen(true);
    setHovered(null);
  };

  useEffect(() => {
    if (!open) return;
    const onDocDown = (e) => {
      if (menuRef.current?.contains(e.target)) return;
      if (btnRef.current?.contains(e.target)) return;
      setOpen(false);
    };
    const onKey = (e) => { if (e.key === 'Escape') setOpen(false); };
    // Close on any ancestor scroll — the menu's fixed position would otherwise
    // detach from the button when the form/panel scrolls.
    const onScroll = () => setOpen(false);
    document.addEventListener('mousedown', onDocDown);
    document.addEventListener('keydown', onKey);
    window.addEventListener('scroll', onScroll, true);
    window.addEventListener('resize', onScroll);
    return () => {
      document.removeEventListener('mousedown', onDocDown);
      document.removeEventListener('keydown', onKey);
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('resize', onScroll);
    };
  }, [open]);

  // Reposition parent menu after it renders. Flip above the trigger button if
  // the menu would clip the viewport bottom; shift left if it would clip the
  // right edge. Mirrors the pattern in TextExpansions Insert/Key pickers.
  useLayoutEffect(() => {
    if (!open || !menuRef.current) return;
    const popup = menuRef.current;
    const rect = popup.getBoundingClientRect();
    const margin = 8;
    let top = pos.top;
    let left = pos.left;
    if (rect.bottom > window.innerHeight - margin) {
      top = pos.btnTop - rect.height - 4;
    }
    if (rect.right > window.innerWidth - margin) {
      left = window.innerWidth - rect.width - margin;
    }
    popup.style.top = `${Math.max(margin, top)}px`;
    popup.style.left = `${Math.max(margin, left)}px`;
  }, [open, pos]);

  // Reposition the active submenu when hover changes. Default is top:-4
  // left:100% (CSS). Shift up by the bottom overflow (clamping so the submenu
  // top stays inside the viewport); swap to right:100% if the right edge clips.
  useLayoutEffect(() => {
    if (!open || !hovered || !submenuRef.current) return;
    const sub = submenuRef.current;
    // Clear any prior inline overrides so measurement reflects the default
    // position before we decide whether to flip.
    sub.style.top = '';
    sub.style.left = '';
    sub.style.right = '';
    sub.style.marginLeft = '';
    sub.style.marginRight = '';
    const rect = sub.getBoundingClientRect();
    const margin = 8;
    const bottomOverflow = rect.bottom - (window.innerHeight - margin);
    if (bottomOverflow > 0) {
      let shift = bottomOverflow;
      // Don't push the submenu top above the viewport top.
      const newTop = rect.top - shift;
      if (newTop < margin) shift -= (margin - newTop);
      sub.style.top = `${-4 - shift}px`;
    }
    if (rect.right > window.innerWidth - margin) {
      sub.style.left = 'auto';
      sub.style.right = '100%';
      sub.style.marginLeft = '0';
      sub.style.marginRight = '-1px';
    }
  }, [hovered, open]);

  const pick = (label) => {
    onChange(label);
    setOpen(false);
  };

  // Which group contains the current value? Used to highlight the parent row
  // so the user can see at-a-glance where the active step type lives.
  const currentGroup = MACRO_STEP_CATEGORIES.find(
    g => g.kind === 'group' && g.items.includes(value)
  )?.label;

  return (
    <>
      <button
        ref={btnRef}
        type="button"
        className={`macro-step-type macro-step-type-btn${open ? ' macro-step-type-btn-open' : ''}`}
        onClick={handleToggle}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        <span className="macro-step-type-label">{macroStepLabel(value)}</span>
        <span className="macro-step-type-caret" aria-hidden="true">▾</span>
      </button>
      {open && createPortal(
        <div
          ref={menuRef}
          className="macro-type-menu"
          style={{ top: pos.top, left: pos.left }}
          role="menu"
        >
          {MACRO_STEP_CATEGORIES.map((entry, idx) => {
            if (entry.kind === 'divider') {
              return <div key={`div-${idx}`} className="macro-type-divider" />;
            }
            if (entry.kind === 'leaf') {
              return (
                <button
                  key={entry.label}
                  type="button"
                  className={`macro-type-item${value === entry.label ? ' macro-type-item-current' : ''}`}
                  onClick={() => pick(entry.label)}
                  role="menuitem"
                >
                  <span>{entry.label}</span>
                </button>
              );
            }
            // group
            const isCurrentGroup = entry.label === currentGroup;
            const isHovered = hovered === entry.label;
            return (
              <div
                key={entry.label}
                className="macro-type-sub"
                onMouseEnter={() => setHovered(entry.label)}
                onMouseLeave={() => setHovered(prev => prev === entry.label ? null : prev)}
              >
                <button
                  type="button"
                  className={`macro-type-item macro-type-item-parent${isCurrentGroup ? ' macro-type-item-current' : ''}${isHovered ? ' macro-type-item-hover' : ''}`}
                  role="menuitem"
                  aria-haspopup="menu"
                >
                  <span>{entry.label}</span>
                  <span className="macro-type-arrow" aria-hidden="true">▸</span>
                </button>
                {isHovered && (
                  <div className="macro-type-submenu" role="menu" ref={submenuRef}>
                    {entry.items.map(item => (
                      <button
                        key={item}
                        type="button"
                        className={`macro-type-item${value === item ? ' macro-type-item-current' : ''}`}
                        onClick={() => pick(item)}
                        role="menuitem"
                      >
                        <span>{macroStepLabel(item)}</span>
                        {PRO_MACRO_STEPS.has(item) && <span className="pro-badge">PRO</span>}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
        </div>,
        document.body
      )}
    </>
  );
}

// ── Fire-target picker (Trigger / Text Expansion macro steps) ──────────────
// Centered portal modal. Mode 'trigger' lists every assignment EXCEPT
// GLOBAL::EXPANSION::* / GLOBAL::AUTOCORRECT::* (those have a separate fire
// step type). Mode 'expansion' lists only GLOBAL::EXPANSION::* entries.
// Triggers group by profile/app (first segment of the storage key);
// expansions group by data.category. Search matches against label + combo
// for triggers, trigger word + preview text for expansions.

function parseAssignmentKey(key) {
  // "Profile::Ctrl+Shift::KeyN" → { container, combo, keyId, suffix }
  // "Profile::BARE::F12" → bare key
  // "Profile::Ctrl::KeyN::double" → double-press variant
  const parts = key.split('::');
  if (parts.length < 3) return null;
  const container = parts[0];
  const combo = parts[1];
  const keyId = parts[2];
  const suffix = parts[3] || '';
  return { container, combo, keyId, suffix };
}

function formatCombo(combo, keyId) {
  const keyLabel = friendlyKeyName(keyId);
  if (combo === 'BARE') return keyLabel;
  return [...combo.split('+'), keyLabel].join('+');
}

export function FireTargetPicker({ mode, assignments, currentValue, onSelect, onClose, profilesOrder }) {
  const [query, setQuery] = useState('');
  const inputRef = useRef(null);
  const panelRef = useRef(null);

  useEffect(() => { inputRef.current?.focus(); }, []);

  useEffect(() => {
    const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); onClose?.(); } };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  // Backdrop click closes; clicks inside the panel are ignored
  const handleBackdrop = (e) => {
    if (panelRef.current && !panelRef.current.contains(e.target)) onClose?.();
  };

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (mode === 'expansion') {
      const items = Object.entries(assignments)
        .filter(([k]) => k.startsWith('GLOBAL::EXPANSION::'))
        .map(([k, v]) => {
          const trigger = k.slice('GLOBAL::EXPANSION::'.length);
          const d = v?.data || {};
          const isImage = d.expansionType === 'image';
          const isVariant = Array.isArray(d.options) && d.options.length > 0;
          const preview = isImage
            ? `Image · ${(d.imagePath || '').split(/[\\/]/).pop() || ''}`
            : isVariant
              ? `${d.options.length} variants`
              : ((d.text || '').replace(/\s+/g, ' ').trim().slice(0, 60));
          const category = d.category || 'Uncategorised';
          const displayName = d.displayName || '';
          // Lead with the user's Name when present; trigger drops into the
          // secondary line alongside the preview. When unnamed, the trigger
          // takes the primary slot (current behaviour for the no-name case).
          const primary = displayName || `:${trigger}`;
          const secondary = displayName
            ? `:${trigger}${preview ? ` · ${preview}` : ''}`
            : preview;
          return {
            value: trigger,
            trigger,
            primary,
            secondary,
            group: category,
            displayName,
          };
        })
        .filter(it => {
          if (!q) return true;
          return it.primary.toLowerCase().includes(q)
              || it.secondary.toLowerCase().includes(q)
              || it.trigger.toLowerCase().includes(q);
        })
        .sort((a, b) => a.primary.localeCompare(b.primary));
      const byGroup = new Map();
      for (const it of items) {
        if (!byGroup.has(it.group)) byGroup.set(it.group, []);
        byGroup.get(it.group).push(it);
      }
      return Array.from(byGroup.entries())
        .sort(([a], [b]) => a.localeCompare(b))
        .map(([name, items]) => ({ name, items }));
    }

    // mode === 'trigger': every keyboard/mouse assignment. Excludes GLOBAL::*
    // namespaces (expansions / autocorrect / quick-actions) — those are either
    // fire-able via the expansion picker (EXPANSION), disabled (AUTOCORRECT),
    // or surfaced from a different UI (QUICKACTION). May surface QUICKACTION
    // here in a future iteration if testers ask, but the picker UX would need
    // a separate group treatment since the storage key has no profile/combo.
    const items = Object.entries(assignments)
      .filter(([k]) => !k.startsWith('GLOBAL::'))
      .map(([k, v]) => {
        const parsed = parseAssignmentKey(k);
        if (!parsed) return null;
        const combo = formatCombo(parsed.combo, parsed.keyId);
        const dbl = parsed.suffix === 'double' ? ' (double)' : '';
        const label = v?.label || `${parsed.container} · ${combo}${dbl}`;
        const macroType = v?.type || '';
        return {
          value: k,
          primary: label,
          secondary: `${combo}${dbl} · ${macroType}`,
          group: parsed.container,
        };
      })
      .filter(Boolean)
      .filter(it => {
        if (!q) return true;
        return it.primary.toLowerCase().includes(q)
            || it.secondary.toLowerCase().includes(q)
            || it.value.toLowerCase().includes(q);
      })
      .sort((a, b) => a.primary.localeCompare(b.primary));
    const byGroup = new Map();
    for (const it of items) {
      if (!byGroup.has(it.group)) byGroup.set(it.group, []);
      byGroup.get(it.group).push(it);
    }
    // Default always leads, then profiles in the user's sidebar order (the
    // config profiles array). Groups not in that array (raw app-name
    // containers from AppName::combo keys) keep the old fallback: after
    // profiles, apps last (apps contain '.' usually — .exe basename), A-Z.
    const order = profilesOrder || [];
    const rank = (name) => {
      if (name === 'Default') return -1;
      const i = order.indexOf(name);
      return i === -1 ? Number.MAX_SAFE_INTEGER : i;
    };
    return Array.from(byGroup.entries())
      .sort(([a], [b]) => {
        const ra = rank(a);
        const rb = rank(b);
        if (ra !== rb) return ra - rb;
        const aApp = a.includes('.');
        const bApp = b.includes('.');
        if (aApp !== bApp) return aApp ? 1 : -1;
        return a.localeCompare(b);
      })
      .map(([name, items]) => ({ name, items }));
  }, [mode, assignments, query, profilesOrder]);

  const totalCount = groups.reduce((sum, g) => sum + g.items.length, 0);
  const title = mode === 'expansion' ? 'Choose a text expansion to fire' : 'Choose a trigger to fire';
  const emptyHint = mode === 'expansion'
    ? 'No text expansions yet. Create one in the Expansions tab.'
    : 'No triggers yet. Map an action to a key first.';

  return createPortal(
    <div className="fire-picker-backdrop" onMouseDown={handleBackdrop}>
      <div ref={panelRef} className="fire-picker" role="dialog" aria-label={title}>
        <div className="fire-picker-header">
          <span className="fire-picker-title">{title}</span>
          <button className="fire-picker-close" type="button" onClick={onClose} aria-label="Close">&#10005;</button>
        </div>
        <div className="fire-picker-search-row">
          <input
            ref={inputRef}
            className="fire-picker-search"
            placeholder={mode === 'expansion'
              ? 'Search by trigger or preview…'
              : 'Search by label, combo, or profile…'}
            value={query}
            onChange={e => setQuery(e.target.value)}
            onKeyDown={e => e.stopPropagation()}
          />
        </div>
        <div className="fire-picker-list">
          {totalCount === 0 && (
            <div className="fire-picker-empty">{query ? 'No matches.' : emptyHint}</div>
          )}
          {groups.map(group => (
            <div key={group.name} className="fire-picker-group">
              <div className="fire-picker-group-header">{group.name}</div>
              {group.items.map(it => {
                const isActive = it.value === currentValue;
                return (
                  <button
                    key={it.value}
                    type="button"
                    className={`fire-picker-item${isActive ? ' fire-picker-item-active' : ''}`}
                    onClick={() => onSelect?.(it.value)}
                  >
                    <span className="fire-picker-item-primary">{it.primary}</span>
                    <span className="fire-picker-item-secondary">{it.secondary}</span>
                  </button>
                );
              })}
            </div>
          ))}
        </div>
      </div>
    </div>,
    document.body
  );
}

// ── Sort Files config modal ─────────────────────────────────────────────────
// Sort Files outgrew the inline step row (mapping table, discovery settings,
// dialog options) — first step to use the modal-config pattern: the row shows
// a summary + Configure button, the full form lives here, portaled to
// document.body like the app picker. Draft state is local — nothing writes
// to the step until Save.
function SortFilesConfigModal({ initial, onSave, onClose }) {
  const [draft, setDraft] = useState(() => {
    const d = { ...SORT_FILES_DEFAULTS, ...initial };
    // Legacy pre-release value: 'ask' (native conflict dialog) folded into
    // the Keyfire clash prompt. Backend maps it too.
    if (d.collision === 'ask') d.collision = 'prompt';
    return d;
  });
  const patch = (p) => setDraft(d => ({ ...d, ...p }));

  useEffect(() => {
    // Capture phase so document-level Esc handling never sees it.
    const onKey = (e) => { if (e.key === 'Escape') { e.stopPropagation(); onClose(); } };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [onClose]);

  const setMapping = (i, p) => setDraft(d => ({ ...d, mappings: d.mappings.map((m, idx) => (idx === i ? { ...m, ...p } : m)) }));
  const removeMapping = (i) => setDraft(d => ({ ...d, mappings: d.mappings.filter((_, idx) => idx !== i) }));
  const addMapping = () => setDraft(d => ({ ...d, mappings: [...d.mappings, { code: '', folder: '' }] }));

  const pickFolder = async (field) => {
    const path = await window.electronAPI?.browseForFolder();
    if (path) patch({ [field]: path });
  };

  return createPortal(
    <div className="app-picker-overlay" onClick={onClose}>
      <div className="sortfiles-modal" onClick={e => e.stopPropagation()}>
        <div className="sortfiles-header">
          <span className="sortfiles-title">Sort Files rules</span>
          <button className="app-picker-close" type="button" onClick={onClose} title="Close">✕</button>
        </div>
        <div className="sortfiles-body">

          {/* 1 — what goes in */}
          <div className="sortfiles-section">
            <div className="sortfiles-section-label">Files to sort</div>
            <div className="sortfiles-grid">
              <span className="sortfiles-label">Source</span>
              <select
                className="form-select"
                style={{ maxWidth: 220 }}
                value={draft.sourceMode}
                onChange={e => patch({ sourceMode: e.target.value })}
              >
                <option value="selected">Selected files in Explorer</option>
                <option value="folder">Files from a folder</option>
              </select>
              {draft.sourceMode === 'folder' && (
                <>
                  <span className="sortfiles-label">Folder</span>
                  <button
                    type="button"
                    className="picker-field"
                    onClick={() => pickFolder('sourcePath')}
                    title={draft.sourcePath || 'Pick the source folder…'}
                  >
                    <span className={`picker-field-value${draft.sourcePath ? '' : ' picker-field-placeholder'}`}>
                      {draft.sourcePath || 'Pick the source folder…'}
                    </span>
                    <span className="picker-field-caret" aria-hidden="true">▾</span>
                  </button>
                  <span className="sortfiles-label">Pattern</span>
                  <div className="sortfiles-inline">
                    <input
                      className="form-input"
                      style={{ maxWidth: 160 }}
                      placeholder="*.pdf; *.docx"
                      value={draft.pattern ?? '*'}
                      onChange={e => patch({ pattern: e.target.value })}
                    />
                    <span className="sortfiles-hint">* and ? wildcards, ; separates</span>
                  </div>
                </>
              )}
            </div>
          </div>

          {/* 2 — where each file goes */}
          <div className="sortfiles-section">
            <div className="sortfiles-section-label">Destination folder</div>
            <div className="sortfiles-grid">
              <span className="sortfiles-label">Search inside</span>
              <button
                type="button"
                className="picker-field"
                onClick={() => pickFolder('rootPath')}
                title={draft.rootPath || 'Pick the folder to search…'}
              >
                <span className={`picker-field-value${draft.rootPath ? '' : ' picker-field-placeholder'}`}>
                  {draft.rootPath || 'Pick the folder to search…'}
                </span>
                <span className="picker-field-caret" aria-hidden="true">▾</span>
              </button>
              <span className="sortfiles-label">Search depth</span>
              <div className="sortfiles-inline">
                <NumberField
                  value={draft.searchDepth}
                  min={1}
                  max={8}
                  defaultOnEmpty={3}
                  onCommit={n => patch({ searchDepth: n })}
                  style={{ width: 56 }}
                />
                <span className="sortfiles-hint">folder levels below the search folder</span>
              </div>
              <span className="sortfiles-label">Match by</span>
              <div className="sortfiles-inline">
                <select
                  className="form-select"
                  style={{ width: 190 }}
                  value={draft.keyMode}
                  onChange={e => patch({ keyMode: e.target.value })}
                >
                  <option value="prefix">First characters of name</option>
                  <option value="segment">A segment of the name</option>
                </select>
                {draft.keyMode === 'prefix' ? (
                  <>
                    <span className="sortfiles-hint">first</span>
                    <NumberField
                      value={draft.keyLength}
                      min={1}
                      max={64}
                      defaultOnEmpty={6}
                      onCommit={n => patch({ keyLength: n })}
                      style={{ width: 56 }}
                    />
                    <span className="sortfiles-hint">characters</span>
                  </>
                ) : (
                  <>
                    <span className="sortfiles-hint">segment</span>
                    <NumberField
                      value={draft.keySegment}
                      min={1}
                      max={32}
                      defaultOnEmpty={1}
                      onCommit={n => patch({ keySegment: n })}
                      style={{ width: 56 }}
                    />
                    <span className="sortfiles-hint">split by</span>
                    <input
                      className="form-input"
                      style={{ width: 44, textAlign: 'center' }}
                      maxLength={3}
                      value={draft.keySeparator ?? '-'}
                      onChange={e => patch({ keySeparator: e.target.value })}
                    />
                  </>
                )}
              </div>
            </div>
            <p className="sortfiles-note">
              The first folder whose name contains the matched text becomes the file's destination.
              <span className="sortfiles-example">PRJ042-A-INV-001.pdf&ensp;→&ensp;PRJ042&ensp;→&ensp;[PRJ042] Acme Office Fit-Out</span>
            </p>
          </div>

          {/* 3 — optional second hop into a subfolder */}
          <div className="sortfiles-section">
            <div className="sortfiles-section-head">
              <span className="sortfiles-section-label">Subfolder routing</span>
              <label className="sortfiles-check">
                <input
                  type="checkbox"
                  checked={!!draft.routeEnabled}
                  onChange={e => patch({ routeEnabled: e.target.checked })}
                />
                Enable
              </label>
            </div>
            {draft.routeEnabled ? (
              <>
                <div className="sortfiles-grid">
                  <span className="sortfiles-label">Code is</span>
                  <div className="sortfiles-inline">
                    <span className="sortfiles-hint">segment</span>
                    <NumberField
                      value={draft.codeSegment}
                      min={1}
                      max={32}
                      defaultOnEmpty={3}
                      onCommit={n => patch({ codeSegment: n })}
                      style={{ width: 56 }}
                    />
                    <span className="sortfiles-hint">of the name, split by</span>
                    <input
                      className="form-input"
                      style={{ width: 44, textAlign: 'center' }}
                      maxLength={3}
                      value={draft.codeSeparator ?? '-'}
                      onChange={e => patch({ codeSeparator: e.target.value })}
                    />
                  </div>
                </div>
                <div className="sortfiles-map-table">
                  <div className="sortfiles-map-head">
                    <span>Code</span>
                    <span />
                    <span>Subfolder name</span>
                    <span />
                  </div>
                  {draft.mappings.map((m, i) => (
                    <div className="sortfiles-map-row" key={i}>
                      <input
                        className="form-input"
                        placeholder="INV"
                        value={m.code}
                        onChange={e => setMapping(i, { code: e.target.value })}
                      />
                      <span className="sortfiles-map-arrow" aria-hidden="true">→</span>
                      <input
                        className="form-input"
                        placeholder="Invoices"
                        value={m.folder}
                        onChange={e => setMapping(i, { folder: e.target.value })}
                      />
                      <button
                        type="button"
                        className="sortfiles-remove"
                        onClick={() => removeMapping(i)}
                        title="Remove mapping"
                        aria-label="Remove mapping"
                      >
                        ✕
                      </button>
                    </div>
                  ))}
                  <button type="button" className="sortfiles-add" onClick={addMapping}>
                    + Add mapping
                  </button>
                </div>
                <p className="sortfiles-note">
                  Files with an unmapped code, or whose subfolder doesn't exist inside the matched
                  folder, are skipped and reported — never guessed.
                </p>
              </>
            ) : (
              <p className="sortfiles-note">Files land directly in the matched folder.</p>
            )}
          </div>

          {/* 4 — behaviour */}
          <div className="sortfiles-section">
            <div className="sortfiles-section-label">Options</div>
            <div className="sortfiles-grid">
              <span className="sortfiles-label">If file exists</span>
              <select
                className="form-select"
                style={{ maxWidth: 220 }}
                value={draft.collision}
                title="One dialog covers all clashes in a run: overwrite, keep both with a date suffix, or stop"
                onChange={e => patch({ collision: e.target.value })}
              >
                <option value="prompt">Ask: overwrite or add date</option>
                <option value="timestamp">Add a date suffix silently</option>
                <option value="skip">Skip the file</option>
              </select>
              <span className="sortfiles-label">Dialogs</span>
              <label className="sortfiles-check">
                <input
                  type="checkbox"
                  checked={draft.confirm !== false}
                  onChange={e => patch({ confirm: e.target.checked })}
                />
                Show the plan before moving, and a report after
              </label>
            </div>
          </div>

        </div>
        <div className="sortfiles-footer">
          <button type="button" className="sortfiles-btn" onClick={onClose}>Cancel</button>
          <button type="button" className="sortfiles-btn sortfiles-btn-primary" onClick={() => onSave(draft)}>
            Save rules
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}

// ── Sortable step row (extracted for @dnd-kit) ─────────────────────────────

let _nextStepId = 1;

function SortableMacroStep({ step, index, updateStep, removeStep, duplicateStep, advancedOpen, toggleAdvanced, assignments, profiles, isPro = false, onShowUpgrade }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: step._id });
  // Sort Files config modal visibility — the step's form lives in a modal
  // (first step to outgrow the inline row), see SortFilesConfigModal.
  const [sortConfigOpen, setSortConfigOpen] = useState(false);
  const style = {
    transform: DndCSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
  };

  // Press Key Win-advisory — the LL hook can't intercept the Win key when
  // Keyfire's own WebView2 has focus (documented Tauri/WebView2 limitation), so
  // direct Win+X capture isn't possible inside our own UI. The advisory
  // sub-row is the deliberate alternative: shown whenever Win is in the combo
  // OR the user just pressed Win during capture.
  const [winPrompted, setWinPrompted] = useState(false);
  // Fire-target picker — null when closed, 'trigger' or 'expansion' when open.
  const [firePickerMode, setFirePickerMode] = useState(null);
  // Audio output devices — enumerated once when the step type is Change Audio
  // Output. Refresh button re-fetches (headphones plugged in mid-edit etc).
  // Kept on the step component rather than hoisted because most macros have
  // ≤1 audio-change step, and the enumeration call is <10ms.
  const [audioDevices, setAudioDevices] = useState(null); // null = loading, [] = enumerated empty
  const refreshAudioDevices = useCallback(() => {
    window.electronAPI?.listAudioOutputDevices?.().then(list => {
      setAudioDevices(Array.isArray(list) ? list : []);
    }).catch(() => setAudioDevices([]));
  }, []);
  useEffect(() => {
    if (step.type !== 'Change Audio Output') return;
    if (audioDevices !== null) return; // already loaded
    refreshAudioDevices();
  }, [step.type, audioDevices, refreshAudioDevices]);
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

  const hasSubRow = ['Press Key', 'Click Mouse', 'Wait for Input', 'Open App', 'Open Folder', 'Focus Window', 'Wait for Window', 'Run AHK Script', 'Click at Position', 'Minimise Window', 'Maximise Window', 'Resize Window', 'Play Audio File', 'Play Video File', 'Create Folder', 'Copy Files', 'Move Files'].includes(step.type) || showWinAdvisory;

  // Parse JSON values for structured step types
  let appData = { kind: 'path', appId: '', appName: '', path: '', args: '', monitor: 'default' };
  if (step.type === 'Open App') { try { appData = { ...appData, ...JSON.parse(step.value || '{}') }; } catch (_) {} }
  let focusData = { process: '', title: '' };
  if (step.type === 'Focus Window') { try { focusData = { ...focusData, ...JSON.parse(step.value || '{}') }; } catch (_) {} }
  // Minimise / Maximise Window — same shape as Focus (process + title). Empty
  // process AND empty title means "the currently-focused window" (Rust arm
  // falls back to GetForegroundWindow / *target_hwnd).
  let minmaxData = { process: '', title: '' };
  if (step.type === 'Minimise Window' || step.type === 'Maximise Window') {
    try { minmaxData = { ...minmaxData, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  }
  // Resize Window — Focus fields + width/height. usePosition + x/y are
  // optional (default: keep current position, only change size).
  let resizeData = { process: '', title: '', width: 1200, height: 800, usePosition: false, x: 100, y: 100 };
  if (step.type === 'Resize Window') {
    try { resizeData = { ...resizeData, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  }
  // Play Audio File / Play Video File — shell-open a media path via the
  // OS default handler (WMP, Groove, VLC, whatever the user has associated).
  // Monitor picker targets the player window when a fresh player instance
  // launches; a no-op for players that reuse an existing window.
  let mediaData = { path: '', monitor: 'default' };
  if (step.type === 'Play Audio File' || step.type === 'Play Video File') {
    try { mediaData = { ...mediaData, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  }
  // Wait for Window: stored as { process, title, timeoutMs }. Matches the same
  // shape as Focus Window plus a timeout. Backend matches on process basename
  // (if set) AND title substring (if set) — see actions.rs Wait for Window arm.
  let waitWindowData = { process: '', title: '', timeoutMs: 30000 };
  if (step.type === 'Wait for Window') { try { waitWindowData = { ...waitWindowData, ...JSON.parse(step.value || '{}') }; } catch (_) {} }
  // Create Folder: {name, promptForName, locationMode: 'current'|'custom',
  // path, templateEnabled, templatePath}. 'current' resolves to the
  // foreground File Explorer folder at run time; 'custom' uses the picked
  // path. Tokens ({date}, {clipboard}, {inc}) work in name. promptForName
  // opens the fill-in window at run time with the name as editable default;
  // templateEnabled copies templatePath's contents into the new folder.
  let createFolderData = { name: '', promptForName: false, locationMode: 'current', path: '', templateEnabled: false, templatePath: '' };
  if (step.type === 'Create Folder') {
    try { createFolderData = { ...createFolderData, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  }
  // Copy Files / Move Files: {sourceMode: 'selected'|'folder', sourcePath,
  // pattern, destMode: 'path'|'subfolder', destPath, destSubfolder,
  // createSubfolder}. 'selected' = the Explorer selection when the macro
  // fires; 'folder' = files in sourcePath matching the wildcard pattern.
  // destMode 'subfolder' resolves destSubfolder against the folder the files
  // come from at run time; missing subfolder aborts the macro unless
  // createSubfolder is ticked.
  let fileOpData = { sourceMode: 'selected', sourcePath: '', pattern: '*', destMode: 'path', destPath: '', destSubfolder: '', createSubfolder: false };
  if (step.type === 'Copy Files' || step.type === 'Move Files') {
    try { fileOpData = { ...fileOpData, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  }
  // Sort Files: full rules object edited in SortFilesConfigModal — the row
  // only shows a summary + Configure button. Defaults merged so partial
  // values never leave fields undefined.
  let sortData = SORT_FILES_DEFAULTS;
  if (step.type === 'Sort Files') {
    try { sortData = { ...SORT_FILES_DEFAULTS, ...JSON.parse(step.value || '{}') }; } catch (_) {}
  }
  // Open Folder: legacy stored step.value as a plain path string. New writes
  // emit JSON {path, monitor}. Detect by leading '{' and parse-or-fallback.
  let folderData = { path: '', monitor: 'default' };
  if (step.type === 'Open Folder') {
    const raw = step.value || '';
    if (raw.trim().startsWith('{')) {
      try { folderData = { ...folderData, ...JSON.parse(raw) }; } catch (_) { folderData.path = raw; }
    } else {
      folderData.path = raw;
    }
  }

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
        <MacroStepTypeMenu
          value={step.type}
          onChange={(t) => {
            // Sort Files is Pro-gated — show the upgrade prompt instead of
            // switching the step type. Backend has a belt-and-braces
            // licence::is_pro() check in the execution arm.
            if (t === 'Sort Files' && !isPro) {
              onShowUpgrade?.('Sort Files macro step');
              return;
            }
            // Seed step.value with a sensible default per type so the step
            // fires on its first click without requiring the user to touch
            // the config widget. Otherwise types with JSON-shaped values
            // (Change Volume, Mouse Scroll) fire with "" and the Rust arm
            // logs an "unknown mode" warning + no-ops.
            let seed = '';
            if (t === 'Click Mouse')   seed = 'LButton';
            else if (t === 'Change Volume') seed = JSON.stringify({ mode: 'increase', amount: 5 });
            else if (t === 'Change Audio Output') seed = JSON.stringify({ deviceId: '', deviceName: '' });
            else if (t === 'Mouse Scroll')  seed = JSON.stringify({ direction: 'down', amount: 3 });
            else if (t === 'Minimise Window' || t === 'Maximise Window') seed = JSON.stringify({ process: '', title: '' });
            else if (t === 'Resize Window') seed = JSON.stringify({ process: '', title: '', width: 1200, height: 800, usePosition: false, x: 100, y: 100 });
            else if (t === 'Play Audio File' || t === 'Play Video File') seed = JSON.stringify({ path: '', monitor: 'default' });
            else if (t === 'Create Folder') seed = JSON.stringify({ name: '', promptForName: false, locationMode: 'current', path: '', templateEnabled: false, templatePath: '' });
            else if (t === 'Copy Files' || t === 'Move Files') seed = JSON.stringify({ sourceMode: 'selected', sourcePath: '', pattern: '*', destMode: 'path', destPath: '', destSubfolder: '', createSubfolder: false });
            else if (t === 'Sort Files') seed = JSON.stringify(SORT_FILES_DEFAULTS);
            updateStep({ ...step, type: t, value: seed });
          }}
        />

        {/* Inline value fields */}
        {step.type === 'Press Key' && (
          <KeyCaptureInput
            value={step.value || ''}
            onChange={v => updateStep({ ...step, value: v })}
            onWinPressed={() => setWinPrompted(true)}
          />
        )}
        {step.type === 'Click Mouse' && (() => {
          // Split the stored value + phase into a normalised { button, phase }
          // pair. Handles legacy suffixed values ("LButtonDown") transparently
          // — see migrateClickMouseValue. On write we always emit the clean
          // shape (button = "LButton"/"RButton"/"MButton", phase in its own
          // field), so touching a legacy step auto-migrates it.
          const { button, phase } = migrateClickMouseValue(step.value, step.phase);
          return (
            <select
              className="form-select macro-step-value"
              // Only three options now (Left / Right / Middle) so the row has
              // ample space alongside ×repeat + duplicate + remove.
              style={{ flex: '0 1 130px', minWidth: 100 }}
              value={button}
              onChange={e => updateStep({ ...step, value: e.target.value, phase })}
            >
              {MOUSE_CLICK_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
            </select>
          );
        })()}
        {step.type === 'Change Volume' && (() => {
          // Value shape: `{ mode: "set"|"increase"|"decrease"|"mute", amount: 0-100 }`.
          // Amount is 0-100 for Set / Increase / Decrease (Rust clamps).
          // Mute has no amount. Backward-compat: legacy string values map
          // via Rust side; UI writes JSON.
          let cv = { mode: 'increase', amount: 5 };
          try {
            if (step.value && step.value.trim().startsWith('{')) {
              cv = { ...cv, ...JSON.parse(step.value) };
            } else if (step.value === 'up')   cv = { mode: 'increase', amount: 5 };
            else if (step.value === 'down')   cv = { mode: 'decrease', amount: 5 };
            else if (step.value === 'mute')   cv = { mode: 'mute', amount: 0 };
          } catch (_) {}
          const showAmount = cv.mode !== 'mute';
          const write = (next) => updateStep({ ...step, value: JSON.stringify(next) });
          return (
            <>
              <select
                className="form-select macro-step-value"
                // Shrinkable — prefers 105px so "Decrease" reads fully, but
                // yields down to 65px under row pressure so the duplicate /
                // close buttons stay inside the 415 column.
                style={{ flex: '0 1 105px', minWidth: 65 }}
                value={cv.mode}
                onChange={e => {
                  const nextMode = e.target.value;
                  // Reset amount to a sensible default when switching modes.
                  const nextAmount = nextMode === 'mute' ? 0 : (nextMode === 'set' ? 50 : 5);
                  write({ mode: nextMode, amount: nextAmount });
                }}
              >
                {VOLUME_MODE_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
              </select>
              {showAmount && (
                <>
                  <NumberField
                    value={cv.amount}
                    min={0}
                    max={100}
                    defaultOnEmpty={0}
                    onCommit={n => write({ ...cv, amount: n })}
                    title="Volume 0-100"
                    // Wider than the default step-repeat-input (36px) so
                    // 3-digit values like "100" render without truncation.
                    style={{ width: 72 }}
                  />
                  <span className="macro-step-hint" style={{ marginLeft: 4 }}>%</span>
                </>
              )}
            </>
          );
        })()}
        {step.type === 'Change Audio Output' && (() => {
          // Value shape: `{ deviceId: string, deviceName: string }`. deviceId
          // is Windows' endpoint ID (e.g. `{0.0.0.00000000}.{guid}`), stable
          // across reboots and used at fire time; deviceName is display-only.
          let cao = { deviceId: '', deviceName: '' };
          try { if (step.value?.startsWith('{')) cao = { ...cao, ...JSON.parse(step.value) }; } catch (_) {}
          const write = (next) => updateStep({ ...step, value: JSON.stringify(next) });
          // If the selected device is no longer in the enumerated list (unplugged,
          // renamed, disabled), keep it in the dropdown so the user can see what
          // was pinned — flagged as "(not connected)". Fire-time enforcement lives
          // in the Rust arm; UI here is descriptive, not prescriptive.
          const list = audioDevices || [];
          const selectedIsMissing = cao.deviceId && !list.some(d => d.id === cao.deviceId);
          return (
            <>
              <select
                className="form-select macro-step-value"
                style={{ flex: '1 1 200px', minWidth: 140 }}
                value={cao.deviceId}
                onChange={e => {
                  const id = e.target.value;
                  const found = list.find(d => d.id === id);
                  write({ deviceId: id, deviceName: found?.friendlyName || cao.deviceName });
                }}
                title={selectedIsMissing ? `Pinned device "${cao.deviceName}" is not currently connected` : ''}
              >
                {audioDevices === null && <option value="">Loading devices…</option>}
                {audioDevices !== null && list.length === 0 && <option value="">No output devices found</option>}
                {audioDevices !== null && list.length > 0 && !cao.deviceId && <option value="">Choose a device…</option>}
                {selectedIsMissing && (
                  <option value={cao.deviceId}>{cao.deviceName || cao.deviceId} (not connected)</option>
                )}
                {list.map(d => (
                  <option key={d.id} value={d.id}>
                    {d.friendlyName}{d.isDefault ? ' — current default' : ''}
                  </option>
                ))}
              </select>
              <button
                type="button"
                className="macro-step-refresh-btn"
                onClick={refreshAudioDevices}
                title="Refresh device list"
                style={{ flex: '0 0 auto' }}
              >⟳</button>
            </>
          );
        })()}
        {step.type === 'Mouse Scroll' && (() => {
          let ms = { direction: 'down', amount: 3 };
          try { ms = { ...ms, ...JSON.parse(step.value || '{}') }; } catch (_) {}
          return (
            <>
              <select
                className="form-select macro-step-value"
                // Compact + shrinkable — prefers 80px so "Right" reads
                // fully, floors at 55 so the buttons never get pushed out.
                style={{ flex: '0 1 80px', minWidth: 55 }}
                value={ms.direction}
                onChange={e => updateStep({ ...step, value: JSON.stringify({ ...ms, direction: e.target.value }) })}
              >
                {SCROLL_DIRECTION_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
              </select>
              <NumberField
                value={ms.amount}
                min={1}
                max={999}
                defaultOnEmpty={1}
                onCommit={n => updateStep({ ...step, value: JSON.stringify({ ...ms, amount: n }) })}
                // Wider (60px) so 3-digit totals like "120" fit clearly —
                // the ×repeat multiplier was removed so this field alone
                // controls the total notches sent per fire.
                style={{ width: 60 }}
                title="Total notches per fire"
              />
            </>
          );
        })()}
        {step.type === 'Type Text' && (
          <input
            className="form-input macro-step-value"
            placeholder="Text to type..."
            value={step.value || ''}
            onChange={e => updateStep({ ...step, value: e.target.value })}
          />
        )}
        {step.type === 'Dynamic Text' && (
          <MacroDynamicTextValue
            value={step.value || ''}
            onChange={v => updateStep({ ...step, value: v })}
          />
        )}
        {step.type === 'Record Macro' && (
          <ReplayRecordingValue
            value={step.value || ''}
            onChange={v => updateStep({ ...step, value: v })}
            isPro={isPro}
            onShowUpgrade={onShowUpgrade}
            assignments={assignments}
            profiles={profiles}
          />
        )}
        {(step.type === 'Fire Trigger' || step.type === 'Fire Text Expansion') && (() => {
          const mode = step.type === 'Fire Trigger' ? 'trigger' : 'expansion';
          // Resolve the friendly label for the currently selected target.
          // null result = missing (deleted/renamed) → red chip + warn at runtime.
          let label = null;
          let isMissing = false;
          if (stepValue) {
            if (mode === 'expansion') {
              const entry = assignments?.[`GLOBAL::EXPANSION::${stepValue}`];
              if (entry) {
                label = entry.data?.displayName || `:${stepValue}`;
              } else {
                isMissing = true;
              }
            } else {
              const entry = assignments?.[stepValue];
              if (entry) {
                const parsed = parseAssignmentKey(stepValue);
                label = entry.label || (parsed ? formatCombo(parsed.combo, parsed.keyId) : stepValue);
              } else {
                isMissing = true;
              }
            }
          }
          const placeholder = mode === 'expansion' ? 'Choose a text expansion…' : 'Choose a trigger…';
          return (
            <button
              type="button"
              className={`fire-target-chip${stepValue ? ' fire-target-chip-set' : ''}${isMissing ? ' fire-target-chip-missing' : ''}`}
              onClick={() => setFirePickerMode(mode)}
              title={isMissing ? `Missing: ${stepValue}` : (label || placeholder)}
            >
              <span className="fire-target-chip-label">
                {isMissing ? `Missing: ${stepValue}` : (label || placeholder)}
              </span>
              <span className="fire-target-chip-caret" aria-hidden="true">▾</span>
            </button>
          );
        })()}
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
          // New default: trigger = 'pressRelease' (was 'press'). Labelled
          // trigger select now sits inline next to the inputType select
          // rather than in the sub-row; the sub-row keeps only the
          // Specific Key capture (which still needs its own row when active).
          let wfi = { inputType: 'LButton', trigger: 'pressRelease', specificKey: '' };
          try { wfi = { ...wfi, ...JSON.parse(step.value || '{}') }; } catch (_) {}
          const updateWfi = (patch) => {
            const next = { ...wfi, ...patch };
            if (patch.inputType && patch.inputType !== 'SpecificKey') next.specificKey = '';
            updateStep({ ...step, value: JSON.stringify(next) });
          };
          return (
            <>
              <select
                className="form-select"
                // Shrinkable so the row's total width can never exceed 415 —
                // the flex algorithm compresses this + the trigger select
                // proportionally rather than pushing the duplicate / close
                // buttons out of the visible column. Prefers 90px, floors at 55.
                style={{ flex: '0 1 90px', minWidth: 55 }}
                value={wfi.inputType}
                onChange={e => updateWfi({ inputType: e.target.value })}
              >
                {WFI_INPUT_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
              </select>
              <select
                className="form-select"
                // Same rationale — prefers 120px, floors at 70.
                style={{ flex: '0 1 120px', minWidth: 70 }}
                value={wfi.trigger}
                onChange={e => updateWfi({ trigger: e.target.value })}
              >
                {WFI_TRIGGER_OPTIONS.map(o => <option key={o.value} value={o.value}>{o.label}</option>)}
              </select>
            </>
          );
        })()}
        {step.type === 'Click at Position' && (
          <ClickPositionButtonSelect step={step} updateStep={updateStep} />
        )}
        {step.type === 'Wait for Window' && (
          <WindowPicker
            value={waitWindowData}
            onChange={next => updateStep({ ...step, value: JSON.stringify(next) })}
          />
        )}
        {step.type === 'Focus Window' && (
          <WindowPicker
            value={focusData}
            onChange={next => updateStep({ ...step, value: JSON.stringify(next) })}
          />
        )}
        {(step.type === 'Minimise Window' || step.type === 'Maximise Window') && (
          <WindowPicker
            value={minmaxData}
            onChange={next => updateStep({ ...step, value: JSON.stringify(next) })}
          />
        )}
        {step.type === 'Resize Window' && (
          <WindowPicker
            value={{ process: resizeData.process, title: resizeData.title }}
            onChange={next => updateStep({ ...step, value: JSON.stringify({ ...resizeData, ...next }) })}
          />
        )}
        {/* Open App / Open Folder / Play Audio / Play Video — picker button
            + monitor picker inline. Each opens a different browse dialog on
            click; the button label shows the picked target (or a placeholder).
            The old separate readonly path input + Browse button + monitor
            picker sub-row was collapsed here so all three read as one control. */}
        {step.type === 'Open App' && (
          <MacroOpenAppInlinePicker
            appData={appData}
            updateValue={(next) => updateStep({ ...step, value: JSON.stringify(next) })}
          />
        )}
        {step.type === 'Open Folder' && (
          <button
            type="button"
            className="picker-field"
            style={{ flex: '1 1 60px', minWidth: 60 }}
            onClick={async () => {
              const path = await window.electronAPI?.browseForFolder();
              if (path) updateStep({ ...step, value: JSON.stringify({ ...folderData, path }) });
            }}
            title={folderData.path || 'Pick a folder…'}
          >
            <span className={`picker-field-value${folderData.path ? '' : ' picker-field-placeholder'}`}>
              {folderData.path || 'Pick a folder…'}
            </span>
            <span className="picker-field-caret" aria-hidden="true">▾</span>
          </button>
        )}
        {(step.type === 'Play Audio File' || step.type === 'Play Video File') && (
          <button
            type="button"
            className="picker-field"
            style={{ flex: '1 1 60px', minWidth: 60 }}
            onClick={async () => {
              const path = step.type === 'Play Audio File'
                ? await window.electronAPI?.browseForAudio()
                : await window.electronAPI?.browseForVideo();
              if (path) updateStep({ ...step, value: JSON.stringify({ ...mediaData, path }) });
            }}
            title={mediaData.path || (step.type === 'Play Audio File' ? 'Pick an audio file…' : 'Pick a video file…')}
          >
            <span className={`picker-field-value${mediaData.path ? '' : ' picker-field-placeholder'}`}>
              {mediaData.path || (step.type === 'Play Audio File' ? 'Pick an audio file…' : 'Pick a video file…')}
            </span>
            <span className="picker-field-caret" aria-hidden="true">▾</span>
          </button>
        )}
        {/* Create Folder — name inline; location (current Explorer folder vs
            picked path) lives in the sub-row. Tokens resolve at run time. */}
        {step.type === 'Create Folder' && (
          <input
            className="form-input macro-step-value"
            style={{ flex: '1 1 60px', minWidth: 60 }}
            placeholder={createFolderData.promptForName
              ? 'Default name offered in the popup (optional)'
              : 'Folder name — {date} and {inc} tokens work'}
            title="{inc} numbers against existing contents: Report {inc:3} becomes Report 007"
            value={createFolderData.name || ''}
            onChange={e => updateStep({ ...step, value: JSON.stringify({ ...createFolderData, name: e.target.value }) })}
          />
        )}
        {/* Sort Files — summary + Configure button; the full rules form
            lives in a portaled modal. */}
        {step.type === 'Sort Files' && (
          <>
            <button
              type="button"
              className="picker-field"
              style={{ flex: '1 1 60px', minWidth: 60 }}
              onClick={() => setSortConfigOpen(true)}
              title="Configure sorting rules"
            >
              <span className={`picker-field-value${sortData.rootPath ? '' : ' picker-field-placeholder'}`}>
                {sortData.rootPath
                  ? `${sortData.rootPath.split(/[\\/]/).pop()} · ${sortData.routeEnabled
                      ? `${sortData.mappings.filter(m => m.code && m.folder).length} rule(s)`
                      : 'no subfolder routing'}`
                  : 'Configure sorting rules…'}
              </span>
              <span className="picker-field-caret" aria-hidden="true">▾</span>
            </button>
            {sortConfigOpen && (
              <SortFilesConfigModal
                initial={sortData}
                onClose={() => setSortConfigOpen(false)}
                onSave={(next) => {
                  updateStep({ ...step, value: JSON.stringify(next) });
                  setSortConfigOpen(false);
                }}
              />
            )}
          </>
        )}
        {/* Copy / Move Files — source mode inline; paths + pattern in the
            sub-row. */}
        {(step.type === 'Copy Files' || step.type === 'Move Files') && (
          <select
            className="form-select macro-step-value"
            // Prefers 150px so "Selected files in Explorer" reads, yields to
            // 110px under row pressure so duplicate/remove stay inside 415.
            style={{ flex: '0 1 150px', minWidth: 110 }}
            value={fileOpData.sourceMode || 'selected'}
            onChange={e => updateStep({ ...step, value: JSON.stringify({ ...fileOpData, sourceMode: e.target.value }) })}
          >
            <option value="selected">Selected files in Explorer</option>
            <option value="folder">Files from a folder</option>
          </select>
        )}
        {[
          'Copy to Clipboard', 'Paste Clipboard', 'Select All',
          // System / no-config steps.
          'Minimise All', 'Restore All', 'Lock Computer',
          'Sleep Computer', 'Log Off', 'Shut Down Computer', 'Control Panel',
        ].includes(step.type) && (
          <span className="macro-step-hint">
            {['Sleep Computer', 'Log Off', 'Shut Down Computer'].includes(step.type)
              ? 'Prompts before firing'
              : 'No additional settings'}
          </span>
        )}
        {['Press Key', 'Click Mouse'].includes(step.type) && (
          <div className="step-repeat">
            <span className="step-repeat-label">×</span>
            <NumberField
              value={step.repeat ?? 1}
              min={1}
              max={99}
              defaultOnEmpty={1}
              onCommit={n => updateStep({ ...step, repeat: n })}
            />
          </div>
        )}
        <button className="step-duplicate" onClick={() => duplicateStep(step._id)} type="button" title="Duplicate step" aria-label="Duplicate step">
          <Copy size={13} strokeWidth={1.75} />
        </button>
        <button className="step-remove" onClick={() => removeStep(step._id)} type="button" aria-label="Remove step">✕</button>
      </div>

      {/* Sub-row: Press Key phase — Press / Hold / Release radios. Hold keeps
          the key or chord down until a later Release step; Press (default) is
          the classic full down+up pulse. Rust clamps repeat to 1 for Hold /
          Release so a stored ×N doesn't misbehave. */}
      {step.type === 'Press Key' && (
        <div className="step-phase-row">
          {PRESS_KEY_PHASE_OPTIONS.map(o => {
            const checked = (step.phase || 'full') === o.value;
            return (
              <label key={o.value} className={`step-phase-option${checked ? ' step-phase-option-checked' : ''}`}>
                <input
                  type="radio"
                  name={`phase-${step._id}`}
                  value={o.value}
                  checked={checked}
                  onChange={() => updateStep({ ...step, phase: o.value })}
                />
                <span>{o.label}{o.value === 'full' ? ' (default)' : ''}</span>
              </label>
            );
          })}
        </div>
      )}

      {/* Sub-row: Click Mouse phase — Click / Hold / Release radios. Mirror
          of the Press Key sub-row so mouse buttons follow the same mental
          model (default = full click; Hold + Release build drags or chords
          across later steps). Legacy suffixed values ("LButtonDown") are
          decoded on render; the first radio interaction migrates them to
          the clean split shape. */}
      {step.type === 'Click Mouse' && (() => {
        const { button, phase } = migrateClickMouseValue(step.value, step.phase);
        return (
          <div className="step-phase-row">
            {MOUSE_CLICK_PHASE_OPTIONS.map(o => {
              const checked = phase === o.value;
              return (
                <label key={o.value} className={`step-phase-option${checked ? ' step-phase-option-checked' : ''}`}>
                  <input
                    type="radio"
                    name={`phase-${step._id}`}
                    value={o.value}
                    checked={checked}
                    onChange={() => updateStep({ ...step, value: button, phase: o.value })}
                  />
                  <span>{o.label}{o.value === 'full' ? ' (default)' : ''}</span>
                </label>
              );
            })}
          </div>
        );
      })()}

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

      {/* Open App sub-rows — monitor picker on its own row, then args /
          advanced. Picker button + step type dropdown stay in the main row. */}
      {step.type === 'Open App' && (
        <MacroOpenAppArgsRow
          appData={appData}
          updateValue={(next) => updateStep({ ...step, value: JSON.stringify(next) })}
          advancedOpen={advancedOpen}
          toggleAdvanced={toggleAdvanced}
        />
      )}

      {/* Open Folder sub-row — monitor picker only. Path button is inline. */}
      {step.type === 'Open Folder' && (
        <div className="wfi-config-row" style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <MonitorPicker
            value={folderData.monitor || 'default'}
            onChange={(m) => updateStep({ ...step, value: JSON.stringify({ ...folderData, monitor: m }) })}
          />
        </div>
      )}

      {/* Create Folder sub-rows — location, run-time name prompt toggle,
          template seed toggle + picker. Base wfi-config-row (NOT -aligned):
          the aligned variant's 184px left padding leaves ~171px of content
          width, too narrow for multi-control rows — they overflow the 415
          column. */}
      {step.type === 'Create Folder' && (
        <div className="wfi-config-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
          {/* Location line: compact dropdown only. The Specific-folder picker
              goes on its OWN full-width line below — pickers never share a
              line with other controls (they get pushed past the 415 cap). */}
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <span className="macro-substep-label">In</span>
            <select
              className="form-select"
              style={{ flex: '0 1 150px', minWidth: 110 }}
              value={createFolderData.locationMode || 'current'}
              onChange={e => updateStep({ ...step, value: JSON.stringify({ ...createFolderData, locationMode: e.target.value }) })}
            >
              <option value="current">Current folder</option>
              <option value="custom">Specific folder</option>
            </select>
          </div>
          {createFolderData.locationMode === 'custom' && (
            <button
              type="button"
              className="picker-field"
              onClick={async () => {
                const path = await window.electronAPI?.browseForFolder();
                if (path) updateStep({ ...step, value: JSON.stringify({ ...createFolderData, path }) });
              }}
              title={createFolderData.path || 'Pick a folder…'}
            >
              <span className={`picker-field-value${createFolderData.path ? '' : ' picker-field-placeholder'}`}>
                {createFolderData.path || 'Pick a folder…'}
              </span>
              <span className="picker-field-caret" aria-hidden="true">▾</span>
            </button>
          )}
          <label
            className="macro-substep-label"
            style={{ display: 'flex', alignItems: 'center', gap: 6, cursor: 'pointer' }}
            title="Opens a Keyfire popup when the macro runs so you can type the folder name. The name above becomes the editable default. Cancelling stops the macro."
          >
            <input
              type="checkbox"
              checked={!!createFolderData.promptForName}
              onChange={e => updateStep({ ...step, value: JSON.stringify({ ...createFolderData, promptForName: e.target.checked }) })}
            />
            Ask for the name when it runs
          </label>
          <label
            className="macro-substep-label"
            style={{ display: 'flex', alignItems: 'center', gap: 6, cursor: 'pointer' }}
            title="Copies everything inside the picked folder (files and subfolders) into the new folder after it's created."
          >
            <input
              type="checkbox"
              checked={!!createFolderData.templateEnabled}
              onChange={e => updateStep({ ...step, value: JSON.stringify({ ...createFolderData, templateEnabled: e.target.checked }) })}
            />
            Copy template files into it
          </label>
          {createFolderData.templateEnabled && (
            <button
              type="button"
              className="picker-field"
              onClick={async () => {
                const path = await window.electronAPI?.browseForFolder();
                if (path) updateStep({ ...step, value: JSON.stringify({ ...createFolderData, templatePath: path }) });
              }}
              title={createFolderData.templatePath || 'Pick the template folder…'}
            >
              <span className={`picker-field-value${createFolderData.templatePath ? '' : ' picker-field-placeholder'}`}>
                {createFolderData.templatePath || 'Pick the template folder…'}
              </span>
              <span className="picker-field-caret" aria-hidden="true">▾</span>
            </button>
          )}
        </div>
      )}

      {/* Copy / Move Files sub-row — source folder + wildcard pattern (folder
          mode only), then the destination picker. Selected-files mode reads
          the Explorer selection at run time so it only needs a destination.
          Base wfi-config-row, not -aligned — see Create Folder comment. */}
      {(step.type === 'Copy Files' || step.type === 'Move Files') && (
        <div className="wfi-config-row" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
          {/* Pickers always get their OWN full-width line — sharing a line
              with other controls pushes them past the 415 cap. */}
          {fileOpData.sourceMode === 'folder' && (
            <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <span className="macro-substep-label">From — matching</span>
                <input
                  className="form-input"
                  style={{ flex: '1 1 90px', minWidth: 80 }}
                  placeholder="*.pdf; *.docx"
                  title="Wildcard pattern — * and ? match file names, separate several with ;"
                  value={fileOpData.pattern ?? '*'}
                  onChange={e => updateStep({ ...step, value: JSON.stringify({ ...fileOpData, pattern: e.target.value }) })}
                />
              </div>
              <button
                type="button"
                className="picker-field"
                onClick={async () => {
                  const path = await window.electronAPI?.browseForFolder();
                  if (path) updateStep({ ...step, value: JSON.stringify({ ...fileOpData, sourcePath: path }) });
                }}
                title={fileOpData.sourcePath || 'Pick the source folder…'}
              >
                <span className={`picker-field-value${fileOpData.sourcePath ? '' : ' picker-field-placeholder'}`}>
                  {fileOpData.sourcePath || 'Pick the source folder…'}
                </span>
                <span className="picker-field-caret" aria-hidden="true">▾</span>
              </button>
            </>
          )}
          <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
            <span className="macro-substep-label">To</span>
            <select
              className="form-select"
              style={{ flex: '0 1 190px', minWidth: 130 }}
              value={fileOpData.destMode || 'path'}
              title="Subfolder mode resolves the name against the folder the files come from at run time — the Explorer folder for selected files, the source folder otherwise"
              onChange={e => updateStep({ ...step, value: JSON.stringify({ ...fileOpData, destMode: e.target.value }) })}
            >
              <option value="path">Specific folder</option>
              <option value="subfolder">Subfolder of current folder</option>
            </select>
          </div>
          {(fileOpData.destMode || 'path') === 'path' ? (
            <button
              type="button"
              className="picker-field"
              onClick={async () => {
                const path = await window.electronAPI?.browseForFolder();
                if (path) updateStep({ ...step, value: JSON.stringify({ ...fileOpData, destPath: path }) });
              }}
              title={fileOpData.destPath || 'Pick the destination folder…'}
            >
              <span className={`picker-field-value${fileOpData.destPath ? '' : ' picker-field-placeholder'}`}>
                {fileOpData.destPath || 'Pick the destination folder…'}
              </span>
              <span className="picker-field-caret" aria-hidden="true">▾</span>
            </button>
          ) : (
            <>
              <input
                className="form-input"
                placeholder="Subfolder name, e.g. Archive"
                value={fileOpData.destSubfolder || ''}
                onChange={e => updateStep({ ...step, value: JSON.stringify({ ...fileOpData, destSubfolder: e.target.value }) })}
              />
              <label
                className="macro-substep-label"
                style={{ display: 'flex', alignItems: 'center', gap: 4, cursor: 'pointer' }}
                title="Unticked: the macro stops if the subfolder doesn't exist in the current folder. Ticked: it gets created."
              >
                <input
                  type="checkbox"
                  checked={!!fileOpData.createSubfolder}
                  onChange={e => updateStep({ ...step, value: JSON.stringify({ ...fileOpData, createSubfolder: e.target.checked }) })}
                />
                Create if missing
              </label>
            </>
          )}
        </div>
      )}

      {/* Play Audio / Play Video sub-row — monitor picker only. */}
      {(step.type === 'Play Audio File' || step.type === 'Play Video File') && (
        <div className="wfi-config-row" style={{ flexDirection: 'row', alignItems: 'center', gap: 6 }}>
          <MonitorPicker
            value={mediaData.monitor || 'default'}
            onChange={(m) => updateStep({ ...step, value: JSON.stringify({ ...mediaData, monitor: m }) })}
          />
        </div>
      )}

      {/* Sub-row: Focus Window — title input below the picker. Aligned with
          main-row inline-value start/end so the input ends flush with where
          Type Text's input would. */}
      {step.type === 'Focus Window' && (
        <div className="wfi-config-row wfi-config-row-aligned" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
          <input
            className="form-input"
            placeholder="Window title (auto-populated when you pick, or type to match)"
            value={focusData.title || ''}
            onChange={e => updateStep({ ...step, value: JSON.stringify({ ...focusData, title: e.target.value }) })}
          />
        </div>
      )}

      {/* Sub-row: Minimise / Maximise Window — title input; empty targets
          the currently-focused window. */}
      {(step.type === 'Minimise Window' || step.type === 'Maximise Window') && (
        <div className="wfi-config-row wfi-config-row-aligned" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
          <input
            className="form-input"
            placeholder="Window title (leave blank for currently-focused window)"
            value={minmaxData.title || ''}
            onChange={e => updateStep({ ...step, value: JSON.stringify({ ...minmaxData, title: e.target.value }) })}
          />
        </div>
      )}

      {/* Sub-row: Resize Window — title, width, height, optional position. */}
      {step.type === 'Resize Window' && (
        <div className="wfi-config-row wfi-config-row-aligned" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
          <input
            className="form-input"
            placeholder="Window title (leave blank for currently-focused window)"
            value={resizeData.title || ''}
            onChange={e => updateStep({ ...step, value: JSON.stringify({ ...resizeData, title: e.target.value }) })}
          />
          <div style={{ display: 'flex', gap: 8, alignItems: 'center', flexWrap: 'wrap' }}>
            <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
              <span className="macro-step-hint">Width</span>
              <NumberField
                value={resizeData.width}
                min={100}
                max={10000}
                defaultOnEmpty={100}
                onCommit={n => updateStep({ ...step, value: JSON.stringify({ ...resizeData, width: n }) })}
                style={{ width: 80 }}
              />
            </label>
            <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
              <span className="macro-step-hint">Height</span>
              <NumberField
                value={resizeData.height}
                min={100}
                max={10000}
                defaultOnEmpty={100}
                onCommit={n => updateStep({ ...step, value: JSON.stringify({ ...resizeData, height: n }) })}
                style={{ width: 80 }}
              />
            </label>
            <label style={{ display: 'flex', alignItems: 'center', gap: 4, cursor: 'pointer' }}>
              <input
                type="checkbox"
                checked={!!resizeData.usePosition}
                onChange={e => updateStep({ ...step, value: JSON.stringify({ ...resizeData, usePosition: e.target.checked }) })}
              />
              <span className="macro-step-hint">Also set position</span>
            </label>
            {resizeData.usePosition && (
              <>
                <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
                  <span className="macro-step-hint">X</span>
                  <NumberField
                    value={resizeData.x}
                    min={-32768}
                    max={32767}
                    defaultOnEmpty={0}
                    onCommit={n => updateStep({ ...step, value: JSON.stringify({ ...resizeData, x: n }) })}
                    style={{ width: 80 }}
                  />
                </label>
                <label style={{ display: 'flex', alignItems: 'center', gap: 4 }}>
                  <span className="macro-step-hint">Y</span>
                  <NumberField
                    value={resizeData.y}
                    min={-32768}
                    max={32767}
                    defaultOnEmpty={0}
                    onCommit={n => updateStep({ ...step, value: JSON.stringify({ ...resizeData, y: n }) })}
                    style={{ width: 80 }}
                  />
                </label>
              </>
            )}
          </div>
        </div>
      )}

      {/* Sub-row: Wait for Window — title input (auto-populated by picker,
          user-editable for substring matching) + hardcoded timeout note.
          Aligned variant so the title input lines up with the picker above. */}
      {step.type === 'Wait for Window' && (
        <div className="wfi-config-row wfi-config-row-aligned" style={{ flexDirection: 'column', alignItems: 'stretch', gap: 6 }}>
          <input
            className="form-input"
            placeholder="Window title (auto-populated when you pick, or type to match)"
            value={waitWindowData.title || ''}
            onChange={e => updateStep({ ...step, value: JSON.stringify({ ...waitWindowData, title: e.target.value }) })}
          />
          <span className="wait-window-timeout-note">
            Max wait: 30 seconds (hardcoded). Macro stops if the window doesn't appear in time.
          </span>
        </div>
      )}

      {/* Sub-row: Wait for Input — trigger + optional specific key */}
      {step.type === 'Wait for Input' && (() => {
        // Trigger dropdown moved to the main row inline (above). Sub-row now
        // renders only when inputType == "Specific Key" — the key capture
        // field is too wide to fit inline alongside the two selects.
        let wfi = { inputType: 'LButton', trigger: 'pressRelease', specificKey: '' };
        try { wfi = { ...wfi, ...JSON.parse(step.value || '{}') }; } catch (_) {}
        if (wfi.inputType !== 'SpecificKey') return null;
        const updateWfi = (patch) => {
          updateStep({ ...step, value: JSON.stringify({ ...wfi, ...patch }) });
        };
        return (
          <div className="wfi-config-row wfi-config-row-nowrap">
            <div className="wfi-field">
              <span className="wfi-label">Key:</span>
              <KeyCaptureInput value={wfi.specificKey || ''} onChange={v => updateWfi({ specificKey: v })} />
            </div>
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
            <div className="form-hint" style={{ marginTop: 4 }}>
              Paste your script as-is. Hotkey labels like <code>^!j::</code> are stripped automatically.
            </div>
          </div>
        );
      })()}
      {step.type === 'Click at Position' && (
        <ClickPositionFields step={step} updateStep={updateStep} />
      )}

      {firePickerMode && (
        <FireTargetPicker
          mode={firePickerMode}
          assignments={assignments || {}}
          profilesOrder={profiles}
          currentValue={step.value || ''}
          onSelect={(v) => {
            updateStep({ ...step, value: v });
            setFirePickerMode(null);
          }}
          onClose={() => setFirePickerMode(null)}
        />
      )}
    </div>
  );
}

export function MacroSequenceForm({ value, onChange, globalInputMethod, assignments, profiles, isPro = false, onShowUpgrade }) {
  const seqMethod = value.inputMethod || 'global';
  const globalLabel = INPUT_METHOD_OPTS.find(o => o.id === globalInputMethod)?.label || globalInputMethod;
  const loopCfg = value.loop || { enabled: false, mode: 'count', count: 5, delayMs: 0 };
  const updateLoop = (patch) => onChange({ ...value, loop: { ...loopCfg, ...patch } });
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

      <div className="seq-method-row">
        <label className="form-label" style={{ marginBottom: 0 }}>Loop this macro</label>
        <input
          type="checkbox"
          className="seq-loop-checkbox"
          checked={!!loopCfg.enabled}
          onChange={e => updateLoop({ enabled: e.target.checked })}
        />
      </div>
      {loopCfg.enabled && (
        <div className="seq-loop-config">
          <div className="seq-loop-mode-row">
            <label className="seq-loop-row">
              <input
                type="radio"
                name="seq-loop-mode"
                checked={loopCfg.mode !== 'forever'}
                onChange={() => updateLoop({ mode: 'count' })}
              />
              <span>Repeat</span>
              <NumberField
                className="seq-loop-count"
                min={2}
                max={9999}
                defaultOnEmpty={5}
                value={loopCfg.count ?? 5}
                disabled={loopCfg.mode === 'forever'}
                onCommit={n => updateLoop({ count: n })}
              />
              <span>times</span>
            </label>

            <label className="seq-loop-row">
              <input
                type="radio"
                name="seq-loop-mode"
                checked={loopCfg.mode === 'forever'}
                onChange={() => updateLoop({ mode: 'forever' })}
              />
              <span>Repeat until stopped</span>
            </label>
          </div>

          <label className="seq-loop-row seq-loop-delay">
            <span>Delay between iterations</span>
            <NumberField
              className="seq-loop-delay-input"
              min={0}
              max={3600000}
              step={50}
              defaultOnEmpty={0}
              value={loopCfg.delayMs ?? 0}
              onCommit={n => updateLoop({ delayMs: n })}
            />
            <span>ms</span>
          </label>

          <div className="seq-loop-hint">
            Re-press the trigger or press Esc to stop a running loop.
          </div>
        </div>
      )}

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
                isPro={isPro}
                onShowUpgrade={onShowUpgrade}
                advancedOpen={!!advancedOpen[step._id]}
                toggleAdvanced={() => setAdvancedOpen(prev => ({ ...prev, [step._id]: !prev[step._id] }))}
                assignments={assignments}
                profiles={profiles}
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

function ReassignOverlay({ currentCombo, currentKeyId, assignments, activeProfile, profileLinked, onConfirm, onCancel, title = 'Reassign Hotkey', titleIcon = '⇄', instruction = 'Press new key, combo, or mouse button…', previewVerb = 'Move to', hideCurrent = false, conflictNote = null }) {
  const [captured, setCaptured] = useState(null);
  const captureRef = useRef(null);

  useLayoutEffect(() => {
    if (!captured) captureRef.current?.focus();
  }, [captured]);

  // Shared tail for the keyboard + mouse capture paths: same-trigger silent
  // dismiss, conflict lookup across ALL press-mode variants, preview state.
  function captureTarget(newCombo, newKeyId, label) {
    if (newCombo === currentCombo && newKeyId === currentKeyId) { onCancel(); return; }
    const base = `${activeProfile}::${newCombo}::${newKeyId}`;
    const existing = assignments[base]
      || assignments[`${base}::double`]
      || assignments[`${base}::hold`]
      || null;
    setCaptured({ combo: newCombo, keyId: newKeyId, label, conflict: existing });
  }

  function collectMods(e) {
    const mods = [];
    if (e.ctrlKey)  mods.push('Ctrl');
    if (e.shiftKey) mods.push('Shift');
    if (e.altKey)   mods.push('Alt');
    if (e.metaKey)  mods.push('Win');
    mods.sort((a, b) => MOD_ORDER.indexOf(a) - MOD_ORDER.indexOf(b));
    return mods;
  }

  function handleKeyDown(e) {
    e.preventDefault();
    e.stopPropagation();
    if (e.key === 'Escape') { onCancel(); return; }
    if (['Control', 'Shift', 'Alt', 'Meta'].includes(e.key)) return;

    const mods = collectMods(e);

    // Bare key (no modifiers held) — app-linked: all keys; static: only non-character keys
    if (mods.length === 0 && !profileLinked && !STATIC_BARE_ALLOWED.has(e.code)) return;

    const newCombo = mods.length === 0 ? 'BARE' : mods.join('+');
    const keyDisplay = e.key.length === 1 ? e.key.toUpperCase() : (KEY_DISPLAY_MAP[e.key] ?? e.key);
    const label = mods.length === 0 ? keyDisplay : [...mods, keyDisplay].join('+');
    captureTarget(newCombo, e.code, label);
  }

  const MOUSE_BUTTON_IDS = { 0: 'MOUSE_LEFT', 1: 'MOUSE_MIDDLE', 2: 'MOUSE_RIGHT', 3: 'MOUSE_SIDE1', 4: 'MOUSE_SIDE2' };

  // Mouse-button capture — same rules as trigger recording: modifier + any
  // button, or bare Middle/Side. Bare Left/Right pass through so the Cancel
  // button (whose own onMouseDown preventDefaults — honoured here) and the
  // rest of the overlay stay clickable.
  function handleMouseDown(e) {
    if (captured) return;
    if (e.defaultPrevented) return;
    const newKeyId = MOUSE_BUTTON_IDS[e.button];
    if (!newKeyId) return;
    const mods = collectMods(e);
    if (mods.length === 0 && (newKeyId === 'MOUSE_LEFT' || newKeyId === 'MOUSE_RIGHT')) return;
    e.preventDefault();
    e.stopPropagation();
    const newCombo = mods.length === 0 ? 'BARE' : mods.join('+');
    const label = mods.length === 0 ? keyIdToLabel(newKeyId) : [...mods, keyIdToLabel(newKeyId)].join('+');
    captureTarget(newCombo, newKeyId, label);
  }

  // Scroll capture — modifier required (a bare scroll would capture while the
  // user just scrolls the UI). React wheel listeners are passive, so no
  // preventDefault — a cosmetic scroll of the page behind is acceptable.
  function handleWheel(e) {
    if (captured) return;
    const mods = collectMods(e);
    if (mods.length === 0) return;
    const newKeyId = e.deltaY < 0 ? 'MOUSE_SCROLL_UP' : 'MOUSE_SCROLL_DOWN';
    const label = [...mods, keyIdToLabel(newKeyId)].join('+');
    captureTarget(mods.join('+'), newKeyId, label);
  }

  const currentLabel = [
    ...(currentCombo === 'BARE' ? ['Bare'] : currentCombo ? currentCombo.split('+') : []),
    keyIdToLabel(currentKeyId),
  ].filter(Boolean);

  return (
    <div
      className="reassign-overlay"
      onMouseDown={handleMouseDown}
      onWheel={handleWheel}
      onContextMenu={e => e.preventDefault()}
    >
      <div className="reassign-panel">
        <div className="reassign-header">
          <span className="reassign-icon">{titleIcon}</span>
          <span className="reassign-title">{title}</span>
        </div>

        {!hideCurrent && (
          <div className="reassign-current">
            Currently:&nbsp;
            {currentLabel.map((k, i) => (
              <React.Fragment key={i}>
                <kbd className="reassign-kbd">{k}</kbd>
                {i < currentLabel.length - 1 && <span className="reassign-plus">+</span>}
              </React.Fragment>
            ))}
          </div>
        )}

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
              {conflictNote && <span className="reassign-conflict-note">{conflictNote}</span>}
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
  holdAssignment = null,
  draftAssignment = null,
  draftDoubleAssignment = null,
  assignments,
  activeProfile,
  profiles,
  profileLinked,
  globalInputMethod = 'direct',
  onAssign,
  onClear,
  onDelete,
  onAssignDouble,
  onClearDouble,
  onAssignHold,
  onClearHold,
  onClose,
  onCancelDraft,
  onReassign,
  onDuplicate,
  onUnassign,
  duplicateOverlaySignal = 0,
  // Unassigned library mode — selectedKey carries the entry's uuid, every
  // assign/clear callback is routed to the library keys by App, the header
  // shows an Unassigned badge and Reassign becomes "Bind to Key".
  libraryMode = false,
  bindOverlaySignal = 0,
  isPro = false,
  voiceEnabled = false,
  onShowUpgrade,
  hiddenTips = [],
  onHideTip,
}) {
  // When the user duplicates an assignment from the sidebar context menu, the
  // cloned action lives in draftAssignment until they pick a destination key.
  // Use it as a fallback so the editor shows the cloned data even before a key
  // is selected, AND once they pick an empty key slot (so the new key inherits
  // the duplicated action).
  const effectiveAssignment = assignment || draftAssignment;
  const effectiveDouble     = doubleAssignment || draftDoubleAssignment;
  // No draft variant for hold — duplicate-from-context carries single + double only.
  const effectiveHold       = holdAssignment;
  const isDraftMode         = !selectedKey && !!draftAssignment;
  // Blank keys open on the Macro editor (Rory call 2026-08-09) — saved
  // assignments and drafts still open on their own type via the effects below.
  const [activeType, setActiveType] = useState('macro');
  // Which Open sub-type (app/url/folder) the merged Open selector button
  // targets — clicking Open returns to the last-used sub-type. Tracks
  // activeType so loading a saved url/folder assignment syncs the bar.
  const [lastOpenType, setLastOpenType] = useState('app');
  useEffect(() => {
    if (OPEN_TYPE_IDS.includes(activeType)) setLastOpenType(activeType);
  }, [activeType]);
  // Same pattern for the Text group (text / expansion sub-pills).
  const [lastTextType, setLastTextType] = useState('text');
  useEffect(() => {
    if (TEXT_TYPE_IDS.includes(activeType)) setLastTextType(activeType);
  }, [activeType]);
  // Per-type form value drafts. Switching action type no longer wipes the
  // previous type's data — users can experiment freely (URL → Type Text →
  // back to URL) without losing what they typed. Only the active type's
  // entry is saved when the user hits Assign. Resets when the user picks
  // a different key (via the selectedKey useEffect below).
  const [formValuesByType, setFormValuesByType] = useState({});
  const formValue = formValuesByType[activeType] || {};
  // When set to a press mode, the next selectedKey/assignment effect run will
  // preserve activeType + pressMode (skip auto-switch). Used by handleClearAction
  // so clearing single-press Text on a key with double-press Hotkey doesn't
  // bump the user into double mode.
  const justClearedRef = useRef(null);
  const setFormValue = useCallback((updater) => {
    setFormValuesByType(prev => {
      const current = prev[activeType] || {};
      const next = typeof updater === 'function' ? updater(current) : updater;
      return { ...prev, [activeType]: next };
    });
  }, [activeType]);
  // Per-type display labels — each action type has its own label so clearing
  // one type doesn't wipe the labels of others. Backward-compat with the old
  // single-`label` field: on load, an entry without `labels` is migrated to
  // `{ [type]: label }`. On save, both `label` (active type, top-level for
  // Sidebar/Rust readers) and `labels` (full map for the editor) are written.
  const [labelByType, setLabelByType] = useState({});
  const label = labelByType[activeType] || '';
  const setLabel = (l) => setLabelByType(prev => ({ ...prev, [activeType]: l }));
  const [voicePhrases, setVoicePhrases] = useState([]);
  const [pressMode, setPressMode] = useState('single'); // 'single' | 'double' | 'hold'
  const [reassigning, setReassigning] = useState(false);
  const [duplicating, setDuplicating] = useState(false);
  const [pendingMouseSave, setPendingMouseSave] = useState(null); // macro pending global-mouse confirmation
  // Inline confirm step for the destructive footer buttons (Clear Key / Delete).
  // Value: null | 'clear' | 'delete'. Reset on key change in the effect below.
  const [confirmingAction, setConfirmingAction] = useState(null);

  useEffect(() => {
    setReassigning(false);
    setDuplicating(false);
    setPendingMouseSave(null);
    setConfirmingAction(null);
    // Seed formValuesByType from the saved assignment's `drafts` field if
    // present (persistent multi-type drafts), otherwise fall back to a
    // single-entry map of the active type's data (backward-compat for
    // assignments saved before the drafts field existed).
    const seedDrafts = (entry) => {
      const t = displayTypeOf(entry);
      if (entry.drafts && typeof entry.drafts === 'object') {
        // Ensure the active type's draft mirrors the saved data even if the
        // user edited and reverted — the assignment is the source of truth.
        return { ...entry.drafts, [t]: entry.data || entry.drafts[t] || {} };
      }
      return { [t]: entry.data || {} };
    };
    const seedLabels = (entry) => {
      const t = displayTypeOf(entry);
      // Top-level `label` always wins for the primary type — covers the
      // Sidebar right-click rename path that updates `label` only.
      if (entry.labels && typeof entry.labels === 'object') {
        const out = { ...entry.labels };
        if (entry.label) out[t] = entry.label;
        return out;
      }
      return entry.label ? { [t]: entry.label } : {};
    };
    // If the user just hit Clear Action, preserve activeType + pressMode and
    // skip the auto-switch-to-double behaviour. Just sync formValuesByType /
    // labelByType / voicePhrases against whichever press mode the user was on.
    const justClearedMode = justClearedRef.current;
    justClearedRef.current = null;
    if (justClearedMode) {
      const activeRecord = justClearedMode === 'double' ? effectiveDouble
        : justClearedMode === 'hold' ? effectiveHold
        : effectiveAssignment;
      if (activeRecord) {
        setFormValuesByType(seedDrafts(activeRecord));
        setLabelByType(seedLabels(activeRecord));
        setVoicePhrases(readVoicePhrases(activeRecord.data));
      } else {
        setFormValuesByType({});
        setLabelByType({});
        setVoicePhrases([]);
      }
      return;
    }
    // Auto-switch to double/hold mode when only that assignment exists
    if (!effectiveAssignment && effectiveDouble) {
      const t = displayTypeOf(effectiveDouble);
      setPressMode('double');
      setActiveType(t);
      setFormValuesByType(seedDrafts(effectiveDouble));
      setLabelByType(seedLabels(effectiveDouble));
      setVoicePhrases(readVoicePhrases(effectiveDouble.data));
    } else if (!effectiveAssignment && effectiveHold) {
      const t = displayTypeOf(effectiveHold);
      setPressMode('hold');
      setActiveType(t);
      setFormValuesByType(seedDrafts(effectiveHold));
      setLabelByType(seedLabels(effectiveHold));
      setVoicePhrases(readVoicePhrases(effectiveHold.data));
    } else {
      setPressMode('single');
      if (effectiveAssignment) {
        const t = displayTypeOf(effectiveAssignment);
        setActiveType(t);
        setFormValuesByType(seedDrafts(effectiveAssignment));
        setLabelByType(seedLabels(effectiveAssignment));
        setVoicePhrases(readVoicePhrases(effectiveAssignment.data));
      } else {
        setActiveType('macro');
        setFormValuesByType({});
        setLabelByType({});
        setVoicePhrases([]);
      }
    }
  }, [selectedKey, effectiveAssignment, effectiveDouble, effectiveHold]);

  // Sidebar right-click → Duplicate: App selects the source item then bumps
  // this signal to open the duplicate-capture overlay. Declared AFTER the
  // selection-reset effect above so its setDuplicating(true) wins over the
  // reset's setDuplicating(false) when both fire in the same commit.
  useEffect(() => {
    if (duplicateOverlaySignal > 0 && selectedKey) setDuplicating(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [duplicateOverlaySignal]);

  // Sidebar "Bind to key…" on an unassigned entry: App selects the entry
  // then bumps this signal to open the bind-capture overlay (the reassign
  // overlay in bind clothing).
  useEffect(() => {
    if (bindOverlaySignal > 0 && libraryMode && selectedKey) setReassigning(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bindOverlaySignal]);

  // When press mode switches, load the appropriate assignment's form values
  useEffect(() => {
    const seedDrafts = (entry) => {
      const t = displayTypeOf(entry);
      if (entry.drafts && typeof entry.drafts === 'object') {
        return { ...entry.drafts, [t]: entry.data || entry.drafts[t] || {} };
      }
      return { [t]: entry.data || {} };
    };
    const seedLabels = (entry) => {
      const t = displayTypeOf(entry);
      // Top-level `label` always wins for the primary type — covers the
      // Sidebar right-click rename path that updates `label` only.
      if (entry.labels && typeof entry.labels === 'object') {
        const out = { ...entry.labels };
        if (entry.label) out[t] = entry.label;
        return out;
      }
      return entry.label ? { [t]: entry.label } : {};
    };
    const record = pressMode === 'double' ? effectiveDouble
      : pressMode === 'hold' ? effectiveHold
      : effectiveAssignment;
    if (record) {
      const t = displayTypeOf(record);
      setActiveType(t);
      setFormValuesByType(seedDrafts(record));
      setLabelByType(seedLabels(record));
      setVoicePhrases(readVoicePhrases(record.data));
    } else {
      setActiveType('macro');
      setFormValuesByType({});
      setLabelByType({});
      setVoicePhrases([]);
    }
  // eslint-disable-next-line
  }, [pressMode]);

  // Clear the action of the currently-active type ONLY. Other types'
  // drafts on this key are preserved. Persists to the saved assignment so
  // the cleared state survives close + reopen.
  //
  // Two cases:
  //  - Active type IS the saved primary type: wipe its data. If drafts
  //    exist, keep the assignment with empty data + drafts. If no drafts
  //    remain, delete the assignment entirely (nothing left to keep).
  //  - Active type is a DRAFT (not the saved primary): remove only that
  //    draft entry from the saved assignment's `drafts` map. The primary
  //    action is left intact.
  // For wiping ALL types on a key, use Clear Key.
  const handleClearAction = () => {
    // Tell the assignment-effect to preserve activeType + pressMode after
    // the parent state update lands. Without this, clearing single-press on
    // a key that also has a double-press assignment would auto-switch the
    // editor into double mode, and clearing the primary on a key would jump
    // the user to a different tab.
    justClearedRef.current = pressMode;
    setFormValuesByType(prev => ({ ...prev, [activeType]: {} }));

    if (!selectedKey) {
      setLabel('');
      setVoicePhrases([]);
      return;
    }
    const activeRecord = pressMode === 'double' ? doubleAssignment
      : pressMode === 'hold' ? holdAssignment
      : assignment;
    if (!activeRecord) {
      setLabel('');
      setVoicePhrases([]);
      return;
    }

    // Build the labels map for the modified assignment: take the latest
    // labelByType minus the type being cleared. Empty strings are dropped.
    const labelsToSave = {};
    for (const [t, l] of Object.entries(labelByType)) {
      if (t === activeType) continue;
      const trimmed = (l || '').trim();
      if (trimmed) labelsToSave[t] = trimmed;
    }

    if (displayTypeOf(activeRecord) === activeType) {
      // Clearing the saved primary action.
      setLabel('');
      setVoicePhrases([]);
      const drafts = activeRecord.drafts || {};
      const hasFilledDrafts = Object.entries(drafts).some(([t, d]) => isDraftFilled(t, d));
      if (!hasFilledDrafts) {
        if (pressMode === 'double') onClearDouble?.(selectedKey);
        else if (pressMode === 'hold') onClearHold?.(selectedKey);
        else onClear?.(selectedKey);
      } else {
        const newMacro = { type: activeType === RECORD_TYPE_ID ? 'macro' : activeType, label: '', data: {}, drafts };
        if (Object.keys(labelsToSave).length > 0) newMacro.labels = labelsToSave;
        if (pressMode === 'double') onAssignDouble?.(selectedKey, newMacro);
        else if (pressMode === 'hold') onAssignHold?.(selectedKey, newMacro);
        else onAssign?.(selectedKey, newMacro);
      }
    } else {
      // Clearing a non-primary draft. Keep voicePhrases (they belong to the
      // primary). Skip the save if this type had neither a draft nor a label.
      const hadDraft = !!activeRecord.drafts?.[activeType];
      const hadLabel = !!activeRecord.labels?.[activeType];
      if (!hadDraft && !hadLabel) return;
      const newDrafts = { ...(activeRecord.drafts || {}) };
      delete newDrafts[activeType];
      const newMacro = { ...activeRecord };
      if (Object.keys(newDrafts).length > 0) newMacro.drafts = newDrafts;
      else delete newMacro.drafts;
      // Primary's label stays top-level; just rewrite the labels map without
      // the cleared draft's entry.
      if (Object.keys(labelsToSave).length > 0) newMacro.labels = labelsToSave;
      else delete newMacro.labels;
      if (pressMode === 'double') onAssignDouble?.(selectedKey, newMacro);
      else if (pressMode === 'hold') onAssignHold?.(selectedKey, newMacro);
      else onAssign?.(selectedKey, newMacro);
    }
  };

  // Returns true if the draft has user-meaningful content for its action type.
  // Used to skip empty drafts on save (no point persisting `{}`) and to show
  // the small dot indicator on type buttons that have stashed content.
  const isDraftFilled = (type, d) => {
    if (!d || typeof d !== 'object') return false;
    switch (type) {
      case 'text':      return !!d.text?.trim();
      case 'expansion': return !!d.trigger?.trim();
      // Bare modifier (Ctrl / Shift / Alt / Win alone) is valid: capture
      // returns modifiers without a main key, and the backend treats key="" +
      // non-empty modifiers as a modifier-only chord.
      case 'hotkey':    return !!d.key || (d.modifiers || []).length > 0;
      case 'app':       return !!(d.path?.trim() || d.appId?.trim());
      case 'folder':    return !!d.path?.trim();
      case 'url':       return !!d.url?.trim();
      case 'macro':     return (d.steps || []).length > 0;
      case 'recordmacro': return !!d.steps?.[0]?.value;
      case 'ahk':       return !!d.script?.trim();
      default:          return false;
    }
  };

  const handleSave = () => {
    if (!selectedKey) return;

    const data = { ...formValue };
    // Empty list removes both new and legacy voice phrase fields.
    writeVoicePhrases(data, voicePhrases);

    // Persist non-empty drafts for OTHER types so users can switch action
    // types later without retyping. Strip empties to keep config.json lean.
    // The active type's draft is omitted here — `data` above is the source
    // of truth; the seedDrafts helper restores it as drafts[activeType] on
    // next load.
    const persistedDrafts = {};
    for (const [t, v] of Object.entries(formValuesByType)) {
      if (t !== activeType && isDraftFilled(t, v)) {
        persistedDrafts[t] = v;
      }
    }

    // Per-type labels. Always include the active type's resolved label
    // (user-typed or auto-generated) so reopening the editor shows what the
    // Sidebar/Rust display. Other types' labels are saved only if user-typed.
    const resolvedActiveLabel = (label || '').trim() || getAutoLabel();
    const persistedLabels = {};
    for (const [t, l] of Object.entries(labelByType)) {
      const trimmed = (l || '').trim();
      if (trimmed) persistedLabels[t] = trimmed;
    }
    persistedLabels[activeType] = resolvedActiveLabel;

    const macro = {
      // Record Macro is a UI-only view over a one-step macro — the stored
      // type must stay 'macro' so config, Rust and every other reader are
      // untouched. displayTypeOf() reverses this on load.
      type: activeType === RECORD_TYPE_ID ? 'macro' : activeType,
      label: resolvedActiveLabel,
      data,
    };
    if (Object.keys(persistedDrafts).length > 0) {
      macro.drafts = persistedDrafts;
    }
    if (Object.keys(persistedLabels).length > 0) {
      macro.labels = persistedLabels;
    }

    if (pressMode === 'double') {
      onAssignDouble?.(selectedKey, macro);
      return;
    }
    if (pressMode === 'hold') {
      onAssignHold?.(selectedKey, macro);
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
      case 'expansion': {
        const trig = formValue.trigger?.trim();
        if (!trig) return 'Fire expansion';
        const entry = assignments?.[`GLOBAL::EXPANSION::${trig}`];
        const name = entry?.data?.displayName;
        return `Fire: ${name || `:${trig}`}`;
      }
      case 'hotkey': {
        const mouseOpt = MOUSE_CLICK_OPTIONS.find(o => o.value === formValue.key);
        if (mouseOpt && (!formValue.modifiers || formValue.modifiers.length === 0)) return mouseOpt.label;
        return [...(formValue.modifiers || []), formValue.key].filter(Boolean).join('+') || 'Key combo';
      }
      case 'app':    return formValue.appName || formValue.path?.split('\\').pop() || (formValue.appId ? 'Installed app' : 'Application');
      case 'folder': return formValue.folderName || formValue.path?.split('\\').pop() || 'Folder';
      case 'url':    return formValue.urlName || formValue.url || 'URL';
      case 'macro':  return `Macro (${(formValue.steps || []).length} steps)`;
      case 'recordmacro': {
        const summary = summariseRecording(formValue.steps?.[0]?.value);
        return summary ? `Recorded macro (${summary.duration})` : 'Recorded macro';
      }
      case 'ahk':    return 'AHK Script';
      default:       return 'Action';
    }
  };

  const isValid = () => {
    switch (activeType) {
      case 'text':      return !!formValue.text?.trim();
      case 'expansion': return !!formValue.trigger?.trim();
      // Bare modifier (Ctrl / Shift / Alt / Win alone) is valid — see isDraftFilled comment.
      case 'hotkey':    return !!formValue.key || (formValue.modifiers || []).length > 0;
      case 'app':       return !!(formValue.path?.trim() || formValue.appId?.trim());
      case 'folder':    return !!formValue.path?.trim();
      case 'url':       return !!formValue.url?.trim();
      case 'macro':     return (formValue.steps || []).length > 0;
      case 'recordmacro': return !!formValue.steps?.[0]?.value;
      case 'ahk':       return !!formValue.script?.trim();
      default:          return false;
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
          title={libraryMode
            ? 'Bind to Key'
            : [assignment, doubleAssignment, holdAssignment].filter(Boolean).length > 1 ? 'Reassign Hotkey (all press modes)' : 'Reassign Hotkey'}
          titleIcon={libraryMode ? '⊕' : '⇄'}
          instruction={libraryMode ? 'Press the key, combo, or mouse button to bind this action to…' : 'Press new key, combo, or mouse button…'}
          previewVerb={libraryMode ? 'Bind to' : 'Move to'}
          hideCurrent={libraryMode}
          conflictNote={libraryMode ? "The key's current action will move to Unassigned, not be deleted." : null}
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
          instruction="Press new key, combo, or mouse button for the duplicate…"
          previewVerb="Duplicate to"
          hideCurrent={libraryMode}
          conflictNote={libraryMode ? "The key's current action will move to Unassigned, not be deleted." : null}
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
          {libraryMode ? (
            <kbd className="selected-key-badge selected-key-badge-unassigned">Unassigned</kbd>
          ) : selectedKey ? (
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
          {/* Any saved variant (single / double / hold) can be moved — the
              handler carries all three suffixes. Mouse triggers included:
              the overlay captures mouse destinations too. */}
          {(assignment || doubleAssignment || holdAssignment) && (
            <button
              className={`reassign-btn${libraryMode ? ' bind-btn' : ''}`}
              onClick={() => setReassigning(true)}
              title={libraryMode ? 'Bind this action to a key, combo, or mouse button' : 'Move this macro to a different trigger'}
              type="button"
            >
              {libraryMode ? '⊕ Bind to Key' : '⇄ Reassign'}
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
          {/* Hold applies to keyboard keys and mouse BUTTONS (engine arms
              ::hold for mouse ids since the mouse-hold work, 2026-07-28).
              Scroll zones are excluded — a wheel tick has no release event
              to time a hold against. */}
          {!selectedKey?.startsWith('MOUSE_SCROLL') && (
            <button
              className={`press-mode-btn${pressMode === 'hold' ? ' active' : ''}`}
              onClick={() => {
                if (!isPro) { onShowUpgrade?.('Hold trigger'); return; }
                setPressMode('hold');
              }}
              type="button"
              title="Fires when the key is held past the threshold (set in Settings)"
            >
              ⏱ Hold <span className="pro-badge">PRO</span>
              {holdAssignment && <span className="press-mode-dot" />}
            </button>
          )}
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
        {/* Action type selector — App/URL/Folder share one Open button,
            Text/Expansion share one Text button. */}
        <div className="type-selector">
          {ACTION_TYPES.map(type => {
            if (OPEN_TYPE_IDS.includes(type.id)) {
              // Render the merged Open button at the first Open slot only.
              if (type.id !== OPEN_TYPE_IDS[0]) return null;
              const openType = ACTION_TYPES.find(t => t.id === lastOpenType) || type;
              const OpenIcon = openType.Icon;
              const isOpenActive = OPEN_TYPE_IDS.includes(activeType);
              const hasDraft = !isOpenActive
                && OPEN_TYPE_IDS.some(id => isDraftFilled(id, formValuesByType[id]));
              return (
                <button
                  key="open"
                  className={`type-btn ${isOpenActive ? 'active' : ''}`}
                  onClick={() => setActiveType(lastOpenType)}
                  type="button"
                >
                  <span className="type-btn-icon"><OpenIcon size={18} strokeWidth={1.75} /></span>
                  <span className="type-btn-label">Open</span>
                  {hasDraft && <span className="type-btn-draft-dot" aria-label="Has saved draft" />}
                </button>
              );
            }
            if (TEXT_TYPE_IDS.includes(type.id)) {
              // Render the merged Text button at the first Text slot only.
              if (type.id !== TEXT_TYPE_IDS[0]) return null;
              const textType = ACTION_TYPES.find(t => t.id === lastTextType) || type;
              const TextIcon = textType.Icon;
              const isTextActive = TEXT_TYPE_IDS.includes(activeType);
              const hasDraft = !isTextActive
                && TEXT_TYPE_IDS.some(id => isDraftFilled(id, formValuesByType[id]));
              return (
                <button
                  key="text-group"
                  className={`type-btn ${isTextActive ? 'active' : ''}`}
                  onClick={() => setActiveType(lastTextType)}
                  type="button"
                >
                  <span className="type-btn-icon"><TextIcon size={18} strokeWidth={1.75} /></span>
                  <span className="type-btn-label">Text</span>
                  {hasDraft && <span className="type-btn-draft-dot" aria-label="Has saved draft" />}
                </button>
              );
            }
            const TypeIcon = type.Icon;
            // Show a dot when this type has a non-empty draft stashed but isn't
            // currently active — signals "you have content here" without
            // forcing the user to click each type to discover it.
            const hasDraft = type.id !== activeType && isDraftFilled(type.id, formValuesByType[type.id]);
            return (
              <button
                key={type.id}
                className={`type-btn ${activeType === type.id ? 'active' : ''}${(type.id === 'macro' || type.id === 'recordmacro') ? ' type-btn-half' : ''}`}
                onClick={() => setActiveType(type.id)}
                type="button"
              >
                <span className="type-btn-icon"><TypeIcon size={18} strokeWidth={1.75} /></span>
                <span className="type-btn-label">{type.label}</span>
                {hasDraft && <span className="type-btn-draft-dot" aria-label="Has saved draft" />}
              </button>
            );
          })}
        </div>

        {/* Sub-pill bar — shown while a grouped button (Open or Text) is active */}
        {OPEN_TYPE_IDS.includes(activeType) && (
          <div className="type-subtype-bar">
            {OPEN_TYPE_IDS.map(id => {
              const t = ACTION_TYPES.find(x => x.id === id);
              const SubIcon = t.Icon;
              const hasDraft = id !== activeType && isDraftFilled(id, formValuesByType[id]);
              return (
                <button
                  key={id}
                  className={`type-subtype-btn${activeType === id ? ' active' : ''}`}
                  onClick={() => setActiveType(id)}
                  type="button"
                >
                  <SubIcon size={13} strokeWidth={1.75} />
                  {t.label}
                  {hasDraft && <span className="press-mode-dot" />}
                </button>
              );
            })}
          </div>
        )}
        {TEXT_TYPE_IDS.includes(activeType) && (
          <div className="type-subtype-bar">
            {TEXT_TYPE_IDS.map(id => {
              const t = ACTION_TYPES.find(x => x.id === id);
              const SubIcon = t.Icon;
              const hasDraft = id !== activeType && isDraftFilled(id, formValuesByType[id]);
              return (
                <button
                  key={id}
                  className={`type-subtype-btn${activeType === id ? ' active' : ''}`}
                  onClick={() => setActiveType(id)}
                  type="button"
                >
                  <SubIcon size={13} strokeWidth={1.75} />
                  {t.label}
                  {hasDraft && <span className="press-mode-dot" />}
                </button>
              );
            })}
          </div>
        )}
        {activeType === 'ahk' && (() => {
          const ahkValue = formValuesByType.ahk || {};
          const ahkVer = ahkValue.ahkVersion || 'v1';
          return (
            <div className="type-subtype-bar">
              {['v1', 'v2'].map(v => (
                <button
                  key={v}
                  className={`type-subtype-btn${ahkVer === v ? ' active' : ''}`}
                  onClick={() => setFormValue(prev => ({ ...prev, ahkVersion: v }))}
                  type="button"
                >
                  AHK {v}
                </button>
              ))}
            </div>
          );
        })()}

        {/* Action-type explainer — gold TIP box matching the expansions /
            radial / templates treatment, dismissible via hiddenTips
            ('action-type' key, restored from Settings like the others). */}
        {!hiddenTips.includes('action-type') && (
          <div className="mp-type-tip">
            <span className="mp-type-tip-badge">TIP</span>
            <span className="mp-type-tip-text">
              {ACTION_TYPES.find(t => t.id === activeType)?.desc}
            </span>
            <button
              type="button"
              className="mp-type-tip-close"
              title="Hide this tip (restore in Settings)"
              aria-label="Hide this tip"
              onClick={() => onHideTip?.('action-type')}
            >&#10005;</button>
          </div>
        )}

        <div className="type-selector-separator" aria-hidden="true" />

        {/* Display label — kept at the top of the editing area so the field
            lives in the same place regardless of which action type is active.
            Placeholder shows the auto-derived label for the current type. */}
        <div className="form-section">
          <label className="form-label">Display label</label>
          <input
            className="form-input"
            placeholder={getAutoLabel() || 'Short label for this key...'}
            value={label}
            onChange={e => setLabel(e.target.value)}
            onKeyDown={e => e.stopPropagation()}
          />
        </div>

        {/* Dynamic form */}
        <div className="form-body">
          {activeType === 'text'      && <TextForm value={formValue} onChange={setFormValue} globalInputMethod={globalInputMethod} />}
          {activeType === 'expansion' && <ExpansionForm value={formValue} onChange={setFormValue} assignments={assignments} />}
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
                    <NumberField
                      className="form-input repeat-interval-input"
                      min={50}
                      max={60000}
                      defaultOnEmpty={50}
                      value={formValue.repeatInterval ?? 100}
                      onCommit={n => setFormValue(prev => ({ ...prev, repeatInterval: n }))}
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
          {activeType === 'macro'  && <MacroSequenceForm value={formValue} onChange={setFormValue} globalInputMethod={globalInputMethod} assignments={assignments} profiles={profiles} isPro={isPro} onShowUpgrade={onShowUpgrade} />}
          {activeType === 'recordmacro' && <RecordMacroForm value={formValue} onChange={setFormValue} isPro={isPro} onShowUpgrade={onShowUpgrade} assignments={assignments} profiles={profiles} globalInputMethod={globalInputMethod} />}
          {activeType === 'ahk'   && <AhkForm value={formValue} onChange={setFormValue} />}
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

      {/* Actions — unified footer for single + double press modes.
          Clear Action is non-destructive (resets editor only). Clear Key and
          Delete remove saved data and gate behind an inline confirmation row. */}
      <div className="macro-panel-footer">
        {(() => {
          const activeRecord = pressMode === 'double' ? doubleAssignment
            : pressMode === 'hold' ? holdAssignment
            : assignment;
          if (!activeRecord) return null;
          if (confirmingAction) {
            const confirmText =
              confirmingAction === 'clear-action'
                ? 'Clear the action for the current type? Other type drafts are preserved.'
                : confirmingAction === 'clear'
                ? (pressMode === 'double'
                    ? 'Delete the double-press action on this key? This cannot be undone.'
                    : pressMode === 'hold'
                    ? 'Delete the hold action on this key? This cannot be undone.'
                    : 'Delete the single-press action on this key? Use Unassign instead to keep it without a key.')
                : libraryMode
                ? 'Delete this action from Unassigned? This cannot be undone.'
                : 'Delete the single, double-press and hold actions on this key? This cannot be undone. Use Unassign instead to keep the action without a key.';
            const handleYes = () => {
              if (confirmingAction === 'clear-action') {
                handleClearAction();
              } else if (confirmingAction === 'clear') {
                if (pressMode === 'double') onClearDouble?.(selectedKey);
                else if (pressMode === 'hold') onClearHold?.(selectedKey);
                else onClear?.(selectedKey);
              } else if (confirmingAction === 'delete') {
                onDelete?.(selectedKey);
              }
              setConfirmingAction(null);
            };
            return (
              <div className="footer-assignment-actions footer-confirm-row">
                <span className="footer-confirm-text">{confirmText}</span>
                <button className="btn-confirm-yes" type="button" onClick={handleYes}>Yes</button>
                <button className="btn-confirm-no" type="button" onClick={() => setConfirmingAction(null)}>Cancel</button>
              </div>
            );
          }
          const clearKeyTitle = pressMode === 'double'
            ? 'Remove the double-press action on this key (keeps any single-press or hold action)'
            : pressMode === 'hold'
            ? 'Remove the hold action on this key (keeps any single or double-press action)'
            : 'Remove the single-press action on this key (keeps any double-press or hold action)';
          return (
            <div className="footer-assignment-actions">
              <button
                className="btn-clear-action"
                onClick={() => setConfirmingAction('clear-action')}
                type="button"
                title="Clears the action for the currently-active type only. Other type drafts on this key are preserved. Use Clear Key to wipe all types."
              >Clear Action</button>
              {!libraryMode && (
                <button
                  className="btn-clear"
                  onClick={() => setConfirmingAction('clear')}
                  type="button"
                  title={clearKeyTitle}
                >Clear Key</button>
              )}
              {!libraryMode && onUnassign && (
                <button
                  className="btn-unassign"
                  onClick={() => onUnassign(selectedKey)}
                  type="button"
                  title="Free this key but keep the action in the Unassigned list, ready to bind again"
                >Unassign</button>
              )}
              <button
                className="btn-duplicate"
                onClick={() => setDuplicating(true)}
                type="button"
                title={libraryMode ? 'Duplicate this action onto a hotkey (keeps the unassigned original)' : 'Duplicate this macro to a different hotkey'}
              >Duplicate</button>
              {onDelete && (
                <button
                  className="btn-delete"
                  onClick={() => setConfirmingAction('delete')}
                  type="button"
                  title={libraryMode ? 'Delete this action from Unassigned' : 'Delete the single, double-press and hold actions on this key'}
                >Delete</button>
              )}
            </div>
          );
        })()}
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
                : pressMode === 'hold'
                ? (holdAssignment ? 'Update Hold' : 'Assign Hold')
                : libraryMode
                ? (assignment ? 'Update' : 'Save to Unassigned')
                : (assignment ? 'Update' : 'Assign to Key')
            }
          </button>
        )}
      </div>
    </div>
  );
}
