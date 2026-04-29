import React, { useState, useEffect, useRef, useLayoutEffect } from 'react';
import './SearchOverlay.css';
import { friendlyKeyName } from './keyboardLayout';

// ── Type metadata ──────────────────────────────────────────────────────────────

const TYPE_META = {
  text:       { icon: '✦', color: '#64b4ff' },
  hotkey:     { icon: '⌨', color: '#c864ff' },
  app:        { icon: '⬡', color: '#50c878' },
  url:        { icon: '⊕', color: '#ffc832' },
  folder:     { icon: '⬢', color: '#40c8a0' },
  macro:      { icon: '◈', color: '#ff783c' },
  expansion:  { icon: '↩', color: '#ffc832' },
  autocorrect:{ icon: '✏', color: '#aaaaaa' },
};

const GROUP_ORDER = ['assignment', 'quickaction', 'expansion', 'autocorrect'];
const GROUP_LABELS = {
  assignment:  'MACROS & HOTKEYS',
  quickaction: 'QUICK ACTIONS',
  expansion:   'TEXT EXPANSIONS',
  autocorrect: 'AUTOCORRECT',
};

// ── comboLabel builder ─────────────────────────────────────────────────────────

function buildComboLabel(combo, keyId) {
  const keyPart = friendlyKeyName(keyId);
  if (combo === 'BARE' || combo === '') {
    return `${keyPart} (bare)`;
  }
  return `${combo}+${keyPart}`;
}

// ── preview builder ────────────────────────────────────────────────────────────

function buildPreview(macro) {
  if (!macro || !macro.data) return '';
  const d = macro.data;
  switch (macro.type) {
    case 'text':
      return (d.text || '').substring(0, 40);
    case 'hotkey': {
      const parts = [];
      if (d.ctrl)  parts.push('Ctrl');
      if (d.alt)   parts.push('Alt');
      if (d.shift) parts.push('Shift');
      if (d.win)   parts.push('Win');
      if (d.key)   parts.push(d.key);
      return parts.join('+');
    }
    case 'app':
      if (d.appName) return d.appName;
      if (d.appPath) return d.appPath.split(/[\\/]/).pop();
      return '';
    case 'url':
      return d.urlName || d.url || '';
    case 'folder':
      if (d.folderName) return d.folderName;
      if (d.folderPath) return d.folderPath.split(/[\\/]/).pop();
      return '';
    case 'macro': {
      const steps = Array.isArray(d.steps) ? d.steps.length : 0;
      return `Sequence (${steps} step${steps !== 1 ? 's' : ''})`;
    }
    default:
      return '';
  }
}

// ── buildItems ─────────────────────────────────────────────────────────────────

function buildItems(data) {
  const { assignments, activeProfile, globalInputMethod, settings } = data;
  const { includeAutocorrect } = settings || {};
  const items = [];

  for (const [storageKey, macro] of Object.entries(assignments || {})) {
    if (storageKey.startsWith(`${activeProfile}::`)) {
      // Regular key assignment: Profile::combo::keyId
      const parts = storageKey.split('::');
      if (parts.length < 3) continue;
      // parts[0] = profile, parts[1] = combo, parts[2] = keyId
      const combo   = parts[1];
      const keyId   = parts[2];
      const comboLabel = buildComboLabel(combo, keyId);
      items.push({
        type:       'assignment',
        storageKey,
        combo,
        keyId,
        comboLabel,
        assignType: macro.type,
        label:      macro.label || '',
        preview:    buildPreview(macro),
      });
    } else if (storageKey.startsWith('GLOBAL::EXPANSION::')) {
      const trigger = storageKey.slice('GLOBAL::EXPANSION::'.length);
      const isImage = macro.data?.expansionType === 'image';
      items.push({
        type:    'expansion',
        storageKey,
        trigger,
        label:   macro.data?.displayName || trigger,
        preview: isImage
          ? `[IMG] ${(macro.data?.imagePath || '').split(/[/\\]/).pop() || 'No image'}`
          : (macro.data?.text || '').substring(0, 60),
        text:    macro.data?.text,
        html:    macro.data?.html,
      });
    } else if (storageKey.startsWith('GLOBAL::QUICKACTION::')) {
      items.push({
        type:       'quickaction',
        storageKey,
        assignType: macro.type,
        label:      macro.label || '',
        preview:    buildPreview(macro),
      });
    } else if (storageKey.startsWith('GLOBAL::AUTOCORRECT::')) {
      if (!includeAutocorrect) continue;
      const typo = storageKey.slice('GLOBAL::AUTOCORRECT::'.length);
      items.push({
        type:    'autocorrect',
        storageKey,
        label:   typo,
        preview: `→ ${macro.data?.correction || ''}`,
        text:    macro.data?.correction,
      });
    }
  }

  return items;
}

// ── scoreMatch ─────────────────────────────────────────────────────────────────

function scoreMatch(text, query) {
  if (!text || !query) return 0;
  const t = text.toLowerCase();
  const q = query.toLowerCase();

  if (t === q) return 5;
  if (t.startsWith(q)) return 4;
  if (t.includes(q)) return 3;

  return 0;
}

// ── searchItems ────────────────────────────────────────────────────────────────

function searchItems(items, query, showAll) {
  if (!query) {
    return [];
  }

  const scored = items
    .map(item => {
      const scoreLabel   = scoreMatch(item.label,          query);
      const scorePreview = scoreMatch(item.preview || '',  query);
      const scoreCombo   = scoreMatch(item.comboLabel || '', query);
      const scoreTrigger = scoreMatch(item.trigger || '',  query);
      const bestScore    = Math.max(scoreLabel, scorePreview, scoreCombo, scoreTrigger);
      return { item, bestScore };
    })
    .filter(({ bestScore }) => bestScore > 0)
    .sort((a, b) => {
      // Primary: group order (matches renderGroups visual layout)
      const groupDiff = GROUP_ORDER.indexOf(a.item.type) - GROUP_ORDER.indexOf(b.item.type);
      if (groupDiff !== 0) return groupDiff;
      // Secondary: best score within group
      return b.bestScore - a.bestScore;
    });

  return scored.slice(0, 8).map(({ item }) => item);
}

// ── HighlightMatch ─────────────────────────────────────────────────────────────

function HighlightMatch({ text, query }) {
  if (!query || !text) return <>{text}</>;

  const t = text.toLowerCase();
  const q = query.toLowerCase();

  // Substring match
  const idx = t.indexOf(q);
  if (idx !== -1) {
    return (
      <>
        {text.slice(0, idx)}
        <span className="hl">{text.slice(idx, idx + q.length)}</span>
        {text.slice(idx + q.length)}
      </>
    );
  }

  return <>{text}</>;
}

// ── Main component ─────────────────────────────────────────────────────────────

export default function SearchOverlay() {
  const [query,         setQuery]         = useState('');
  const [allItems,      setAllItems]      = useState([]);
  const [displayItems,  setDisplayItems]  = useState([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [settings,      setSettings]      = useState({
    showAll: false, closeAfterFiring: true, includeAutocorrect: false,
  });
  const [ready, setReady] = useState(false);

  // ── Search Template state machine ──
  // mode: 'main' (normal search) | 'query' (typing a search template query)
  const [mode, setMode]                     = useState('main');
  const [activeTemplate, setActiveTemplate] = useState(null);
  const [triggerToken, setTriggerToken]     = useState('');
  const [searchTemplates, setSearchTemplates] = useState([]);

  const inputRef   = useRef(null);
  const resultsRef = useRef(null);
  const rowRefs    = useRef([]);

  // ── Receive data from main process ──
  useEffect(() => {
    if (!window.electronAPI?.onOverlaySearchData) return;

    window.electronAPI.onOverlaySearchData((data) => {
      // Apply theme before rendering so colours are correct on first paint
      document.documentElement.setAttribute('data-theme', data.theme || 'dark');
      const { settings: newSettings } = data;
      setSettings(newSettings || { showAll: false, closeAfterFiring: true, includeAutocorrect: false });
      const items = buildItems(data);
      setAllItems(items);
      setSearchTemplates(data.searchTemplates || []);
      setQuery('');
      setSelectedIndex(0);
      setMode('main');
      setActiveTemplate(null);
      setTriggerToken('');
      setReady(true);
      // Focus the input each time the overlay opens (data arrives on every show)
      setTimeout(() => inputRef.current?.focus(), 0);
    });
  }, []);

  // ── Arrow-key handler on window ──
  useEffect(() => {
    function handleKeyDown(e) {
      // In query mode, suppress arrow keys (no result list)
      if (mode === 'query') return;
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex(i => Math.min(i + 1, displayItems.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex(i => Math.max(i - 1, 0));
      }
    }
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [displayItems.length, mode]);

  // ── Update displayItems when query/allItems/settings change (Main mode only) ──
  useEffect(() => {
    if (mode === 'query') {
      setDisplayItems([]);
      return;
    }
    const results = searchItems(allItems, query, settings.showAll);
    setDisplayItems(results);
    setSelectedIndex(0);
  }, [query, allItems, settings.showAll, mode]);

  // ── Resize overlay window whenever displayItems or mode change ──
  const panelRef = useRef(null);
  useEffect(() => {
    // Double-rAF ensures React has committed the DOM update before we measure.
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const el = panelRef.current;
        if (!el) return;
        const scrollH = el.scrollHeight;
        const rect = el.getBoundingClientRect();
        // scrollHeight = full content height (not clipped by overflow)
        // 9 = top margin, 13 = border + shadow breathing room
        const windowH = Math.ceil(scrollH + 9 + 13);
        import('@tauri-apps/api/core').then(({ invoke: inv }) =>
          inv('log_debug', { message: `[OVERLAY-JS] scrollH=${scrollH} rectH=${rect.height} top=${rect.top} → windowH=${windowH}` })
        ).catch(() => {});
        window.electronAPI?.resizeOverlay(windowH);
      });
    });
  }, [displayItems, mode]);

  // ── Scroll selected row into view ──
  useLayoutEffect(() => {
    const el = rowRefs.current[selectedIndex];
    if (el) el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [selectedIndex]);

  // ── Fire an item ──
  function fireItem(item) {
    if (!item) return;

    const payload = { type: item.type };
    if (item.storageKey) payload.storageKey = item.storageKey;
    if (item.label)      payload.label      = item.label;
    if (item.text  != null) payload.text  = item.text;
    if (item.html  != null) payload.html  = item.html;

    if (settings.closeAfterFiring) {
      window.electronAPI?.closeOverlay();
      window.electronAPI?.executeSearchResult(payload);
    } else {
      window.electronAPI?.executeSearchResult(payload);
    }
  }

  // ── Fire search template ──
  function fireSearchTemplate() {
    if (!activeTemplate || !query.trim()) return;
    const payload = {
      type: 'search_template',
      url_template: activeTemplate.url_template,
      query: query.trim(),
      encode_query: activeTemplate.encode_query ?? true,
      label: activeTemplate.label,
      trigger: triggerToken,
    };
    window.electronAPI?.closeOverlay();
    window.electronAPI?.executeSearchResult(payload);
  }

  // ── Input change handler — trigger detection ──
  function handleInputChange(e) {
    const value = e.target.value;

    if (mode === 'query') {
      // Backspace-to-Main: if user clears the entire query, restore trigger in main mode
      if (value === '') {
        setMode('main');
        setQuery(triggerToken);
        setActiveTemplate(null);
        setTriggerToken('');
        return;
      }
      setQuery(value);
      return;
    }

    // Main mode — check for trigger + space
    if (value.endsWith(' ') && searchTemplates.length > 0) {
      const candidate = value.slice(0, -1).trim().toLowerCase();
      if (candidate) {
        const match = searchTemplates.find(t => t.trigger.toLowerCase() === candidate);
        if (match) {
          // Transition to query mode
          setMode('query');
          setActiveTemplate(match);
          setTriggerToken(candidate);
          setQuery('');
          return;
        }
      }
    }

    setQuery(value);
  }

  // ── Input keydown ──
  function handleInputKeyDown(e) {
    if (e.key === 'Enter') {
      e.preventDefault();
      if (mode === 'query') {
        fireSearchTemplate();
      } else {
        fireItem(displayItems[selectedIndex]);
      }
    } else if (e.key === 'Escape') {
      e.preventDefault();
      if (mode === 'query') {
        // First Escape: return to Main mode, don't close
        setMode('main');
        setQuery('');
        setActiveTemplate(null);
        setTriggerToken('');
        setTimeout(() => inputRef.current?.focus(), 0);
      } else {
        window.electronAPI?.closeOverlay();
      }
    }
    // ArrowUp/Down handled by window listener
  }

  // ── Grouped rendering ──
  function renderGroups() {
    // Build ordered groups
    const groups = {};
    for (const type of GROUP_ORDER) groups[type] = [];
    for (const item of displayItems) {
      if (groups[item.type]) groups[item.type].push(item);
      else groups[item.type] = [item];
    }

    const nodes = [];
    let rowIdx  = 0;  // flat index into displayItems for selectedIndex tracking

    for (const type of GROUP_ORDER) {
      const groupItems = groups[type];
      if (!groupItems || groupItems.length === 0) continue;

      nodes.push(
        <div className="search-group-header" key={`hdr-${type}`}>
          {GROUP_LABELS[type]}
        </div>
      );

      for (const item of groupItems) {
        const idx      = rowIdx;
        const isSelected = idx === selectedIndex;
        const meta     = item.type === 'assignment'
          ? (TYPE_META[item.assignType] || { icon: '◈', color: '#aaa' })
          : (TYPE_META[item.type]       || { icon: '?', color: '#aaa' });

        nodes.push(
          <div
            key={item.storageKey || `${item.type}-${item.label}`}
            className={`search-result-row${isSelected ? ' selected' : ''}`}
            onClick={() => fireItem(item)}
            ref={el => { rowRefs.current[idx] = el; }}
          >
            <span className="result-type-icon" style={{ color: meta.color }}>
              {meta.icon}
            </span>
            <div className="result-content">
              <div className="result-label">
                <HighlightMatch text={item.label} query={query} />
              </div>
              {item.preview && (
                <div className="result-preview">
                  <HighlightMatch text={item.preview} query={query} />
                </div>
              )}
            </div>
            {item.comboLabel && item.type === 'assignment' && (
              <span className="result-combo">{item.comboLabel}</span>
            )}
            {item.trigger && item.type === 'expansion' && (
              <span className="result-combo">{item.trigger} + Space</span>
            )}
          </div>
        );

        rowIdx++;
      }
    }

    // Clear stale refs beyond current count
    rowRefs.current = rowRefs.current.slice(0, rowIdx);

    return nodes;
  }

  return (
    <div className="search-overlay">
      <div className="search-panel" ref={panelRef}>
        {/* Query mode: template label bar */}
        {mode === 'query' && activeTemplate && (
          <div className="search-template-bar">
            <span className="search-template-label">{activeTemplate.label}</span>
          </div>
        )}

        <div className="search-input-row">
          {mode === 'query' ? (
            <span className="search-back-hint" title="Esc to go back">←</span>
          ) : (
            <span className="search-icon">⌕</span>
          )}
          <input
            ref={inputRef}
            className="search-input"
            type="text"
            placeholder={
              mode === 'query' && activeTemplate
                ? `Search ${activeTemplate.label}…`
                : 'Search macros, hotkeys, expansions…'
            }
            value={query}
            onChange={handleInputChange}
            onKeyDown={handleInputKeyDown}
            spellCheck={false}
            autoComplete="off"
            autoCorrect="off"
          />
          <span className="search-esc-hint">Esc</span>
        </div>

        {mode === 'main' && displayItems.length > 0 && (
          <div className="search-results" ref={resultsRef}>
            {renderGroups()}
          </div>
        )}

        {mode === 'main' && query && displayItems.length === 0 && ready && (
          <div className="search-empty">No results for "{query}"</div>
        )}
      </div>
    </div>
  );
}
