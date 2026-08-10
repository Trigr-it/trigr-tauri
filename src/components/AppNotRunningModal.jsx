import React, { useRef } from 'react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './AppNotRunningModal.css';

// Popped when a distilled Record Macro tries to fire but its bound target app
// isn't running. Backend precheck emits `record-macro-app-missing` with { exe,
// hint }; App.jsx registers the listener and pushes state through here.
// Per Rory's 2026-08-10 call: no auto-launch — PC startup times vary too much
// for a reliable "wait for app to open" flow. User launches manually + fires
// the macro again.
export default function AppNotRunningModal({ exe, hint, onClose }) {
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onClose);

  const displayName = hint || exe || 'the target app';

  return (
    <div
      className="modal-overlay app-missing-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="app-missing-title"
      onClick={onClose}
    >
      <div
        className="modal-panel app-missing-modal"
        ref={panelRef}
        onClick={e => e.stopPropagation()}
      >
        <button
          className="app-missing-close-btn"
          onClick={onClose}
          type="button"
          aria-label="Close"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
          </svg>
        </button>

        <div className="app-missing-pill">
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" aria-hidden="true">
            <path d="M12 2L1 21h22L12 2z" stroke="currentColor" strokeWidth="2" strokeLinejoin="round"/>
            <line x1="12" y1="10" x2="12" y2="14" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
            <circle cx="12" cy="17" r="1" fill="currentColor"/>
          </svg>
          Assigned Window Not Available
        </div>

        <h1 className="app-missing-title" id="app-missing-title">
          {displayName} isn't running
        </h1>

        <p className="app-missing-body">
          This macro was recorded against <strong>{displayName}</strong>. Keyfire
          couldn't find that window on your desktop, so the macro didn't run.
        </p>

        <p className="app-missing-body app-missing-hint">
          Open <strong>{displayName}</strong>, wait for it to load, then fire the
          macro again.
        </p>

        <div className="app-missing-actions">
          <button
            className="app-missing-ok-btn"
            onClick={onClose}
            type="button"
            autoFocus
          >
            OK
          </button>
        </div>
      </div>
    </div>
  );
}
