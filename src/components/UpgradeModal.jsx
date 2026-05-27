import React, { useRef, useState } from 'react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './UpgradeModal.css';

const WEB3FORMS_ACCESS_KEY = '7f4062ca-0332-4b78-a215-04feaf7dc9ba';

const TEAMS_LADDER_FEATURES = new Set([
  'Shared config (cross-machine sync)',
  'App-specific profiles',
  'Global variables',
]);

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export default function UpgradeModal({ featureName, onClose, onOpenSettings }) {
  const isTeamsLadder = TEAMS_LADDER_FEATURES.has(featureName);
  const isRenewal = featureName === 'Keep Trigr Pro';
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onClose);
  const [email, setEmail] = useState('');
  const [status, setStatus] = useState('idle');

  const emailValid = EMAIL_RE.test(email.trim());

  async function handleSubmit(e) {
    e.preventDefault();
    if (!emailValid || status === 'submitting') return;
    setStatus('submitting');
    try {
      const response = await fetch('https://api.web3forms.com/submit', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json',
          'Accept': 'application/json',
        },
        body: JSON.stringify({
          access_key: WEB3FORMS_ACCESS_KEY,
          subject: `Trigr Pro beta key request from ${email.trim()}`,
          email: email.trim(),
          from_name: 'Trigr Beta Request',
          message: `${email.trim()} requested a Pro beta key from inside the Trigr app.\n\nReply to this email with their TRIGR-PRO key.`,
        }),
      });
      const data = await response.json().catch(() => ({}));
      if (response.ok && data.success) {
        setStatus('success');
      } else {
        setStatus('error');
      }
    } catch (err) {
      setStatus('error');
    }
  }

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

        <div className="upgrade-pill">{isRenewal ? 'Trigr Pro' : 'Pro feature'}</div>
        <h1 className="upgrade-title" id="upgrade-title">{featureName}</h1>

        <p className="upgrade-body">
          {isRenewal
            ? "Your trial unlocked everything in Trigr Pro. Most of Trigr stays free forever; a beta key keeps the Pro features you've been using."
            : `${featureName} is part of Trigr Pro. Most of Trigr stays free forever; Pro adds the features power users reach for daily, including syncing your setup across every machine you work on.`}
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
          free for 30 days. Drop your email and we'll send your key shortly.
        </p>

        {status === 'success' ? (
          <div className="upgrade-success" role="status" aria-live="polite">
            <p>
              Got it. We'll send your beta key shortly.
            </p>
          </div>
        ) : (
          <form className="upgrade-form" onSubmit={handleSubmit} noValidate>
            <input
              type="email"
              className="upgrade-email-input"
              placeholder="you@example.com"
              value={email}
              onChange={e => setEmail(e.target.value)}
              disabled={status === 'submitting'}
              autoFocus
              required
              aria-label="Email address"
            />
            <button
              type="submit"
              className="upgrade-cta-btn"
              disabled={!emailValid || status === 'submitting'}
            >
              {status === 'submitting' ? 'Sending…' : 'Request a Beta Key'}
            </button>
            {status === 'error' && (
              <p className="upgrade-status-error" role="alert">
                Couldn't send. Check your connection and try again.
              </p>
            )}
          </form>
        )}

        {status !== 'success' && onOpenSettings && (
          <button
            className="upgrade-have-key-link"
            onClick={() => { onClose(); onOpenSettings(); }}
            type="button"
          >
            I already have a key
          </button>
        )}
        <button
          className="upgrade-skip-link"
          onClick={onClose}
          type="button"
        >
          {status === 'success' ? 'Close' : 'Maybe later'}
        </button>
      </div>
    </div>
  );
}
