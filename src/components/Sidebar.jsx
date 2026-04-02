import React, { useState, useEffect } from 'react';
import './Sidebar.css';

const TYPE_META = {
  text:   { color: '#64b4ff' },
  hotkey: { color: '#c864ff' },
  app:    { color: '#50c878' },
  url:    { color: '#ffc832' },
  macro:  { color: '#ff783c' },
  folder: { color: '#40c8a0' },
};

const TYPE_NAMES = {
  text: 'Text', hotkey: 'Hotkey', app: 'App',
  url: 'URL', macro: 'Macro', folder: 'Folder',
};

export default function Sidebar({
  activeProfile,
  assignments,
  currentCombo,
  selectedKey,
  onSelectAssignment,
  onSelectCombo,
  profileLinked,
}) {
  // All hotkey assignments for this profile, excluding text expansions and ::double
  // variants (double-tap assignments are shown as a ×2 badge on the primary entry).
  const profileEntries = Object.entries(assignments)
    .filter(([k]) => {
      if (!k.startsWith(activeProfile + '::')) return false;
      if (k.includes('::EXPANSION::')) return false;
      const parts = k.split('::');
      if (parts[parts.length - 1] === 'double') return false;
      return true;
    })
    .map(([k, v]) => {
      const parts = k.split('::');
      return {
        combo:     parts[1] || '',
        keyId:     parts[2] || '',
        macro:     v,
        hasDouble: !!assignments[k + '::double'],
      };
    });

  // Sorted distinct combos that have assignments
  const combos = [...new Set(profileEntries.map(e => e.combo))].sort((a, b) => {
    if (a.length !== b.length) return a.length - b.length;
    return a.localeCompare(b);
  });

  const [activeTab, setActiveTab] = useState('All');

  // Reset to All tab whenever the active profile changes
  useEffect(() => {
    setActiveTab('All');
  }, [activeProfile]);

  // Sync active tab when the keyboard modifier layer changes
  useEffect(() => {
    setActiveTab(currentCombo || 'All');
  }, [currentCombo]);

  // Always show BARE tab for app-linked profiles (even when no assignments yet)
  const allCombos = profileLinked && !combos.includes('BARE') ? [...combos, 'BARE'] : combos;
  const tabs = ['All', ...allCombos];

  const filtered = activeTab === 'All'
    ? profileEntries
    : profileEntries.filter(e => e.combo === activeTab);

  // For the All tab, group by combo with headers; for specific tabs, flat list
  const grouped = {};
  if (activeTab === 'All') {
    filtered.forEach(e => {
      if (!grouped[e.combo]) grouped[e.combo] = [];
      grouped[e.combo].push(e);
    });
  }
  const sortedGroupCombos = Object.keys(grouped).sort((a, b) => {
    if (a.length !== b.length) return a.length - b.length;
    return a.localeCompare(b);
  });

  const MOUSE_KEY_LABELS = {
    MOUSE_LEFT: '🖱 Left', MOUSE_RIGHT: '🖱 Right', MOUSE_MIDDLE: '🖱 Mid',
    MOUSE_SCROLL_UP: '🖱 Scroll↑', MOUSE_SCROLL_DOWN: '🖱 Scroll↓',
    MOUSE_SIDE1: '🖱 Side1', MOUSE_SIDE2: '🖱 Side2',
  };

  function renderItem({ combo, keyId, macro, hasDouble }) {
    const meta = TYPE_META[macro.type] || { color: 'var(--text-muted)' };
    const displayKey = MOUSE_KEY_LABELS[keyId] ||
      keyId.replace('Key', '').replace('Digit', '').replace('Arrow', '');
    const isSelected = selectedKey === keyId && combo === currentCombo;
    const isBareItem = combo === 'BARE';
    const typeName = TYPE_NAMES[macro.type] || macro.type;
    const displayLabel = macro.label || macro.data?.text || macro.data?.url || macro.data?.path || typeName;
    return (
      <div
        key={`${combo}::${keyId}`}
        className={`sidebar-item type-${macro.type}${isSelected ? ' sidebar-item-active' : ''}${isBareItem ? ' bare-item' : ''}`}
        onClick={() => onSelectAssignment(keyId, combo)}
        title={`Edit ${isBareItem ? 'Bare' : combo}+${displayKey}`}
      >
        <span className="sidebar-key-badge" style={{ borderColor: meta.color + '55', color: meta.color }}>
          {displayKey}
        </span>
        <div className="sidebar-item-info">
          <div className="sidebar-item-label">
            {displayLabel}
            {hasDouble && <span className="sidebar-double-badge">×2</span>}
          </div>
          <div className="sidebar-item-type">
            <span className="type-dot" style={{ background: meta.color }} />
            {typeName}
          </div>
        </div>
      </div>
    );
  }

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <span className="sidebar-title">Assignments</span>
        <span className="sidebar-count">{profileEntries.length}</span>
      </div>

      {/* Tab bar */}
      <div className="sidebar-tabs">
        {tabs.map(tab => (
          <button
            key={tab}
            className={`sidebar-tab${tab === 'BARE' ? ' bare-tab' : ''}${activeTab === tab ? ' sidebar-tab-active' : ''}`}
            onClick={() => {
              setActiveTab(tab);
              // Note: modifier state is intentionally NOT changed here.
              // The modifier bar on the keyboard is the only way to change
              // which layer is active — sidebar tabs are a display filter only.
              onSelectCombo?.(tab);
            }}
            type="button"
          >
            {tab === 'BARE' ? 'Bare' : tab}
          </button>
        ))}
      </div>

      <div className="sidebar-list">
        {profileEntries.length === 0 && activeTab !== 'BARE' ? (
          <div className="sidebar-empty">
            <div className="sidebar-empty-icon">⌨</div>
            <p>Select modifiers above the keyboard, then click a key to assign a hotkey</p>
          </div>
        ) : filtered.length === 0 ? (
          <div className="sidebar-empty">
            {activeTab === 'BARE' ? (
              <>
                <div className="sidebar-empty-icon">⚡</div>
                <p>No bare key assignments yet. Select <strong>Bare Keys</strong> in the modifier bar, then click a key on the keyboard.</p>
              </>
            ) : (
              <p>No assignments on this layer yet</p>
            )}
          </div>
        ) : activeTab === 'All' ? (
          sortedGroupCombos.map(combo => (
            <div key={combo} className={`sidebar-group${combo === currentCombo ? ' active-group' : ''}`}>
              <div className="sidebar-group-header">
                {combo === 'BARE' ? (
                  <kbd className="sidebar-mod-key sidebar-mod-bare">Bare</kbd>
                ) : (
                  combo.split('+').map((m, i, arr) => (
                    <React.Fragment key={m}>
                      <kbd className="sidebar-mod-key">{m}</kbd>
                      {i < arr.length - 1 && <span className="sidebar-mod-plus">+</span>}
                    </React.Fragment>
                  ))
                )}
                <span className="sidebar-group-count">{grouped[combo].length}</span>
              </div>
              {grouped[combo].map(renderItem)}
            </div>
          ))
        ) : (
          filtered.map(renderItem)
        )}
      </div>

      <div className="sidebar-footer">
        <div className="legend-item"><span className="legend-dot assigned" />Assigned</div>
        <div className="legend-item"><span className="legend-dot selected" />Selected</div>
        <div className="legend-item"><span className="legend-dot system-ld" />System Key</div>
      </div>
    </aside>
  );
}
