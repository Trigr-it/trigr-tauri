import React, { useState, useEffect, useLayoutEffect, useRef, useCallback } from 'react';
import { Search, SearchX } from 'lucide-react';
import './SettingsPanel.css';
import TemplatesPanel from './TemplatesPanel';
import { friendlyKeyName } from './keyboardLayout';
import { openFeedback } from '../utils/feedback';

const GLOBAL_INPUT_METHODS = [
  { id: 'direct',       label: 'Type Each Key',     hint: 'Simulates real key presses. Works in CAD, games, and password fields.' },
  { id: 'shift-insert', label: 'Paste All at Once', hint: 'Fastest for long text. Sends everything in a single paste.' },
];

const MACRO_SPEED_PRESETS = [
  { id: 'safe',    label: 'Safe',    hint: 'Maximum compatibility. Works in all apps.',  keystrokeDelay: 30, macroTriggerDelay: 150, doubleTapWindow: 300 },
  { id: 'fast',    label: 'Fast',    hint: 'Reduced delays. Good for most apps.',        keystrokeDelay: 15, macroTriggerDelay: 75,  doubleTapWindow: 200 },
  { id: 'custom',  label: 'Custom',  hint: 'Manual slider control' },
];

// ── Excluded-apps editor ────────────────────────────────────────────────────
// Chips list + manual add + pick-from-open-windows dropdown. Normalization
// (lowercase, strip .exe, dedupe) is performed by the parent before persisting
// to Rust, so this component can pass raw strings through.
function ClipboardExcludedAppsEditor({ apps, onChange }) {
  const [typed, setTyped]               = useState('');
  const [pickerOpen, setPickerOpen]     = useState(false);
  const [openWindows, setOpenWindows]   = useState(null);
  const pickerRef = useRef(null);
  const pickerDropdownRef = useRef(null);

  useEffect(() => {
    if (!pickerOpen) return;
    function onDown(e) {
      if (pickerRef.current && !pickerRef.current.contains(e.target)) setPickerOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [pickerOpen]);

  // Flip the "Pick from open apps" dropdown upward when its default below-button
  // position would clip the viewport. The clipboard section sits mid-panel, so
  // the picker often opens near the bottom of the visible settings area.
  // Remeasures when openWindows loads (placeholder → real list changes height).
  useLayoutEffect(() => {
    if (!pickerOpen || !pickerDropdownRef.current) return;
    const el = pickerDropdownRef.current;
    el.style.top = '';
    el.style.bottom = '';
    el.style.marginTop = '';
    el.style.marginBottom = '';
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = 'auto';
      el.style.bottom = '100%';
      el.style.marginTop = '0';
      el.style.marginBottom = '4px';
    }
  }, [pickerOpen, openWindows]);

  const handleRemove = (app) => {
    onChange?.((apps || []).filter(a => a !== app));
  };

  const handleAddTyped = (e) => {
    e?.preventDefault?.();
    const v = typed.trim();
    if (!v) return;
    onChange?.([...(apps || []), v]);
    setTyped('');
  };

  const handleOpenPicker = async () => {
    if (pickerOpen) { setPickerOpen(false); return; }
    setOpenWindows(null);
    setPickerOpen(true);
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      const list = await invoke('list_open_windows');
      // Dedupe by process name. We don't care about per-window titles here.
      const seen = new Set();
      const unique = [];
      for (const w of (list || [])) {
        const p = (w?.process || '').toLowerCase();
        if (!p || seen.has(p)) continue;
        seen.add(p);
        unique.push(w.process);
      }
      unique.sort((a, b) => a.localeCompare(b));
      setOpenWindows(unique);
    } catch {
      setOpenWindows([]);
    }
  };

  const handlePickProcess = (proc) => {
    onChange?.([...(apps || []), proc]);
    setPickerOpen(false);
  };

  return (
    <div className="settings-excluded-apps">
      <div className="settings-excluded-apps-header">
        <span className="settings-toggle-label">Excluded apps</span>
        <span className="settings-toggle-sub">
          Clipboard copies from these apps are silently ignored. Useful for password managers and terminals.
        </span>
      </div>

      {(apps && apps.length > 0) ? (
        <div className="settings-excluded-chips">
          {apps.map(app => (
            <span key={app} className="settings-excluded-chip">
              <span className="settings-excluded-chip-label">{app}</span>
              <button
                type="button"
                className="settings-excluded-chip-x"
                onClick={() => handleRemove(app)}
                aria-label={`Remove ${app}`}
                title={`Remove ${app}`}
              >×</button>
            </span>
          ))}
        </div>
      ) : (
        <div className="settings-excluded-empty">No apps excluded.</div>
      )}

      <form className="settings-excluded-add" onSubmit={handleAddTyped}>
        <input
          type="text"
          className="form-input settings-excluded-input"
          placeholder="Process name (e.g. keepass)"
          value={typed}
          onChange={e => setTyped(e.target.value)}
        />
        <button
          type="submit"
          className="settings-action-btn settings-excluded-add-btn"
          disabled={!typed.trim()}
        >Add</button>
        <div ref={pickerRef} className="settings-excluded-picker-wrap">
          <button
            type="button"
            className="settings-action-btn settings-excluded-pick-btn"
            onClick={handleOpenPicker}
            title="Pick from currently open apps"
          >
            Pick from open apps ▾
          </button>
          {pickerOpen && (
            <div className="settings-excluded-picker-dropdown" ref={pickerDropdownRef}>
              {openWindows === null ? (
                <div className="settings-excluded-picker-loading">Loading…</div>
              ) : openWindows.length === 0 ? (
                <div className="settings-excluded-picker-loading">No open apps found.</div>
              ) : (
                openWindows.map(p => (
                  <div
                    key={p}
                    className="settings-excluded-picker-item"
                    onClick={() => handlePickProcess(p)}
                  >{p}</div>
                ))
              )}
            </div>
          )}
        </div>
      </form>
    </div>
  );
}

export default function SettingsPanel({
  onClose,
  macrosEnabledOnStartup,
  onToggleMacrosOnStartup,
  onExportConfig,
  onImportConfig,
  onRestoreBackup,
  globalInputMethod = 'direct',
  macroSpeed        = 'safe',
  keystrokeDelay    = 30,
  macroTriggerDelay = 150,
  doubleTapWindow   = 300,
  defaultDateFormat = 'DD/MM/YYYY',
  onUpdateGlobalSettings,
  searchOverlayHotkey      = 'Ctrl+Space',
  overlayShowAll            = true,
  overlayCloseAfterFiring   = true,
  overlayIncludeAutocorrect = false,
  onUpdateSearchSettings,
  globalPauseToggleKey  = null,
  onSetPauseKey,
  onClearPauseKey,
  voiceEnabled          = false,
  onToggleVoiceEnabled,
  voiceHotkey           = '',
  onSetVoiceKey,
  onClearVoiceKey,
  onRestartOnboarding,
  activeProfile = 'Default',
  onImportTemplate,
  onImportCadTemplate,
  isPro = false,
  licenceStatus = {},
  onLicenceStatusChange,
  onShowUpgrade,
  onShowProTrial,
  onResetTrial,
  clipboardCaptureEnabled = true,
  onToggleClipboardCapture,
  clipboardExcludedApps = [],
  onUpdateClipboardExcludedApps,
  clipboardPasteHotkey = 'Ctrl+Shift+V',
  onSetClipboardPasteKey,
  onClearClipboardPasteKey,
}) {
  const [configPath, setConfigPath]           = useState('');
  const [startWithWindows, setStartWithWindows] = useState(false);
  const [capturingHotkey, setCapturingHotkey] = useState(false);
  const [capturedHotkey, setCapturedHotkey]   = useState(null);
  const [capturingPauseKey, setCapturingPauseKey] = useState(false);
  const [capturingVoiceKey, setCapturingVoiceKey] = useState(false);
  const [capturedVoiceKey, setCapturedVoiceKey]   = useState(null);
  const [voiceConflict, setVoiceConflict]         = useState(null);
  const [micTesting, setMicTesting]               = useState(false);
  const [micLevel, setMicLevel]                   = useState(0);
  const micTestRef = useRef(null); // { stream, audioCtx, analyser, animFrame }
  const [capturedPauseKey, setCapturedPauseKey]   = useState(null);
  const [pauseConflict, setPauseConflict]         = useState(null);
  const [capturingClipPasteKey, setCapturingClipPasteKey] = useState(false);
  const [capturedClipPasteKey, setCapturedClipPasteKey]   = useState(null);
  const [clipPasteConflict, setClipPasteConflict]         = useState(null);
  const [backupList, setBackupList]           = useState(null);
  const [confirmRestore, setConfirmRestore]   = useState(null);
  const [appVersion, setAppVersion]           = useState('');
  // Accordion state — keys match the SECTION_IDS list. Empty = all collapsed.
  // Search query temporarily overrides via isExpanded() so users can find
  // matches inside collapsed sections.
  const [expandedSections, setExpandedSections] = useState(() => new Set());
  const [clipboardRetention, setClipboardRetention] = useState(7);
  const [licenceKey, setLicenceKey]             = useState('');
  const [licenceActivating, setLicenceActivating] = useState(false);
  const [licenceError, setLicenceError]         = useState(null);
  const [licenceDeactivating, setLicenceDeactivating] = useState(false);
  const [sharedConfigPath, setSharedConfigPath] = useState(null); // null = local, string = shared
  const [sharedConfigBusy, setSharedConfigBusy] = useState(false);
  const [sharedConfigError, setSharedConfigError] = useState(null);
  const [confirmClearShared, setConfirmClearShared] = useState(false);
  const [sharedExistsPrompt, setSharedExistsPrompt] = useState(null); // { path } when needs_choice
  const [searchQuery, setSearchQuery] = useState('');

  useEffect(() => {
    window.electronAPI?.getConfigPath().then(p  => setConfigPath(p || ''));
    window.electronAPI?.getStartupEnabled().then(v => setStartWithWindows(!!v));
    window.electronAPI?.getAppVersion().then(v => setAppVersion(v || ''));
    window.electronAPI?.getSharedConfigPath?.().then(p => setSharedConfigPath(p || null));
    window.electronAPI?.getClipboardSettings?.().then(s => {
      if (s?.retention_days) setClipboardRetention(s.retention_days);
    });
    // Refresh the shared-config row if the grace-period banner triggers a
    // migration while this panel is open. Without this, the path display
    // would stay stale until the user closes + reopens Settings.
    window.electronAPI?.onSharedConfigMigrated?.(() => {
      setSharedConfigPath(null);
      window.electronAPI?.getConfigPath().then(p => setConfigPath(p || ''));
    });
  }, []);

  // ESC dismisses the currently-open inline confirmation first; closes the
  // whole settings panel only if no confirmation is active. Prevents an ESC
  // press from accidentally exiting Settings mid-flow.
  useEffect(() => {
    const handler = (e) => {
      if (e.key !== 'Escape') return;
      if (confirmClearShared) {
        e.preventDefault(); e.stopPropagation();
        setConfirmClearShared(false);
      } else if (sharedExistsPrompt) {
        e.preventDefault(); e.stopPropagation();
        setSharedExistsPrompt(null);
      } else if (confirmRestore) {
        e.preventDefault(); e.stopPropagation();
        setConfirmRestore(null);
      } else {
        // No inline confirmation active — close the whole Settings panel.
        e.preventDefault(); e.stopPropagation();
        onClose?.();
      }
    };
    window.addEventListener('keydown', handler, true);
    return () => window.removeEventListener('keydown', handler, true);
  }, [confirmClearShared, sharedExistsPrompt, confirmRestore, onClose]);

  // ── Accordion helpers ─────────────────────────────────────────────────
  // All settings sections, in JSX render order. Used by expand/collapse all.
  const SECTION_IDS = [
    'licence',
    'help-documentation',
    'starter-templates',
    'about',
    'general',
    'privacy-security',
    'global-pause',
    'quick-search',
    'clipboard',
    'voice-commands',
    'compatibility',
    'backup-restore',
  ];
  const isSearching = searchQuery.trim().length > 0;
  function isExpanded(id) {
    return isSearching || expandedSections.has(id);
  }
  function toggleSection(id) {
    setExpandedSections(prev => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  }
  function expandAllSections() {
    setExpandedSections(new Set(SECTION_IDS));
  }
  function collapseAllSections() {
    setExpandedSections(new Set());
  }

  // ── Settings search: filter visible sections by title match (DOM walk,
  // post-render). Keeps JSX intact; toggles a class instead of conditional
  // rendering so React's state tree doesn't reset on every keystroke.
  useEffect(() => {
    const root = document.querySelector('.settings-panel .settings-body');
    if (!root) return;
    const q = searchQuery.trim().toLowerCase();
    const sections = root.querySelectorAll(':scope > .settings-section');
    let visibleCount = 0;
    sections.forEach(section => {
      if (!q) {
        section.classList.remove('settings-section-hidden');
        visibleCount++;
        return;
      }
      const title = section.querySelector('.settings-section-title')?.textContent?.toLowerCase() || '';
      const bodyText = section.textContent?.toLowerCase() || '';
      const match = title.includes(q) || bodyText.includes(q);
      section.classList.toggle('settings-section-hidden', !match);
      if (match) visibleCount++;
    });
    // Toggle "no results" empty state
    const empty = root.querySelector('.settings-search-empty');
    if (empty) empty.style.display = (q && visibleCount === 0) ? 'flex' : 'none';
  }, [searchQuery]);

  const stopMicTest = useCallback(() => {
    if (micTestRef.current) {
      cancelAnimationFrame(micTestRef.current.animFrame);
      micTestRef.current.stream.getTracks().forEach(t => t.stop());
      micTestRef.current.audioCtx.close();
      micTestRef.current = null;
    }
    setMicTesting(false);
    setMicLevel(0);
  }, []);

  const startMicTest = useCallback(() => {
    if (micTesting) { stopMicTest(); return; }
    navigator.mediaDevices.getUserMedia({ audio: true })
      .then(async stream => {
        const audioCtx = new AudioContext();
        if (audioCtx.state === 'suspended') await audioCtx.resume();
        const source = audioCtx.createMediaStreamSource(stream);
        const analyser = audioCtx.createAnalyser();
        analyser.fftSize = 256;
        analyser.smoothingTimeConstant = 0.3;
        source.connect(analyser);
        const dataArray = new Uint8Array(analyser.frequencyBinCount);

        micTestRef.current = { stream, audioCtx, analyser, animFrame: 0 };
        setMicTesting(true);

        function poll() {
          if (!micTestRef.current) return;
          analyser.getByteTimeDomainData(dataArray);
          // RMS of waveform — much more responsive than frequency averaging
          let sum = 0;
          for (let i = 0; i < dataArray.length; i++) {
            const v = (dataArray[i] - 128) / 128;
            sum += v * v;
          }
          const rms = Math.sqrt(sum / dataArray.length);
          setMicLevel(Math.min(100, Math.round(rms * 400))); // amplify for visibility
          micTestRef.current.animFrame = requestAnimationFrame(poll);
        }
        poll();
      })
      .catch(() => {
        setMicTesting(false);
        setMicLevel(0);
      });
  }, [micTesting, stopMicTest]);

  function loadBackups() {
    window.electronAPI?.listBackups().then(data => setBackupList(data || { backups: [], lastKnownGood: null }));
  }

  function handleConfirmRestore(filename) {
    onRestoreBackup?.(filename);
    setConfirmRestore(null);
    setBackupList(null);
  }

  function handleToggleStartup(val) {
    setStartWithWindows(val);
    window.electronAPI?.setStartupEnabled(val);
  }

  return (
    <aside className="settings-panel">
      <div className="settings-header">
        <span className="settings-title">Settings</span>
        <button className="settings-close-btn" onClick={onClose} title="Close settings" aria-label="Close settings" type="button">
          <svg width="10" height="10" viewBox="0 0 10 10">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
          </svg>
        </button>
      </div>

      <div className="settings-body">

        {/* ── Search + accordion controls ────────────────── */}
        <div className="settings-search-bar">
          <div className="settings-search-row">
            <span className="settings-search-icon" aria-hidden="true">
              <Search size={14} strokeWidth={1.75} />
            </span>
            <input
              type="text"
              className="settings-search-input"
              placeholder="Search settings…"
              value={searchQuery}
              onChange={e => setSearchQuery(e.target.value)}
              spellCheck={false}
              autoCorrect="off"
              autoComplete="off"
            />
          </div>
          <button
            type="button"
            className="settings-accordion-btn"
            onClick={expandAllSections}
            title="Expand all sections"
          >Expand all</button>
          <button
            type="button"
            className="settings-accordion-btn"
            onClick={collapseAllSections}
            title="Collapse all sections"
          >Collapse all</button>
        </div>

        <div className="settings-search-empty" style={{ display: 'none' }}>
          <span className="settings-search-empty-icon" aria-hidden="true">
            <SearchX size={28} strokeWidth={1.5} />
          </span>
          <span className="settings-search-empty-text">No settings match "{searchQuery}"</span>
        </div>

        {/* ── LICENCE ───────────────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('licence')}
          >
            LICENCE
            <span className={`settings-accordion-chevron${isExpanded('licence') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('licence') && (<>
          {licenceStatus.key_entered ? (
            <div className="settings-licence-active">
              <div className="settings-licence-status-row">
                <span className={`settings-licence-badge ${licenceStatus.is_pro ? 'pro' : 'free'}`}>
                  {licenceStatus.is_pro ? 'PRO' : 'FREE'}
                </span>
                {licenceStatus.product_name && (
                  <span className="settings-licence-product">{licenceStatus.product_name}</span>
                )}
              </div>
              <div className="settings-licence-detail">
                Status: {licenceStatus.status || 'unknown'}
                {licenceStatus.expires_at && ` \u00b7 Expires: ${new Date(licenceStatus.expires_at).toLocaleDateString()}`}
              </div>
              {licenceStatus.status === 'expired' && (
                <p className="settings-toggle-sub">
                  Your beta key has expired. Email{' '}
                  <a href="mailto:admin@usetrigr.com?subject=Trigr%20beta%20key%20renewal">admin@usetrigr.com</a>
                  {' '}for a new one.
                </p>
              )}
              {licenceStatus.status === 'invalid' && (
                <p className="settings-toggle-sub">
                  This key failed verification. It may be from an older build, or the file may have been edited.
                  Try entering it again, or email{' '}
                  <a href="mailto:admin@usetrigr.com?subject=Trigr%20beta%20key%20issue">admin@usetrigr.com</a>.
                </p>
              )}
              <button
                type="button"
                className="settings-action-btn settings-danger-btn"
                onClick={async () => {
                  setLicenceDeactivating(true);
                  const result = await window.electronAPI?.deactivateLicence();
                  setLicenceDeactivating(false);
                  if (result?.ok && result.status) {
                    onLicenceStatusChange?.(result.status);
                  } else {
                    setLicenceError(result?.error || 'Deactivation failed');
                  }
                }}
                disabled={licenceDeactivating}
              >
                {licenceDeactivating ? 'Removing...' : 'Remove Licence'}
              </button>
              {licenceError && <div className="settings-shared-error">{licenceError}</div>}
            </div>
          ) : (
            <div className="settings-licence-entry">
              {/* Trial card — three mutually exclusive states above the key-entry form. */}
              {licenceStatus.trial_active ? (
                <div className="settings-trial-card settings-trial-card--active">
                  <div className="settings-trial-header">
                    <span className="settings-licence-badge pro">PRO TRIAL</span>
                    <span className="settings-trial-countdown">
                      {licenceStatus.trial_days_remaining} {licenceStatus.trial_days_remaining === 1 ? 'day' : 'days'} left
                    </span>
                  </div>
                  <p className="settings-toggle-sub">
                    All Pro features unlocked for the 14-day trial.
                  </p>
                  <p className="settings-toggle-sub">
                    To keep Pro after the trial ends, request a free beta key, then activate it below.
                  </p>
                  <button
                    type="button"
                    className="settings-action-btn"
                    onClick={() => onShowUpgrade?.('Keep Trigr Pro')}
                  >
                    Request a beta key
                  </button>
                </div>
              ) : licenceStatus.trial_used ? (
                <div className="settings-trial-card settings-trial-card--expired">
                  <p className="settings-toggle-sub">
                    <strong>Your Pro trial has ended.</strong> Trigr has dropped back to Free.
                  </p>
                  <p className="settings-toggle-sub">
                    Request a free beta key to continue using Pro features, then activate it below.
                  </p>
                  <button
                    type="button"
                    className="settings-action-btn settings-action-btn--primary"
                    onClick={() => onShowUpgrade?.('Keep Trigr Pro')}
                  >
                    Request a beta key
                  </button>
                </div>
              ) : (
                <div className="settings-trial-card settings-trial-card--offer">
                  <h3 className="settings-trial-title">Try Pro free for 14 days</h3>
                  <p className="settings-toggle-sub">
                    Unlock app-specific profiles, shared config sync, double-tap actions and global variables. No card needed. Trigr drops back to Free automatically when the trial ends.
                  </p>
                  <button
                    type="button"
                    className="settings-action-btn settings-action-btn--primary"
                    onClick={() => onShowProTrial?.()}
                  >
                    Start 14-day Pro trial
                  </button>
                </div>
              )}

              {import.meta.env.DEV && (
                <button
                  type="button"
                  className="settings-action-btn"
                  onClick={() => onResetTrial?.()}
                >
                  Reset Pro trial (dev)
                </button>
              )}

              <p className="settings-toggle-sub">
                Have a beta key already? Paste it below to activate.
              </p>
              <div className="settings-licence-input-row">
                <input
                  type="text"
                  className="form-input settings-licence-input"
                  placeholder="TRIGR-PRO.…"
                  value={licenceKey}
                  onChange={e => { setLicenceKey(e.target.value.trim()); setLicenceError(null); }}
                />
                <button
                  type="button"
                  className="settings-action-btn"
                  onClick={async () => {
                    if (!licenceKey) return;
                    setLicenceActivating(true);
                    setLicenceError(null);
                    const result = await window.electronAPI?.activateLicence(licenceKey);
                    setLicenceActivating(false);
                    if (result?.ok && result.status) {
                      onLicenceStatusChange?.(result.status);
                      setLicenceKey('');
                    } else {
                      setLicenceError(result?.error || 'Activation failed');
                    }
                  }}
                  disabled={licenceActivating || !licenceKey}
                >
                  {licenceActivating ? 'Activating...' : 'Activate'}
                </button>
              </div>
              {licenceError && <div className="settings-shared-error">{licenceError}</div>}
            </div>
          )}
          </>)}
        </section>

        {/* ── HELP & DOCUMENTATION ───────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('help-documentation')}
          >
            HELP &amp; DOCUMENTATION
            <span className={`settings-accordion-chevron${isExpanded('help-documentation') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('help-documentation') && (<>
          <div className="settings-help-row">
            <button
              type="button"
              className="settings-action-btn settings-help-btn"
              onClick={() => window.electronAPI?.openHelp()}
            >
              <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <circle cx="8" cy="8" r="6.5" stroke="currentColor" strokeWidth="1.4"/>
                <path d="M6.5 6.2C6.5 5.37 7.17 4.7 8 4.7s1.5.67 1.5 1.5c0 1-1.5 1.5-1.5 2.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round"/>
                <circle cx="8" cy="11.2" r="0.7" fill="currentColor"/>
              </svg>
              Open User Guide
            </button>
            <button
              type="button"
              className="settings-action-btn settings-help-btn"
              onClick={onRestartOnboarding}
            >
              <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <path d="M2 8a6 6 0 0 1 10.5-4" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round"/>
                <path d="M12.5 1.5v3h-3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round"/>
                <path d="M14 8a6 6 0 0 1-10.5 4" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round"/>
                <path d="M3.5 14.5v-3h3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round"/>
              </svg>
              Restart Onboarding Tour
            </button>
            <button
              type="button"
              className="settings-action-btn settings-feedback-btn"
              onClick={openFeedback}
            >
              <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <rect x="1.5" y="3.5" width="13" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.4"/>
                <path d="M1.5 5l6.5 4.5L14.5 5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round"/>
              </svg>
              Send Feedback
            </button>
          </div>
          </>)}
        </section>

        {/* ── STARTER TEMPLATES (accordion) ─────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('starter-templates')}
          >
            STARTER TEMPLATES
            <span className={`settings-accordion-chevron${isExpanded('starter-templates') ? ' open' : ''}`}>▾</span>
          </div>
          {!isExpanded('starter-templates') && (
            <p className="settings-section-sub">Import pre-built hotkey and expansion packs</p>
          )}
          {isExpanded('starter-templates') && (
            <div className="settings-tpl-wrap">
              <TemplatesPanel
                activeProfile={activeProfile}
                onImportTemplate={onImportTemplate}
                onImportCadTemplate={onImportCadTemplate}
              />
            </div>
          )}
        </section>

        {/* ── ABOUT ──────────────────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('about')}
          >
            ABOUT
            <span className={`settings-accordion-chevron${isExpanded('about') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('about') && (<>
          <div className="settings-about">
            <div className="settings-about-header">
              <span className="settings-about-name">Trigr</span>
              <span className="settings-about-version">{appVersion ? `v${appVersion}` : ''}</span>
            </div>
            <p className="settings-about-desc">Keyboard macro manager with global hotkeys, text expansions and autocorrect. All data stored locally.</p>
            <p className="settings-about-credits">Includes <a href="#" onClick={e => { e.preventDefault(); window.electronAPI?.openExternal('https://www.autohotkey.com'); }}>AutoHotkey</a> v1 + v2 (<a href="#" onClick={e => { e.preventDefault(); window.electronAPI?.openExternal('https://github.com/AutoHotkey/AutoHotkey'); }}>GPL v2 source code</a>).</p>
          </div>
          </>)}
        </section>

        {/* ── GENERAL ────────────────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('general')}
          >
            GENERAL
            <span className={`settings-accordion-chevron${isExpanded('general') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('general') && (<>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Start with Windows</span>
              <span className="settings-toggle-sub">Launch automatically at login</span>
            </div>
            <button
              type="button"
              className={`settings-toggle${startWithWindows ? ' on' : ''}`}
              onClick={() => handleToggleStartup(!startWithWindows)}
              role="switch"
              aria-checked={startWithWindows}
              title={startWithWindows ? 'Disable start with Windows' : 'Enable start with Windows'}
            />
          </div>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Enable macros on startup</span>
              <span className="settings-toggle-sub">Macros active immediately on launch</span>
            </div>
            <button
              type="button"
              className={`settings-toggle${macrosEnabledOnStartup ? ' on' : ''}`}
              onClick={() => onToggleMacrosOnStartup(!macrosEnabledOnStartup)}
              role="switch"
              aria-checked={macrosEnabledOnStartup}
              title={macrosEnabledOnStartup ? 'Disable macros on startup' : 'Enable macros on startup'}
            />
          </div>
          </>)}
        </section>

        {/* ── PRIVACY & SECURITY ─────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('privacy-security')}
          >
            PRIVACY &amp; SECURITY
            <span className={`settings-accordion-chevron${isExpanded('privacy-security') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('privacy-security') && (<>

          <div className="settings-privacy-block">
            <p>All your data is stored locally on this device. Trigr never transmits assignments, expansions, keystrokes or usage stats to any server. Nothing leaves your machine.</p>
            <p className="settings-config-path-row">
              Config file:
              <code className="settings-config-path" title={configPath}>{configPath || '…'}</code>
            </p>
            <button
              type="button"
              className="settings-action-btn"
              onClick={() => window.electronAPI?.openConfigFolder()}
            >
              Open config folder
            </button>
            <button
              type="button"
              className="settings-action-btn"
              onClick={() => window.electronAPI?.openLogsFolder()}
            >
              Open logs folder
            </button>
          </div>

          {/* ── Shared Config ─────────────────────────────── */}
          <div className="settings-shared-config">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Shared config <span className="pro-badge">PRO</span></span>
              <span className="settings-toggle-sub">
                Sync your config across machines via a cloud folder (OneDrive, Dropbox, Google Drive). Trigr reads and writes <code>keyforge-config.json</code> there.
              </span>
            </div>

            {sharedConfigPath ? (
              <div className="settings-shared-active">
                <div className="settings-shared-path-row">
                  <span className="settings-shared-badge">Shared</span>
                  <code className="settings-config-path" title={sharedConfigPath}>{sharedConfigPath}</code>
                </div>
                {confirmClearShared ? (
                  <div className="settings-shared-confirm">
                    <span>Revert to local config?</span>
                    <button
                      type="button"
                      className="settings-action-btn"
                      onClick={() => setConfirmClearShared(false)}
                    >Cancel</button>
                    <button
                      type="button"
                      className="settings-action-btn settings-danger-btn"
                      onClick={async () => {
                        setSharedConfigBusy(true);
                        await window.electronAPI?.clearSharedConfigPath?.();
                        setSharedConfigPath(null);
                        setConfirmClearShared(false);
                        setSharedConfigBusy(false);
                        // Refresh displayed config path
                        window.electronAPI?.getConfigPath().then(p => setConfigPath(p || ''));
                      }}
                    >Use Local Config</button>
                  </div>
                ) : (
                  <button
                    type="button"
                    className="settings-action-btn"
                    onClick={() => setConfirmClearShared(true)}
                    disabled={sharedConfigBusy}
                  >
                    Use Local Config
                  </button>
                )}
              </div>
            ) : sharedExistsPrompt ? (
              <div className="settings-shared-exists">
                <p className="settings-shared-exists-msg">
                  A config file already exists at this location.
                </p>
                <div className="settings-shared-exists-btns">
                  <button
                    type="button"
                    className="settings-action-btn"
                    disabled={sharedConfigBusy}
                    onClick={async () => {
                      setSharedConfigBusy(true);
                      const result = await window.electronAPI?.setSharedConfigPath?.(sharedExistsPrompt.path, 'use_existing');
                      if (result?.ok) {
                        setSharedConfigPath(sharedExistsPrompt.path);
                        window.electronAPI?.getConfigPath().then(p => setConfigPath(p || ''));
                        // Reload config from the existing shared file
                        window.electronAPI?.loadConfig();
                      } else {
                        setSharedConfigError(result?.error || 'Failed to set shared config path.');
                      }
                      setSharedExistsPrompt(null);
                      setSharedConfigBusy(false);
                    }}
                  >Use Existing</button>
                  <button
                    type="button"
                    className="settings-action-btn"
                    disabled={sharedConfigBusy}
                    onClick={async () => {
                      setSharedConfigBusy(true);
                      const result = await window.electronAPI?.setSharedConfigPath?.(sharedExistsPrompt.path, 'replace');
                      if (result?.ok) {
                        setSharedConfigPath(sharedExistsPrompt.path);
                        window.electronAPI?.getConfigPath().then(p => setConfigPath(p || ''));
                      } else {
                        setSharedConfigError(result?.error || 'Failed to set shared config path.');
                      }
                      setSharedExistsPrompt(null);
                      setSharedConfigBusy(false);
                    }}
                  >Replace with Mine</button>
                  <button
                    type="button"
                    className="settings-action-btn"
                    onClick={() => setSharedExistsPrompt(null)}
                  >Cancel</button>
                </div>
              </div>
            ) : (
              <button
                type="button"
                className="settings-action-btn"
                disabled={sharedConfigBusy}
                onClick={async () => {
                  if (!isPro) { onShowUpgrade?.('Shared config (cross-machine sync)'); return; }
                  setSharedConfigError(null);
                  const folder = await window.electronAPI?.browseForFolder();
                  if (!folder) return;
                  setSharedConfigBusy(true);
                  try {
                    const result = await window.electronAPI?.setSharedConfigPath?.(folder);
                    if (result?.ok) {
                      setSharedConfigPath(folder);
                      window.electronAPI?.getConfigPath().then(p => setConfigPath(p || ''));
                    } else if (result?.needs_choice) {
                      // Config file already exists at destination — prompt user
                      setSharedExistsPrompt({ path: folder });
                    } else {
                      setSharedConfigError(result?.error || 'Failed to set shared config path.');
                    }
                  } catch (e) {
                    setSharedConfigError(String(e));
                  }
                  setSharedConfigBusy(false);
                }}
              >
                Set Shared Folder…
              </button>
            )}

            {sharedConfigError && (
              <div className="settings-shared-error">{sharedConfigError}</div>
            )}
          </div>

          <div className="settings-security-notice">
            <svg width="13" height="13" viewBox="0 0 16 16" fill="none" className="settings-notice-icon" aria-hidden="true">
              <path d="M8 2L1.5 14h13L8 2Z" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round"/>
              <line x1="8" y1="7" x2="8" y2="10.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
              <circle cx="8" cy="12.5" r="0.7" fill="currentColor"/>
            </svg>
            <span>Avoid storing passwords or sensitive credentials as text expansions. Use a password manager like Bitwarden or 1Password instead.</span>
          </div>
          </>)}
        </section>

        {/* ── GLOBAL PAUSE ───────────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('global-pause')}
          >
            GLOBAL PAUSE
            <span className={`settings-accordion-chevron${isExpanded('global-pause') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('global-pause') && (<>

          <div className="settings-pause-stack">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Pause hotkey</span>
              <span className="settings-toggle-sub">Toggles Trigr on/off from any app. Modifier required.</span>
            </div>
            <div className="settings-qs-hotkey-ctrl">
              {capturingPauseKey ? (
                <div
                  className="settings-qs-capture"
                  tabIndex={0}
                  autoFocus
                  onBlur={() => { setCapturingPauseKey(false); setCapturedPauseKey(null); setPauseConflict(null); }}
                  onKeyUp={e => { e.preventDefault(); e.stopPropagation(); }}
                  onKeyDown={async e => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (['Control','Shift','Alt','Meta'].includes(e.key)) return;
                    const mods = [];
                    if (e.ctrlKey)  mods.push('Ctrl');
                    if (e.shiftKey) mods.push('Shift');
                    if (e.altKey)   mods.push('Alt');
                    if (e.metaKey)  mods.push('Win');
                    if (mods.length === 0) return;
                    mods.sort((a, b) => ['Ctrl','Shift','Alt','Win'].indexOf(a) - ['Ctrl','Shift','Alt','Win'].indexOf(b));
                    const keyDisplay = e.key.length === 1 ? e.key.toUpperCase() : e.key;
                    const combo = [...mods, e.code].join('+');
                    const label = [...mods, keyDisplay].join('+');
                    const result = await window.electronAPI?.checkHotkeyConflict(combo, 'pause');
                    setPauseConflict(result?.conflict ? `Already used by ${result.conflictWith}. Pick a different one.` : null);
                    setCapturedPauseKey({ combo, label });
                  }}
                >
                  {capturedPauseKey ? (
                    <span className="settings-qs-captured">{capturedPauseKey.label}</span>
                  ) : (
                    <span className="settings-qs-waiting">Press combo…</span>
                  )}
                  {capturedPauseKey && !pauseConflict && (
                    <button
                      className="settings-qs-save-btn"
                      type="button"
                      onMouseDown={e => e.preventDefault()}
                      onClick={() => {
                        onSetPauseKey?.(capturedPauseKey.combo);
                        setCapturingPauseKey(false);
                        setCapturedPauseKey(null);
                        setPauseConflict(null);
                      }}
                    >
                      Save
                    </button>
                  )}
                  <button
                    className="settings-qs-cancel-btn"
                    type="button"
                    onMouseDown={e => e.preventDefault()}
                    onClick={() => { setCapturingPauseKey(false); setCapturedPauseKey(null); setPauseConflict(null); }}
                  >
                    ✕
                  </button>
                </div>
              ) : globalPauseToggleKey ? (
                <>
                  <span className="settings-qs-hotkey-badge">
                    {globalPauseToggleKey.split('+').map((p, i, arr) => (
                        <React.Fragment key={i}>
                          <kbd className="settings-qs-kbd">{friendlyKeyName(p)}</kbd>
                          {i < arr.length - 1 && <span className="settings-qs-plus">+</span>}
                        </React.Fragment>
                    ))}
                  </span>
                  <button
                    className="settings-action-btn"
                    type="button"
                    onClick={() => setCapturingPauseKey(true)}
                  >
                    Change
                  </button>
                  <button
                    className="settings-action-btn settings-danger-btn"
                    type="button"
                    onClick={() => onClearPauseKey?.()}
                    title="Remove pause hotkey"
                  >
                    Remove
                  </button>
                </>
              ) : (
                <button
                  className="settings-action-btn"
                  type="button"
                  onClick={() => setCapturingPauseKey(true)}
                >
                  Set hotkey
                </button>
              )}
            </div>
          </div>
          {pauseConflict && (
            <div className="settings-conflict-warn">{pauseConflict}</div>
          )}
          </>)}
        </section>

        {/* ── QUICK SEARCH ───────────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('quick-search')}
          >
            QUICK SEARCH
            <span className={`settings-accordion-chevron${isExpanded('quick-search') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('quick-search') && (<>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Global hotkey</span>
              <span className="settings-toggle-sub">Opens Quick Search from any app</span>
            </div>
            <div className="settings-qs-hotkey-ctrl">
              {capturingHotkey ? (
                <div
                  className="settings-qs-capture"
                  tabIndex={0}
                  autoFocus
                  onBlur={() => { setCapturingHotkey(false); setCapturedHotkey(null); }}
                  onKeyUp={e => { e.preventDefault(); e.stopPropagation(); }}
                  onKeyDown={e => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (['Control','Shift','Alt','Meta'].includes(e.key)) return;
                    const mods = [];
                    if (e.ctrlKey)  mods.push('Ctrl');
                    if (e.shiftKey) mods.push('Shift');
                    if (e.altKey)   mods.push('Alt');
                    if (e.metaKey)  mods.push('Win');
                    if (mods.length === 0) return;
                    mods.sort((a, b) => ['Ctrl','Shift','Alt','Win'].indexOf(a) - ['Ctrl','Shift','Alt','Win'].indexOf(b));
                    const keyDisplay = e.key.length === 1 ? e.key.toUpperCase() : e.key;
                    const combo = [...mods, e.code].join('+');
                    const label = [...mods, keyDisplay].join('+');
                    setCapturedHotkey({ combo, label });
                  }}
                >
                  {capturedHotkey ? (
                    <span className="settings-qs-captured">{capturedHotkey.label}</span>
                  ) : (
                    <span className="settings-qs-waiting">Press combo…</span>
                  )}
                  {capturedHotkey && (
                    <button
                      className="settings-qs-save-btn"
                      type="button"
                      onMouseDown={e => e.preventDefault()}
                      onClick={() => {
                        onUpdateSearchSettings?.({ searchOverlayHotkey: capturedHotkey.combo });
                        setCapturingHotkey(false);
                        setCapturedHotkey(null);
                      }}
                    >
                      Save
                    </button>
                  )}
                  <button
                    className="settings-qs-cancel-btn"
                    type="button"
                    onMouseDown={e => e.preventDefault()}
                    onClick={() => { setCapturingHotkey(false); setCapturedHotkey(null); }}
                  >
                    ✕
                  </button>
                </div>
              ) : (
                <>
                  <span className="settings-qs-hotkey-badge">
                    {searchOverlayHotkey.split('+').map((p, i, arr) => (
                        <React.Fragment key={i}>
                          <kbd className="settings-qs-kbd">{friendlyKeyName(p)}</kbd>
                          {i < arr.length - 1 && <span className="settings-qs-plus">+</span>}
                        </React.Fragment>
                    ))}
                  </span>
                  <button
                    className="settings-action-btn"
                    type="button"
                    onClick={() => setCapturingHotkey(true)}
                  >
                    Change
                  </button>
                </>
              )}
            </div>
          </div>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Show all items when search is empty</span>
              <span className="settings-toggle-sub">Browse everything when the search box is empty</span>
            </div>
            <button
              type="button"
              className={`settings-toggle${overlayShowAll ? ' on' : ''}`}
              onClick={() => onUpdateSearchSettings?.({ overlayShowAll: !overlayShowAll })}
              role="switch"
              aria-checked={overlayShowAll}
            />
          </div>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Close after firing</span>
              <span className="settings-toggle-sub">Dismiss the overlay after activating a result</span>
            </div>
            <button
              type="button"
              className={`settings-toggle${overlayCloseAfterFiring ? ' on' : ''}`}
              onClick={() => onUpdateSearchSettings?.({ overlayCloseAfterFiring: !overlayCloseAfterFiring })}
              role="switch"
              aria-checked={overlayCloseAfterFiring}
            />
          </div>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Include autocorrect entries</span>
              <span className="settings-toggle-sub">Include autocorrect in results (can be noisy)</span>
            </div>
            <button
              type="button"
              className={`settings-toggle${overlayIncludeAutocorrect ? ' on' : ''}`}
              onClick={() => onUpdateSearchSettings?.({ overlayIncludeAutocorrect: !overlayIncludeAutocorrect })}
              role="switch"
              aria-checked={overlayIncludeAutocorrect}
            />
          </div>
          </>)}
        </section>

        {/* ── CLIPBOARD ──────────────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('clipboard')}
          >
            CLIPBOARD
            <span className={`settings-accordion-chevron${isExpanded('clipboard') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('clipboard') && (<>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Capture clipboard history</span>
              <span className="settings-toggle-sub">
                Existing history stays accessible via the quick-paste hotkey. Clear the hotkey below if you want the overlay fully disabled.
              </span>
            </div>
            <button
              type="button"
              className={`settings-toggle${clipboardCaptureEnabled ? ' on' : ''}`}
              onClick={() => onToggleClipboardCapture?.(!clipboardCaptureEnabled)}
              role="switch"
              aria-checked={clipboardCaptureEnabled}
              title={clipboardCaptureEnabled ? 'Stop recording clipboard history' : 'Resume recording clipboard history'}
            />
          </div>

          {clipboardCaptureEnabled && (
            <>
              <div className="settings-pause-stack">
                <div className="settings-toggle-info">
                  <span className="settings-toggle-label">Quick-paste hotkey</span>
                  <span className="settings-toggle-sub">Opens the clipboard overlay from any app. Modifier required.</span>
                </div>
                <div className="settings-qs-hotkey-ctrl">
                  {capturingClipPasteKey ? (
                    <div
                      className="settings-qs-capture"
                      tabIndex={0}
                      autoFocus
                      onBlur={() => { setCapturingClipPasteKey(false); setCapturedClipPasteKey(null); setClipPasteConflict(null); }}
                      onKeyUp={e => { e.preventDefault(); e.stopPropagation(); }}
                      onKeyDown={async e => {
                        e.preventDefault();
                        e.stopPropagation();
                        if (['Control','Shift','Alt','Meta'].includes(e.key)) return;
                        const mods = [];
                        if (e.ctrlKey)  mods.push('Ctrl');
                        if (e.shiftKey) mods.push('Shift');
                        if (e.altKey)   mods.push('Alt');
                        if (e.metaKey)  mods.push('Win');
                        if (mods.length === 0) return;
                        mods.sort((a, b) => ['Ctrl','Shift','Alt','Win'].indexOf(a) - ['Ctrl','Shift','Alt','Win'].indexOf(b));
                        const keyDisplay = e.key.length === 1 ? e.key.toUpperCase() : e.key;
                        const combo = [...mods, e.code].join('+');
                        const label = [...mods, keyDisplay].join('+');
                        const result = await window.electronAPI?.checkHotkeyConflict(combo, 'clipboard_paste');
                        setClipPasteConflict(result?.conflict ? `Already used by ${result.conflictWith}. Pick a different one.` : null);
                        setCapturedClipPasteKey({ combo, label });
                      }}
                    >
                      {capturedClipPasteKey ? (
                        <span className="settings-qs-captured">{capturedClipPasteKey.label}</span>
                      ) : (
                        <span className="settings-qs-waiting">Press combo…</span>
                      )}
                      {capturedClipPasteKey && !clipPasteConflict && (
                        <button
                          className="settings-qs-save-btn"
                          type="button"
                          onMouseDown={e => e.preventDefault()}
                          onClick={() => {
                            onSetClipboardPasteKey?.(capturedClipPasteKey.combo);
                            setCapturingClipPasteKey(false);
                            setCapturedClipPasteKey(null);
                            setClipPasteConflict(null);
                          }}
                        >
                          Save
                        </button>
                      )}
                      <button
                        className="settings-qs-cancel-btn"
                        type="button"
                        onMouseDown={e => e.preventDefault()}
                        onClick={() => { setCapturingClipPasteKey(false); setCapturedClipPasteKey(null); setClipPasteConflict(null); }}
                      >
                        ✕
                      </button>
                    </div>
                  ) : clipboardPasteHotkey ? (
                    <>
                      <span className="settings-qs-hotkey-badge">
                        {clipboardPasteHotkey.split('+').map((p, i, arr) => (
                          <React.Fragment key={i}>
                            <kbd className="settings-qs-kbd">{friendlyKeyName(p)}</kbd>
                            {i < arr.length - 1 && <span className="settings-qs-plus">+</span>}
                          </React.Fragment>
                        ))}
                      </span>
                      <button
                        className="settings-action-btn"
                        type="button"
                        onClick={() => setCapturingClipPasteKey(true)}
                      >
                        Change
                      </button>
                      <button
                        className="settings-action-btn settings-danger-btn"
                        type="button"
                        onClick={() => onClearClipboardPasteKey?.()}
                        title="Remove quick-paste hotkey"
                      >
                        Remove
                      </button>
                    </>
                  ) : (
                    <button
                      className="settings-action-btn"
                      type="button"
                      onClick={() => setCapturingClipPasteKey(true)}
                    >
                      Set hotkey
                    </button>
                  )}
                </div>
              </div>
              {clipPasteConflict && (
                <div className="settings-conflict-warn">{clipPasteConflict}</div>
              )}

              <ClipboardExcludedAppsEditor
                apps={clipboardExcludedApps}
                onChange={onUpdateClipboardExcludedApps}
              />

              <div className="settings-toggle-row">
                <div className="settings-toggle-info">
                  <span className="settings-toggle-label">History retention <span className="pro-badge">PRO</span></span>
                  <span className="settings-toggle-sub">
                    Days to keep history. Free: 7. Pro: 30.
                  </span>
                </div>
                <div className="settings-retention-input">
                  <input
                    type="number"
                    className="form-input settings-retention-num"
                    min={1}
                    max={30}
                    value={clipboardRetention}
                    onChange={e => {
                      let v = parseInt(e.target.value, 10);
                      if (isNaN(v)) v = 7;
                      v = Math.max(1, Math.min(30, v));
                      // Pro gate: Free users can request up to 30 but it clamps to 7
                      // and the upgrade modal explains why.
                      if (!isPro && v > 7) {
                        onShowUpgrade?.('Extended clipboard history (up to 30 days)');
                        v = 7;
                      }
                      setClipboardRetention(v);
                      window.electronAPI?.setClipboardSettings(v);
                    }}
                  />
                  <span className="settings-retention-unit">days</span>
                </div>
              </div>
            </>
          )}

          <button
            type="button"
            className="settings-action-btn"
            onClick={async () => {
              if (window.confirm('Clear all clipboard history? This cannot be undone.')) {
                const ok = await window.electronAPI?.clearClipboardHistory();
                if (!ok) {
                  window.alert('Failed to clear clipboard history. Check the log for details.');
                }
              }
            }}
          >
            Clear Clipboard History
          </button>

          <button
            type="button"
            className="settings-action-btn"
            onClick={() => window.electronAPI?.openClipboardFolder()}
            title="Opens the AppData folder containing trigr-clipboard.db so you can manage the files manually."
          >
            Open clipboard folder
          </button>
          </>)}
        </section>

        {/* ── VOICE COMMANDS ─────────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('voice-commands')}
          >
            <span>VOICE COMMANDS <span className="pro-badge">PRO</span> <span className="experimental-badge">EXPERIMENTAL</span></span>
            <span className={`settings-accordion-chevron${isExpanded('voice-commands') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('voice-commands') && (<>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Enable voice activation</span>
              <span className="settings-toggle-sub">Trigger actions by voice. Experimental, may not work reliably in all environments.</span>
            </div>
            <button
              type="button"
              className={`settings-toggle${voiceEnabled ? ' on' : ''}`}
              onClick={() => {
                onToggleVoiceEnabled?.(!voiceEnabled);
              }}
              role="switch"
              aria-checked={voiceEnabled}
              title={voiceEnabled ? 'Disable voice activation' : 'Enable voice activation'}
            />
          </div>

          {voiceEnabled && (<>
          <div className="settings-pause-stack">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Voice hotkey</span>
              <span className="settings-toggle-sub">Activates voice mode. Speak a configured command to fire an action.</span>
            </div>
            <div className="settings-qs-hotkey-ctrl">
              {capturingVoiceKey ? (
                <div
                  className="settings-qs-capture"
                  tabIndex={0}
                  autoFocus
                  onBlur={() => { setCapturingVoiceKey(false); setCapturedVoiceKey(null); setVoiceConflict(null); }}
                  onKeyUp={e => { e.preventDefault(); e.stopPropagation(); }}
                  onKeyDown={async e => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (['Control','Shift','Alt','Meta'].includes(e.key)) return;
                    const mods = [];
                    if (e.ctrlKey)  mods.push('Ctrl');
                    if (e.shiftKey) mods.push('Shift');
                    if (e.altKey)   mods.push('Alt');
                    if (e.metaKey)  mods.push('Win');
                    if (mods.length === 0) return;
                    mods.sort((a, b) => ['Ctrl','Shift','Alt','Win'].indexOf(a) - ['Ctrl','Shift','Alt','Win'].indexOf(b));
                    const keyDisplay = e.key.length === 1 ? e.key.toUpperCase() : e.key;
                    const combo = [...mods, e.code].join('+');
                    const label = [...mods, keyDisplay].join('+');
                    const result = await window.electronAPI?.checkHotkeyConflict(combo, 'voice');
                    setVoiceConflict(result?.conflict ? `Already used by ${result.conflictWith}. Pick a different one.` : null);
                    setCapturedVoiceKey({ combo, label });
                  }}
                >
                  {capturedVoiceKey ? (
                    <span className="settings-qs-captured">{capturedVoiceKey.label}</span>
                  ) : (
                    <span className="settings-qs-waiting">Press combo…</span>
                  )}
                  {capturedVoiceKey && !voiceConflict && (
                    <button
                      className="settings-qs-save-btn"
                      type="button"
                      onMouseDown={e => e.preventDefault()}
                      onClick={() => {
                        onSetVoiceKey?.(capturedVoiceKey.combo);
                        setCapturingVoiceKey(false);
                        setCapturedVoiceKey(null);
                        setVoiceConflict(null);
                      }}
                    >
                      Save
                    </button>
                  )}
                  <button
                    className="settings-qs-cancel-btn"
                    type="button"
                    onMouseDown={e => e.preventDefault()}
                    onClick={() => { setCapturingVoiceKey(false); setCapturedVoiceKey(null); setVoiceConflict(null); }}
                  >
                    ✕
                  </button>
                </div>
              ) : voiceHotkey ? (
                <>
                  <span className="settings-qs-hotkey-badge">
                    {voiceHotkey.split('+').map((p, i, arr) => (
                        <React.Fragment key={i}>
                          <kbd className="settings-qs-kbd">{friendlyKeyName(p)}</kbd>
                          {i < arr.length - 1 && <span className="settings-qs-plus">+</span>}
                        </React.Fragment>
                    ))}
                  </span>
                  <button
                    className="settings-action-btn"
                    type="button"
                    onClick={() => setCapturingVoiceKey(true)}
                  >
                    Change
                  </button>
                  <button
                    className="settings-action-btn settings-danger-btn"
                    type="button"
                    onClick={() => onClearVoiceKey?.()}
                    title="Remove voice hotkey"
                  >
                    Remove
                  </button>
                </>
              ) : (
                <button
                  className="settings-action-btn"
                  type="button"
                  onClick={() => setCapturingVoiceKey(true)}
                >
                  Set hotkey
                </button>
              )}
            </div>
          </div>
          {voiceConflict && (
            <div className="settings-conflict-warn">{voiceConflict}</div>
          )}

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Microphone</span>
              <span className="settings-toggle-sub">Uses your Windows default input device. Change via Windows Settings &gt; System &gt; Sound.</span>
            </div>
            <button
              type="button"
              className={`settings-action-btn${micTesting ? ' settings-danger-btn' : ''}`}
              onClick={startMicTest}
            >
              {micTesting ? 'Stop' : 'Test Microphone'}
            </button>
          </div>
          {micTesting && (
            <div className="settings-mic-meter">
              <div className="settings-mic-meter-bar" style={{ width: `${micLevel}%` }} />
            </div>
          )}
          </>)}
          </>)}
        </section>

        {/* ── COMPATIBILITY ──────────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('compatibility')}
          >
            COMPATIBILITY
            <span className={`settings-accordion-chevron${isExpanded('compatibility') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('compatibility') && (<>
          <p className="settings-compat-desc">
            How Trigr injects text into other apps. Use <strong>Type Each Key</strong> for CAD and games.
          </p>

          <label className="settings-field-label">Global input method</label>
          <div className="settings-method-grid">
            {GLOBAL_INPUT_METHODS.map(m => (
              <label
                key={m.id}
                className={`settings-method-opt${globalInputMethod === m.id ? ' active' : ''}`}
              >
                <input
                  type="radio"
                  name="globalInputMethod"
                  value={m.id}
                  checked={globalInputMethod === m.id}
                  onChange={() => onUpdateGlobalSettings?.({ globalInputMethod: m.id })}
                />
                <span className="settings-method-label">{m.label}</span>
                <span className="settings-method-hint">{m.hint}</span>
              </label>
            ))}
          </div>

          <label className="settings-field-label" style={{ marginTop: 12 }}>Macro speed</label>
          <div className="settings-method-grid">
            {MACRO_SPEED_PRESETS.map(m => (
              <label
                key={m.id}
                className={`settings-method-opt${macroSpeed === m.id ? ' active' : ''}`}
              >
                <input
                  type="radio"
                  name="macroSpeed"
                  value={m.id}
                  checked={macroSpeed === m.id}
                  onChange={() => {
                    const patch = { macroSpeed: m.id };
                    if (m.keystrokeDelay != null) {
                      patch.keystrokeDelay = m.keystrokeDelay;
                      patch.macroTriggerDelay = m.macroTriggerDelay;
                      patch.doubleTapWindow = m.doubleTapWindow;
                    }
                    onUpdateGlobalSettings?.(patch);
                  }}
                />
                <span className="settings-method-label">{m.label}</span>
                <span className="settings-method-hint">{m.hint}</span>
              </label>
            ))}
          </div>

          {globalInputMethod === 'direct' && (
            <div className="settings-slider-row">
              <div className="settings-slider-info">
                <span className="settings-toggle-label">Keystroke delay</span>
                <span className="settings-toggle-sub">Pause between each character</span>
              </div>
              <div className="settings-slider-ctrl">
                <input
                  type="range"
                  className="settings-slider"
                  min="0" max="200" step="5"
                  value={keystrokeDelay}
                  onChange={e => onUpdateGlobalSettings?.({ keystrokeDelay: Number(e.target.value), macroSpeed: 'custom' })}
                />
                <span className="settings-slider-val">{keystrokeDelay}ms</span>
                {keystrokeDelay !== 30 && (
                  <button
                    type="button"
                    className="settings-slider-reset"
                    onClick={() => onUpdateGlobalSettings?.({ keystrokeDelay: 30, macroSpeed: 'custom' })}
                    title="Reset to default (30ms)"
                    aria-label="Reset keystroke delay"
                  >↺</button>
                )}
              </div>
            </div>
          )}

          <div className="settings-slider-row">
            <div className="settings-slider-info">
              <span className="settings-toggle-label">Pre-execution delay</span>
              <span className="settings-toggle-sub">Pause before sending any output</span>
            </div>
            <div className="settings-slider-ctrl">
              <input
                type="range"
                className="settings-slider"
                min="0" max="500" step="10"
                value={macroTriggerDelay}
                onChange={e => onUpdateGlobalSettings?.({ macroTriggerDelay: Number(e.target.value), macroSpeed: 'custom' })}
              />
              <span className="settings-slider-val">{macroTriggerDelay}ms</span>
              {macroTriggerDelay !== 150 && (
                <button
                  type="button"
                  className="settings-slider-reset"
                  onClick={() => onUpdateGlobalSettings?.({ macroTriggerDelay: 150, macroSpeed: 'custom' })}
                  title="Reset to default (150ms)"
                  aria-label="Reset pre-execution delay"
                >↺</button>
              )}
            </div>
          </div>

          <div className="settings-slider-row">
            <div className="settings-slider-info">
              <span className="settings-toggle-label">Double-tap window</span>
              <span className="settings-toggle-sub">Max gap between two presses to register a double-tap</span>
            </div>
            <div className="settings-slider-ctrl">
              <input
                type="range"
                className="settings-slider"
                min="150" max="500" step="10"
                value={doubleTapWindow}
                onChange={e => onUpdateGlobalSettings?.({ doubleTapWindow: Number(e.target.value), macroSpeed: 'custom' })}
              />
              <span className="settings-slider-val">{doubleTapWindow}ms</span>
              {doubleTapWindow !== 300 && (
                <button
                  type="button"
                  className="settings-slider-reset"
                  onClick={() => onUpdateGlobalSettings?.({ doubleTapWindow: 300, macroSpeed: 'custom' })}
                  title="Reset to default (300ms)"
                  aria-label="Reset double-tap window"
                >↺</button>
              )}
            </div>
          </div>

          <label className="settings-field-label">Default date format</label>
          <p className="settings-compat-desc">
            Used by bare <code>{'{date}'}</code> and Date Math tokens. Explicit formats like <code>{'{date:DD/MM/YYYY}'}</code> override this.
          </p>
          <select
            className="settings-select"
            value={defaultDateFormat}
            onChange={e => onUpdateGlobalSettings?.({ defaultDateFormat: e.target.value })}
          >
            <option value="DD/MM/YYYY">DD/MM/YYYY (UK), e.g. 31/12/2026</option>
            <option value="MM/DD/YYYY">MM/DD/YYYY (US), e.g. 12/31/2026</option>
            <option value="YYYY-MM-DD">YYYY-MM-DD (ISO), e.g. 2026-12-31</option>
          </select>
          </>)}
        </section>

        {/* ── BACKUP & RESTORE ───────────────────────────── */}
        <section className="settings-section">
          <div
            className="settings-section-title settings-accordion-header"
            onClick={() => toggleSection('backup-restore')}
          >
            BACKUP &amp; RESTORE
            <span className={`settings-accordion-chevron${isExpanded('backup-restore') ? ' open' : ''}`}>▾</span>
          </div>
          {isExpanded('backup-restore') && (<>
          <p className="settings-backup-desc">
            Export to back up or move your config to another machine. Import to restore. Trigr also creates automatic backups on every launch and save.
          </p>
          <div className="settings-backup-row">
            <button
              type="button"
              className="settings-action-btn settings-export-btn"
              onClick={onExportConfig}
            >
              <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <path d="M8 2v8M5 7l3 3 3-3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
                <path d="M2 12v1a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1v-1" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
              </svg>
              Export Config
            </button>
            <button
              type="button"
              className="settings-action-btn settings-import-btn"
              onClick={onImportConfig}
            >
              <svg width="12" height="12" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <path d="M8 10V2M5 5l3-3 3 3" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
                <path d="M2 12v1a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1v-1" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
              </svg>
              Import Config
            </button>
          </div>

          {backupList === null ? (
            <button
              type="button"
              className="settings-action-btn settings-restore-toggle-btn"
              onClick={loadBackups}
            >
              Restore from Automatic Backup…
            </button>
          ) : (
            <div className="settings-backup-list-wrap">
              <div className="settings-backup-list-header">
                <span className="settings-backup-list-title">Automatic Backups</span>
                <button
                  type="button"
                  className="settings-backup-list-close"
                  onClick={() => { setBackupList(null); setConfirmRestore(null); }}
                >✕</button>
              </div>

              {confirmRestore ? (
                <div className="settings-backup-confirm">
                  <p>Restore from <strong>{
                    confirmRestore === 'keyforge-config-last-known-good.json'
                      ? 'Last Known Good'
                      : confirmRestore.replace('keyforge-config-', '').replace('.json', '')
                  }</strong>?</p>
                  <p className="settings-backup-confirm-sub">Replaces your current config. Cannot be undone.</p>
                  <div className="settings-backup-confirm-btns">
                    <button type="button" className="settings-action-btn" onClick={() => setConfirmRestore(null)}>Cancel</button>
                    <button type="button" className="settings-action-btn settings-restore-confirm-btn" onClick={() => handleConfirmRestore(confirmRestore)}>Restore</button>
                  </div>
                </div>
              ) : (
                <>
                  {backupList.lastKnownGood && (
                    <div className="settings-backup-item settings-backup-item-lkg">
                      <div className="settings-backup-item-info">
                        <span className="settings-backup-item-name">Last Known Good</span>
                        <span className="settings-backup-item-date">{backupList.lastKnownGood.date}</span>
                        <span className="settings-backup-item-summary">
                          {backupList.lastKnownGood.profileCount} profile{backupList.lastKnownGood.profileCount !== 1 ? 's' : ''},
                          {' '}{backupList.lastKnownGood.assignmentCount} assignment{backupList.lastKnownGood.assignmentCount !== 1 ? 's' : ''},
                          {' '}{backupList.lastKnownGood.expansionCount} expansion{backupList.lastKnownGood.expansionCount !== 1 ? 's' : ''}
                        </span>
                      </div>
                      <button type="button" className="settings-backup-restore-btn" onClick={() => setConfirmRestore(backupList.lastKnownGood.filename)}>Restore</button>
                    </div>
                  )}

                  {backupList.backups.length === 0 && !backupList.lastKnownGood && (
                    <p className="settings-backup-empty">No automatic backups found yet. Backups are created on each launch and save.</p>
                  )}

                  {backupList.backups.map(b => (
                    <div key={b.filename} className={`settings-backup-item${b.invalid ? ' settings-backup-item-invalid' : ''}`}>
                      <div className="settings-backup-item-info">
                        <span className="settings-backup-item-date">{b.date}</span>
                        {!b.invalid && (
                          <span className="settings-backup-item-summary">
                            {b.profileCount} profile{b.profileCount !== 1 ? 's' : ''},
                            {' '}{b.assignmentCount} assignment{b.assignmentCount !== 1 ? 's' : ''},
                            {' '}{b.expansionCount} expansion{b.expansionCount !== 1 ? 's' : ''}
                          </span>
                        )}
                        {b.invalid && <span className="settings-backup-item-invalid-label">Unreadable</span>}
                      </div>
                      {!b.invalid && (
                        <button type="button" className="settings-backup-restore-btn" onClick={() => setConfirmRestore(b.filename)}>Restore</button>
                      )}
                    </div>
                  ))}
                </>
              )}
            </div>
          )}
          </>)}
        </section>

      </div>
    </aside>
  );
}
