import React, { useState, useEffect, useRef } from 'react';
import './TemplatesPanel.css';

// ── Starter template packs ────────────────────────────────────────────────

function buildTemplatePack(expansions, hotkeys, profile) {
  const out = {};
  for (const exp of expansions) {
    out[`GLOBAL::EXPANSION::${exp.trigger}`] = {
      type: 'expansion',
      label: exp.displayName || `Expand: ${exp.trigger}`,
      data: { html: '', text: exp.text, category: exp.category || null, triggerMode: 'space', displayName: exp.displayName || null },
    };
  }
  for (const hk of hotkeys) {
    out[`${profile}::${hk.combo}::${hk.keyId}`] = {
      type: hk.type,
      label: hk.label,
      data: hk.data,
    };
  }
  return out;
}

const TEMPLATE_PACKS = [
  {
    id: 'general',
    name: 'General / Office',
    desc: 'Email signatures, dates, meeting requests, and quick replies',
    expansions: [
      { trigger: ';sig', text: 'Kind regards,\n{fillIn:Your Name}\n{fillIn:Your Title}\n{fillIn:Your Company}', displayName: 'Email Signature' },
      { trigger: ';dt', text: '{date:DD/MM/YYYY}', displayName: "Today's Date" },
      { trigger: ';ad', text: '{fillIn:Street}\n{fillIn:City}\n{fillIn:Postcode}', displayName: 'Address Block' },
      { trigger: ';ty', text: "Thank you for your email. I'll get back to you shortly.", displayName: 'Quick Thank You' },
      { trigger: ';ooo', text: "Thank you for your email. I'm currently out of the office until {fillIn:Return Date} with limited access to email. For urgent matters please contact {fillIn:Colleague Name} at {fillIn:Colleague Email}.", displayName: 'Out of Office' },
      { trigger: ';mtg', text: 'Would any of the following times work for a call?\n\n- {fillIn:Option 1}\n- {fillIn:Option 2}\n- {fillIn:Option 3}\n\nLet me know what suits.', displayName: 'Meeting Request' },
      { trigger: ';kr', text: 'Kind regards,', displayName: 'Kind Regards' },
    ],
    hotkeys: [
      { combo: 'Ctrl+Alt', keyId: 'KeyN', type: 'app', label: 'Open Notepad', data: { path: 'notepad.exe', args: '' } },
    ],
  },
  {
    id: 'cad',
    name: 'CAD / Engineering',
    desc: 'Bare key CAD commands, revision notes, and drawing annotations — creates an app-specific profile',
    requiresApp: true,
    expansions: [
      { trigger: ';rev', text: 'Rev {fillIn:Number} — {fillIn:Date} — {fillIn:Description}', displayName: 'Revision Note' },
      { trigger: ';nc', text: 'Not to scale', displayName: 'Not to Scale' },
      { trigger: ';tbc', text: 'To be confirmed', displayName: 'TBC' },
      { trigger: ';na', text: 'Not applicable', displayName: 'N/A' },
      { trigger: ';sp', text: 'Specification reference: {fillIn:Reference}', displayName: 'Spec Reference' },
    ],
    bareKeys: [
      { keyId: 'KeyF', text: 'FILLET ', label: 'Fillet' },
      { keyId: 'KeyX', text: 'EXPLODE ', label: 'Explode' },
      { keyId: 'KeyH', text: 'HATCH ', label: 'Hatch' },
      { keyId: 'KeyZ', text: 'ZOOM E ', label: 'Zoom Extents' },
      { keyId: 'KeyA', text: 'ARRAY ', label: 'Array' },
      { keyId: 'KeyO', text: 'OFFSET ', label: 'Offset' },
      { keyId: 'KeyT', text: 'TRIM ', label: 'Trim' },
      { keyId: 'KeyD', text: 'DIMLINEAR ', label: 'Linear Dimension' },
    ],
    hotkeys: [],
  },
  {
    id: 'sales',
    name: 'Sales / Business Development',
    desc: 'Follow-ups, proposals, LinkedIn outreach, and CRM notes',
    expansions: [
      { trigger: ';fu', text: "Just following up on my previous email regarding {fillIn:Topic} — wanted to make sure it didn't get buried.", displayName: 'Follow Up' },
      { trigger: ';prop', text: "Please find attached our proposal for {fillIn:Project}. We'd welcome the opportunity to discuss further at your convenience.", displayName: 'Proposal Attached' },
      { trigger: ';ci', text: 'I came across {fillIn:Company} and wanted to reach out — we work with similar businesses on {fillIn:Topic} and thought there might be a good fit.', displayName: 'Cold Intro' },
      { trigger: ';li', text: "Hi {fillIn:Name}, I came across your profile and would love to connect — I work in {fillIn:Your Field} and think there's potential for a useful conversation.", displayName: 'LinkedIn Connect' },
      { trigger: ';dk', text: 'Please find attached our credentials deck for your review.', displayName: 'Credentials Deck' },
      { trigger: ';ns', text: 'What would be the best next step from your side?', displayName: 'Next Step' },
      { trigger: ';crm', text: 'Call with {fillIn:Name} — {fillIn:Date} — Outcome: {fillIn:Outcome} — Next action: {fillIn:Next Action}', displayName: 'CRM Call Note' },
    ],
    hotkeys: [
      { combo: 'Ctrl+Alt', keyId: 'KeyL', type: 'url', label: 'Open LinkedIn', data: { url: 'https://linkedin.com' } },
      { combo: 'Ctrl+Alt', keyId: 'KeyR', type: 'url', label: 'Open CRM', data: { url: 'https://your-crm-url.com' } },
    ],
  },
];

export default function TemplatesPanel({ activeProfile, onImportTemplate, onImportCadTemplate, onDismiss, showDismiss = false }) {
  const [templateResult, setTemplateResult] = useState(null);
  const [cadPickState, setCadPickState] = useState(null);
  const [cadWindowList, setCadWindowList] = useState([]);
  const [cadSelectedExe, setCadSelectedExe] = useState(null);
  const [cadDropdownOpen, setCadDropdownOpen] = useState(false);
  const cadDropdownRef = useRef(null);

  useEffect(() => {
    if (!cadDropdownOpen) return;
    function onDown(e) {
      if (cadDropdownRef.current && !cadDropdownRef.current.contains(e.target)) setCadDropdownOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [cadDropdownOpen]);

  return (
    <div className="tpl-panel">
      <div className="tpl-header">
        <span className="tpl-title">Starter Templates</span>
        {showDismiss && (
          <button className="tpl-close" type="button" onClick={onDismiss}>✕</button>
        )}
      </div>
      <p className="tpl-note">Importing adds to your existing assignments — nothing will be overwritten</p>
      {templateResult && (
        <div className="tpl-result">
          {typeof templateResult === 'string'
            ? templateResult
            : `${templateResult.added} added${templateResult.skipped > 0 ? `, ${templateResult.skipped} skipped (already assigned)` : ''}`
          }
        </div>
      )}
      <div className="tpl-grid">
        {TEMPLATE_PACKS.map(pack => (
          <div key={pack.id} className="tpl-card">
            <div className="tpl-card-name">{pack.name}</div>
            <div className="tpl-card-desc">{pack.desc}</div>
            <div className="tpl-card-counts">
              {pack.expansions.length} expansion{pack.expansions.length !== 1 ? 's' : ''}
              {pack.bareKeys?.length > 0 && ` · ${pack.bareKeys.length} bare key${pack.bareKeys.length !== 1 ? 's' : ''}`}
              {pack.hotkeys.length > 0 && ` · ${pack.hotkeys.length} hotkey${pack.hotkeys.length !== 1 ? 's' : ''}`}
            </div>

            {!pack.requiresApp && (
              <button
                className="tpl-import-btn"
                type="button"
                onClick={() => {
                  const built = buildTemplatePack(pack.expansions, pack.hotkeys, activeProfile);
                  const result = onImportTemplate?.(built);
                  setTemplateResult(result || { added: 0, skipped: 0 });
                }}
              >
                Import
              </button>
            )}

            {pack.requiresApp && !cadPickState && (
              <button
                className="tpl-import-btn"
                type="button"
                onClick={() => { setCadPickState('picking'); setCadSelectedExe(null); setTemplateResult(null); }}
              >
                Import
              </button>
            )}

            {pack.requiresApp && cadPickState && (
              <div className="tpl-cad-flow">
                {!cadSelectedExe && (
                  <p className="tpl-cad-hint">Open your CAD software first, then click Pick App to select it.</p>
                )}
                <div className="tpl-cad-pick-row" ref={cadDropdownRef}>
                  {cadSelectedExe ? (
                    <span className="tpl-badge">
                      {cadSelectedExe}
                      <button className="tpl-badge-clear" type="button" onClick={() => setCadSelectedExe(null)}>✕</button>
                    </span>
                  ) : (
                    <button
                      className="tpl-pick-btn"
                      type="button"
                      onClick={async () => {
                        setCadDropdownOpen(true);
                        setCadWindowList([]);
                        try {
                          const { invoke } = await import('@tauri-apps/api/core');
                          const list = await invoke('list_open_windows');
                          const seen = new Set();
                          const unique = [];
                          for (const w of (list || [])) {
                            const lower = w.process.toLowerCase();
                            if (!seen.has(lower)) { seen.add(lower); unique.push(w.process); }
                          }
                          unique.sort((a, b) => a.toLowerCase().localeCompare(b.toLowerCase()));
                          setCadWindowList(unique);
                        } catch (e) {
                          console.error('[Trigr] list_open_windows failed:', e);
                          setCadWindowList([]);
                        }
                      }}
                    >
                      ⊞ Pick App
                    </button>
                  )}
                  {cadDropdownOpen && !cadSelectedExe && (
                    <div className="tpl-pick-dropdown">
                      {cadWindowList.length === 0 ? (
                        <div className="tpl-pick-loading">Loading windows…</div>
                      ) : (
                        cadWindowList.map((exe, i) => (
                          <div key={i} className="tpl-pick-item" onClick={() => { setCadSelectedExe(exe); setCadDropdownOpen(false); }}>
                            <span className="tpl-pick-process">{exe}</span>
                          </div>
                        ))
                      )}
                    </div>
                  )}
                </div>
                <div className="tpl-cad-actions">
                  {cadSelectedExe && (
                    <button
                      className="tpl-import-btn"
                      type="button"
                      onClick={() => {
                        const profileName = `CAD — ${cadSelectedExe}`;
                        const bareAssignments = {};
                        for (const bk of pack.bareKeys) {
                          bareAssignments[`${profileName}::BARE::${bk.keyId}`] = {
                            type: 'text', label: bk.label, data: { text: bk.text },
                          };
                        }
                        const expAssignments = {};
                        for (const exp of pack.expansions) {
                          expAssignments[`GLOBAL::EXPANSION::${exp.trigger}`] = {
                            type: 'expansion',
                            label: exp.displayName || `Expand: ${exp.trigger}`,
                            data: { html: '', text: exp.text, category: null, triggerMode: 'space', displayName: exp.displayName || null },
                          };
                        }
                        const result = onImportCadTemplate?.(cadSelectedExe, expAssignments, bareAssignments);
                        if (result) {
                          setTemplateResult(`Profile "${result.profileName}" created. ${result.bareAdded} bare key${result.bareAdded !== 1 ? 's' : ''} added, ${result.expAdded} expansion${result.expAdded !== 1 ? 's' : ''} added${result.skipped > 0 ? `, ${result.skipped} skipped` : ''}.`);
                        }
                        setCadPickState(null);
                        setCadSelectedExe(null);
                      }}
                    >
                      Confirm Import
                    </button>
                  )}
                  <button
                    className="tpl-cad-cancel"
                    type="button"
                    onClick={() => { setCadPickState(null); setCadSelectedExe(null); setCadDropdownOpen(false); }}
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
