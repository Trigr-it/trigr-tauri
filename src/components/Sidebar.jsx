import React, { useState, useEffect, useRef } from 'react';
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

function SortableProfileRow({ profile, isActive, isFallback, hasLink, linkedAppName, onSelect, onDoubleClick, onContextMenu }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: profile });
  const style = {
    transform: DndCSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={`profile-row${isActive ? ' active' : ''}`}
      onClick={onSelect}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
    >
      <div className="profile-drag-handle" {...attributes} {...listeners}>⠿</div>
      <span className="profile-row-name">
        {isFallback && <span className="profile-fallback-dot" />}
        {profile}
      </span>
      {hasLink && <span className="profile-row-link" title={linkedAppName}>⊞</span>}
    </div>
  );
}

// ── Profile Accordion ───────────────────────────────────────────────────────

function ProfileAccordion({
  profiles, activeProfile, activeGlobalProfile, profileSettings,
  onProfileChange, onAddProfile, onRenameProfile, onDeleteProfile,
  onReorderProfiles, onDuplicateProfile, onSetActiveGlobalProfile,
}) {
  const [isExpanded, setIsExpanded] = useState(false);
  const [isAdding, setIsAdding] = useState(false);
  const [newName, setNewName] = useState('');
  const [renaming, setRenaming] = useState(null);
  const [renameValue, setRenameValue] = useState('');
  const [contextMenu, setContextMenu] = useState(null); // { profile, x, y }
  const [activeDragId, setActiveDragId] = useState(null);
  const ctxRef = useRef(null);
  const addInputRef = useRef(null);

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }));

  // Split profiles into static and app-specific
  const staticProfiles = profiles.filter(p => !profileSettings[p]?.linkedApp);
  const appProfiles = profiles.filter(p => !!profileSettings[p]?.linkedApp);
  // Non-Default for sortable (Default always first in static, not sortable)
  const staticSortable = staticProfiles.filter(p => p !== 'Default');

  // Close context menu on outside click or Escape
  useEffect(() => {
    if (!contextMenu) return;
    function onDown(e) {
      if (ctxRef.current && !ctxRef.current.contains(e.target)) setContextMenu(null);
    }
    function onKey(e) {
      if (e.key === 'Escape') setContextMenu(null);
    }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [contextMenu]);

  useEffect(() => {
    if (isAdding) addInputRef.current?.focus();
  }, [isAdding]);

  function handleSelect(name) {
    onProfileChange(name);
    setIsExpanded(false);
    setContextMenu(null);
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
    setContextMenu(null);
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

  function handleContextMenu(e, name) {
    e.preventDefault();
    e.stopPropagation();
    setContextMenu({ profile: name, x: e.clientX, y: e.clientY });
  }

  function handleDragEnd(event) {
    setActiveDragId(null);
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    // Check both are in same group
    const activeIsApp = !!profileSettings[active.id]?.linkedApp;
    const overIsApp = !!profileSettings[over.id]?.linkedApp;
    if (activeIsApp !== overIsApp) return; // cross-group drag — cancel

    if (activeIsApp) {
      const oldIdx = appProfiles.indexOf(active.id);
      const newIdx = appProfiles.indexOf(over.id);
      if (oldIdx === -1 || newIdx === -1) return;
      const reorderedApp = arrayMove(appProfiles, oldIdx, newIdx);
      onReorderProfiles?.([...staticProfiles, ...reorderedApp]);
    } else {
      const oldIdx = staticSortable.indexOf(active.id);
      const newIdx = staticSortable.indexOf(over.id);
      if (oldIdx === -1 || newIdx === -1) return;
      const reorderedStatic = arrayMove(staticSortable, oldIdx, newIdx);
      onReorderProfiles?.(['Default', ...reorderedStatic, ...appProfiles]);
    }
  }

  function renderProfileRow(p, { sortable = true } = {}) {
    const linkedApp = profileSettings[p]?.linkedApp;
    const isFallback = p === activeGlobalProfile;

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

    if (sortable) {
      return (
        <SortableProfileRow
          key={p}
          profile={p}
          isActive={activeProfile === p}
          isFallback={isFallback}
          hasLink={!!linkedApp}
          linkedAppName={linkedApp ? linkedApp.split(/[/\\]/).pop() : ''}
          onSelect={() => handleSelect(p)}
          onDoubleClick={() => startRename(p)}
          onContextMenu={e => handleContextMenu(e, p)}
        />
      );
    }

    // Non-sortable (Default)
    return (
      <div
        key={p}
        className={`profile-row${activeProfile === p ? ' active' : ''}`}
        onClick={() => handleSelect(p)}
        onContextMenu={e => handleContextMenu(e, p)}
      >
        <div className="profile-drag-handle profile-drag-placeholder" />
        <span className="profile-row-name">
          {isFallback && <span className="profile-fallback-dot" />}
          {p}
        </span>
      </div>
    );
  }

  // Collapsed header display
  const isSameProfile = activeProfile === activeGlobalProfile;

  return (
    <div className="profile-accordion">
      {/* Header — always visible */}
      <div className="profile-accordion-header" onClick={() => setIsExpanded(v => !v)}>
        <span className="profile-accordion-label">PROFILES</span>
        <span className="profile-accordion-active">
          <span className="profile-fallback-dot" />
          {activeGlobalProfile}
          {!isSameProfile && (
            <>
              <span className="profile-accordion-sep">|</span>
              <span className="profile-accordion-editing">{activeProfile}</span>
            </>
          )}
        </span>
        <span className="profile-accordion-chevron">{isExpanded ? '▴' : '▾'}</span>
      </div>

      {/* Expanded list */}
      {isExpanded && (
        <div className="profile-accordion-list">
          <DndContext sensors={sensors} onDragStart={e => setActiveDragId(e.active.id)} onDragEnd={handleDragEnd}>
            {/* Static profiles group */}
            <div className="profile-group-label">STATIC</div>
            <SortableContext items={staticSortable} strategy={verticalListSortingStrategy}>
              {renderProfileRow('Default', { sortable: false })}
              {staticSortable.map(p => renderProfileRow(p))}
            </SortableContext>

            {/* App-specific profiles group */}
            {appProfiles.length > 0 && (
              <>
                <div className="profile-group-divider" />
                <div className="profile-group-label">APP-SPECIFIC</div>
                <SortableContext items={appProfiles} strategy={verticalListSortingStrategy}>
                  {appProfiles.map(p => renderProfileRow(p))}
                </SortableContext>
              </>
            )}

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

      {/* Context menu */}
      {contextMenu && (() => {
        const isDefault = contextMenu.profile === 'Default';
        const isStatic = !profileSettings[contextMenu.profile]?.linkedApp;
        const isFallback = contextMenu.profile === activeGlobalProfile;
        return (
          <div
            ref={ctxRef}
            className="profile-ctx-menu"
            style={{ top: contextMenu.y, left: contextMenu.x }}
          >
            {!isDefault && (
              <button className="profile-ctx-item" onClick={() => { startRename(contextMenu.profile); }}>Rename</button>
            )}
            <button className="profile-ctx-item" onClick={() => { onDuplicateProfile?.(contextMenu.profile); setContextMenu(null); }}>Duplicate</button>
            {isStatic && !isFallback && (
              <button className="profile-ctx-item" onClick={() => { onSetActiveGlobalProfile?.(contextMenu.profile); setContextMenu(null); }}>
                Set as default fallback
              </button>
            )}
            {!isDefault && (
              <>
                <div className="profile-ctx-divider" />
                <button className="profile-ctx-item profile-ctx-delete" onClick={() => { onDeleteProfile?.(contextMenu.profile); setContextMenu(null); }}>Delete</button>
              </>
            )}
          </div>
        );
      })()}
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
  activeGlobalProfile = 'Default',
  profileSettings = {},
  onProfileChange,
  onAddProfile,
  onRenameProfile,
  onDeleteProfile,
  onReorderProfiles,
  onDuplicateProfile,
  onSetActiveGlobalProfile,
}) {
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

  const combos = [...new Set(profileEntries.map(e => e.combo))].sort((a, b) => {
    if (a.length !== b.length) return a.length - b.length;
    return a.localeCompare(b);
  });

  const [activeTab, setActiveTab] = useState('All');

  useEffect(() => {
    setActiveTab('All');
  }, [activeProfile]);

  useEffect(() => {
    setActiveTab(currentCombo || 'All');
  }, [currentCombo]);

  const allCombos = profileLinked && !combos.includes('BARE') ? [...combos, 'BARE'] : combos;
  const tabs = ['All', ...allCombos];

  const filtered = activeTab === 'All'
    ? profileEntries
    : profileEntries.filter(e => e.combo === activeTab);

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
        activeGlobalProfile={activeGlobalProfile}
        profileSettings={profileSettings}
        onProfileChange={onProfileChange}
        onAddProfile={onAddProfile}
        onRenameProfile={onRenameProfile}
        onDeleteProfile={onDeleteProfile}
        onReorderProfiles={onReorderProfiles}
        onDuplicateProfile={onDuplicateProfile}
        onSetActiveGlobalProfile={onSetActiveGlobalProfile}
      />

      <div className="sidebar-header">
        <span className="sidebar-title">Assignments</span>
        <span className="sidebar-count">{profileEntries.length}</span>
      </div>

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
