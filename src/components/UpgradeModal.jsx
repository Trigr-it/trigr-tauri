import React, { useRef } from 'react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './UpgradeModal.css';

// Features that extend into the planned v2.0 Teams tier. The modal shows an
// extra paragraph for these, so Pro buyers see the upgrade path early.
const TEAMS_LADDER_FEATURES = new Set([
  'Shared config (cross-machine sync)',
  'App-specific profiles',
  'Global variables',
]);

export default function UpgradeModal({ featureName, onClose }) {
  const subject = encodeURIComponent('Trigr Pro beta key request');
  const body = encodeURIComponent(
    `Hi,\n\nI'd like a Pro beta key for Trigr. I want to try the Pro features during the 30-day testing window.\n\nThanks`
  );
  const mailto = `mailto:admin@usetrigr.com?subject=${subject}&body=${body}`;
  const isTeamsLadder = TEAMS_LADDER_FEATURES.has(featureName);
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onClose);

  return (
    <div
      className="modal-overlay upgrade-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="upgrade-title"
      onClick={onClose}
    >
      <div className="modal-panel upgrade-modal" ref={panelRef} onClick={e => e.stopPropagation()}>
        <button
          className="upgrade-close-btn"
          onClick={onClose}
          type="button"
          aria-label="Close"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
          </svg>
        </button>

        <div className="upgrade-pill">Pro feature</div>
        <h1 className="upgrade-title" id="upgrade-title">{featureName}</h1>

        <p className="upgrade-body">
          {featureName} is part of Trigr Pro. Most of Trigr stays free forever; Pro adds the
          features power users reach for daily, including syncing your setup across every
          machine you work on.
        </p>
        {isTeamsLadder && (
          <p className="upgrade-body upgrade-teams-note">
            {featureName} also extends into Trigr Teams (planned for v2.0): shared libraries,
            centrally deployed profiles, and team-wide variables. Pro users get the upgrade
            path when it ships.
          </p>
        )}
        <p className="upgrade-body">
          We're early in beta and your feedback genuinely shapes what ships next. Beta keys are
          free for 30 days. Email us and we'll send one back within a few minutes.
        </p>

        <button
          className="upgrade-cta-btn"
          onClick={() => { window.location.href = mailto; }}
          type="button"
        >
          Request a Beta Key
        </button>
        <button
          className="upgrade-skip-link"
          onClick={onClose}
          type="button"
        >
          Maybe later
        </button>
      </div>
    </div>
  );
}
