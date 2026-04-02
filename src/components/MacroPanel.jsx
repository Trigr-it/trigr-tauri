import React, { useState, useEffect, useLayoutEffect, useRef, Fragment } from 'react';
import './MacroPanel.css';

const ACTION_TYPES = [
  {
    id: 'text',
    icon: '✦',
    label: 'Type Text',
    desc: 'Types a text snippet when key is pressed',
    color: '#64b4ff',
  },
  {
    id: 'hotkey',
    icon: '⌨',
    label: 'Send Hotkey',
    desc: 'Triggers a key combination like Ctrl+C',
    color: '#c864ff',
  },
  {
    id: 'app',
    icon: '⬡',
    label: 'Open App',
    desc: 'Launch an application or file',
    color: '#50c878',
  },
  {
    id: 'url',
    icon: '⊕',
    label: 'Open URL',
    desc: 'Open a website in your browser',
    color: '#ffc832',
  },
  {
    id: 'folder',
    icon: '⬢',
    label: 'Open Folder',
    desc: 'Open a folder in File Explorer',
    color: '#40c8a0',
  },
  {
    id: 'macro',
    icon: '◈',
    label: 'Macro Sequence',
    desc: 'Run a sequence of actions one after another',
    color: '#ff783c',
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

const MACRO_STEP_TYPES = ['Type Text', 'Press Key', 'Wait (ms)', 'Open URL', 'Wait for Input'];

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

const INPUT_METHOD_OPTS = [
  { id: 'global',       label: 'Global default',             hint: 'Use the method set in Settings → Compatibility' },
  { id: 'direct',       label: 'Direct keystrokes',          hint: 'Simulates real keypresses — works in CAD, games' },
  { id: 'shift-insert', label: 'Clipboard (Shift+Insert)',   hint: 'Fast for long text — universal paste shortcut' },
  { id: 'ctrl-v',       label: 'Clipboard (Ctrl+V)',          hint: 'Standard paste — may conflict in CAD apps' },
  { id: 'send-input',   label: 'SendInput API',               hint: 'Windows low-level injection — bypasses app filtering' },
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
  const divRef        = useRef(null);
  const onChangeRef   = useRef(onChange);
  const valueRef      = useRef(value);
  const capturingRef  = useRef(false);
  useEffect(() => { onChangeRef.current = onChange; }, [onChange]);
  useEffect(() => { valueRef.current    = value;    }, [value]);
  // Keep capturingRef in sync so the IPC handler can gate on it
  useEffect(() => { capturingRef.current = capturing; }, [capturing]);

  // IPC path: main process captures keypresses (including Win key) and sends result
  useEffect(() => {
    if (!window.electronAPI?.onKeyCaptured) return;
    const handler = (combo) => {
      if (!capturingRef.current) return; // guard: only process if this instance is active
      onChangeRef.current({ ...valueRef.current, ...parseHotkeyCapture(combo) });
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
    // Only intercept Escape — all other keys are captured by the main process
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      window.electronAPI?.stopKeyCapture();
      setCapturing(false);
      divRef.current?.blur();
    }
  }

  function handleBlur() {
    if (capturing) {
      window.electronAPI?.stopKeyCapture();
      setCapturing(false);
    }
  }

  const currentCombo = hotkeyDataToString(value);

  return (
    <div className="form-section">
      <label className="form-label">Hotkey</label>
      <div
        ref={divRef}
        className={`key-capture${capturing ? ' key-capture-active' : ''}`}
        tabIndex={0}
        onClick={startCapture}
        onKeyDown={capturing ? handleKeyDown : undefined}
        onBlur={handleBlur}
        role="button"
        aria-label={capturing ? 'Press your hotkey combination' : currentCombo || 'Click to capture hotkey'}
      >
        {capturing ? (
          <span className="key-capture-prompt">Press your hotkey combination…</span>
        ) : currentCombo ? (
          <span className="key-capture-value"><KeyChips combo={currentCombo} /></span>
        ) : (
          <span className="key-capture-placeholder">Click to capture hotkey…</span>
        )}
      </div>
    </div>
  );
}

function AppForm({ value, onChange }) {
  async function handleBrowse() {
    const path = await window.electronAPI?.browseForFile();
    if (path) onChange({ ...value, path });
  }
  return (
    <div className="form-section">
      <label className="form-label">Application path</label>
      <div className="file-input-row">
        <input
          className="form-input"
          placeholder="C:\Program Files\App\app.exe"
          value={value.path || ''}
          onChange={e => onChange({ ...value, path: e.target.value })}
        />
        <button className="browse-btn" type="button" onClick={handleBrowse}>Browse</button>
      </div>
      <label className="form-label" style={{ marginTop: 12 }}>Display name (optional)</label>
      <input
        className="form-input"
        placeholder="My App"
        value={value.appName || ''}
        onChange={e => onChange({ ...value, appName: e.target.value })}
      />
      <div className="form-hint">Enter the full path to the .exe or any file to open.</div>
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
          <kbd>{k}</kbd>
          {i < keys.length - 1 && <span className="key-capture-plus">+</span>}
        </Fragment>
      ))}
    </>
  );
}

function KeyCaptureInput({ value, onChange }) {
  const [capturing, setCapturing] = useState(false);
  const divRef       = useRef(null);
  const onChangeRef  = useRef(onChange);
  const capturingRef = useRef(false);
  useEffect(() => { onChangeRef.current  = onChange;   }, [onChange]);
  useEffect(() => { capturingRef.current = capturing;  }, [capturing]);

  // IPC path: main process captures keypresses (including Win key) and sends result
  useEffect(() => {
    if (!window.electronAPI?.onKeyCaptured) return;
    const handler = (combo) => {
      if (!capturingRef.current) return; // guard: only process if this instance is active
      onChangeRef.current(combo);
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
    // Only intercept Escape — all other keys are captured by the main process
    if (e.key === 'Escape') {
      e.preventDefault();
      e.stopPropagation();
      window.electronAPI?.stopKeyCapture();
      setCapturing(false);
      divRef.current?.blur();
    }
  }

  function handleBlur() {
    if (capturing) {
      window.electronAPI?.stopKeyCapture();
      setCapturing(false);
    }
  }

  return (
    <div
      ref={divRef}
      className={`key-capture macro-step-value${capturing ? ' key-capture-active' : ''}`}
      tabIndex={0}
      onClick={startCapture}
      onKeyDown={capturing ? handleKeyDown : undefined}
      onBlur={handleBlur}
      role="button"
      aria-label={capturing ? 'Press a key combination' : value || 'Click to capture key'}
    >
      {capturing ? (
        <span className="key-capture-prompt">Press a key…</span>
      ) : value ? (
        <span className="key-capture-value"><KeyChips combo={value} /></span>
      ) : (
        <span className="key-capture-placeholder">Click to capture…</span>
      )}
    </div>
  );
}

function MacroSequenceForm({ value, onChange, globalInputMethod }) {
  const steps = value.steps || [];
  const seqMethod = value.inputMethod || 'global';
  const globalLabel = INPUT_METHOD_OPTS.find(o => o.id === globalInputMethod)?.label || globalInputMethod;
  const [dragIndex, setDragIndex] = useState(null);
  const [dropIndex, setDropIndex] = useState(null);

  const addStep = () => {
    onChange({ ...value, steps: [...steps, { type: 'Type Text', value: '' }] });
  };

  const updateStep = (i, updated) => {
    const newSteps = steps.map((s, idx) => idx === i ? updated : s);
    onChange({ ...value, steps: newSteps });
  };

  const removeStep = (i) => {
    onChange({ ...value, steps: steps.filter((_, idx) => idx !== i) });
  };

  function handleDragStart(e, i) {
    setDragIndex(i);
    e.dataTransfer.effectAllowed = 'move';
    e.dataTransfer.setData('text/plain', String(i)); // required for Firefox
  }

  function handleDragOver(e, i) {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'move';
    const rect = e.currentTarget.getBoundingClientRect();
    setDropIndex(e.clientY < rect.top + rect.height / 2 ? i : i + 1);
  }

  function handleDrop(e) {
    e.preventDefault();
    if (dragIndex === null || dropIndex === null) return;
    const newSteps = [...steps];
    const [moved] = newSteps.splice(dragIndex, 1);
    // Adjust target index after the splice removed the source item
    const target = dropIndex > dragIndex ? dropIndex - 1 : dropIndex;
    newSteps.splice(target, 0, moved);
    onChange({ ...value, steps: newSteps });
    setDragIndex(null);
    setDropIndex(null);
  }

  function handleDragEnd() {
    setDragIndex(null);
    setDropIndex(null);
  }

  // Show drop line at position pos only when it represents an actual move
  function showLineAt(pos) {
    return (
      dragIndex !== null &&
      dropIndex === pos &&
      pos !== dragIndex &&      // dropping at own position — no-op
      pos !== dragIndex + 1     // dropping immediately after self — no-op
    );
  }

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
      <div
        className="macro-steps"
        onDragOver={e => e.preventDefault()}
        onDrop={handleDrop}
      >
        {steps.length === 0 && (
          <div className="macro-empty">No steps yet — add your first action below</div>
        )}
        {steps.map((step, i) => (
          <Fragment key={i}>
            {showLineAt(i) && <div className="step-drop-line" />}
            <div
              className={`macro-step${dragIndex === i ? ' macro-step-dragging' : ''}${step.type === 'Wait for Input' ? ' macro-step-wfi' : ''}`}
              onDragOver={e => handleDragOver(e, i)}
              onDrop={handleDrop}
            >
              {/* Row 1: drag handle, step number, type dropdown, value/delete */}
              <div className="macro-step-row">
                <div
                  className="step-drag-handle"
                  draggable
                  onDragStart={e => handleDragStart(e, i)}
                  onDragEnd={handleDragEnd}
                  title="Drag to reorder"
                >
                  ⠿
                </div>
                <div className="macro-step-num">{i + 1}</div>
                <select
                  className="form-select macro-step-type"
                  value={step.type}
                  onChange={e => updateStep(i, { ...step, type: e.target.value })}
                >
                  {MACRO_STEP_TYPES.map(t => <option key={t}>{t}</option>)}
                </select>
                {step.type !== 'Wait for Input' && (
                  step.type === 'Press Key' ? (
                    <KeyCaptureInput
                      value={step.value || ''}
                      onChange={v => updateStep(i, { ...step, value: v })}
                    />
                  ) : (
                    <input
                      className="form-input macro-step-value"
                      placeholder={
                        step.type === 'Wait (ms)' ? '500' :
                        step.type === 'Open URL'  ? 'https://...' :
                        'Text to type...'
                      }
                      value={step.value || ''}
                      onChange={e => updateStep(i, { ...step, value: e.target.value })}
                    />
                  )
                )}
                <button className="step-remove" onClick={() => removeStep(i)} type="button">✕</button>
              </div>
              {/* Row 2 (Wait for Input only): labelled config dropdowns */}
              {step.type === 'Wait for Input' && (() => {
                let wfi = { inputType: 'LButton', trigger: 'press', specificKey: '' };
                try { wfi = { ...wfi, ...JSON.parse(step.value || '{}') }; } catch (_) {}
                const updateWfi = (patch) => {
                  const next = { ...wfi, ...patch };
                  if (patch.inputType && patch.inputType !== 'SpecificKey') next.specificKey = '';
                  updateStep(i, { ...step, value: JSON.stringify(next) });
                };
                return (
                  <div className="wfi-config-row">
                    <div className="wfi-field">
                      <span className="wfi-label">Wait for:</span>
                      <select
                        className="form-select wfi-select"
                        value={wfi.inputType}
                        onChange={e => updateWfi({ inputType: e.target.value })}
                      >
                        {WFI_INPUT_OPTIONS.map(o => (
                          <option key={o.value} value={o.value}>{o.label}</option>
                        ))}
                      </select>
                    </div>
                    <div className="wfi-field">
                      <span className="wfi-label">Trigger:</span>
                      <select
                        className="form-select wfi-select"
                        value={wfi.trigger}
                        onChange={e => updateWfi({ trigger: e.target.value })}
                      >
                        {WFI_TRIGGER_OPTIONS.map(o => (
                          <option key={o.value} value={o.value}>{o.label}</option>
                        ))}
                      </select>
                    </div>
                    {wfi.inputType === 'SpecificKey' && (
                      <div className="wfi-field">
                        <span className="wfi-label">Key:</span>
                        <KeyCaptureInput
                          value={wfi.specificKey || ''}
                          onChange={v => updateWfi({ specificKey: v })}
                        />
                      </div>
                    )}
                  </div>
                );
              })()}
            </div>
          </Fragment>
        ))}
        {showLineAt(steps.length) && <div className="step-drop-line" />}
      </div>
      <button className="add-step-btn" onClick={addStep} type="button">
        + Add Step
      </button>
    </div>
  );
}

// ── Helpers ───────────────────────────────────────────────────────────────────

const MOD_ORDER = ['Ctrl', 'Shift', 'Alt', 'Win'];

function keyIdToLabel(keyId) {
  if (!keyId) return '';
  if (keyId.startsWith('Key'))   return keyId.slice(3);
  if (keyId.startsWith('Digit')) return keyId.slice(5);
  return keyId;
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

    // Bare key (no modifiers held) — only valid in app-linked profiles
    if (mods.length === 0 && !profileLinked) return;

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
                <span className="reassign-bare-note">Bare keys only available in app-linked profiles</span>
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
  onReassign,
  onCopyToProfile,
  onMoveToProfile,
  onDuplicate,
}) {
  const [activeType, setActiveType] = useState('text');
  const [formValue, setFormValue] = useState({});
  const [label, setLabel] = useState('');
  const [pressMode, setPressMode] = useState('single'); // 'single' | 'double'
  const [reassigning, setReassigning] = useState(false);
  const [duplicating, setDuplicating] = useState(false);
  const [profilePopover, setProfilePopover] = useState(null); // null | 'copy' | 'move'
  const [pendingMouseSave, setPendingMouseSave] = useState(null); // macro pending global-mouse confirmation
  const popoverRef = useRef(null);

  const otherProfiles = (profiles || []).filter(p => p !== activeProfile);

  // Close profile popover on outside click
  useEffect(() => {
    if (!profilePopover) return;
    function onDown(e) {
      if (!popoverRef.current?.contains(e.target)) setProfilePopover(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [profilePopover]);

  useEffect(() => {
    setReassigning(false);
    setDuplicating(false);
    setProfilePopover(null);
    setPendingMouseSave(null);
    setPressMode('single');
    if (assignment) {
      setActiveType(assignment.type || 'text');
      setFormValue(assignment.data || {});
      setLabel(assignment.label || '');
    } else {
      setActiveType('text');
      setFormValue({});
      setLabel('');
    }
  }, [selectedKey, assignment]);

  // When press mode switches, load the appropriate assignment's form values
  useEffect(() => {
    if (pressMode === 'double') {
      if (doubleAssignment) {
        setActiveType(doubleAssignment.type || 'text');
        setFormValue(doubleAssignment.data || {});
        setLabel(doubleAssignment.label || '');
      } else {
        setActiveType('text');
        setFormValue({});
        setLabel('');
      }
    } else {
      if (assignment) {
        setActiveType(assignment.type || 'text');
        setFormValue(assignment.data || {});
        setLabel(assignment.label || '');
      } else {
        setActiveType('text');
        setFormValue({});
        setLabel('');
      }
    }
  // eslint-disable-next-line
  }, [pressMode]);

  const handleSave = () => {
    if (!selectedKey) return;

    const macro = {
      type: activeType,
      label: label || getAutoLabel(),
      data: formValue,
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
      case 'hotkey': return [...(formValue.modifiers || []), formValue.key].filter(Boolean).join('+') || 'Key combo';
      case 'app':    return formValue.appName || formValue.path?.split('\\').pop() || 'Application';
      case 'folder': return formValue.folderName || formValue.path?.split('\\').pop() || 'Folder';
      case 'url':    return formValue.urlName || formValue.url || 'URL';
      case 'macro':  return `Macro (${(formValue.steps || []).length} steps)`;
      default:       return 'Action';
    }
  };

  const isValid = () => {
    switch (activeType) {
      case 'text':   return !!formValue.text?.trim();
      case 'hotkey': return !!formValue.key;
      case 'app':    return !!formValue.path?.trim();
      case 'folder': return !!formValue.path?.trim();
      case 'url':    return !!formValue.url?.trim();
      case 'macro':  return (formValue.steps || []).length > 0;
      default:       return false;
    }
  };

  if (!selectedKey) {
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
          <kbd className="selected-key-badge">
            {selectedKey?.startsWith('MOUSE_')
              ? ({ MOUSE_LEFT: 'Left Click', MOUSE_RIGHT: 'Right Click', MOUSE_MIDDLE: 'Middle Click',
                   MOUSE_SCROLL_UP: 'Scroll ↑', MOUSE_SCROLL_DOWN: 'Scroll ↓',
                   MOUSE_SIDE1: 'Side 1', MOUSE_SIDE2: 'Side 2' })[selectedKey] ?? selectedKey
              : selectedKey}
          </kbd>
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
            onClick={() => setPressMode('double')}
            type="button"
          >
            ×2 Double Press
            {doubleAssignment && <span className="press-mode-dot" />}
          </button>
        </div>
      )}

      <div className="macro-panel-body">
        {/* Action type selector */}
        <div className="type-selector">
          {ACTION_TYPES.map(type => (
            <button
              key={type.id}
              className={`type-btn ${activeType === type.id ? 'active' : ''}`}
              style={{ '--type-color': type.color }}
              onClick={() => { setActiveType(type.id); setFormValue({}); }}
              type="button"
            >
              <span className="type-btn-icon">{type.icon}</span>
              <span className="type-btn-label">{type.label}</span>
            </button>
          ))}
        </div>

        {/* Type description */}
        <div className="type-desc">
          {ACTION_TYPES.find(t => t.id === activeType)?.desc}
        </div>

        {/* Dynamic form */}
        <div className="form-body">
          {activeType === 'text'   && <TextForm value={formValue} onChange={setFormValue} globalInputMethod={globalInputMethod} />}
          {activeType === 'hotkey' && <HotkeyCaptureInput value={formValue} onChange={setFormValue} />}
          {activeType === 'app'    && <AppForm value={formValue} onChange={setFormValue} />}
          {activeType === 'folder' && <FolderForm value={formValue} onChange={setFormValue} />}
          {activeType === 'url'    && <UrlForm value={formValue} onChange={setFormValue} />}
          {activeType === 'macro'  && <MacroSequenceForm value={formValue} onChange={setFormValue} globalInputMethod={globalInputMethod} />}
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
              {otherProfiles.length > 0 && (
                <div className="profile-action-group" ref={popoverRef}>
                  <button
                    className={`btn-profile${profilePopover === 'copy' ? ' active' : ''}`}
                    onClick={() => setProfilePopover(v => v === 'copy' ? null : 'copy')}
                    type="button"
                    title="Duplicate this macro to another profile"
                  >
                    Copy ▾
                  </button>
                  <button
                    className={`btn-profile btn-profile-move${profilePopover === 'move' ? ' active' : ''}`}
                    onClick={() => setProfilePopover(v => v === 'move' ? null : 'move')}
                    type="button"
                    title="Move this macro to another profile"
                  >
                    Move ▾
                  </button>
                  {profilePopover && (
                    <div className="profile-popover">
                      <div className="profile-popover-label">
                        {profilePopover === 'copy' ? 'Copy to:' : 'Move to:'}
                      </div>
                      {otherProfiles.map(p => (
                        <button
                          key={p}
                          className="profile-popover-item"
                          onClick={() => {
                            if (profilePopover === 'copy') onCopyToProfile?.(p);
                            else onMoveToProfile?.(p);
                            setProfilePopover(null);
                          }}
                          type="button"
                        >
                          {p}
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              )}
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
            disabled={!isValid()}
            type="button"
          >
            {pressMode === 'double'
              ? (doubleAssignment ? 'Update Double-Tap' : 'Assign Double-Tap')
              : (assignment ? 'Update' : 'Assign to Key')
            }
          </button>
        )}
      </div>
    </div>
  );
}
