import React, { useRef } from 'react';
import {
  Keyboard, Type, Workflow, Search, Disc, ClipboardList,
} from 'lucide-react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './WelcomeModal.css';

const FEATURES = [
  {
    Icon: Keyboard,
    name: 'Triggers',
    desc: 'Map hotkeys to any key on your keyboard or mouse.',
    accent: 'a',
  },
  {
    Icon: Workflow,
    name: 'Macros',
    desc: 'Sequence keystrokes, clicks, delays and app launches.',
    accent: 'b',
  },
  {
    Icon: Disc,
    name: 'Radial Menu',
    desc: 'Pie menu of your most-used actions. Click a wedge or press its number to fire.',
    accent: 'c',
  },
  {
    Icon: Type,
    name: 'Text Expansions',
    desc: 'Type a short trigger word and expand it instantly anywhere.',
    accent: 'd',
  },
  {
    Icon: Search,
    name: 'Quick Search',
    desc: '{{QS}} launcher for apps, URLs and search templates.',
    accent: 'e',
  },
  {
    Icon: ClipboardList,
    name: 'Clipboard History',
    desc: '{{CB}} to recall and paste anything you’ve copied.',
    accent: 'f',
  },
];

export default function WelcomeModal({ onContinue, onSkip, onDismiss, searchOverlayHotkey = 'Ctrl+Space', clipboardPasteHotkey = 'Ctrl+Shift+V' }) {
  // Live hotkeys in the tile copy (the tour is re-runnable after rebinding).
  const spaced = (combo) => (combo ? String(combo).split('+').join(' + ') : 'No hotkey set');
  const fillDesc = (d) => d.replace('{{QS}}', spaced(searchOverlayHotkey)).replace('{{CB}}', spaced(clipboardPasteHotkey));
  // Backwards-compat: existing call sites that only pass onDismiss get the
  // same handler used for both Get Started and Skip. App.jsx splits them.
  const handleContinue = onContinue || onDismiss;
  const handleSkip = onSkip || onDismiss;
  const panelRef = useRef(null);
  // Esc used to run the permanent skip (onboarding_complete = true) — a
  // reflexive Esc on the very first screen lost the tour for good. Esc now
  // does nothing here; the explicit "Skip the tour" link remains.
  useModalKeyboard(panelRef, () => {});

  return (
    <div className="modal-overlay welcome-overlay" role="dialog" aria-modal="true" aria-labelledby="welcome-title">
      <div className="modal-panel welcome-modal" ref={panelRef}>

        {/* Header band — logo + brand + intro */}
        <div className="welcome-header">
          <svg
            className="welcome-logo-img"
            width="56"
            height="56"
            viewBox="0 0 64 64"
            role="img"
            aria-label="Keyfire"
          >
            <defs>
              <linearGradient id="welcome-trigr-base" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="#f0b942"/>
                <stop offset="100%" stopColor="#c8860a"/>
              </linearGradient>
              <linearGradient id="welcome-trigr-keytop" x1="0" y1="0" x2="0" y2="1">
                <stop offset="0%" stopColor="#ffffff"/>
                <stop offset="100%" stopColor="#e8e5dc"/>
              </linearGradient>
            </defs>
            <rect x="0" y="0" width="64" height="64" rx="9" fill="url(#welcome-trigr-base)"/>
            <rect x="7.68" y="6.4" width="48.64" height="43.52" rx="6.5" fill="url(#welcome-trigr-keytop)"/>
            <rect x="7.68" y="46.5" width="48.64" height="3.42" rx="1.5" fill="#000000" opacity="0.06"/>
            <path d="M 33 14 C 36 18, 41 23, 41 30 C 41 37, 36 41, 32 41 C 26 41, 22 37, 22 32 C 22 28, 25 26, 27 23 C 28 26, 30 27, 30 24 C 30 20, 32 17, 33 14 Z" fill="#c8860a"/>
          </svg>
          <div className="welcome-pill">WELCOME TO KEYFIRE</div>
          <h1 className="welcome-title" id="welcome-title">Out of the box</h1>
          <p className="welcome-subtitle">
            Six core surfaces, one keyboard. Here's what you'll find inside.
          </p>
        </div>

        {/* 3×2 feature grid */}
        <div className="welcome-tiles" role="list">
          {FEATURES.map(f => {
            const FeatureIcon = f.Icon;
            return (
              <div key={f.name} className={`welcome-tile welcome-tile--${f.accent}`} role="listitem">
                <span className="welcome-tile-icon" aria-hidden="true">
                  <FeatureIcon size={22} strokeWidth={1.75} />
                </span>
                <span className="welcome-tile-name">{f.name}</span>
                <span className="welcome-tile-desc">{fillDesc(f.desc)}</span>
              </div>
            );
          })}
        </div>

        {/* Actions */}
        <div className="welcome-actions">
          <button
            className="welcome-cta-btn"
            onClick={handleContinue}
            type="button"
            autoFocus
          >
            Get Started
          </button>
          <button
            className="welcome-skip-link"
            onClick={handleSkip}
            type="button"
          >
            Skip the tour
          </button>
        </div>

      </div>
    </div>
  );
}
