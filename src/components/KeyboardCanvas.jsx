import React, { useCallback, useState, useEffect, useRef } from 'react';
import './KeyboardCanvas.css';
import {
  KEYBOARD_ROWS, SYSTEM_KEYS, KEY_UNIT, KEY_GAP, KEY_HEIGHT,
  KEYBOARD_NATURAL_WIDTH, KEYBOARD_NATURAL_HEIGHT,
} from './keyboardLayout';
import NumpadCanvas from './NumpadCanvas';

const MODIFIERS = [
  { id: 'Ctrl',  label: 'Ctrl',   color: '#64b4ff' },
  { id: 'Alt',   label: 'Alt',    color: '#c864ff' },
  { id: 'Shift', label: 'Shift',  color: '#50c878' },
  { id: 'Win',   label: '⊞ Win', color: '#ffc832' },
];

// Build the display string for the current modifier combo e.g. "Ctrl+Alt"
export function comboString(modifiers) {
  if (modifiers.includes('BARE')) return 'BARE';
  const order = ['Ctrl', 'Shift', 'Alt', 'Win'];
  return order.filter(m => modifiers.includes(m)).join('+');
}

export function ModifierBar({ activeModifiers, onToggle, profileLinked, isRecording, onStartRecord, onStopRecord, recordCapture }) {
  const isBare = activeModifiers.includes('BARE');
  const combo  = comboString(activeModifiers);
  const recordStartTime = useRef(0);

  // Track when recording starts so we can ignore the synthesized click
  useEffect(() => {
    if (isRecording) recordStartTime.current = Date.now();
  }, [isRecording]);

  const guardedStopRecord = useCallback(() => {
    // Ignore clicks within 200ms of recording starting — these are synthesized
    // from the mousedown/mouseup cycle that started recording
    if (Date.now() - recordStartTime.current < 200) return;
    onStopRecord();
  }, [onStopRecord]);

  return (
    <div className="modifier-bar">
      <span className="modifier-bar-label">Modifier Layer</span>

      <div className="modifier-bar-keys">
        {MODIFIERS.map(mod => {
          const isActive = activeModifiers.includes(mod.id);
          return (
            <button
              key={mod.id}
              className={`mod-layer-btn ${isActive ? 'active' : ''}`}
              style={isActive ? { '--mod-color': mod.color } : {}}
              onClick={isRecording ? undefined : () => onToggle(mod.id)}
              disabled={isRecording}
            >
              {mod.label}
            </button>
          );
        })}

        <span className="modifier-bar-sep" />
        <button
          className={`mod-layer-btn bare-key-btn${isBare ? ' active' : ''}${!profileLinked ? ' bare-key-unavailable' : ''}`}
          style={isBare ? { '--mod-color': '#ff9040' } : {}}
          onClick={isRecording || !profileLinked ? undefined : () => onToggle('BARE')}
          disabled={isRecording || !profileLinked}
          title={profileLinked
            ? "Bare key assignments — fire with no modifier held, only when this profile's linked app is focused"
            : "Bare keys are only available in app-specific profiles. Create a profile linked to an app to use bare key assignments."}
        >
          Bare Keys
        </button>

        <span className="modifier-bar-sep" />
        {isRecording ? (
          <button
            className="mod-layer-btn record-btn recording"
            onClick={guardedStopRecord}
            title="Press any key combination to capture it — click to stop"
          >
            <span className="record-dot" />
            Recording…
          </button>
        ) : (
          <button
            className="mod-layer-btn record-btn"
            onMouseDown={onStartRecord}
            title="Click then press any key combination to select it"
          >
            ⏺ Record
          </button>
        )}
      </div>

      <div className="modifier-bar-combo">
        {isRecording ? (
          <span className="combo-hint record-hint">Press any key combination — click Recording to cancel</span>
        ) : recordCapture ? (
          <span className="combo-hint record-captured">Captured: {recordCapture}</span>
        ) : activeModifiers.length === 0 ? (
          <span className="combo-hint">↑ Select 1–3 modifiers to view that hotkey layer</span>
        ) : isBare ? (
          <span className="combo-active">
            <span className="combo-active-label">Layer:</span>
            <kbd className="combo-key combo-key-bare">Bare</kbd>
            <span className="combo-plus">+</span>
            <kbd className="combo-key combo-key-target">key</kbd>
            <span className="combo-bare-hint"> — fires only when linked app is focused</span>
          </span>
        ) : (
          <span className="combo-active">
            <span className="combo-active-label">Layer:</span>
            {combo.split('+').map((m, i, arr) => (
              <React.Fragment key={m}>
                <kbd className="combo-key">{m}</kbd>
                {i < arr.length - 1 && <span className="combo-plus">+</span>}
              </React.Fragment>
            ))}
            <span className="combo-plus">+</span>
            <kbd className="combo-key combo-key-target">key</kbd>
          </span>
        )}
      </div>
    </div>
  );
}

function Key({ keyDef, isSelected, isAssigned, isDouble, isSystem, isFiring, noLayer, onClick, onContextMenu }) {
  const width = keyDef.width * KEY_UNIT + (keyDef.width - 1) * KEY_GAP;

  const classNames = [
    'key',
    isSelected ? 'selected'  : '',
    isAssigned ? 'assigned'  : '',
    isSystem   ? 'system'    : '',
    isFiring   ? 'firing'    : '',
    noLayer    ? 'no-layer'  : '',
    keyDef.id === 'Space' ? 'spacebar'  : '',
    keyDef.id === 'Enter' ? 'enter-key' : '',
  ].filter(Boolean).join(' ');

  return (
    <div
      className={classNames}
      style={{ width, height: KEY_HEIGHT, flexShrink: 0 }}
      onClick={isSystem || noLayer ? undefined : onClick}
      onContextMenu={isAssigned && !isSystem && !noLayer ? onContextMenu : undefined}
      title={
        isSystem  ? 'Modifier key — part of combos' :
        noLayer   ? 'Select a modifier layer above first' :
        isAssigned ? 'Click to edit this assignment' :
        'Click to assign a macro here'
      }
    >
      {isAssigned && !isSelected && <span className="key-assigned-dot" />}
      {isDouble && <span className="key-double-badge">×2</span>}
      {keyDef.sublabel && <span className="key-sublabel">{keyDef.sublabel}</span>}
      <span className="key-label">{keyDef.label}</span>
    </div>
  );
}

export default function KeyboardCanvas({
  selectedKey,
  onKeySelect,
  getKeyAssignment,
  hasDoubleAssignment,
  lastFired,
  activeModifiers,
  onToggleModifier,
  profileLinked,
  isRecording,
  onStartRecord,
  onStopRecord,
  recordCapture,
  hasAnyAssignments,
  numpadOpen = false,
  onToggleNumpad,
  currentCombo,
  onRenameAssignment,
  onClearAssignment,
  onDuplicateFromContext,
}) {
  const [firingKeyId, setFiringKeyId] = useState(null);
  const [scale, setScale]             = useState(1);
  const containerRef                  = useRef(null);

  // ── Assignment context menu ──
  const [keyCtx, setKeyCtx] = useState(null); // { keyId, x, y }
  const [keyRenaming, setKeyRenaming] = useState(null); // keyId
  const [keyRenameVal, setKeyRenameVal] = useState('');
  const [keyClearing, setKeyClearing] = useState(null); // keyId
  const [popoverPos, setPopoverPos] = useState({ x: 0, y: 0 }); // saved from context menu
  const keyCtxRef = useRef(null);

  useEffect(() => {
    if (!keyCtx) return;
    function onDown(e) {
      if (keyCtxRef.current && !keyCtxRef.current.contains(e.target)) setKeyCtx(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [keyCtx]);

  useEffect(() => {
    if (lastFired?.keyId) {
      setFiringKeyId(lastFired.keyId);
      const t = setTimeout(() => setFiringKeyId(null), 600);
      return () => clearTimeout(t);
    }
  }, [lastFired]);

  // Observe the container width and compute a CSS scale factor so the keyboard
  // always fits without horizontal overflow, but also grows when space allows.
  // Guard: only update scale when the WIDTH actually changes — height-only changes
  // (e.g. from internal content reflows) must not trigger a scale recalculation,
  // as that would create a feedback loop via keyboard-scale-wrap's inline height.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    let lastWidth = 0;
    const ro = new ResizeObserver(entries => {
      const availableWidth = entries[0].contentRect.width;
      if (availableWidth > 0 && Math.abs(availableWidth - lastWidth) >= 1) {
        lastWidth = availableWidth;
        const scaleX = availableWidth / KEYBOARD_NATURAL_WIDTH;
        setScale(Math.min(1.4, Math.max(0.3, scaleX)));
      }
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const handleKeyClick = useCallback((keyId) => {
    onKeySelect(keyId);
  }, [onKeySelect]);

  const handleKeyContextMenu = useCallback((e, keyId) => {
    e.preventDefault();
    e.stopPropagation();
    setKeyCtx({ keyId, x: e.clientX, y: e.clientY });
    setKeyRenaming(null);
    setKeyClearing(null);
  }, []);

  const noLayer = activeModifiers.length === 0;
  const isBare  = activeModifiers.includes('BARE');
  const combo   = comboString(activeModifiers);

  return (
    <div className="keyboard-canvas-wrap">

      <ModifierBar
        activeModifiers={activeModifiers}
        onToggle={onToggleModifier}
        profileLinked={profileLinked}
        isRecording={isRecording}
        onStartRecord={onStartRecord}
        onStopRecord={onStopRecord}
        recordCapture={recordCapture}
      />

      {/* Empty state — shown only when no modifier is selected AND no assignments exist anywhere */}
      {noLayer && !hasAnyAssignments && (
        <div className="keyboard-empty-state">
          <span className="keyboard-empty-icon">⌨</span>
          <span className="keyboard-empty-heading">No hotkeys assigned yet</span>
          <span className="keyboard-empty-sub">Select a modifier key above, then click any key on the keyboard to assign your first hotkey</span>
          <span className="keyboard-empty-record-hint">Or press <strong>Record →</strong> to capture a key combo instantly</span>
        </div>
      )}

      <div className="keyboard-label">
        {noLayer ? (
          <span className="label-muted">Select modifier keys above, then click a key to assign a hotkey</span>
        ) : isBare ? (
          selectedKey ? (
            <span className="label-assigning">
              Assigning: <strong>Bare</strong> + <strong>{selectedKey}</strong>
            </span>
          ) : (
            <span className="label-muted">
              Click any key to assign a <strong className="label-combo">bare key</strong> macro — fires only when linked app is focused
            </span>
          )
        ) : selectedKey ? (
          <span className="label-assigning">
            Assigning: {combo.split('+').map((m, i, arr) => (
              <React.Fragment key={m}>
                <strong>{m}</strong>{i < arr.length - 1 ? ' + ' : ''}
              </React.Fragment>
            ))} + <strong>{selectedKey}</strong>
          </span>
        ) : (
          <span className="label-muted">
            Click any key to assign a macro to <strong className="label-combo">{combo} + key</strong>
          </span>
        )}
      </div>

      {/* keyboard-body-row: flex row so numpad sits flush beside the keyboard body */}
      <div className="keyboard-body-row">
        <div
          ref={containerRef}
          className="keyboard-scale-wrap"
          style={{ height: KEYBOARD_NATURAL_HEIGHT * scale }}
        >
          <div
            className={`keyboard-outer${isRecording ? ' recording' : ''}`}
            style={{ transform: `scale(${scale})`, transformOrigin: 'top center' }}
          >
            <div className="keyboard-body">
              {KEYBOARD_ROWS.map((row, rowIdx) => (
                <div key={rowIdx} className="keyboard-row">
                  {row.map((keyDef) => {
                    if (keyDef.spacer) {
                      return <div key={keyDef.id} style={{ width: keyDef.width * KEY_UNIT, flexShrink: 0 }} />;
                    }
                    const isSelected = selectedKey === keyDef.id;
                    const isAssigned = !!getKeyAssignment(keyDef.id);
                    const isDouble   = hasDoubleAssignment ? hasDoubleAssignment(keyDef.id) : false;
                    const isSystem   = SYSTEM_KEYS.has(keyDef.id);
                    const isFiring   = firingKeyId === keyDef.id;

                    return (
                      <Key
                        key={keyDef.id}
                        keyDef={keyDef}
                        isSelected={isSelected}
                        isAssigned={isAssigned}
                        isDouble={isDouble}
                        isSystem={isSystem}
                        isFiring={isFiring}
                        noLayer={noLayer}
                        onClick={() => handleKeyClick(keyDef.id)}
                        onContextMenu={e => handleKeyContextMenu(e, keyDef.id)}
                      />
                    );
                  })}
                </div>
              ))}
            </div>
          </div>
        </div>

        <button
          className={`numpad-toggle-btn${numpadOpen ? ' open' : ''}`}
          onClick={onToggleNumpad}
          title={numpadOpen ? 'Hide Numpad' : 'Show Numpad'}
          type="button"
        >
          {numpadOpen ? '◂' : '▸'}
        </button>

        <div className={`numpad-slide${numpadOpen ? ' numpad-slide--open' : ''}`}>
          <NumpadCanvas
            selectedKey={selectedKey}
            onKeySelect={onKeySelect}
            getKeyAssignment={getKeyAssignment}
            lastFired={lastFired}
            activeModifiers={activeModifiers}
          />
        </div>
      </div>

      <div className="keyboard-hint-row">
        <div className="hint-chip"><span className="hint-dot assigned-dot" /> Assigned on this layer</div>
        <div className="hint-chip"><span className="hint-dot selected-dot" /> Selected</div>
        <div className="hint-chip"><span className="hint-dot system-dot" /> Modifier key</div>
      </div>

      {/* Assignment context menu */}
      {keyCtx && (
        <div
          ref={keyCtxRef}
          className="assign-ctx-menu"
          style={{ top: keyCtx.y, left: keyCtx.x }}
        >
          <button className="assign-ctx-item" type="button" onClick={() => {
            const assignment = getKeyAssignment(keyCtx.keyId);
            setPopoverPos({ x: keyCtx.x, y: keyCtx.y });
            setKeyRenaming(keyCtx.keyId);
            setKeyRenameVal(assignment?.label || '');
            setKeyCtx(null);
          }}>Rename</button>
          <button className="assign-ctx-item" type="button" onClick={() => {
            onDuplicateFromContext?.(currentCombo, keyCtx.keyId);
            setKeyCtx(null);
          }}>Duplicate</button>
          <div className="assign-ctx-divider" />
          <button className="assign-ctx-item assign-ctx-danger" type="button" onClick={() => {
            setPopoverPos({ x: keyCtx.x, y: keyCtx.y });
            setKeyClearing(keyCtx.keyId);
            setKeyCtx(null);
          }}>Clear</button>
        </div>
      )}

      {/* Rename popover */}
      {keyRenaming && (
        <div className="key-popover" style={{ top: popoverPos.y, left: popoverPos.x }}>
          <input
            autoFocus
            className="sidebar-rename-input"
            value={keyRenameVal}
            onChange={e => setKeyRenameVal(e.target.value)}
            onKeyDown={e => {
              if (e.key === 'Enter' && keyRenameVal.trim()) {
                onRenameAssignment?.(currentCombo, keyRenaming, keyRenameVal.trim());
                setKeyRenaming(null);
              }
              if (e.key === 'Escape') setKeyRenaming(null);
            }}
            onBlur={() => setKeyRenaming(null)}
          />
        </div>
      )}

      {/* Clear confirmation popover */}
      {keyClearing && (
        <div className="key-popover key-popover-clear">
          <span className="sidebar-confirm-text">Clear this key?</span>
          <button className="sidebar-confirm-yes" type="button" onClick={() => {
            onClearAssignment?.(currentCombo, keyClearing);
            setKeyClearing(null);
          }}>Yes</button>
          <button className="sidebar-confirm-no" type="button" onClick={() => setKeyClearing(null)}>No</button>
        </div>
      )}
    </div>
  );
}
