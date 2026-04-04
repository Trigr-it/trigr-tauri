import React, { useState, useEffect, useRef, useCallback } from 'react';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
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

// ── Sortable profile row ────────────────────────────────────────────────────

function SortableProfileRow({ profile, isActive, hasLink, linkedAppName, onSelect, onDoubleClick }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: profile });
  const style = {
    transform: DndCSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
  };

  return (
    <div ref={setNodeRef} style={style} className={`profile-row${isActive ? ' active' : ''}`} onClick={onSelect} onDoubleClick={onDoubleClick}>
      <div className="profile-drag-handle" {...attributes} {...listeners}>⠿</div>
      <span className="profile-row-name">{profile}</span>
      {hasLink && <span className="profile-row-link" title={linkedAppName}>⊞</span>}
    </div>
  );
}

// ── Profile Accordion ───────────────────────────────────────────────────────

function ProfileAccordion({
  profiles, activeProfile, profileSettings,
  onProfileChange, onAddProfile, onRenameProfile, onDeleteProfile, onReorderProfiles, onDuplicateProfile,
}) {
  const [isExpanded, setIsExpanded] = useState(false);
  const [isAdding, setIsAdding] = useState(false);
  const [newName, setNewName] = useState('');
  const [renaming, setRenaming] = useState(null);
  const [renameValue, setRenameValue] = useState('');
  const [menuFor, setMenuFor] = useState(null);
  const [activeDragId, setActiveDragId] = useState(null);
  const menuRef = useRef(null);
  const addInputRef = useRef(null);

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }));

  // Close menu on outside click
  useEffect(() => {
    if (!menuFor) return;
    function onDown(e) {
      if (menuRef.current && !menuRef.current.contains(e.target)) setMenuFor(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [menuFor]);

  // Focus add input when shown
  useEffect(() => {
    if (isAdding) addInputRef.current?.focus();
  }, [isAdding]);

  function handleSelect(name) {
    onProfileChange(name);
    setIsExpanded(false);
    setMenuFor(null);
  }

  function handleAdd(e) {
    e.preventDefault();
    const trimmed = newName.trim();
    if (trimmed) {
      onAddProfile(trimmed);
      setNewName('');
      setIsAdding(false);
    }
  }

  function startRename(name) {
    setMenuFor(null);
    setRenaming(name);
    setRenameValue(name);
  }

  function commitRename() {
    const trimmed = renameValue.trim();
    if (trimmed && trimmed !== renaming) {
      onRenameProfile?.(renaming, trimmed);
    }
    setRenaming(null);
    setRenameValue('');
  }

  function handleDragEnd(event) {
    setActiveDragId(null);
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const nonDefault = profiles.slice(1);
    const oldIndex = nonDefault.indexOf(active.id);
    const newIndex = nonDefault.indexOf(over.id);
    if (oldIndex === -1 || newIndex === -1) return;
    const reordered = arrayMove(nonDefault, oldIndex, newIndex);
    onReorderProfiles?.([profiles[0], ...reordered]);
  }

  const nonDefault = profiles.slice(1);

  return (
    <div className="profile-accordion">
      {/* Header — always visible */}
      <div className="profile-accordion-header" onClick={() => setIsExpanded(v => !v)}>
        <span className="profile-accordion-label">PROFILES</span>
        <span className="profile-accordion-active">{activeProfile}</span>
        <span className="profile-accordion-chevron">{isExpanded ? '▴' : '▾'}</span>
      </div>

      {/* Expanded list */}
      {isExpanded && (
        <div className="profile-accordion-list">
          <DndContext sensors={sensors} onDragStart={e => setActiveDragId(e.active.id)} onDragEnd={handleDragEnd}>
            <SortableContext items={nonDefault} strategy={verticalListSortingStrategy}>
              {/* Default profile — always first, no drag */}
              <div
                className={`profile-row${activeProfile === 'Default' ? ' active' : ''}`}
                onClick={() => handleSelect('Default')}
              >
                <div className="profile-drag-handle profile-drag-placeholder" />
                <span className="profile-row-name">Default</span>
              </div>

              {/* Non-default profiles — sortable */}
              {nonDefault.map(p => {
                const linkedApp = profileSettings[p]?.linkedApp;
                if (renaming === p) {
                  return (
                    <div key={p} className="profile-row">
                      <div className="profile-drag-handle profile-drag-placeholder" />
                      <input
                        autoFocus
                        className="profile-rename-input"
                        value={renameValue}
                        onChange={e => setRenameValue(e.target.value)}
                        onKeyDown={e => {
                          if (e.key === 'Enter') commitRename();
                          if (e.key === 'Escape') { setRenaming(null); setRenameValue(''); }
                        }}
                        onBlur={commitRename}
                        onClick={e => e.stopPropagation()}
                      />
                    </div>
                  );
                }
                return (
                  <SortableProfileRow
                    key={p}
                    profile={p}
                    isActive={activeProfile === p}
                    hasLink={!!linkedApp}
                    linkedAppName={linkedApp ? linkedApp.split(/[/\\]/).pop() : ''}
                    onSelect={() => handleSelect(p)}
                    onDoubleClick={() => startRename(p)}
                  />
                );
              })}
            </SortableContext>

            <DragOverlay>
              {activeDragId ? (
                <div className="profile-row profile-row-ghost">
                  <div className="profile-drag-handle">⠿</div>
                  <span className="profile-row-name">{activeDragId}</span>
                </div>
              ) : null}
            </DragOverlay>
          </DndContext>

          {/* Add profile */}
          {isAdding ? (
            <form className="profile-add-row" onSubmit={handleAdd}>
              <input
                ref={addInputRef}
                className="profile-rename-input"
                value={newName}
                onChange={e => setNewName(e.target.value)}
                placeholder="Profile name..."
                onBlur={() => { setIsAdding(false); setNewName(''); }}
                onKeyDown={e => e.key === 'Escape' && setIsAdding(false)}
              />
            </form>
          ) : (
            <button className="profile-add-btn" type="button" onClick={() => setIsAdding(true)}>
              + Add Profile
            </button>
          )}
        </div>
      )}
    </div>
  );
}

// ── Sidebar ─────────────────────────────────────────────────────────────────

export default function Sidebar({
  activeProfile,
  assignments,
  currentCombo,
  selectedKey,
  onSelectAssignment,
  onSelectCombo,
  profileLinked,
  // Profile management props
  profiles = ['Default'],
  profileSettings = {},
  onProfileChange,
  onAddProfile,
  onRenameProfile,
  onDeleteProfile,
  onReorderProfiles,
  onDuplicateProfile,
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
      <ProfileAccordion
        profiles={profiles}
        activeProfile={activeProfile}
        profileSettings={profileSettings}
        onProfileChange={onProfileChange}
        onAddProfile={onAddProfile}
        onRenameProfile={onRenameProfile}
        onDeleteProfile={onDeleteProfile}
        onReorderProfiles={onReorderProfiles}
        onDuplicateProfile={onDuplicateProfile}
      />

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
