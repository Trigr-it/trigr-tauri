import React, { useState, useEffect, useRef } from 'react';
import './FillInWindow.css';

export default function FillInWindow() {
  const [fields, setFields] = useState([]);
  const [values, setValues] = useState({});
  const inputRefs = useRef([]);

  useEffect(() => {
    // Tell main process this renderer is mounted and ready to receive fill-in data.
    // Main process waits for this before calling win.show() — prevents the black-box
    // flash on first launch where the window was shown before React had rendered.
    window.electronAPI?.fillInReady?.();

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
    <div className="fillin-win">
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
