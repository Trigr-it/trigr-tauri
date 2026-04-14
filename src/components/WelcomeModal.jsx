import React from 'react';
import './WelcomeModal.css';

const FEATURES = [
  {
    icon: '⌨',
    name: 'Key Mapping',
    desc: 'Assign hotkeys and macros to any key or mouse button',
  },
  {
    icon: '✦',
    name: 'Text Expansions',
    desc: 'Type a short trigger word and expand it into full text instantly',
  },
  {
    icon: '⬡',
    name: 'Profiles',
    desc: 'Create app-specific profiles that switch automatically',
  },
];

export default function WelcomeModal({ onDismiss }) {
  return (
    <div className="welcome-overlay" role="dialog" aria-modal="true" aria-labelledby="welcome-title">
      <div className="welcome-modal">

        {/* Logo mark */}
        <div className="welcome-logo">
          <svg width="28" height="28" viewBox="0 0 20 20" fill="none" aria-hidden="true">
            <rect x="2"  y="5"  width="6"  height="4" rx="1.5" fill="var(--accent)" opacity="0.9"/>
            <rect x="10" y="5"  width="4"  height="4" rx="1.5" fill="var(--accent)" opacity="0.6"/>
            <rect x="16" y="5"  width="2"  height="4" rx="1"   fill="var(--accent)" opacity="0.4"/>
            <rect x="2"  y="11" width="4"  height="4" rx="1.5" fill="var(--accent)" opacity="0.5"/>
            <rect x="8"  y="11" width="10" height="4" rx="1.5" fill="var(--accent)" opacity="0.8"/>
          </svg>
        </div>

        <h1 className="welcome-title" id="welcome-title">Trigr</h1>
        <p className="welcome-subtitle">Your visual hotkey, macro and text expansion manager</p>

        {/* Feature cards */}
        <div className="welcome-cards">
          {FEATURES.map(f => (
            <div key={f.name} className="welcome-card">
              <span className="welcome-card-icon">{f.icon}</span>
              <span className="welcome-card-name">{f.name}</span>
              <span className="welcome-card-desc">{f.desc}</span>
            </div>
          ))}
        </div>

        {/* Actions */}
        <button
          className="welcome-cta-btn"
          onClick={onDismiss}
          type="button"
          autoFocus
        >
          Get Started
        </button>
        <button
          className="welcome-skip-link"
          onClick={onDismiss}
          type="button"
        >
          Skip
        </button>

      </div>
    </div>
  );
}
