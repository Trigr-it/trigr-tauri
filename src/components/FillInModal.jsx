import React, { useRef, useState } from 'react';
import { Edit3 } from 'lucide-react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './FillInModal.css';

export default function FillInModal({ label, onSubmit, onCancel }) {
  const [value, setValue] = useState('');
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onCancel);

  return (
    <div
      className="modal-overlay fillin-overlay"
      role="dialog"
      aria-modal="true"
      onClick={onCancel}
    >
      <div className="modal-panel fillin-modal" ref={panelRef} onClick={e => e.stopPropagation()}>
        <div className="fillin-header">
          <span className="fillin-icon" aria-hidden="true">
            <Edit3 size={14} strokeWidth={1.75} />
          </span>
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
