import React, { useMemo, useRef, useState } from 'react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import './AppProfilesModal.css';

/**
 * One-shot offer of ready-made app-specific profiles, shown right after the
 * Pro trial announcement (so the user is on Pro and the profiles will fire).
 *
 * `templates` is `get_app_profile_templates()`: every template with
 * `installed` / `path` resolved against this machine. Installed apps are
 * listed first and pre-ticked; apps not found are still offered, collapsed,
 * unticked. Apps that already have a profile are shown as added and cannot
 * be re-imported. `null` while the detection is running.
 *
 * `onImport(ids)` is App.jsx handleImportAppProfiles and returns
 * `{ added: [names], skipped: [names], actions }` or null (Free tier: the
 * parent opens the upgrade prompt instead).
 */
export default function AppProfilesModal({ templates, existingProfiles = [], onImport, onClose }) {
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onClose);

  const loading = templates == null;
  const existing = useMemo(() => new Set(existingProfiles), [existingProfiles]);
  const installed = useMemo(() => (templates || []).filter((t) => t.installed), [templates]);
  const missing = useMemo(() => (templates || []).filter((t) => !t.installed), [templates]);

  const [selected, setSelected] = useState(null); // null = not initialised yet
  const [showMissing, setShowMissing] = useState(false);
  const [result, setResult] = useState(null);

  // Pre-tick every installed app that has no profile yet, once templates land.
  const picked = selected ?? new Set(installed.filter((t) => !existing.has(t.name)).map((t) => t.id));
  const toggle = (id) => {
    const next = new Set(picked);
    if (next.has(id)) next.delete(id); else next.add(id);
    setSelected(next);
  };

  const submit = () => {
    const r = onImport?.([...picked]);
    if (r) setResult(r);
  };

  const row = (t) => {
    const added = existing.has(t.name);
    const count = Object.keys(t.assignments || {}).length;
    return (
      <li key={t.id} className={`appprof-row${added ? ' appprof-row--added' : ''}`}>
        <label className="appprof-row-main">
          <input
            type="checkbox"
            className="appprof-check"
            checked={!added && picked.has(t.id)}
            disabled={added}
            onChange={() => toggle(t.id)}
          />
          <span className="appprof-row-text">
            <span className="appprof-row-head">
              <span className="appprof-row-name">{t.name}</span>
              <span className="appprof-row-meta">
                {t.scheme} · {count} {count === 1 ? 'action' : 'actions'}{t.radial?.length ? ' · radial wheel' : ''}
              </span>
              {added && <span className="appprof-badge appprof-badge--added">Added</span>}
              {!added && t.installed && <span className="appprof-badge">Installed</span>}
            </span>
            <span className="appprof-row-blurb">{t.blurb}</span>
          </span>
        </label>
      </li>
    );
  };

  return (
    <div
      className="modal-overlay appprof-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="appprof-title"
      onClick={onClose}
    >
      <div className="modal-panel appprof-modal" ref={panelRef} onClick={(e) => e.stopPropagation()}>
        <button className="appprof-close-btn" onClick={onClose} type="button" aria-label="Close">
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>

        <div className="appprof-pill">App profiles</div>
        <h1 className="appprof-title" id="appprof-title">
          {result ? 'Profiles added' : 'Ready-made profiles for the apps on this PC'}
        </h1>

        {result ? (
          <>
            <p className="appprof-subtitle">
              {result.added.length
                ? `${result.added.join(', ')} ${result.added.length === 1 ? 'is' : 'are'} set up with ${result.actions} ${result.actions === 1 ? 'action' : 'actions'}. Each profile switches on by itself when that app is in front.`
                : 'Nothing new to add. Those profiles already exist.'}
              {result.skipped.length ? ` Skipped ${result.skipped.join(', ')} (already added).` : ''}
            </p>
            <p className="appprof-subtitle">
              Find them in the sidebar under app profiles. Every action can be changed or removed like any other.
            </p>
            <button className="appprof-cta-btn" onClick={onClose} type="button">Done</button>
          </>
        ) : (
          <>
            <p className="appprof-subtitle">
              Each profile is a few one-chord shortcuts for things that normally take a ribbon trip or several presses, plus a radial wheel on Ctrl+Shift+Space. It switches on only while that app is in front.
            </p>

            {loading ? (
              <p className="appprof-loading">Checking installed apps…</p>
            ) : (
              <>
                {installed.length === 0 && (
                  <p className="appprof-loading">None of the supported apps were found on this PC. You can still add a profile below.</p>
                )}
                <ul className="appprof-rows">{installed.map(row)}</ul>
                {missing.length > 0 && (
                  <button
                    type="button"
                    className="appprof-more-btn"
                    onClick={() => setShowMissing((v) => !v)}
                  >
                    {showMissing ? 'Hide' : 'Show'} {missing.length} not installed here
                  </button>
                )}
                {showMissing && <ul className="appprof-rows appprof-rows--missing">{missing.map(row)}</ul>}
              </>
            )}

            <button
              className="appprof-cta-btn"
              onClick={submit}
              type="button"
              disabled={loading || picked.size === 0}
            >
              {picked.size === 0 ? 'Select profiles to add' : `Add ${picked.size} ${picked.size === 1 ? 'profile' : 'profiles'}`}
            </button>
            <button className="appprof-secondary-btn" onClick={onClose} type="button">Not now</button>
            <p className="appprof-footnote">Also available any time under Templates.</p>
          </>
        )}
      </div>
    </div>
  );
}
