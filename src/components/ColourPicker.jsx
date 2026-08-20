import React, { useEffect, useRef, useState } from 'react';
import './ColourPicker.css';

// Canonical palette shared across every colour-choosing surface in the app
// (text expansion categories, search-template categories, radial-wheel icon
// tint). Any new picker should import this component and use these values.
export const CATEGORY_COLOURS = [
  { hex: null,      label: 'None'   },
  { hex: '#e84040', label: 'Red'    },
  { hex: '#e87040', label: 'Orange' },
  { hex: '#e8a020', label: 'Gold'   },
  { hex: '#50c878', label: 'Green'  },
  { hex: '#40b0b0', label: 'Teal'   },
  { hex: '#4a9eff', label: 'Blue'   },
  { hex: '#6a7eff', label: 'Indigo' },
  { hex: '#9a6eff', label: 'Purple' },
  { hex: '#c864ff', label: 'Violet' },
  { hex: '#ff6eb4', label: 'Pink'   },
  { hex: '#8a8799', label: 'Grey'   },
  { hex: '#c0b090', label: 'Sand'   },
];

// ── HSV/hex conversion helpers ─────────────────────────────────────────────
// Chromium's native <input type="color"> popup has no confirm button and
// closes on click-outside — Rory found that unintuitive. So we render a
// fully in-panel custom picker instead: hue slider + 2D S/V area + hex text
// + OK/Cancel. Nothing is ever OS-native, so the OK button is unambiguous.

function clamp(n, lo, hi) { return Math.max(lo, Math.min(hi, n)); }

function hexToHsv(hex) {
  const h = (hex || '').replace('#', '').trim();
  if (h.length !== 6 || !/^[0-9a-fA-F]{6}$/.test(h)) return { h: 0, s: 0, v: 100 };
  const r = parseInt(h.slice(0, 2), 16) / 255;
  const g = parseInt(h.slice(2, 4), 16) / 255;
  const b = parseInt(h.slice(4, 6), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  let hue = 0;
  if (d !== 0) {
    if (max === r) hue = ((g - b) / d) % 6;
    else if (max === g) hue = (b - r) / d + 2;
    else hue = (r - g) / d + 4;
    hue = Math.round(hue * 60);
    if (hue < 0) hue += 360;
  }
  const s = max === 0 ? 0 : Math.round((d / max) * 100);
  const v = Math.round(max * 100);
  return { h: hue, s, v };
}

function hsvToHex(h, s, v) {
  const sd = clamp(s, 0, 100) / 100;
  const vd = clamp(v, 0, 100) / 100;
  const hd = ((h % 360) + 360) % 360;
  const c = vd * sd;
  const x = c * (1 - Math.abs(((hd / 60) % 2) - 1));
  const m = vd - c;
  let r = 0, g = 0, b = 0;
  if (hd < 60)      { r = c; g = x; b = 0; }
  else if (hd < 120){ r = x; g = c; b = 0; }
  else if (hd < 180){ r = 0; g = c; b = x; }
  else if (hd < 240){ r = 0; g = x; b = c; }
  else if (hd < 300){ r = x; g = 0; b = c; }
  else              { r = c; g = 0; b = x; }
  const to255 = n => Math.round((n + m) * 255).toString(16).padStart(2, '0');
  return `#${to255(r)}${to255(g)}${to255(b)}`;
}

// value:      current selection — hex string, null, or '' (radial's auto).
// onChange:   fires with the chosen hex string, or noneHex when None is clicked.
// noneHex:    the value emitted for the "None" swatch (default null; radial passes '').
// allowCustom: append a "+" swatch that expands the in-panel custom picker.
export default function ColourPicker({
  value,
  onChange,
  presets = CATEGORY_COLOURS,
  noneHex = null,
  allowCustom = true,
}) {
  const norm = (v) => (v == null || v === '') ? null : String(v).toLowerCase();
  const currentNorm = norm(value);
  const isPreset = presets.some(p => norm(p.hex) === currentNorm);
  const isCustomActive = allowCustom && currentNorm && !isPreset;

  const [expanded, setExpanded] = useState(false);
  const [draft, setDraft] = useState(isCustomActive ? value : '#e8a020');
  const [hexInput, setHexInput] = useState(isCustomActive ? value : '#e8a020');
  const hsv = hexToHsv(draft);
  const svAreaRef = useRef(null);
  const draggingRef = useRef(null); // 'sv' | 'hue' | null

  useEffect(() => {
    if (expanded) {
      const start = isCustomActive && typeof value === 'string' ? value : '#e8a020';
      setDraft(start);
      setHexInput(start);
    }
  }, [expanded]); // eslint-disable-line react-hooks/exhaustive-deps

  function updateFromHexInput(raw) {
    setHexInput(raw);
    const cleaned = raw.trim();
    if (/^#[0-9a-fA-F]{6}$/.test(cleaned)) {
      setDraft(cleaned.toLowerCase());
    }
  }

  function applyCustom() {
    const finalHex = /^#[0-9a-fA-F]{6}$/.test(draft) ? draft.toLowerCase() : null;
    if (finalHex) onChange(finalHex);
    setExpanded(false);
  }

  // Pointer handlers for the 2D saturation/value area.
  function handleSvPointer(e) {
    const rect = svAreaRef.current?.getBoundingClientRect();
    if (!rect) return;
    const x = clamp(e.clientX - rect.left, 0, rect.width);
    const y = clamp(e.clientY - rect.top, 0, rect.height);
    const s = Math.round((x / rect.width) * 100);
    const v = 100 - Math.round((y / rect.height) * 100);
    const hex = hsvToHex(hsv.h, s, v);
    setDraft(hex);
    setHexInput(hex);
  }

  function handleSvPointerDown(e) {
    e.preventDefault();
    draggingRef.current = 'sv';
    svAreaRef.current?.setPointerCapture(e.pointerId);
    handleSvPointer(e);
  }
  function handleSvPointerMove(e) {
    if (draggingRef.current !== 'sv') return;
    handleSvPointer(e);
  }
  function handleSvPointerUp(e) {
    if (draggingRef.current === 'sv') {
      draggingRef.current = null;
      try { svAreaRef.current?.releasePointerCapture(e.pointerId); } catch {}
    }
  }

  function handleHueChange(e) {
    const newH = parseInt(e.target.value, 10);
    const hex = hsvToHex(newH, hsv.s, hsv.v);
    setDraft(hex);
    setHexInput(hex);
  }

  return (
    <div className="cf-colour-picker-wrap">
      <div className="cf-colour-picker">
        {presets.map((c) => {
          const isNone = c.hex == null;
          const selected = isNone
            ? (currentNorm == null)
            : norm(c.hex) === currentNorm;
          return (
            <button
              key={c.label}
              type="button"
              className={`cf-swatch${isNone ? ' cf-swatch-none' : ''}${selected ? ' selected' : ''}`}
              style={c.hex ? { '--swatch-color': c.hex, background: c.hex } : undefined}
              onMouseDown={e => e.preventDefault()}
              onClick={() => onChange(isNone ? noneHex : c.hex)}
              title={c.label}
            />
          );
        })}
        {allowCustom && (
          <button
            type="button"
            className={`cf-swatch cf-swatch-custom${isCustomActive ? ' selected' : ''}${expanded ? ' expanded' : ''}`}
            style={isCustomActive ? { background: value } : undefined}
            onMouseDown={e => e.preventDefault()}
            onClick={() => setExpanded(!expanded)}
            title="Custom colour"
          >
            {!isCustomActive && <span className="cf-swatch-custom-plus">+</span>}
          </button>
        )}
      </div>

      {allowCustom && expanded && (
        <div className="cf-custom-panel">
          {/* 2D saturation / value area — background is a pure hue, layered
              with white-horizontal + black-vertical gradients to produce the
              full S/V colour space. Drag or click anywhere to pick. */}
          <div
            ref={svAreaRef}
            className="cf-sv-area"
            style={{ background: `hsl(${hsv.h}, 100%, 50%)` }}
            onPointerDown={handleSvPointerDown}
            onPointerMove={handleSvPointerMove}
            onPointerUp={handleSvPointerUp}
            onPointerCancel={handleSvPointerUp}
          >
            <div className="cf-sv-white" />
            <div className="cf-sv-black" />
            <div
              className="cf-sv-marker"
              style={{ left: `${hsv.s}%`, top: `${100 - hsv.v}%` }}
            />
          </div>

          {/* Hue slider — rainbow gradient background, native range input. */}
          <input
            type="range"
            min="0"
            max="360"
            value={hsv.h}
            onChange={handleHueChange}
            className="cf-hue-slider"
            aria-label="Hue"
          />

          <div className="cf-custom-top-row">
            <div className="cf-custom-preview cf-custom-preview--static" style={{ background: draft }} />
            <div className="cf-custom-hex-wrap">
              <label className="cf-custom-hex-label">HEX</label>
              <input
                type="text"
                className="cf-custom-hex-input"
                value={hexInput}
                onChange={e => updateFromHexInput(e.target.value)}
                onKeyDown={e => {
                  e.stopPropagation();
                  if (e.key === 'Enter') { e.preventDefault(); applyCustom(); }
                  if (e.key === 'Escape') { e.preventDefault(); setExpanded(false); }
                }}
                spellCheck={false}
                maxLength={7}
                placeholder="#RRGGBB"
              />
            </div>
          </div>

          <div className="cf-custom-actions">
            <button type="button" className="cf-custom-btn cf-custom-btn-cancel" onClick={() => setExpanded(false)}>Cancel</button>
            <button type="button" className="cf-custom-btn cf-custom-btn-apply" onClick={applyCustom}>OK</button>
          </div>
        </div>
      )}
    </div>
  );
}
