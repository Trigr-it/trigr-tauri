import React, { useRef, useState } from 'react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './ProTrialModal.css';

/**
 * Post-onboarding (or migration) Pro trial offer.
 *
 * - `onAccept(status)`: called after start_trial succeeds. `status` is the
 *   updated LicenceStatus payload returned by the backend.
 * - `onDismiss()`: called when the user picks "Maybe later" or closes.
 *
 * The parent is responsible for calling markTrialOfferShown after either
 * outcome so the modal never re-fires on subsequent launches.
 */
export default function ProTrialModal({ onAccept, onDismiss }) {
  const panelRef = useRef(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  useModalKeyboard(panelRef, onDismiss);

  const handleStart = async () => {
    if (busy) return;
    setBusy(true);
    setError(null);
    try {
      const result = await window.electronAPI?.startTrial?.();
      if (result?.ok) {
        onAccept?.(result.status);
      } else {
        setError(result?.error || 'Could not start the trial. Try again from Settings.');
        setBusy(false);
      }
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  return (
    <div
      className="modal-overlay protrial-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="protrial-title"
      onClick={onDismiss}
    >
      <div className="modal-panel protrial-modal" ref={panelRef} onClick={(e) => e.stopPropagation()}>
        <button
          className="protrial-close-btn"
          onClick={onDismiss}
          type="button"
          aria-label="Close"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>

        <div className="protrial-pill">14-day free trial</div>
        <h1 className="protrial-title" id="protrial-title">Try Trigr Pro free for 14 days</h1>
        <p className="protrial-subtitle">
          Unlock everything below. No card needed, no auto-charge — when the 14 days are up, Trigr drops back to Free automatically.
        </p>

        <div className="protrial-hero-card">
          <span className="protrial-card-badge">Most popular</span>
          <h2 className="protrial-card-title">App-specific profiles</h2>
          <p className="protrial-card-body">
            The same hotkey fires different actions in different apps — Trigr auto-switches profiles based on whatever's in the foreground. Build one set for Excel, another for Photoshop, another for your IDE.
          </p>
        </div>

        <div className="protrial-feature-grid">
          <div className="protrial-feature-card">
            <h3 className="protrial-feature-title">Double-tap actions</h3>
            <p className="protrial-feature-body">
              Tap a key once for one action, twice quickly for another. Effectively doubles every key on your keyboard.
            </p>
          </div>
          <div className="protrial-feature-card">
            <h3 className="protrial-feature-title">Shared config sync</h3>
            <p className="protrial-feature-body">
              Point Trigr at a folder (Dropbox, OneDrive, network share) and your hotkeys + expansions follow you to every machine.
            </p>
          </div>
          <div className="protrial-feature-card">
            <h3 className="protrial-feature-title">Global variables</h3>
            <p className="protrial-feature-body">
              Define values once (name, email, project codes) and reuse them across every expansion. Change once, updates everywhere.
            </p>
          </div>
        </div>

        {error && <p className="protrial-error">{error}</p>}

        <button
          className="protrial-cta-btn"
          onClick={handleStart}
          type="button"
          disabled={busy}
        >
          {busy ? 'Starting your trial…' : 'Start 14-day Pro trial'}
        </button>
        <button
          className="protrial-skip-link"
          onClick={onDismiss}
          type="button"
        >
          Maybe later
        </button>
        <p className="protrial-footnote">
          Available later from Settings if you change your mind.
        </p>
      </div>
    </div>
  );
}
