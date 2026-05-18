import React, { useRef } from 'react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './ReservedShortcutModal.css';

export default function ReservedShortcutModal({
  comboDisplay,
  osFunction,
  profileName,
  onContinue,
  onCancel,
}) {
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onCancel);

  return (
    <div
      className="modal-overlay reserved-shortcut-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="reserved-shortcut-title"
      onClick={onCancel}
    >
      <div
        className="modal-panel reserved-shortcut-modal"
        ref={panelRef}
        onClick={e => e.stopPropagation()}
      >
        <button
          className="reserved-shortcut-close-btn"
          onClick={onCancel}
          type="button"
          aria-label="Close"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
          </svg>
        </button>

        <div className="reserved-shortcut-pill">
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <path d="M12 2L1 21h22L12 2z" stroke="currentColor" strokeWidth="2" strokeLinejoin="round"/>
            <line x1="12" y1="10" x2="12" y2="14" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
            <circle cx="12" cy="17" r="1" fill="currentColor"/>
          </svg>
          Reserved Windows Shortcut
        </div>

        <h1 className="reserved-shortcut-title" id="reserved-shortcut-title">
          {comboDisplay} is the Windows {osFunction} shortcut
        </h1>

        <p className="reserved-shortcut-body">
          Mapping <kbd className="reserved-shortcut-kbd">{comboDisplay}</kbd> will shadow
          Windows <strong>{osFunction}</strong> while the <strong>{profileName}</strong> profile
          is active. If this profile is your global default, {osFunction} will stop working
          system-wide.
        </p>

        <p className="reserved-shortcut-body reserved-shortcut-hint">
          Tip: a double-press mapping of {comboDisplay} leaves the single-press Windows
          shortcut working, so you can have both.
        </p>

        <div className="reserved-shortcut-actions">
          <button
            className="reserved-shortcut-cancel-btn"
            onClick={onCancel}
            type="button"
          >
            Cancel
          </button>
          <button
            className="reserved-shortcut-continue-btn"
            onClick={onContinue}
            type="button"
          >
            Continue Anyway
          </button>
        </div>
      </div>
    </div>
  );
}
