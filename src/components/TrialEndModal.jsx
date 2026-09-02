import React, { useMemo, useRef } from 'react';
import { useModalKeyboard } from '../hooks/useModalKeyboard';
import { PRO_MACRO_STEPS } from './MacroPanel';
import './TrialEndModal.css';

/**
 * One-shot end-of-trial modal. Fires once when the 14-day Pro trial lapses
 * with no key entered (App.jsx `trialJustEnded`), then `mark_trial_end_shown`
 * pins it closed for good.
 *
 * The body is usage-aware: `usage` is `get_trial_usage(trial_started_at)`
 * (per-trigger fire counts across the trial) and each Pro feature is turned
 * into a row ONLY when the user actually exercised it. A feature they never
 * touched is dropped, never rendered as "0" (a zero reads as "you don't need
 * this"). With no usage at all the modal falls back to a short generic list
 * of what Pro adds, without numbers.
 *
 * `usage` is null while the query is in flight; the rows render once it lands.
 */

const EMPTY_USAGE = { triggers: [], autocorrect: 0 };

// Rows shown in full; the rest collapse into one "Also used" line so the
// Keep Pro button stays above the fold instead of behind a scroll.
const MAX_ROWS = 5;

const plural = (n, one, many) => `${n} ${n === 1 ? one : many}`;

/** "Excel", "Excel and Chrome", "Excel, Chrome and 2 more". */
function joinApps(names) {
  const list = [...names];
  if (list.length <= 2) return list.join(' and ');
  return `${list[0]}, ${list[1]} and ${plural(list.length - 2, 'more', 'more')}`;
}

/** Display name from a linkedApp path: "C:\...\EXCEL.EXE" -> "Excel". */
function appDisplayName(path) {
  const stem = String(path || '').split(/[\\/]/).pop().replace(/\.exe$/i, '');
  if (!stem) return '';
  return stem === stem.toUpperCase() ? stem[0] + stem.slice(1).toLowerCase() : stem;
}

/**
 * Turn raw usage + live config into the rows shown. Exported for tests /
 * the dev bridge; pure.
 */
export function buildTrialLossRows({ usage, assignments, profileSettings, radialLayouts, sharedActive }) {
  const u = usage || EMPTY_USAGE;
  const rows = [];

  // Fire counts by full storage key, plus expansion fires by trigger word.
  const byKey = new Map();
  const expansionFires = new Map();
  for (const t of u.triggers || []) {
    const key = t?.trigger_key;
    if (!key) continue;
    const n = Number(t.count) || 0;
    byKey.set(key, (byKey.get(key) || 0) + n);
    if (t.action_type === 'expansion') expansionFires.set(key, (expansionFires.get(key) || 0) + n);
  }
  const sumKeys = (pred) => {
    let n = 0;
    for (const [k, c] of byKey) if (pred(k)) n += c;
    return n;
  };

  // 1. App-specific profiles: fires whose profile segment is app-linked.
  const linked = new Map(); // profile -> app display name
  for (const [profile, s] of Object.entries(profileSettings || {})) {
    if (s?.linkedApp) linked.set(profile, appDisplayName(s.linkedApp) || profile);
  }
  if (linked.size) {
    const appsHit = new Set();
    let fires = 0;
    for (const [k, c] of byKey) {
      const profile = k.split('::')[0];
      if (linked.has(profile)) { fires += c; appsHit.add(linked.get(profile)); }
    }
    if (fires > 0) {
      rows.push({
        id: 'app-profiles',
        weight: fires,
        title: 'App-specific profiles',
        stat: `${plural(fires, 'action', 'actions')} fired inside ${joinApps(appsHit)}`,
        loss: 'Profiles no longer switch with the app in front. Those hotkeys fall back to your Default profile.',
      });
    }
  }

  // 2. Double press + 3. hold triggers (storage-key suffixes).
  const doubleFires = sumKeys((k) => k.split('::').includes('double'));
  if (doubleFires > 0) {
    rows.push({
      id: 'double-press',
      weight: doubleFires,
      title: 'Double-press actions',
      stat: `${plural(doubleFires, 'double-press action', 'double-press actions')} fired`,
      loss: 'Double-press bindings stop firing. The single-press action on the same key keeps working.',
    });
  }
  const holdFires = sumKeys((k) => k.split('::').includes('hold'));
  if (holdFires > 0) {
    rows.push({
      id: 'hold',
      weight: holdFires,
      title: 'Hold triggers',
      stat: `${plural(holdFires, 'hold action', 'hold actions')} fired`,
      loss: 'Hold bindings stop firing.',
    });
  }

  // 4. Autocorrect.
  const autocorrect = Number(u.autocorrect) || 0;
  if (autocorrect > 0) {
    rows.push({
      id: 'autocorrect',
      weight: autocorrect,
      title: 'Autocorrect',
      stat: `${plural(autocorrect, 'typo', 'typos')} fixed for you`,
      loss: 'Autocorrect is off. Typos stay as typed.',
    });
  }

  // 5-9. Expansion features, matched on the expansion's trigger word.
  const expansions = Object.entries(assignments || {})
    .filter(([k, v]) => k.startsWith('GLOBAL::EXPANSION::') && !v?.data?.isAlias)
    .map(([k, v]) => ({ trigger: k.slice('GLOBAL::EXPANSION::'.length), data: v?.data || {} }));
  const firesOf = (list) => list.reduce((n, e) => n + (expansionFires.get(e.trigger) || 0), 0);
  const content = (e) => `${e.data.html || ''}\n${e.data.text || ''}`;

  const withVariants = expansions.filter((e) => Array.isArray(e.data.options) && e.data.options.length > 1);
  const variantFires = firesOf(withVariants);
  if (variantFires > 0) {
    rows.push({
      id: 'variants',
      weight: variantFires,
      title: 'Expansion variants',
      stat: `${plural(variantFires, 'fire', 'fires')} across ${plural(withVariants.length, 'expansion', 'expansions')} with variants`,
      loss: 'Only the first variant fires now. The picker is gone until you upgrade.',
    });
  }

  const withVars = expansions.filter((e) => content(e).includes('{{'));
  const varFires = firesOf(withVars);
  if (varFires > 0) {
    rows.push({
      id: 'global-variables',
      weight: varFires,
      title: 'Global variables',
      stat: `${plural(varFires, 'expansion fire', 'expansion fires')} using global variables`,
      loss: 'Variable tokens are left as literal text instead of filling in.',
    });
  }

  const withFormulas = expansions.filter((e) => /\{=|\{if\b|\{set\b/.test(content(e)));
  const formulaFires = firesOf(withFormulas);
  if (formulaFires > 0) {
    rows.push({
      id: 'formulas',
      weight: formulaFires,
      title: 'Formulas and conditions',
      stat: `${plural(formulaFires, 'expansion fire', 'expansion fires')} using formulas or conditions`,
      loss: 'Formula and conditional blocks are left as literal text instead of evaluating.',
    });
  }

  const images = expansions.filter((e) => e.data.expansionType === 'image');
  const imageFires = firesOf(images);
  if (imageFires > 0) {
    rows.push({
      id: 'image-expansions',
      weight: imageFires,
      title: 'Image expansions',
      stat: `${plural(imageFires, 'image', 'images')} pasted by expansion`,
      loss: 'Image expansions stop firing. The images stay saved for when you upgrade.',
    });
  }

  const inSubFolders = expansions.filter((e) => typeof e.data.category === 'string' && e.data.category.includes('/'));
  if (inSubFolders.length > 0) {
    rows.push({
      id: 'sub-folders',
      weight: inSubFolders.length,
      title: 'Expansion sub-folders',
      stat: `${plural(inSubFolders.length, 'expansion', 'expansions')} filed in sub-folders`,
      loss: 'Sub-folders lock. The expansions inside them still fire, but you cannot file new ones.',
    });
  }

  // 10. Pro macro steps: runs of macros that contain at least one.
  const proMacroKeys = Object.entries(assignments || {})
    .filter(([, v]) => v?.type === 'macro' && (v.data?.steps || []).some((s) => PRO_MACRO_STEPS.has(s?.type)))
    .map(([k]) => k);
  if (proMacroKeys.length) {
    const runs = proMacroKeys.reduce((n, k) => n + (byKey.get(k) || 0), 0);
    if (runs > 0) {
      rows.push({
        id: 'pro-macro-steps',
        weight: runs,
        title: 'Pro macro steps',
        stat: `${plural(runs, 'run', 'runs')} of ${plural(proMacroKeys.length, 'macro', 'macros')} using Pro steps`,
        loss: 'Wait for Pixel, Text and Window, Sort Files, Change Audio Output and Run AHK Script steps are skipped when those macros run.',
      });
    }
  }

  // 11. Extra radial layouts (setup count; fires cannot be attributed).
  const extraLayouts = Array.isArray(radialLayouts) ? radialLayouts.length : 0;
  if (extraLayouts > 0) {
    rows.push({
      id: 'radial-layouts',
      weight: extraLayouts,
      title: 'Radial layouts per device',
      stat: `${plural(extraLayouts, 'extra radial layout', 'extra radial layouts')} set up`,
      loss: 'This device falls back to the Default wheel.',
    });
  }

  // 12. Shared config sync (state, not a count).
  if (sharedActive) {
    rows.push({
      id: 'shared-config',
      weight: Number.MAX_SAFE_INTEGER, // losing sync is the biggest loss; always shown first
      title: 'Shared config sync',
      stat: 'Your setup is syncing from a shared folder',
      loss: 'Sync pauses. After the grace period Keyfire copies your config to this machine and works from the local copy.',
    });
  }

  // Biggest losses first; the modal shows MAX_ROWS in full and names the rest.
  rows.sort((a, b) => (b.weight || 0) - (a.weight || 0));
  return rows;
}

const GENERIC_ROWS = [
  { id: 'g-app-profiles', title: 'App-specific profiles', loss: 'The same hotkey doing different things in Excel, your browser and your IDE, switching automatically.' },
  { id: 'g-double-press', title: 'Double-press and hold actions', loss: 'Two or three actions on every key instead of one.' },
  { id: 'g-shared-config', title: 'Shared config sync', loss: 'One setup across every machine you work on, through a folder you already sync.' },
  { id: 'g-autocorrect', title: 'Autocorrect', loss: 'Common typos fixed as you type, in every app.' },
  { id: 'g-variants', title: 'Expansion variants', loss: 'Several versions of one expansion, picked from a popup when you type it.' },
  { id: 'g-clipboard', title: '30-day clipboard history', loss: 'Clipboard history kept for 30 days instead of 7, with text extracted from screenshots.' },
];

export default function TrialEndModal({
  usage,
  assignments,
  profileSettings,
  radialLayouts,
  sharedActive = false,
  onKeepPro,
  onClose,
}) {
  const panelRef = useRef(null);
  useModalKeyboard(panelRef, onClose);

  const loading = usage == null;
  const rows = useMemo(
    () => (loading ? [] : buildTrialLossRows({ usage, assignments, profileSettings, radialLayouts, sharedActive })),
    [loading, usage, assignments, profileSettings, radialLayouts, sharedActive],
  );
  const hasUsage = rows.length > 0;
  const shown = hasUsage ? rows.slice(0, MAX_ROWS) : GENERIC_ROWS;
  const overflow = hasUsage ? rows.slice(MAX_ROWS) : [];

  return (
    <div
      className="modal-overlay trialend-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="trialend-title"
      onClick={onClose}
    >
      <div className="modal-panel trialend-modal" ref={panelRef} onClick={(e) => e.stopPropagation()}>
        <button
          className="trialend-close-btn"
          onClick={onClose}
          type="button"
          aria-label="Close"
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
          </svg>
        </button>

        <div className="trialend-pill">Pro trial ended</div>
        <h1 className="trialend-title" id="trialend-title">Your 14-day Pro trial has ended</h1>
        <p className="trialend-subtitle">
          {hasUsage
            ? 'Keyfire has dropped back to Free. Everything you built is still saved, but the Pro features you used during the trial have stopped working:'
            : 'Keyfire has dropped back to Free. Your hotkeys, expansions and clipboard history keep working. Pro features are locked until you add a key.'}
        </p>

        {!loading && (
          <ul className={`trialend-rows${hasUsage ? '' : ' trialend-rows--generic'}`}>
            {shown.map((r) => (
              <li key={r.id} className="trialend-row">
                <div className="trialend-row-head">
                  <span className="trialend-row-title">{r.title}</span>
                  {r.stat && <span className="trialend-row-stat">{r.stat}</span>}
                </div>
                <p className="trialend-row-loss">{r.loss}</p>
              </li>
            ))}
          </ul>
        )}
        {overflow.length > 0 && (
          <p className="trialend-overflow">
            Also used during the trial: {overflow.map((r) => r.title).join(', ')}.
          </p>
        )}

        <button className="trialend-cta-btn" onClick={onKeepPro} type="button">
          Keep Keyfire Pro
        </button>
        <button className="trialend-secondary-btn" onClick={onClose} type="button">
          Continue with Free
        </button>
        <p className="trialend-footnote">
          Already have a key? Paste it under Settings, Licence.
        </p>
      </div>
    </div>
  );
}
