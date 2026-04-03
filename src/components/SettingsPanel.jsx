import React, { useState, useEffect } from 'react';
import './SettingsPanel.css';

const GLOBAL_INPUT_METHODS = [
  { id: 'direct',       label: 'Direct keystrokes',        hint: 'Simulates real keypresses — works in CAD, games, any app' },
  { id: 'shift-insert', label: 'Clipboard (Shift+Insert)', hint: 'Fast for long text — universal paste shortcut' },
  { id: 'ctrl-v',       label: 'Clipboard (Ctrl+V)',        hint: 'Standard paste — may conflict in CAD applications' },
  { id: 'send-input',   label: 'SendInput API',             hint: 'Windows low-level injection — bypasses all app input filtering' },
];

export default function SettingsPanel({
  onClose,
  macrosEnabledOnStartup,
  onToggleMacrosOnStartup,
  onExportConfig,
  onImportConfig,
  onRestoreBackup,
  globalInputMethod = 'direct',
  keystrokeDelay    = 30,
  macroTriggerDelay = 150,
  doubleTapWindow   = 300,
  onUpdateGlobalSettings,
  searchOverlayHotkey      = 'Ctrl+Space',
  overlayShowAll            = true,
  overlayCloseAfterFiring   = true,
  overlayIncludeAutocorrect = false,
  onUpdateSearchSettings,
  globalPauseToggleKey  = null,
  onSetPauseKey,
  onClearPauseKey,
  onRestartOnboarding,
}) {
  const [configPath, setConfigPath]           = useState('');
  const [startWithWindows, setStartWithWindows] = useState(false);
  const [capturingHotkey, setCapturingHotkey] = useState(false);
  const [capturedHotkey, setCapturedHotkey]   = useState(null);
  const [capturingPauseKey, setCapturingPauseKey] = useState(false);
  const [capturedPauseKey, setCapturedPauseKey]   = useState(null);
  const [pauseConflict, setPauseConflict]         = useState(null);
  const [backupList, setBackupList]           = useState(null);
  const [confirmRestore, setConfirmRestore]   = useState(null);
  const [appVersion, setAppVersion]           = useState('');

  useEffect(() => {
    window.electronAPI?.getConfigPath().then(p  => setConfigPath(p || ''));
    window.electronAPI?.getStartupEnabled().then(v => setStartWithWindows(!!v));
    window.electronAPI?.getAppVersion().then(v => setAppVersion(v || ''));
  }, []);

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
        <button className="settings-close-btn" onClick={onClose} title="Close settings" type="button">
          <svg width="10" height="10" viewBox="0 0 10 10">
            <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
            <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
          </svg>
        </button>
      </div>

      <div className="settings-body">

        {/* ── HELP & DOCUMENTATION ───────────────────────── */}
        <section className="settings-section">
          <div className="settings-section-title">HELP &amp; DOCUMENTATION</div>
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
              onClick={() => window.electronAPI?.openExternal('https://docs.google.com/forms/d/e/1FAIpQLScFygiarZG2MGV_JJSaLZux1ZNYC0w-ne5QSZ-HyTNQk5XEWA/viewform')}
            >
              <svg width="13" height="13" viewBox="0 0 16 16" fill="none" aria-hidden="true">
                <rect x="1.5" y="3.5" width="13" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.4"/>
                <path d="M1.5 5l6.5 4.5L14.5 5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round"/>
              </svg>
              Send Feedback
            </button>
          </div>
        </section>

        {/* ── ABOUT ──────────────────────────────────────── */}
        <section className="settings-section">
          <div className="settings-section-title">ABOUT</div>
          <div className="settings-about">
            <div className="settings-about-header">
              <span className="settings-about-name">Trigr</span>
              <span className="settings-about-version">{appVersion ? `v${appVersion}` : ''}</span>
            </div>
            <p className="settings-about-desc">Keyboard macro manager with global hotkeys, text expansions and autocorrect — all stored locally on your device.</p>
          </div>
        </section>

        {/* ── PRIVACY & SECURITY ─────────────────────────── */}
        <section className="settings-section">
          <div className="settings-section-title">PRIVACY &amp; SECURITY</div>

          <div className="settings-privacy-block">
            <p>All your data is stored locally on this device only. Trigr never transmits your assignments, expansions or keystrokes to any server. No analytics, telemetry or usage tracking of any kind.</p>
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

          <div className="settings-security-notice">
            <svg width="13" height="13" viewBox="0 0 16 16" fill="none" className="settings-notice-icon" aria-hidden="true">
              <path d="M8 2L1.5 14h13L8 2Z" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round"/>
              <line x1="8" y1="7" x2="8" y2="10.5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
              <circle cx="8" cy="12.5" r="0.7" fill="currentColor"/>
            </svg>
            <span>Avoid storing passwords or sensitive credentials as text expansions. Use a dedicated password manager like Bitwarden or 1Password for that purpose.</span>
          </div>
        </section>

        {/* ── GENERAL ────────────────────────────────────── */}
        <section className="settings-section">
          <div className="settings-section-title">GENERAL</div>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Start with Windows</span>
              <span className="settings-toggle-sub">Launch automatically when you log in</span>
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
              <span className="settings-toggle-sub">Macros are active immediately when Trigr launches</span>
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
        </section>

        {/* ── GLOBAL PAUSE ───────────────────────────────── */}
        <section className="settings-section">
          <div className="settings-section-title">GLOBAL PAUSE</div>

          <div className="settings-pause-stack">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Pause hotkey</span>
              <span className="settings-toggle-sub">Press this from any app to toggle Trigr on/off. Must include at least one modifier.</span>
            </div>
            <div className="settings-qs-hotkey-ctrl">
              {capturingPauseKey ? (
                <div
                  className="settings-qs-capture"
                  tabIndex={0}
                  autoFocus
                  onBlur={() => { setCapturingPauseKey(false); setCapturedPauseKey(null); setPauseConflict(null); }}
                  onKeyDown={async e => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (e.key === 'Escape') { setCapturingPauseKey(false); setCapturedPauseKey(null); setPauseConflict(null); return; }
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
                    const result = await window.electronAPI?.checkHotkeyConflict(combo);
                    setPauseConflict(result?.conflict ? `Conflicts with: ${result.with}` : null);
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
                    {globalPauseToggleKey.split('+').map((p, i, arr) => {
                      const display = p.startsWith('Key') ? p.slice(3) : p.startsWith('Digit') ? p.slice(5) : p;
                      return (
                        <React.Fragment key={i}>
                          <kbd className="settings-qs-kbd">{display}</kbd>
                          {i < arr.length - 1 && <span className="settings-qs-plus">+</span>}
                        </React.Fragment>
                      );
                    })}
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
        </section>

        {/* ── QUICK SEARCH ───────────────────────────────── */}
        <section className="settings-section">
          <div className="settings-section-title">QUICK SEARCH</div>

          <div className="settings-toggle-row">
            <div className="settings-toggle-info">
              <span className="settings-toggle-label">Global hotkey</span>
              <span className="settings-toggle-sub">Press this to open Quick Search from any app</span>
            </div>
            <div className="settings-qs-hotkey-ctrl">
              {capturingHotkey ? (
                <div
                  className="settings-qs-capture"
                  tabIndex={0}
                  autoFocus
                  onBlur={() => { setCapturingHotkey(false); setCapturedHotkey(null); }}
                  onKeyDown={e => {
                    e.preventDefault();
                    e.stopPropagation();
                    if (e.key === 'Escape') { setCapturingHotkey(false); setCapturedHotkey(null); return; }
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
                    {searchOverlayHotkey.split('+').map((p, i, arr) => {
                      const display = p.startsWith('Key') ? p.slice(3) : p.startsWith('Digit') ? p.slice(5) : p;
                      return (
                        <React.Fragment key={i}>
                          <kbd className="settings-qs-kbd">{display}</kbd>
                          {i < arr.length - 1 && <span className="settings-qs-plus">+</span>}
                        </React.Fragment>
                      );
                    })}
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
              <span className="settings-toggle-sub">Browse all macros and expansions when nothing is typed</span>
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
              <span className="settings-toggle-sub">Dismiss the overlay immediately when a result is activated</span>
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
              <span className="settings-toggle-sub">Search across your autocorrect dictionary (may add many results)</span>
            </div>
            <button
              type="button"
              className={`settings-toggle${overlayIncludeAutocorrect ? ' on' : ''}`}
              onClick={() => onUpdateSearchSettings?.({ overlayIncludeAutocorrect: !overlayIncludeAutocorrect })}
              role="switch"
              aria-checked={overlayIncludeAutocorrect}
            />
          </div>
        </section>

        {/* ── COMPATIBILITY ──────────────────────────────── */}
        <section className="settings-section">
          <div className="settings-section-title">COMPATIBILITY</div>
          <p className="settings-compat-desc">
            Controls how Trigr injects text into other applications.
            Use <strong>Direct keystrokes</strong> for CAD software and games.
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

          {(globalInputMethod === 'direct' || globalInputMethod === 'send-input') && (
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
                  onChange={e => onUpdateGlobalSettings?.({ keystrokeDelay: Number(e.target.value) })}
                />
                <span className="settings-slider-val">{keystrokeDelay}ms</span>
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
                onChange={e => onUpdateGlobalSettings?.({ macroTriggerDelay: Number(e.target.value) })}
              />
              <span className="settings-slider-val">{macroTriggerDelay}ms</span>
            </div>
          </div>

          <div className="settings-slider-row">
            <div className="settings-slider-info">
              <span className="settings-toggle-label">Double-tap window</span>
              <span className="settings-toggle-sub">Maximum gap between two presses to count as a double-tap</span>
            </div>
            <div className="settings-slider-ctrl">
              <input
                type="range"
                className="settings-slider"
                min="150" max="500" step="10"
                value={doubleTapWindow}
                onChange={e => onUpdateGlobalSettings?.({ doubleTapWindow: Number(e.target.value) })}
              />
              <span className="settings-slider-val">{doubleTapWindow}ms</span>
            </div>
          </div>
        </section>

        {/* ── BACKUP & RESTORE ───────────────────────────── */}
        <section className="settings-section">
          <div className="settings-section-title">BACKUP &amp; RESTORE</div>
          <p className="settings-backup-desc">
            Export your full config to back it up or transfer to another machine. Import to restore from a file.
            Trigr also creates automatic backups on every launch and save.
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
                  <p className="settings-backup-confirm-sub">This will replace your current config. This cannot be undone.</p>
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
        </section>

      </div>
    </aside>
  );
}
