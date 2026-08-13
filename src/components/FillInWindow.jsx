/*
 * FILL-IN WINDOW SIZING — DO NOT MODIFY WITHOUT READING THIS
 *
 * This window uses content-based auto-resize via IPC:
 * 1. After fields load, a useEffect measures .fillin-win scrollHeight
 * 2. It calls resizeFillin(height) which invokes the fillin_resize
 *    Rust command
 * 3. Rust resizes the window to exactly match the content height
 *
 * DO NOT:
 * - Add margin to .fillin-win (causes gap between panel and window edge)
 * - Add box-shadow to .fillin-win (visible against transparent background)
 * - Add border to .fillin-win (visible against transparent background)
 * - Set fixed heights on .fillin-win or its children
 * - Remove the resize useEffect in FillInWindow.jsx
 * - Remove the fillin_resize command in lib.rs
 *
 * The window background is transparent(true) with WebView2 COM fix
 * (SetDefaultBackgroundColor alpha=0) applied in lib.rs setup().
 * Removing either will cause a white or dark box to appear around
 * the panel.
 */

import React, { useState, useEffect, useRef, useCallback } from 'react';
import { Grid2X2, Edit3 } from 'lucide-react';
import './FillInWindow.css';

// Normalise an incoming field payload into a canonical typed shape. Backend
// emits objects (typed fields) but variant-mode legacy payloads or older Rust
// builds may send bare strings — coerce those to a text field.
function normaliseField(raw) {
  if (typeof raw === 'string') {
    return { label: raw, kind: 'text', options: [], default: null };
  }
  const kind = (raw?.kind || 'text').toLowerCase();
  const allowedKinds = ['text', 'multiline', 'dropdown', 'checkbox', 'number', 'date'];
  return {
    label: raw?.label || '',
    kind: allowedKinds.includes(kind) ? kind : 'text',
    options: Array.isArray(raw?.options) ? raw.options : [],
    default: raw?.default ?? null,
  };
}

// Seed an initial value for a field from its default. Different kinds need
// different empty-state defaults to render their input correctly.
function seedValue(field) {
  if (field.default !== null && field.default !== undefined && field.default !== '') {
    return String(field.default);
  }
  if (field.kind === 'checkbox') return 'no';
  if (field.kind === 'dropdown' && field.options.length > 0) return field.options[0];
  return '';
}

export default function FillInWindow() {
  const [mode, setMode] = useState(null); // 'fillin' | 'variant'
  const [fields, setFields] = useState([]); // normalised FillInField objects
  const [values, setValues] = useState({}); // keyed by label
  const [options, setOptions] = useState([]);
  const [previews, setPreviews] = useState([]);
  const [selectedIdx, setSelectedIdx] = useState(0);
  const inputRefs = useRef([]);
  const panelRef = useRef(null);

  // Ctrl+Shift+V intercept: the LL keyboard hook does not fire for keys while
  // a Trigr WebView2 window has focus (confirmed empirically 2026-07-03 via
  // [CLIPBOARD-FILLIN] diagnostics), so the standard clipboard-paste hotkey
  // path is unreachable from inside the fill-in. Catch it at the DOM layer and
  // call the fill-in-mode show command directly. `capture: true` ensures we
  // beat any element-level paste handler that might swallow the combo.
  useEffect(() => {
    function onKeyDownGlobal(e) {
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && (e.key === 'V' || e.key === 'v')) {
        e.preventDefault();
        e.stopPropagation();
        window.electronAPI?.showClipboardOverlayForFillIn?.();
      }
    }
    document.addEventListener('keydown', onKeyDownGlobal, true);
    return () => document.removeEventListener('keydown', onKeyDownGlobal, true);
  }, []);

  // Receive the picked clipboard text back from the popup. Inserts at the
  // caret of whichever fill-in input has focus; if focus was lost (e.g. user
  // clicked around the popup) fall back to the first field. Also updates
  // React state so the resolved expansion picks up the inserted value.
  useEffect(() => {
    if (!window.electronAPI?.onFillInInsertText) return;
    window.electronAPI.onFillInInsertText((payload) => {
      const text = payload?.text ?? '';
      if (!text) return;
      let target = document.activeElement;
      if (!target || !(target.tagName === 'INPUT' || target.tagName === 'TEXTAREA')) {
        target = inputRefs.current[0] || null;
      }
      if (!target) return;
      target.focus();
      const label = target.dataset?.fillinLabel;
      const isTextish = target.tagName === 'TEXTAREA'
        || (target.tagName === 'INPUT' && ['text', 'number', 'search', 'email', 'tel', 'url', ''].includes(target.type || ''));
      if (isTextish && typeof target.selectionStart === 'number') {
        const start = target.selectionStart;
        const end   = target.selectionEnd;
        const before = (target.value || '').slice(0, start);
        const after  = (target.value || '').slice(end);
        const next   = before + text + after;
        target.value = next;
        const caret  = start + text.length;
        try { target.setSelectionRange(caret, caret); } catch {}
        if (label) updateValue(label, next);
      } else if (label) {
        updateValue(label, text);
      }
    });
  }, []);

  useEffect(() => {
    window.electronAPI?.fillInReady?.();

    window.electronAPI?.onFillInRequestReady?.(() => {
      window.electronAPI?.fillInReady?.();
    });

    if (!window.electronAPI?.onFillInShow) return;
    window.electronAPI.onFillInShow((data) => {
      document.documentElement.setAttribute('data-theme', data.theme || 'dark');

      if (data.mode === 'variant') {
        // Variant selection mode
        setMode('variant');
        setOptions(data.options || []);
        setPreviews(data.previews || []);
        setSelectedIdx(0);
        setFields([]);
        setValues({});
      } else {
        // Fill-in fields mode (default). Coerce each raw field to typed shape
        // so the renderer always sees label/kind/options/default.
        const normalised = (data.fields || []).map(normaliseField).filter(f => f.label);
        setMode('fillin');
        setFields(normalised);
        const init = {};
        normalised.forEach(f => { init[f.label] = seedValue(f); });
        setValues(init);
        setOptions([]);
        setPreviews([]);
        setSelectedIdx(0);
        setTimeout(() => inputRefs.current[0]?.focus(), 60);
      }
    });
  }, []);

  // Auto-resize window to match panel content height.
  //
  // IMPORTANT: `panelRef.current.scrollHeight` is NOT a reliable measure of the
  // natural content height. Because `.fillin-win` has `max-height` set (so the
  // fields div can scroll internally when content exceeds the work area), and
  // `.fillin-win-fields` is a flex:1 + overflow:auto child, scrollHeight on
  // the panel returns the CAPPED (visible) height, not the true content size.
  // That creates a feedback loop: the initial small window height becomes the
  // reported scrollHeight, the resize asks for that, the window stays small.
  //
  // Measuring the three children separately works because the fields div's
  // own scrollHeight reports its overflow-aware natural content height
  // regardless of how its flex parent is sized.
  useEffect(() => {
    if (!mode) return;
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const el = panelRef.current;
        if (!el) return;
        const FRAME_PAD = 12; // must match .fillin-win-frame padding (top+bottom each)
        const header  = el.querySelector('.fillin-win-header');
        const fields  = el.querySelector('.fillin-win-fields');
        const actions = el.querySelector('.fillin-win-actions');
        const variant = el.querySelector('.fillin-variant-list');
        const variantHint = el.querySelector('.fillin-variant-hint');

        // In fill-in mode: header + fields (scrollHeight, uncapped) + actions.
        // In variant mode: header + variant list + hint.
        const naturalContent =
          (header?.offsetHeight || 0) +
          (fields?.scrollHeight || 0) +
          (actions?.offsetHeight || 0) +
          (variant?.scrollHeight || 0) +
          (variantHint?.offsetHeight || 0);

        const panelBorders = 2; // .fillin-win has 1px top + 1px bottom border
        const windowH = Math.ceil(naturalContent) + panelBorders + FRAME_PAD * 2;
        window.electronAPI?.resizeFillin(windowH);
      });
    });
  }, [mode, fields, options]);

  function submit() {
    window.electronAPI?.submitFillIn(values);
  }

  function cancel() {
    window.electronAPI?.submitFillIn(null);
  }

  function selectVariant(idx) {
    window.electronAPI?.submitFillIn({ __variant_index: String(idx) });
  }

  function updateValue(label, value) {
    setValues(v => ({ ...v, [label]: value }));
  }

  function onFieldKeyDown(e, idx, kind) {
    // Multiline accepts Enter as newline; Ctrl+Enter advances/submits.
    if (kind === 'multiline') {
      if (e.key === 'Enter' && !(e.ctrlKey || e.metaKey)) return;
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        if (idx < fields.length - 1) inputRefs.current[idx + 1]?.focus();
        else submit();
        return;
      }
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (idx < fields.length - 1) inputRefs.current[idx + 1]?.focus();
      else submit();
      return;
    }
    if (e.key === 'Escape') cancel();
  }

  // Guard against stray keystrokes reaching the picker in the first moments
  // after it opens. Windows can deliver a keyup or a buffered keystroke to the
  // fillin webview a hair after `win.show()` + `set_focus()`, and any digit in
  // that grace period would fire the wrong variant and close the picker.
  // Refreshed each time the variant mode is entered.
  const variantOpenedAtRef = useRef(0);
  useEffect(() => {
    if (mode === 'variant') variantOpenedAtRef.current = performance.now();
  }, [mode]);

  const onVariantKeyDown = useCallback((e) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSelectedIdx(i => Math.min(i + 1, options.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSelectedIdx(i => Math.max(i - 1, 0));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      selectVariant(selectedIdx);
    } else if (e.key === 'Escape') {
      cancel();
    } else if (
      !e.ctrlKey && !e.altKey && !e.metaKey && !e.shiftKey &&
      !e.repeat && e.code &&
      (performance.now() - variantOpenedAtRef.current) > 200
    ) {
      // Number-key direct fire — 1-9 fire options[0..8], 0 fires options[9].
      // Rules:
      // - e.code (not e.key) so numpad works regardless of NumLock and layout
      //   dead keys don't produce phantom digits.
      // - Modifiers excluded — including Shift so Shift+1 doesn't smash a fire
      //   through on a US layout ("!" typed intentionally).
      // - !e.repeat so a held-down number doesn't cascade into the next picker.
      // - 200ms grace period from mode entry so a stray post-show keystroke
      //   can't close a just-opened picker.
      // For >10 variants the tail needs arrow-key nav — no chord fallback.
      if (e.code.startsWith('Digit') || e.code.startsWith('Numpad')) {
        const raw = e.code.replace(/^(Digit|Numpad)/, '');
        const n = parseInt(raw, 10);
        if (!isNaN(n) && n >= 0 && n <= 9) {
          const idx = n === 0 ? 9 : n - 1;
          if (idx < options.length) {
            e.preventDefault();
            selectVariant(idx);
          }
        }
      }
    }
  }, [selectedIdx, options.length]);

  // Keyboard handler for variant mode
  useEffect(() => {
    if (mode !== 'variant') return;
    window.addEventListener('keydown', onVariantKeyDown);
    return () => window.removeEventListener('keydown', onVariantKeyDown);
  }, [mode, onVariantKeyDown]);

  function renderInput(field, idx) {
    const value = values[field.label] ?? '';
    const refCb = el => { inputRefs.current[idx] = el; };
    const commonProps = {
      ref: refCb,
      onKeyDown: e => onFieldKeyDown(e, idx, field.kind),
      spellCheck: false,
      // Reflected on the DOM element so the Ctrl+Shift+V clipboard-insert
      // handler can map document.activeElement back to its field label
      // without walking React refs.
      'data-fillin-label': field.label,
    };

    if (field.kind === 'multiline') {
      return (
        <textarea
          {...commonProps}
          className="fillin-win-input fillin-win-textarea"
          value={value}
          onChange={e => updateValue(field.label, e.target.value)}
          placeholder={`Enter ${field.label}…`}
          rows={3}
        />
      );
    }
    if (field.kind === 'dropdown') {
      return (
        <select
          {...commonProps}
          className="fillin-win-input fillin-win-select"
          value={value}
          onChange={e => updateValue(field.label, e.target.value)}
        >
          {field.options.map((opt, i) => (
            <option key={i} value={opt}>{opt}</option>
          ))}
        </select>
      );
    }
    if (field.kind === 'checkbox') {
      const checked = value === 'yes' || value === 'true';
      return (
        <label className="fillin-win-checkbox-row">
          <input
            {...commonProps}
            type="checkbox"
            className="fillin-win-checkbox"
            checked={checked}
            onChange={e => updateValue(field.label, e.target.checked ? 'yes' : 'no')}
          />
          <span className="fillin-win-checkbox-hint">{checked ? 'Yes' : 'No'}</span>
        </label>
      );
    }
    if (field.kind === 'number') {
      return (
        <input
          {...commonProps}
          type="number"
          className="fillin-win-input"
          value={value}
          onChange={e => updateValue(field.label, e.target.value)}
          placeholder={`Enter ${field.label}…`}
        />
      );
    }
    if (field.kind === 'date') {
      return (
        <input
          {...commonProps}
          type="date"
          className="fillin-win-input"
          value={value}
          onChange={e => updateValue(field.label, e.target.value)}
        />
      );
    }
    // text (default)
    return (
      <input
        {...commonProps}
        className="fillin-win-input"
        value={value}
        onChange={e => updateValue(field.label, e.target.value)}
        placeholder={`Enter ${field.label}…`}
      />
    );
  }

  if (!mode) return <div className="fillin-win-empty" />;

  // ── Variant selection mode ──
  if (mode === 'variant') {
    return (
      <div className="fillin-win-frame">
        <div className="fillin-win" ref={panelRef}>
          <div className="fillin-win-header">
            <span className="fillin-win-icon" aria-hidden="true"><Grid2X2 size={14} strokeWidth={1.75} /></span>
            <span className="fillin-win-title">Select Variant</span>
            <button className="fillin-win-close" onClick={cancel} tabIndex={-1} aria-label="Cancel">✕</button>
          </div>
          <div className="fillin-variant-list">
            {options.map((label, i) => (
              <div
                key={i}
                className={`fillin-variant-row${i === selectedIdx ? ' selected' : ''}`}
                onClick={() => selectVariant(i)}
                onMouseEnter={() => setSelectedIdx(i)}
              >
                <span className="fillin-variant-num">{i + 1}</span>
                <div className="fillin-variant-text">
                  <span className="fillin-variant-label">{label}</span>
                  {previews[i] && (
                    <span className="fillin-variant-preview">{previews[i]}</span>
                  )}
                </div>
              </div>
            ))}
          </div>
          <div className="fillin-variant-hint">
            <kbd>1</kbd>–<kbd>{options.length >= 10 ? '0' : String(options.length)}</kbd> fire &nbsp; <kbd>↑↓</kbd> navigate &nbsp; <kbd>Enter</kbd> select &nbsp; <kbd>Esc</kbd> cancel
          </div>
        </div>
      </div>
    );
  }

  // ── Fill-in fields mode ──
  return (
    <div className="fillin-win-frame">
      <div className="fillin-win" ref={panelRef}>
        <div className="fillin-win-header">
          <span className="fillin-win-icon" aria-hidden="true"><Edit3 size={14} strokeWidth={1.75} /></span>
          <span className="fillin-win-title">Fill In</span>
          <button className="fillin-win-close" onClick={cancel} tabIndex={-1} aria-label="Cancel">✕</button>
        </div>
        <div className="fillin-win-fields">
          {fields.map((field, i) => (
            <div key={field.label} className="fillin-win-field">
              <label className="fillin-win-label">{field.label}</label>
              {renderInput(field, i)}
            </div>
          ))}
        </div>
        <div className="fillin-win-actions">
          <button className="fillin-win-cancel" onClick={cancel}>Cancel</button>
          <button className="fillin-win-ok" onClick={submit}>Insert</button>
        </div>
      </div>
    </div>
  );
}
