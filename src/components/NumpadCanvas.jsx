import React, { useState, useEffect } from 'react';
import { useDroppable, useDraggable } from '@dnd-kit/core';
import './NumpadCanvas.css';
import { comboString } from './KeyboardCanvas';

// Numpad/nav key — also a drop target for sidebar bind-action drags (same
// dropKind + keyId contract as the main keyboard's Key component), and a
// drag SOURCE when assigned (drop on another key to move or swap — App's
// shared bind-action handlers do the rest).
function NumpadKey({ id, label, col, row, colSpan, rowSpan, className, title, disabled, draggable, dragCombo, dragLabel, onClick, onContextMenu, children }) {
  const { setNodeRef, isOver } = useDroppable({
    id: `canvas-key-${id}`,
    data: { dropKind: 'canvas-key', keyId: id },
    disabled,
  });
  const { setNodeRef: setDragRef, listeners, isDragging } = useDraggable({
    id: `key-drag-${id}`,
    data: { kind: 'bind-action', source: 'bound', combo: dragCombo, keyId: id, label: dragLabel },
    disabled: !draggable,
  });
  return (
    <button
      ref={node => { setNodeRef(node); setDragRef(node); }}
      {...listeners}
      className={`${className}${isOver ? ' drop-over' : ''}${isDragging ? ' dragging' : ''}`}
      style={{
        gridColumn: colSpan > 1 ? `${col} / span ${colSpan}` : col,
        gridRow:    rowSpan > 1 ? `${row} / span ${rowSpan}` : row,
      }}
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={title}
      type="button"
    >
      {children}
    </button>
  );
}

// ── Navigation / editing keys (3 cols × 3 rows, mirrors physical keyboard) ──
const NAV_KEYS = [
  { id: 'PrintScreen', label: 'PrtSc',  col: 1, row: 1, colSpan: 1, rowSpan: 1 },
  { id: 'ScrollLock',  label: 'Scr\nLk', col: 2, row: 1, colSpan: 1, rowSpan: 1 },
  { id: 'Pause',       label: 'Pause',  col: 3, row: 1, colSpan: 1, rowSpan: 1 },
  { id: 'Insert',      label: 'Ins',    col: 1, row: 2, colSpan: 1, rowSpan: 1 },
  { id: 'Home',        label: 'Home',   col: 2, row: 2, colSpan: 1, rowSpan: 1 },
  { id: 'PageUp',      label: 'Pg\nUp', col: 3, row: 2, colSpan: 1, rowSpan: 1 },
  { id: 'Delete',      label: 'Del',    col: 1, row: 3, colSpan: 1, rowSpan: 1 },
  { id: 'End',         label: 'End',    col: 2, row: 3, colSpan: 1, rowSpan: 1 },
  { id: 'PageDown',    label: 'Pg\nDn', col: 3, row: 3, colSpan: 1, rowSpan: 1 },
  // Arrow keys — inverted-T layout
  { id: 'ArrowUp',     label: '↑',      col: 2, row: 4, colSpan: 1, rowSpan: 1 },
  { id: 'ArrowLeft',   label: '←',      col: 1, row: 5, colSpan: 1, rowSpan: 1 },
  { id: 'ArrowDown',   label: '↓',      col: 2, row: 5, colSpan: 1, rowSpan: 1 },
  { id: 'ArrowRight',  label: '→',      col: 3, row: 5, colSpan: 1, rowSpan: 1 },
];

// ── Numpad key grid (4 cols × 5 rows) ───────────────────────────────────────
const NUMPAD_KEYS = [
  { id: 'NumLock',        label: 'Num\nLock', col: 1, row: 1, colSpan: 1, rowSpan: 1 },
  { id: 'NumpadDivide',   label: '/',          col: 2, row: 1, colSpan: 1, rowSpan: 1 },
  { id: 'NumpadMultiply', label: '×',          col: 3, row: 1, colSpan: 1, rowSpan: 1 },
  { id: 'NumpadSubtract', label: '−',          col: 4, row: 1, colSpan: 1, rowSpan: 1 },
  { id: 'Numpad7',        label: '7',          col: 1, row: 2, colSpan: 1, rowSpan: 1 },
  { id: 'Numpad8',        label: '8',          col: 2, row: 2, colSpan: 1, rowSpan: 1 },
  { id: 'Numpad9',        label: '9',          col: 3, row: 2, colSpan: 1, rowSpan: 1 },
  { id: 'NumpadAdd',      label: '+',          col: 4, row: 2, colSpan: 1, rowSpan: 2 },
  { id: 'Numpad4',        label: '4',          col: 1, row: 3, colSpan: 1, rowSpan: 1 },
  { id: 'Numpad5',        label: '5',          col: 2, row: 3, colSpan: 1, rowSpan: 1 },
  { id: 'Numpad6',        label: '6',          col: 3, row: 3, colSpan: 1, rowSpan: 1 },
  { id: 'Numpad1',        label: '1',          col: 1, row: 4, colSpan: 1, rowSpan: 1 },
  { id: 'Numpad2',        label: '2',          col: 2, row: 4, colSpan: 1, rowSpan: 1 },
  { id: 'Numpad3',        label: '3',          col: 3, row: 4, colSpan: 1, rowSpan: 1 },
  { id: 'NumpadEnter',    label: 'Enter',      col: 4, row: 4, colSpan: 1, rowSpan: 2 },
  { id: 'Numpad0',        label: '0',          col: 1, row: 5, colSpan: 2, rowSpan: 1 },
  { id: 'NumpadDecimal',  label: '.',          col: 3, row: 5, colSpan: 1, rowSpan: 1 },
];

const NAV_KEY_IDS = new Set(NAV_KEYS.map(k => k.id));

export default function NumpadCanvas({
  selectedKey,
  onKeySelect,
  getKeyAssignment,
  // Double / hold variants — a key bound only as ×2 or Hold must still read
  // as assigned (dot, badge, context menu, drag source). KeyboardCanvas and
  // MouseCanvas already OR all three; numpad was keyed off single only.
  getDoubleAssignment,
  getHoldAssignment,
  lastFired,
  activeModifiers,
  isRecording = false,
  // Routes right-clicks on assigned keys into KeyboardCanvas's shared
  // assignment context menu (rename / duplicate / unassign / copy-move /
  // delete) — numpad ids are ordinary key ids, so every handler works as-is.
  onKeyContextMenu,
}) {
  const [firingKeyId, setFiringKeyId] = useState(null);

  useEffect(() => {
    const id = lastFired?.keyId;
    if (!id) return;
    if (id.startsWith('Numpad') || id === 'NumLock' || NAV_KEY_IDS.has(id)) {
      setFiringKeyId(id);
      const t = setTimeout(() => setFiringKeyId(null), 600);
      return () => clearTimeout(t);
    }
  }, [lastFired]);

  const noLayer = activeModifiers.length === 0;
  const combo   = comboString(activeModifiers);

  // All three trigger variants for a key id.
  function variants(id) {
    const single = getKeyAssignment(id);
    const dbl    = getDoubleAssignment ? getDoubleAssignment(id) : null;
    const hold   = getHoldAssignment ? getHoldAssignment(id) : null;
    return { single, dbl, hold, any: single || dbl || hold };
  }

  function keyClass(id) {
    const isSelected = selectedKey === id;
    const isAssigned = !!variants(id).any;
    const isFiring   = firingKeyId === id;
    return [
      'np-key',
      isSelected ? 'selected'  : '',
      isAssigned ? 'assigned'  : '',
      isFiring   ? 'firing'    : '',
      noLayer    ? 'no-layer'  : '',
    ].filter(Boolean).join(' ');
  }

  function keyTitle(id, label) {
    const displayLabel = label.replace('\n', ' ');
    if (noLayer) return 'Select a modifier layer above first';
    const { single, dbl, hold, any } = variants(id);
    if (any) {
      const parts = [];
      if (single?.label) parts.push(single.label);
      if (dbl?.label)    parts.push(`×2: ${dbl.label}`);
      if (hold?.label)   parts.push(`Hold: ${hold.label}`);
      return `${parts.join('\n') || 'Assigned action'}\nClick to edit. Drag onto another key to move or swap.`;
    }
    return `Assign macro to: ${combo === 'BARE' ? displayLabel : `${combo}+${displayLabel}`}`;
  }

  function renderKey({ id, label, col, row, colSpan, rowSpan }) {
    const { single, dbl, hold, any } = variants(id);
    const isAssigned = !!any;
    const isSelected = selectedKey === id;
    return (
      <NumpadKey
        key={id}
        id={id}
        label={label}
        col={col}
        row={row}
        colSpan={colSpan}
        rowSpan={rowSpan}
        className={keyClass(id)}
        disabled={noLayer}
        draggable={isAssigned && !noLayer && !isRecording}
        dragCombo={combo}
        dragLabel={single?.label || dbl?.label || hold?.label || 'Action'}
        onClick={noLayer ? undefined : () => onKeySelect(id)}
        onContextMenu={isAssigned && !noLayer ? (e) => onKeyContextMenu?.(e, id) : undefined}
        title={keyTitle(id, label)}
      >
        <span className="np-key-label">{label}</span>
        {isAssigned && !isSelected && <span className="np-key-dot" />}
        {dbl && <span className="np-key-double-badge">×2</span>}
        {hold && <span className="np-key-hold-badge" title="Hold trigger">⏱</span>}
      </NumpadKey>
    );
  }

  return (
    <div className="numpad-canvas">
      <div className="numpad-section">
        <div className="numpad-label">Nav</div>
        <div className="nav-grid">
          {NAV_KEYS.map(renderKey)}
        </div>
      </div>
      <div className="numpad-section-divider" />
      <div className="numpad-section">
        <div className="numpad-label">Numpad</div>
        <div className="numpad-grid">
          {NUMPAD_KEYS.map(renderKey)}
        </div>
      </div>
    </div>
  );
}
