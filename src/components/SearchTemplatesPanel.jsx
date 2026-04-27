import React, { useState, useRef, useEffect } from 'react';
import './SearchTemplatesPanel.css';

// ── Bundled presets ─────────────────────────────────────────────────────────

const PRESETS = [
  { label: 'Google',            trigger: 'g',      urlTemplate: 'https://www.google.com/search?q={query}' },
  { label: 'YouTube',           trigger: 'yt',     urlTemplate: 'https://www.youtube.com/results?search_query={query}' },
  { label: 'GitHub',            trigger: 'gh',     urlTemplate: 'https://github.com/search?q={query}&type=repositories' },
  { label: 'Stack Overflow',    trigger: 'so',     urlTemplate: 'https://stackoverflow.com/search?q={query}' },
  { label: 'MDN',               trigger: 'mdn',    urlTemplate: 'https://developer.mozilla.org/en-US/search?q={query}' },
  { label: 'npm',               trigger: 'npm',    urlTemplate: 'https://www.npmjs.com/search?q={query}' },
  { label: 'Reddit',            trigger: 'r',      urlTemplate: 'https://www.reddit.com/search/?q={query}' },
  { label: 'Wikipedia',         trigger: 'wiki',   urlTemplate: 'https://en.wikipedia.org/w/index.php?search={query}' },
  { label: 'Google Maps',       trigger: 'maps',   urlTemplate: 'https://www.google.com/maps/search/{query}' },
  { label: 'DuckDuckGo',        trigger: 'ddg',    urlTemplate: 'https://duckduckgo.com/?q={query}' },
  { label: 'Hacker News',       trigger: 'hn',     urlTemplate: 'https://hn.algolia.com/?q={query}' },
  { label: 'Companies House',   trigger: 'ch',     urlTemplate: 'https://find-and-update.company-information.service.gov.uk/search?q={query}' },
  { label: 'Planning Portal',   trigger: 'plan',   urlTemplate: 'https://www.planningportal.co.uk/planning/search?q={query}' },
  { label: 'Ordnance Survey',   trigger: 'os',     urlTemplate: 'https://osdatahub.os.uk/search?q={query}' },
  { label: 'BSI Knowledge',     trigger: 'bsi',    urlTemplate: 'https://knowledge.bsigroup.com/search?q={query}' },
];

// ── Helpers ─────────────────────────────────────────────────────────────────

function buildPreviewUrl(urlTemplate, sampleQuery) {
  if (!urlTemplate || !sampleQuery) return '';
  const encoded = encodeURIComponent(sampleQuery).replace(/%20/g, '+');
  return urlTemplate.replace('{query}', encoded);
}

function truncateUrl(url, maxLen = 60) {
  if (!url || url.length <= maxLen) return url;
  return url.slice(0, maxLen) + '…';
}

// ── Main component ──────────────────────────────────────────────────────────

export default function SearchTemplatesPanel({
  searchTemplates = [],
  isPro = false,
  onAdd,
  onUpdate,
  onDelete,
  onShowNotification,
}) {
  const [selectedId, setSelectedId]         = useState(null);
  const [showPresets, setShowPresets]        = useState(false);
  const [presetFilter, setPresetFilter]     = useState('');

  // Form state
  const [formLabel, setFormLabel]           = useState('');
  const [formTrigger, setFormTrigger]       = useState('');
  const [formUrl, setFormUrl]               = useState('');
  const [formEncode, setFormEncode]         = useState(true);
  const [formSource, setFormSource]         = useState('custom');
  const [triggerError, setTriggerError]     = useState('');
  const [isNew, setIsNew]                   = useState(false);

  // Test state
  const [testQuery, setTestQuery]           = useState('');
  const [showHelp, setShowHelp]             = useState(false);
  const helpRef = useRef(null);

  // Close help popover on outside click
  useEffect(() => {
    if (!showHelp) return;
    function onDown(e) {
      if (helpRef.current && !helpRef.current.contains(e.target)) setShowHelp(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [showHelp]);

  // ── Trigger validation ──────────────────────────────────────────────────

  function validateTrigger(value, excludeId) {
    if (!value) return 'Trigger is required';
    if (!/^[a-z0-9]+$/.test(value)) return 'Lowercase letters and numbers only';
    if (value.length > 10) return 'Max 10 characters';
    const exists = searchTemplates.some(
      t => t.trigger.toLowerCase() === value.toLowerCase() && t.id !== excludeId
    );
    if (exists) return 'Trigger already in use';
    return '';
  }

  // ── Select a template ───────────────────────────────────────────────────

  function selectTemplate(template) {
    setSelectedId(template.id);
    setFormLabel(template.label);
    setFormTrigger(template.trigger);
    setFormUrl(template.url_template);
    setFormEncode(template.encode_query ?? true);
    setFormSource(template.source || 'custom');
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(false);
  }

  function closePanel() {
    setSelectedId(null);
    setIsNew(false);
  }

  // ── Open new from preset ────────────────────────────────────────────────

  function openNewFromPreset(preset) {
    let trigger = preset.trigger;
    const taken = new Set(searchTemplates.map(t => t.trigger.toLowerCase()));
    if (taken.has(trigger)) {
      for (let i = 2; i <= 99; i++) {
        const candidate = `${preset.trigger}${i}`;
        if (!taken.has(candidate) && candidate.length <= 10) { trigger = candidate; break; }
      }
    }
    setSelectedId(null);
    setFormLabel(preset.label);
    setFormTrigger(trigger);
    setFormUrl(preset.urlTemplate);
    setFormEncode(true);
    setFormSource('preset');
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(true);
    setShowPresets(false);
  }

  function openNewCustom() {
    setSelectedId(null);
    setFormLabel('');
    setFormTrigger('');
    setFormUrl('');
    setFormEncode(true);
    setFormSource('custom');
    setTriggerError('');
    setTestQuery('');
    setShowHelp(false);
    setIsNew(true);
    setShowPresets(false);
  }

  // ── Save ────────────────────────────────────────────────────────────────

  function handleSave() {
    const excludeId = isNew ? null : selectedId;
    const err = validateTrigger(formTrigger, excludeId);
    if (err) { setTriggerError(err); return; }
    if (!formLabel.trim()) return;
    if (!formUrl.includes('{query}')) return;

    if (isNew) {
      const newId = crypto.randomUUID();
      onAdd?.({
        id: newId,
        label: formLabel.trim(),
        trigger: formTrigger.trim().toLowerCase(),
        url_template: formUrl.trim(),
        encode_query: formEncode,
        source: formSource,
      });
      setSelectedId(newId);
      setIsNew(false);
      onShowNotification?.('Template added', 'success');
    } else {
      onUpdate?.(selectedId, {
        label: formLabel.trim(),
        trigger: formTrigger.trim().toLowerCase(),
        url_template: formUrl.trim(),
        encode_query: formEncode,
        source: formSource,
      });
      onShowNotification?.('Template updated', 'success');
    }
  }

  function handleDelete(id) {
    onDelete?.(id);
    if (selectedId === id) closePanel();
    onShowNotification?.('Template deleted', 'success');
  }

  function handleTest() {
    if (!testQuery.trim() || !formUrl) return;
    const encoded = formEncode
      ? encodeURIComponent(testQuery.trim()).replace(/%20/g, '+')
      : testQuery.trim();
    const finalUrl = formUrl.replace('{query}', encoded);
    window.electronAPI?.openExternal(finalUrl);
  }

  function handleNewClick() {
    if (atCap) return;
    setPresetFilter('');
    setShowPresets(true);
  }

  // Free tier cap: 5 templates
  const atCap = !isPro && searchTemplates.length >= 5;
  const editOpen = selectedId !== null || isNew;
  const canSave = formLabel.trim() && formTrigger && !triggerError && formUrl.includes('{query}');

  // ── Render ──────────────────────────────────────────────────────────────

  return (
    <div className="stp-panel">
      {/* Header — matches te-header style */}
      <div className="stp-header">
        <div className="stp-mode-tabs">
          <button className="stp-mode-tab active" type="button">
            ⌕ Quick Search Templates
          </button>
        </div>
        <div className="stp-header-right">
          {atCap ? (
            /* TODO: Replace with full upgrade flow component when Pro licensing ships */
            <span className="stp-cap-nudge" title="Upgrade to Pro for unlimited templates">
              5/5 — Upgrade for more
            </span>
          ) : (
            <button className="stp-add-btn" onClick={handleNewClick} type="button">
              + New Template
            </button>
          )}
        </div>
      </div>

      {/* Preset picker overlay */}
      {showPresets && (
        <div className="stp-presets-overlay">
          <div className="stp-presets">
            <div className="stp-presets-header">
              <span className="stp-presets-title">Choose a preset or create custom</span>
              <button className="stp-back-btn" onClick={() => setShowPresets(false)} type="button">Cancel</button>
            </div>
            <input
              className="stp-preset-filter"
              type="text"
              placeholder="Filter presets…"
              value={presetFilter}
              onChange={e => setPresetFilter(e.target.value)}
              spellCheck={false}
              autoFocus
            />
            <div className="stp-preset-list">
              {(presetFilter
                ? PRESETS.filter(p => p.label.toLowerCase().includes(presetFilter.toLowerCase()) || p.trigger.includes(presetFilter.toLowerCase()))
                : PRESETS
              ).map(p => (
                <button key={p.trigger} className="stp-preset-row" onClick={() => openNewFromPreset(p)} type="button">
                  <span className="stp-preset-label">{p.label}</span>
                  <span className="stp-preset-trigger">{p.trigger}</span>
                  <span className="stp-preset-url">{truncateUrl(p.urlTemplate, 50)}</span>
                </button>
              ))}
            </div>
            <button className="stp-custom-btn" onClick={openNewCustom} type="button">
              + Create custom template
            </button>
          </div>
        </div>
      )}

      {/* Body: list + edit panel */}
      <div className="stp-body">
        {/* Left: list */}
        <div className="stp-list">
          {searchTemplates.length === 0 && !isNew ? (
            <div className="stp-empty-state">
              <div className="stp-empty-icon">⌕</div>
              <div className="stp-empty-heading">No search templates yet</div>
              <div className="stp-empty-sub">
                Add one to search Google, GitHub, or your own URLs from Quick Search.
              </div>
              <button className="stp-add-btn stp-empty-cta" onClick={handleNewClick} type="button">
                + New Template
              </button>
            </div>
          ) : (
            searchTemplates.map(t => (
              <div
                key={t.id}
                className={`stp-item${selectedId === t.id ? ' active' : ''}`}
                onClick={() => selectTemplate(t)}
              >
                <span className="stp-item-trigger">{t.trigger}</span>
                <span className="stp-item-label">{t.label}</span>
                <span className="stp-item-url">{truncateUrl(t.url_template, 40)}</span>
                <div className="stp-item-actions">
                  <button
                    className="stp-item-del"
                    onClick={e => { e.stopPropagation(); handleDelete(t.id); }}
                    title="Delete"
                    type="button"
                  >✕</button>
                </div>
              </div>
            ))
          )}
        </div>

        {/* Right: edit panel */}
        {editOpen ? (
          <div className="stp-edit-panel">
            <div className="stp-ep-header">
              <span className="stp-ep-title">{isNew ? 'New Template' : 'Edit Template'}</span>
              <button className="stp-ep-close" onClick={closePanel} type="button">✕</button>
            </div>

            <div className="stp-ep-fields">
              <div className="stp-field">
                <label className="stp-label">Label</label>
                <input
                  className="stp-input"
                  type="text"
                  value={formLabel}
                  onChange={e => setFormLabel(e.target.value)}
                  placeholder="e.g. Google"
                  spellCheck={false}
                />
              </div>

              <div className="stp-field">
                <label className="stp-label">Trigger</label>
                <input
                  className={`stp-input stp-trigger-input${triggerError ? ' error' : ''}`}
                  type="text"
                  value={formTrigger}
                  onChange={e => {
                    const v = e.target.value.toLowerCase().replace(/[^a-z0-9]/g, '').slice(0, 10);
                    setFormTrigger(v);
                    setTriggerError(validateTrigger(v, isNew ? null : selectedId));
                  }}
                  placeholder="e.g. g"
                  spellCheck={false}
                  maxLength={10}
                />
                {triggerError && <div className="stp-trigger-error">{triggerError}</div>}
                <div className="stp-field-hint">Type this in Quick Search + Space to activate</div>
              </div>

              <div className="stp-field">
                <div className="stp-label-row">
                  <label className="stp-label">URL Template</label>
                  <button
                    className="stp-help-btn"
                    onClick={() => setShowHelp(v => !v)}
                    type="button"
                    title="How to find the right URL"
                  >?</button>
                </div>
                {showHelp && (
                  <div className="stp-help-popover" ref={helpRef}>
                    <p><strong>How to find the right URL pattern:</strong></p>
                    <p>1. Go to the website and search for a word (e.g. "test")</p>
                    <p>2. Copy the URL from your browser's address bar</p>
                    <p>3. Paste it here and replace "test" with <code>{'{query}'}</code></p>
                    <p className="stp-help-example">Example: https://google.com/search?q=test
                       becomes https://google.com/search?q={'{query}'}</p>
                  </div>
                )}
                <input
                  className="stp-input stp-url-input"
                  type="text"
                  value={formUrl}
                  onChange={e => setFormUrl(e.target.value)}
                  placeholder="https://example.com/search?q={query}"
                  spellCheck={false}
                />
                {formUrl && !formUrl.includes('{query}') && (
                  <div className="stp-trigger-error">URL must contain {'{query}'} placeholder</div>
                )}
                {formUrl && formUrl.includes('{query}') && (
                  <div className="stp-preview-line">
                    Example: typing "tauri" would open {truncateUrl(buildPreviewUrl(formUrl, 'tauri'), 80)}
                  </div>
                )}
              </div>

              <div className="stp-field">
                <label className="stp-toggle-label">
                  <input
                    type="checkbox"
                    checked={formEncode}
                    onChange={e => setFormEncode(e.target.checked)}
                  />
                  URL-encode query
                </label>
              </div>
            </div>

            <div className="stp-ep-test">
              <input
                className="stp-input stp-test-input"
                type="text"
                value={testQuery}
                onChange={e => setTestQuery(e.target.value)}
                placeholder="Test query…"
                spellCheck={false}
                onKeyDown={e => { if (e.key === 'Enter') handleTest(); }}
              />
              <button
                className="stp-test-btn"
                onClick={handleTest}
                disabled={!testQuery.trim() || !formUrl.includes('{query}')}
                type="button"
              >Test</button>
            </div>

            <div className="stp-ep-footer">
              <button className="stp-save-btn" onClick={handleSave} disabled={!canSave} type="button">
                {isNew ? 'Add Template' : 'Save Changes'}
              </button>
              {!isNew && (
                <button className="stp-delete-btn" onClick={() => handleDelete(selectedId)} type="button">
                  Delete
                </button>
              )}
            </div>
          </div>
        ) : (
          <div className="stp-edit-panel stp-panel-idle">
            <span className="stp-idle-text">Select a template to edit, or add a new one</span>
          </div>
        )}
      </div>
    </div>
  );
}
