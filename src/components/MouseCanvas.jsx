import React, { useState, useEffect, useRef } from 'react';
import './MouseCanvas.css';
import { ModifierBar, comboString } from './KeyboardCanvas';

const MOUSE_ZONES = [
  { id: 'MOUSE_LEFT',         label: 'Left Click',    short: 'Left'   },
  { id: 'MOUSE_RIGHT',        label: 'Right Click',   short: 'Right'  },
  { id: 'MOUSE_MIDDLE',       label: 'Middle Click',  short: '●'      },
  { id: 'MOUSE_SCROLL_UP',    label: 'Scroll Up',     short: '▲'      },
  { id: 'MOUSE_SCROLL_DOWN',  label: 'Scroll Down',   short: '▼'      },
  { id: 'MOUSE_SIDE1',        label: 'Side Button 1', short: 'Side 1' },
  { id: 'MOUSE_SIDE2',        label: 'Side Button 2', short: 'Side 2' },
];

const RISKY_COMBOS = new Set([
  'Ctrl::MOUSE_LEFT',
  'Shift::MOUSE_LEFT',
  'Ctrl+Shift::MOUSE_LEFT',
]);

function isRiskyZone(combo, zoneId) {
  if (zoneId === 'MOUSE_MIDDLE') return true;
  return RISKY_COMBOS.has(`${combo}::${zoneId}`);
}

const BARE_MOUSE_ALLOWED = new Set([
  'MOUSE_MIDDLE', 'MOUSE_SIDE1', 'MOUSE_SIDE2',
  'MOUSE_SCROLL_UP', 'MOUSE_SCROLL_DOWN',
]);

// ── SVG mouse design constants ────────────────────────────────────────────────
// Coordinate space: 200 × 326 (cable nub at y=0, body base at y=318)

// Mouse body silhouette path
const BODY_PATH =
  'M 65 18 Q 100 8 135 18 ' +
  'C 163 18 190 48 190 82 ' +
  'L 190 232 ' +
  'C 190 288 155 318 100 318 ' +
  'C 45 318 10 288 10 232 ' +
  'L 10 82 ' +
  'C 10 48 37 18 65 18 Z';

// Clickable zone rectangles [x, y, width, height]
// These are clipped to BODY_PATH — corners outside the silhouette are hidden
const ZONE_RECTS = {
  MOUSE_LEFT:        [10,  18, 76, 132],  // Left button
  MOUSE_RIGHT:       [114, 18, 76, 132],  // Right button
  MOUSE_SCROLL_UP:   [86,  18, 28,  46],  // Scroll up   (top of centre strip)
  MOUSE_MIDDLE:      [86,  64, 28,  32],  // Middle click (scroll wheel click)
  MOUSE_SCROLL_DOWN: [86,  96, 28,  54],  // Scroll down  (bottom of centre strip)
  MOUSE_SIDE2:       [10, 165, 48,  55],  // Side 2 — Forward (upper side button)
  MOUSE_SIDE1:       [10, 220, 48,  55],  // Side 1 — Back    (lower side button)
};

// Label: [centerX, centerY, text] — null text = no text (scroll wheel drawn separately)
const ZONE_LABEL = {
  MOUSE_LEFT:        [48,  84, 'Left' ],
  MOUSE_RIGHT:       [152, 84, 'Right'],
  MOUSE_SCROLL_UP:   [100, 41, '▲'   ],
  MOUSE_MIDDLE:      [100, 80, null  ],   // scroll wheel glyph rendered below
  MOUSE_SCROLL_DOWN: [100,123, '▼'   ],
  MOUSE_SIDE2:       [34, 192, 'S2'  ],
  MOUSE_SIDE1:       [34, 247, 'S1'  ],
};

// Assigned indicator dot positions [cx, cy]
const ZONE_DOT = {
  MOUSE_LEFT:        [82,  24],
  MOUSE_RIGHT:       [185, 24],
  MOUSE_SCROLL_UP:   [109, 24],
  MOUSE_MIDDLE:      [109, 68],
  MOUSE_SCROLL_DOWN: [109,100],
  MOUSE_SIDE2:       [53, 169],
  MOUSE_SIDE1:       [53, 224],
};

// ×2 badge positions [x, y] — top-right of each zone (textAnchor="end")
const ZONE_X2 = {
  MOUSE_LEFT:        [80,  28],
  MOUSE_RIGHT:       [184, 28],
  MOUSE_SCROLL_UP:   [108, 28],
  MOUSE_MIDDLE:      [108, 74],
  MOUSE_SCROLL_DOWN: [108, 106],
  MOUSE_SIDE2:       [52,  175],
  MOUSE_SIDE1:       [52,  230],
};

export default function MouseCanvas({
  selectedKey,
  onKeySelect,
  getKeyAssignment,
  hasDoubleAssignment,
  lastFired,
  activeModifiers,
  onToggleModifier,
  profileLinked,
  onAddProfile,
  isRecording,
  onStartRecord,
  onStopRecord,
  recordCapture,
}) {
  const [firingZoneId, setFiringZoneId]       = useState(null);
  const [creatingProfile, setCreatingProfile] = useState(false);
  const [newProfileName, setNewProfileName]   = useState('');
  const newProfileInputRef                    = useRef(null);

  useEffect(() => {
    if (lastFired?.keyId?.startsWith('MOUSE_')) {
      setFiringZoneId(lastFired.keyId);
      const t = setTimeout(() => setFiringZoneId(null), 600);
      return () => clearTimeout(t);
    }
  }, [lastFired]);

  useEffect(() => {
    if (creatingProfile) newProfileInputRef.current?.focus();
  }, [creatingProfile]);

  const noMods         = activeModifiers.length === 0;
  const isBare         = activeModifiers.includes('BARE');
  const hasRegularMods = !noMods && !isBare;
  const combo          = comboString(activeModifiers);
  const showAdvisory   = !profileLinked && hasRegularMods;

  function zone(id) {
    const bareBlocked = isBare && !BARE_MOUSE_ALLOWED.has(id);
    return {
      isSelected: selectedKey === id,
      isAssigned: !!getKeyAssignment(id),
      isFiring:   firingZoneId === id,
      noLayer:    noMods,
      bareBlocked,
      isRisky:    showAdvisory && isRiskyZone(combo, id),
      onClick:    () => onKeySelect(id),
    };
  }

  function zoneClass(id) {
    const z = zone(id);
    return ['mc-zone',
      z.isSelected  ? 'selected'     : '',
      z.isAssigned  ? 'assigned'     : '',
      z.isFiring    ? 'firing'       : '',
      z.noLayer     ? 'no-layer'     : '',
      z.bareBlocked ? 'bare-blocked' : '',
      z.isRisky     ? 'risky'        : '',
    ].filter(Boolean).join(' ');
  }

  function zoneTitle(id) {
    const z   = zone(id);
    const lbl = MOUSE_ZONES.find(mz => mz.id === id)?.label || id;
    if (z.bareBlocked) return 'Left and right click require a modifier key';
    if (z.noLayer)     return 'Select a modifier layer above first';
    if (z.isRisky)     return `⚠ May conflict with system shortcuts — ${z.isAssigned ? 'click to edit' : 'click to assign'}`;
    if (z.isAssigned)  return `Click to edit: ${lbl}`;
    return `Assign macro to: ${lbl}`;
  }

  function handleCreateProfile(e) {
    e.preventDefault();
    const name = newProfileName.trim();
    if (!name) return;
    onAddProfile?.(name);
    setNewProfileName('');
    setCreatingProfile(false);
  }

  return (
    <div className="mouse-canvas-wrap">
      <ModifierBar
        activeModifiers={activeModifiers}
        onToggle={onToggleModifier}
        profileLinked={profileLinked}
        isRecording={isRecording}
        onStartRecord={onStartRecord}
        onStopRecord={onStopRecord}
        recordCapture={recordCapture}
      />

      <div className="mouse-label">
        {noMods ? (
          <span className="label-muted">Select modifier keys above, then click a mouse button to assign a macro</span>
        ) : isBare ? (
          selectedKey?.startsWith('MOUSE_') && BARE_MOUSE_ALLOWED.has(selectedKey) ? (
            <span className="label-assigning">
              Assigning bare: <strong>{MOUSE_ZONES.find(z => z.id === selectedKey)?.label || selectedKey}</strong>
            </span>
          ) : (
            <span className="label-muted">
              Click <strong>Middle Click</strong>, <strong>Scroll</strong>, or <strong>Side Buttons</strong> to assign a bare macro
            </span>
          )
        ) : selectedKey?.startsWith('MOUSE_') ? (
          <span className="label-assigning">
            Assigning:{' '}
            {combo.split('+').map((m, i, arr) => (
              <React.Fragment key={m}>
                <strong>{m}</strong>{i < arr.length - 1 ? ' + ' : ''}
              </React.Fragment>
            ))}{' '}
            + <strong>{MOUSE_ZONES.find(z => z.id === selectedKey)?.label || selectedKey}</strong>
          </span>
        ) : (
          <span className="label-muted">
            Click a button to assign a macro to <strong className="label-combo">{combo} + button</strong>
          </span>
        )}
      </div>

      {showAdvisory && (
        <div className="mouse-advisory">
          <div className="mouse-advisory-body">
            <span className="mouse-advisory-icon">⚠</span>
            <div className="mouse-advisory-text">
              <strong>Global profile — potential conflicts.</strong>{' '}
              Mouse combos like Ctrl+Click and Middle Click are used by browsers and system shortcuts.
              App-specific profiles only intercept when that app is focused, and also allow middle click, scroll, and side buttons to be assigned without a modifier.
            </div>
          </div>
          {creatingProfile ? (
            <form className="mouse-advisory-create" onSubmit={handleCreateProfile}>
              <input
                ref={newProfileInputRef}
                className="mouse-advisory-input"
                placeholder="Profile name…"
                value={newProfileName}
                onChange={e => setNewProfileName(e.target.value)}
                onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') setCreatingProfile(false); }}
              />
              <button className="mouse-advisory-btn-confirm" type="submit" disabled={!newProfileName.trim()}>
                Create
              </button>
              <button className="mouse-advisory-btn-cancel" type="button" onClick={() => { setCreatingProfile(false); setNewProfileName(''); }}>
                ✕
              </button>
            </form>
          ) : (
            <button className="mouse-advisory-link" type="button" onClick={() => setCreatingProfile(true)}>
              + Create app profile
            </button>
          )}
        </div>
      )}

      <div className="mouse-diagram">
        <svg viewBox="0 0 200 326" className="mouse-svg" aria-label="Mouse diagram">
          <defs>
            {/* Clip path for all zone fills — shapes them to the mouse silhouette */}
            <clipPath id="mc-clip">
              <path d={BODY_PATH} />
            </clipPath>
            {/* Subtle inner shadow filter for selected/assigned zones */}
            <filter id="mc-glow" x="-20%" y="-20%" width="140%" height="140%">
              <feGaussianBlur stdDeviation="3" result="blur" />
              <feComposite in="SourceGraphic" in2="blur" operator="over" />
            </filter>
          </defs>

          {/* ── Cable nub ──────────────────────────────────────────────────── */}
          <rect x="88" y="0" width="24" height="20" rx="4" ry="4" className="mc-cable" />
          <rect x="89" y="1" width="22" height="18" rx="3" ry="3" className="mc-cable-shine" />

          {/* ── Mouse body base fill ───────────────────────────────────────── */}
          <path d={BODY_PATH} className="mc-body-fill" />

          {/* ── Clickable zones (clipped to body silhouette) ──────────────── */}
          <g clipPath="url(#mc-clip)">
            {/* Button zones */}
            {Object.entries(ZONE_RECTS).map(([id, [x, y, w, h]]) => {
              const z       = zone(id);
              const blocked = z.noLayer || z.bareBlocked;
              return (
                <rect
                  key={id}
                  x={x} y={y} width={w} height={h}
                  className={zoneClass(id)}
                  onClick={blocked ? undefined : z.onClick}
                  style={{ cursor: blocked ? (z.bareBlocked ? 'not-allowed' : 'default') : 'pointer' }}
                >
                  <title>{zoneTitle(id)}</title>
                </rect>
              );
            })}
            {/* Palm area — non-interactive visual fill for lower body */}
            <rect x="58" y="150" width="142" height="170" className="mc-palm" />
          </g>

          {/* ── Structural dividers ────────────────────────────────────────── */}
          {/* Horizontal: button section / palm */}
          <line x1="10"  y1="150" x2="190" y2="150" className="mc-div" />
          {/* Vertical: left/right button split */}
          <line x1="100" y1="18"  x2="100" y2="150" className="mc-div" />
          {/* Scroll strip left border */}
          <line x1="86"  y1="18"  x2="86"  y2="150" className="mc-div" />
          {/* Scroll strip right border */}
          <line x1="114" y1="18"  x2="114" y2="150" className="mc-div" />
          {/* Middle-click top border */}
          <line x1="86"  y1="64"  x2="114" y2="64"  className="mc-div" />
          {/* Middle-click bottom border */}
          <line x1="86"  y1="96"  x2="114" y2="96"  className="mc-div" />
          {/* Side button panel right border */}
          <line x1="58"  y1="165" x2="58"  y2="275" className="mc-div" />
          {/* Side button panel top border */}
          <line x1="10"  y1="165" x2="58"  y2="165" className="mc-div" />
          {/* Between side buttons */}
          <line x1="10"  y1="220" x2="58"  y2="220" className="mc-div" />
          {/* Side button panel bottom border */}
          <line x1="10"  y1="275" x2="58"  y2="275" className="mc-div" />

          {/* ── Body outline drawn on top to crisp up silhouette edges ──────── */}
          <path d={BODY_PATH} className="mc-body-outline" />
          {/* Cable nub outline */}
          <rect x="88" y="0" width="24" height="20" rx="4" ry="4" className="mc-cable-outline" />

          {/* ── Scroll wheel graphic ────────────────────────────────────────── */}
          <rect x="92" y="66" width="16" height="22" rx="5" ry="5" className="mc-wheel" />
          <line x1="93" y1="71" x2="107" y2="71" className="mc-wheel-ridge" />
          <line x1="93" y1="75" x2="107" y2="75" className="mc-wheel-ridge" />
          <line x1="93" y1="79" x2="107" y2="79" className="mc-wheel-ridge" />
          <line x1="93" y1="83" x2="107" y2="83" className="mc-wheel-ridge" />

          {/* ── Zone labels ─────────────────────────────────────────────────── */}
          {Object.entries(ZONE_LABEL).map(([id, [cx, cy, text]]) => {
            if (!text) return null;
            const z = zone(id);
            return (
              <text
                key={`lbl-${id}`}
                x={cx} y={cy}
                className={['mc-label',
                  z.isSelected                  ? 'selected' : '',
                  z.isAssigned                  ? 'assigned' : '',
                  (z.noLayer || z.bareBlocked)  ? 'dimmed'   : '',
                ].filter(Boolean).join(' ')}
                textAnchor="middle"
                dominantBaseline="middle"
                pointerEvents="none"
              >{text}</text>
            );
          })}

          {/* ── Assigned indicator dots ─────────────────────────────────────── */}
          {Object.entries(ZONE_DOT).map(([id, [cx, cy]]) => {
            const z = zone(id);
            if (!z.isAssigned || z.isSelected) return null;
            return (
              <circle
                key={`dot-${id}`}
                cx={cx} cy={cy} r="3"
                className="mc-dot"
                pointerEvents="none"
              />
            );
          })}

          {/* ── Risky zone warning icons ─────────────────────────────────────── */}
          {Object.entries(ZONE_RECTS).map(([id, [x, y]]) => {
            if (!zone(id).isRisky) return null;
            return (
              <text
                key={`warn-${id}`}
                x={x + 4} y={y + 10}
                className="mc-risky-icon"
                pointerEvents="none"
              >⚠</text>
            );
          })}

          {/* ── Double-click (×2) badges ─────────────────────────────────────── */}
          {hasDoubleAssignment && Object.entries(ZONE_X2).map(([id, [x, y]]) => {
            if (!hasDoubleAssignment(id)) return null;
            return (
              <text
                key={`x2-${id}`}
                x={x} y={y}
                className="mc-double-badge"
                textAnchor="end"
                dominantBaseline="middle"
                pointerEvents="none"
              >×2</text>
            );
          })}
        </svg>
      </div>

      <div className="mouse-hint-row">
        <div className="hint-chip"><span className="hint-dot assigned-dot" /> Assigned on this layer</div>
        <div className="hint-chip"><span className="hint-dot selected-dot" /> Selected</div>
        {showAdvisory && (
          <div className="hint-chip"><span className="hint-dot risky-dot" /> Potential conflict</div>
        )}
      </div>
    </div>
  );
}
