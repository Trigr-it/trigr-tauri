import React from 'react';
import './UpgradeModal.css';

export default function UpgradeModal({ featureName, onClose }) {
  const subject = encodeURIComponent('Trigr Pro beta key request');
  const body = encodeURIComponent(
    `Hi,\n\nI'd like a Pro beta key for Trigr. I want to try the Pro features during the 30-day testing window.\n\nThanks`
  );
  const mailto = `mailto:admin@usetrigr.com?subject=${subject}&body=${body}`;

  return (
    <div
      className="upgrade-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="upgrade-title"
      onClick={onClose}
    >
      <div className="upgrade-modal" onClick={e => e.stopPropagation()}>
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
          {featureName} is part of Trigr Pro. The bulk of Trigr stays free forever; a few features
          need a Pro key.
        </p>
        <p className="upgrade-body">
          We're early in beta and your feedback genuinely shapes what ships next. Beta keys are
          free for 30 days. Email us and we'll send one back within a few minutes.
        </p>

        <a href={mailto} className="upgrade-cta-btn">
          Request a Beta Key
        </a>
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
