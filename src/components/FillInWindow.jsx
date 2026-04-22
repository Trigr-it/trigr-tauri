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
import './FillInWindow.css';

export default function FillInWindow() {
  const [mode, setMode] = useState(null); // 'fillin' | 'variant'
  const [fields, setFields] = useState([]);
  const [values, setValues] = useState({});
  const [options, setOptions] = useState([]);
  const [selectedIdx, setSelectedIdx] = useState(0);
  const inputRefs = useRef([]);
  const panelRef = useRef(null);

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
        setSelectedIdx(0);
        setFields([]);
        setValues({});
      } else {
        // Fill-in fields mode (default)
        setMode('fillin');
        setFields(data.fields || []);
        const init = {};
        (data.fields || []).forEach(f => { init[f] = ''; });
        setValues(init);
        setOptions([]);
        setSelectedIdx(0);
        setTimeout(() => inputRefs.current[0]?.focus(), 60);
      }
    });
  }, []);

  // Auto-resize window to match panel content height
  useEffect(() => {
    if (!mode) return;
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const el = panelRef.current;
        if (!el) return;
        const windowH = Math.ceil(el.scrollHeight);
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

  function onFieldKeyDown(e, idx) {
    if (e.key === 'Enter') {
      if (idx < fields.length - 1) {
        inputRefs.current[idx + 1]?.focus();
      } else {
        submit();
      }
    }
    if (e.key === 'Escape') cancel();
  }

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
    }
  }, [selectedIdx, options.length]);

  // Keyboard handler for variant mode
  useEffect(() => {
    if (mode !== 'variant') return;
    window.addEventListener('keydown', onVariantKeyDown);
    return () => window.removeEventListener('keydown', onVariantKeyDown);
  }, [mode, onVariantKeyDown]);

  if (!mode) return <div className="fillin-win-empty" />;

  // ── Variant selection mode ──
  if (mode === 'variant') {
    return (
      <div className="fillin-win" ref={panelRef}>
        <div className="fillin-win-header">
          <span className="fillin-win-icon">◇</span>
          <span className="fillin-win-title">Select Variant</span>
          <button className="fillin-win-close" onClick={cancel} tabIndex={-1}>✕</button>
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
              <span className="fillin-variant-label">{label}</span>
            </div>
          ))}
        </div>
        <div className="fillin-variant-hint">
          <kbd>↑↓</kbd> navigate &nbsp; <kbd>Enter</kbd> select &nbsp; <kbd>Esc</kbd> cancel
        </div>
      </div>
    );
  }

  // ── Fill-in fields mode ──
  return (
    <div className="fillin-win" ref={panelRef}>
      <div className="fillin-win-header">
        <span className="fillin-win-icon">✎</span>
        <span className="fillin-win-title">Fill In</span>
        <button className="fillin-win-close" onClick={cancel} tabIndex={-1}>✕</button>
      </div>
      <div className="fillin-win-fields">
        {fields.map((label, i) => (
          <div key={label} className="fillin-win-field">
            <label className="fillin-win-label">{label}</label>
            <input
              ref={el => { inputRefs.current[i] = el; }}
              className="fillin-win-input"
              value={values[label] || ''}
              onChange={e => setValues(v => ({ ...v, [label]: e.target.value }))}
              onKeyDown={e => onFieldKeyDown(e, i)}
              placeholder={`Enter ${label}…`}
              spellCheck={false}
            />
          </div>
        ))}
      </div>
      <div className="fillin-win-actions">
        <button className="fillin-win-cancel" onClick={cancel}>Cancel</button>
        <button className="fillin-win-ok" onClick={submit}>Insert</button>
      </div>
    </div>
  );
}
