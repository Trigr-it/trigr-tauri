import React, { useRef } from 'react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './ProTrialModal.css';

/**
 * Post-onboarding (or migration) Pro trial announcement.
 *
 * The 14-day trial is auto-activated by the parent (App.jsx) BEFORE this modal
 * is shown, so this is an announcement, not an offer: it tells the user the
 * trial is live and highlights what's now unlocked. There is no accept/decline.
 *
 * - `onClose()`: called when the user dismisses (button, close icon, ESC, or
 *   backdrop click). The parent calls markTrialOfferShown so it never re-fires.
 */
export default function ProTrialModal({ onClose }) {
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onClose);

  return (
    <div
      className="modal-overlay protrial-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="protrial-title"
      onClick={onClose}
    >
      <div className="modal-panel protrial-modal" ref={panelRef} onClick={(e) => e.stopPropagation()}>
        <button
          className="protrial-close-btn"
          onClick={onClose}
          type="button"
          aria-label="Close"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>

        <div className="protrial-pill">Pro trial active</div>
        <h1 className="protrial-title" id="protrial-title">You're on Keyfire Pro, free for 14 days</h1>
        <p className="protrial-subtitle">
          Everything below is unlocked, starting now. No card needed. Keyfire drops back to Free automatically when the 14 days are up.
        </p>

        <div className="protrial-hero-grid">
          <div className="protrial-hero-card">
            <span className="protrial-card-badge">Most popular</span>
            <h2 className="protrial-card-title">App-specific profiles</h2>
            <p className="protrial-card-body">
              The same hotkey fires different actions in different apps. Keyfire auto-switches profiles based on the foreground app. Build one set for Excel, another for Photoshop, another for your IDE.
            </p>
          </div>
          <div className="protrial-hero-card">
            <span className="protrial-card-badge">Most popular</span>
            <h2 className="protrial-card-title">Shared config sync</h2>
            <p className="protrial-card-body">
              Point Keyfire at a folder (Dropbox, OneDrive, network share). Your hotkeys and expansions sync across all 3 machines your licence covers.
            </p>
          </div>
        </div>

        <div className="protrial-feature-grid">
          <div className="protrial-feature-card">
            <h3 className="protrial-feature-title">Double-tap actions</h3>
            <p className="protrial-feature-body">
              Tap a key once for one action, twice quickly for another. Doubles every key on your keyboard.
            </p>
          </div>
          <div className="protrial-feature-card">
            <h3 className="protrial-feature-title">Global variables</h3>
            <p className="protrial-feature-body">
              Define values once (name, email, project codes), reuse across every expansion. Change once, updates everywhere.
            </p>
          </div>
          <div className="protrial-feature-card">
            <h3 className="protrial-feature-title">Expansion variants</h3>
            <p className="protrial-feature-body">
              Give one text expansion several options, then pick which to insert from a quick popup when you type it.
            </p>
          </div>
        </div>

        <button
          className="protrial-cta-btn"
          onClick={onClose}
          type="button"
        >
          Let's go
        </button>
        <p className="protrial-footnote">
          Track your remaining days anytime in Settings.
        </p>
      </div>
    </div>
  );
}
