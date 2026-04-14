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

import React, { useState, useEffect, useRef } from 'react';
import './FillInWindow.css';

export default function FillInWindow() {
  const [fields, setFields] = useState([]);
  const [values, setValues] = useState({});
  const inputRefs = useRef([]);
  const panelRef = useRef(null);

  useEffect(() => {
    // Tell main process this renderer is mounted and ready to receive fill-in data.
    window.electronAPI?.fillInReady?.();

    // Listen for ready requests on subsequent shows (window is not remounted after first use)
    window.electronAPI?.onFillInRequestReady?.(() => {
      window.electronAPI?.fillInReady?.();
    });

    if (!window.electronAPI?.onFillInShow) return;
    window.electronAPI.onFillInShow(({ fields: flds, theme }) => {
      document.documentElement.setAttribute('data-theme', theme || 'dark');
      console.log('[FillInWindow] fill-in-show received, fields:', flds);
      setFields(flds);
      const init = {};
      flds.forEach(f => { init[f] = ''; });
      setValues(init);
      setTimeout(() => inputRefs.current[0]?.focus(), 60);
    });
  }, []);

  // Auto-resize window to match panel content height
  useEffect(() => {
    if (!fields.length) return;
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const el = panelRef.current;
        if (!el) return;
        const windowH = Math.ceil(el.scrollHeight);
        window.electronAPI?.resizeFillin(windowH);
      });
    });
  }, [fields]);

  function submit() {
    console.log('[FillInWindow] submit called, values:', JSON.stringify(values));
    window.electronAPI?.submitFillIn(values);
  }

  function cancel() {
    console.log('[FillInWindow] cancel called');
    window.electronAPI?.submitFillIn(null);
  }

  function onKeyDown(e, idx) {
    if (e.key === 'Enter') {
      if (idx < fields.length - 1) {
        inputRefs.current[idx + 1]?.focus();
      } else {
        submit();
      }
    }
    if (e.key === 'Escape') cancel();
  }

  if (!fields.length) return <div className="fillin-win-empty" />;

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
              onKeyDown={e => onKeyDown(e, i)}
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
