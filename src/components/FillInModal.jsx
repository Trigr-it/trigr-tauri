import React, { useState } from 'react';
import './FillInModal.css';

export default function FillInModal({ label, onSubmit, onCancel }) {
  const [value, setValue] = useState('');

  return (
    <div className="fillin-overlay">
      <div className="fillin-modal">
        <div className="fillin-header">
          <span className="fillin-icon">✎</span>
          <span className="fillin-title">Fill In</span>
        </div>
        <p className="fillin-label">{label}</p>
        <input
          autoFocus
          className="fillin-input"
          value={value}
          onChange={e => setValue(e.target.value)}
          onKeyDown={e => {
            if (e.key === 'Enter') onSubmit(value);
            if (e.key === 'Escape') onCancel();
          }}
          placeholder={`Enter ${label}…`}
          spellCheck={false}
        />
        <div className="fillin-actions">
          <button className="fillin-cancel" onClick={onCancel}>Cancel</button>
          <button className="fillin-ok" onClick={() => onSubmit(value)}>Insert</button>
        </div>
      </div>
    </div>
  );
}
