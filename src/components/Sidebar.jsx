import React, { useState, useEffect, useRef } from 'react';
import ReactDOM from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import './Sidebar.css';
import { friendlyKeyName } from './keyboardLayout';

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
  onUpdateProfileSettings, onExportProfile, onImportProfile,
  importPrompt, onImportProfileResolve, onImportPromptDismiss,
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
  // Link to App picker state
  const [linkPicker, setLinkPicker] = useState(null); // profileName or null
  const [linkWindowList, setLinkWindowList] = useState([]);
  const [linkSelectedExe, setLinkSelectedExe] = useState(null);
  const [linkDropdownOpen, setLinkDropdownOpen] = useState(false);
  const linkDropdownRef = useRef(null);
  const linkDropdownPortalRef = useRef(null);
  const pickAppBtnRef = useRef(null);
  const [linkDropdownPos, setLinkDropdownPos] = useState(null);
  const importPromptRef = useRef(null);

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

  // Close link picker dropdown on outside click
  useEffect(() => {
    if (!linkDropdownOpen) return;
    function onDown(e) {
      const inRow = linkDropdownRef.current && linkDropdownRef.current.contains(e.target);
      const inPortal = linkDropdownPortalRef.current && linkDropdownPortalRef.current.contains(e.target);
      if (!inRow && !inPortal) setLinkDropdownOpen(false);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [linkDropdownOpen]);

  // Dismiss import prompt on outside click or Escape
  useEffect(() => {
    if (!importPrompt) return;
    function onDown(e) {
      if (importPromptRef.current && !importPromptRef.current.contains(e.target)) onImportPromptDismiss?.();
    }
    function onKey(e) {
      if (e.key === 'Escape') onImportPromptDismiss?.();
    }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [importPrompt, onImportPromptDismiss]);

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
          <button className="profile-add-btn profile-import-btn" type="button" onClick={() => onImportProfile?.()}>
            ↓ Import Profile
          </button>
          {importPrompt && (
            <div className="profile-import-prompt" ref={importPromptRef}>
              <div className="profile-import-prompt-msg">
                A profile named "<strong>{importPrompt.name}</strong>" already exists.
              </div>
              <div className="profile-import-prompt-btns">
                <button className="profile-import-prompt-btn" type="button" onClick={() => onImportProfileResolve?.('copy')}>Copy</button>
                <button className="profile-import-prompt-btn profile-import-prompt-btn--overwrite" type="button" onClick={() => onImportProfileResolve?.('overwrite')}>Overwrite</button>
              </div>
            </div>
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
            <button className="profile-ctx-item" onClick={() => { onExportProfile?.(contextMenu.profile); setContextMenu(null); }}>Export Profile</button>
            {isStatic && !isFallback && (
              <button className="profile-ctx-item" onClick={() => { onSetActiveGlobalProfile?.(contextMenu.profile); setContextMenu(null); }}>
                Set as default fallback
              </button>
            )}
            {!isDefault && isStatic && (
              <button className="profile-ctx-item" onClick={() => {
                setLinkPicker(contextMenu.profile);
                setLinkSelectedExe(null);
                setContextMenu(null);
              }}>
                Link to App
              </button>
            )}
            {!isStatic && (
              <button className="profile-ctx-item" onClick={() => {
                onUpdateProfileSettings?.(contextMenu.profile, { linkedApp: null });
                setContextMenu(null);
              }}>
                Unlink App
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

      {/* Link to App picker */}
      {linkPicker && (
        <div className="profile-link-picker">
          <div className="profile-link-picker-header">
            <span className="profile-link-picker-title">Link "{linkPicker}" to App</span>
            <button className="profile-link-picker-close" type="button" onClick={() => { setLinkPicker(null); setLinkSelectedExe(null); setLinkDropdownOpen(false); }}>✕</button>
          </div>
          <p className="profile-link-picker-hint">Open the app first, then pick it below.</p>
          <div className="profile-link-picker-row" ref={linkDropdownRef}>
            {linkSelectedExe ? (
              <span className="pick-window-badge">
                {linkSelectedExe}
                <button className="pick-window-badge-clear" type="button" onClick={() => setLinkSelectedExe(null)}>✕</button>
              </span>
            ) : (
              <>
                <button className="browse-btn" ref={pickAppBtnRef} type="button" onClick={async () => {
                  const rowEl = linkDropdownRef.current;
                  if (rowEl) {
                    const rect = rowEl.getBoundingClientRect();
                    setLinkDropdownPos({ top: rect.bottom + 4, left: rect.left, width: rect.width });
                  }
                  setLinkDropdownOpen(true);
                  setLinkWindowList([]);
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
                    setLinkWindowList(unique);
                  } catch (e) {
                    console.error('[Trigr] list_open_windows failed:', e);
                    setLinkWindowList([]);
                  }
                }}>
                  ⊞ Pick App
                </button>
                <button className="browse-btn" type="button" onClick={async () => {
                  const path = await window.electronAPI?.browseForFile();
                  if (path) {
                    const filename = path.split(/[/\\]/).pop() || path;
                    setLinkSelectedExe(filename);
                  }
                }}>
                  Browse…
                </button>
              </>
            )}
            {linkDropdownOpen && !linkSelectedExe && linkDropdownPos && ReactDOM.createPortal(
              <div className="pick-window-dropdown pick-window-dropdown--portal" ref={linkDropdownPortalRef} style={{ top: linkDropdownPos.top, left: linkDropdownPos.left, width: linkDropdownPos.width }}>
                {linkWindowList.length === 0 ? (
                  <div className="pick-window-loading">Loading windows…</div>
                ) : (
                  linkWindowList.map((exe, i) => (
                    <div key={i} className="pick-window-item" onClick={() => { setLinkSelectedExe(exe); setLinkDropdownOpen(false); }}>
                      <span className="pick-window-process">{exe}</span>
                    </div>
                  ))
                )}
              </div>,
              document.body
            )}
          </div>
          <div className="profile-link-picker-actions">
            {linkSelectedExe && (
              <button className="profile-link-picker-confirm" type="button" onClick={() => {
                onUpdateProfileSettings?.(linkPicker, { linkedApp: linkSelectedExe });
                setLinkPicker(null);
                setLinkSelectedExe(null);
              }}>
                Confirm
              </button>
            )}
            <button className="profile-link-picker-cancel" type="button" onClick={() => { setLinkPicker(null); setLinkSelectedExe(null); setLinkDropdownOpen(false); }}>
              Cancel
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

const MODIFIERS = [
  { id: 'Ctrl',  label: 'Ctrl',   color: '#64b4ff' },
  { id: 'Alt',   label: 'Alt',    color: '#c864ff' },
  { id: 'Shift', label: 'Shift',  color: '#50c878' },
  { id: 'Win',   label: '⊞ Win', color: '#ffc832' },
];

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
  onUpdateProfileSettings,
  onExportProfile,
  onImportProfile,
  importPrompt,
  onImportProfileResolve,
  onImportPromptDismiss,
  // List view props
  listViewActive = false,
  isRecording = false,
  onStartRecord,
  onStopRecord,
  recordCapture,
  onToggleModifier,
  activeModifiers = [],
  sidebarComboFilter = null,
  // Context menu handlers
  onRenameAssignment,
  onClearAssignment,
  onDuplicateFromContext,
  onCopyToProfile,
  onMoveToProfile,
}) {
  const profileEntries = (() => {
    const entries = [];
    const seen = new Set();
    // First pass: collect single-press entries
    for (const [k, v] of Object.entries(assignments)) {
      if (!v) continue; // skip null/undefined (corrupted entry)
      if (!k.startsWith(activeProfile + '::')) continue;
      if (k.includes('::EXPANSION::')) continue;
      const parts = k.split('::');
      if (parts[parts.length - 1] === 'double') continue;
      const baseKey = k;
      seen.add(baseKey);
      entries.push({
        combo:      parts[1] || '',
        keyId:      parts[2] || '',
        macro:      v,
        hasDouble:  !!assignments[baseKey + '::double'],
        doubleOnly: false,
      });
    }
    // Second pass: collect double-only entries (no matching single)
    for (const [k, v] of Object.entries(assignments)) {
      if (!k.startsWith(activeProfile + '::')) continue;
      if (k.includes('::EXPANSION::')) continue;
      const parts = k.split('::');
      if (parts[parts.length - 1] !== 'double') continue;
      const baseKey = parts.slice(0, -1).join('::');
      if (seen.has(baseKey)) continue; // already listed via single entry
      entries.push({
        combo:      parts[1] || '',
        keyId:      parts[2] || '',
        macro:      v,
        hasDouble:  true,
        doubleOnly: true,
      });
    }
    return entries;
  })();

  const otherProfiles = (profiles || []).filter(p => p !== activeProfile);

  const combos = [...new Set(profileEntries.map(e => e.combo))].sort((a, b) => {
    if (a.length !== b.length) return a.length - b.length;
    return a.localeCompare(b);
  });

  const [activeTab, setActiveTab] = useState('All');
  const [assignFilter, setAssignFilter] = useState('');

  // ── Assignment context menu + inline actions ──
  const [assignCtx, setAssignCtx] = useState(null); // { combo, keyId, macro, x, y }
  const [renaming, setRenaming] = useState(null); // { combo, keyId }
  const [renameVal, setRenameVal] = useState('');
  const [clearing, setClearing] = useState(null); // { combo, keyId }
  const assignCtxRef = useRef(null);

  useEffect(() => {
    if (!assignCtx) return;
    function onDown(e) {
      if (assignCtxRef.current && !assignCtxRef.current.contains(e.target)) setAssignCtx(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [assignCtx]);

  useEffect(() => {
    setActiveTab('All');
  }, [activeProfile]);

  useEffect(() => {
    setActiveTab(sidebarComboFilter || 'All');
  }, [sidebarComboFilter]);

  const allCombos = !combos.includes('BARE') ? [...combos, 'BARE'] : combos;
  const tabs = ['All', ...allCombos];

  // Text search filter — matches label, key name, combo, and action type
  const filterQ = assignFilter.trim().toLowerCase();
  function matchesFilter(e) {
    if (!filterQ) return true;
    const label = (e.macro?.label || e.macro?.data?.text || e.macro?.data?.url || e.macro?.data?.path || '').toLowerCase();
    const keyName = friendlyKeyName(e.keyId).toLowerCase();
    const typeName = (TYPE_NAMES[e.macro?.type] || '').toLowerCase();
    const combo = e.combo.toLowerCase();
    return label.includes(filterQ) || keyName.includes(filterQ) || typeName.includes(filterQ) || combo.includes(filterQ);
  }

  const filtered = (activeTab === 'All'
    ? profileEntries
    : profileEntries.filter(e => e.combo === activeTab)
  ).filter(matchesFilter);

  const grouped = {};
  if (activeTab === 'All') {
    filtered.forEach(e => {
      if (!grouped[e.combo]) grouped[e.combo] = [];
      grouped[e.combo].push(e);
    });
  }
  const sortedGroupCombos = Object.keys(grouped).sort((a, b) => {
    if (a === 'BARE') return -1;
    if (b === 'BARE') return 1;
    if (a.length !== b.length) return a.length - b.length;
    return a.localeCompare(b);
  });

  const MOUSE_KEY_LABELS = {
    MOUSE_LEFT: '🖱 Left', MOUSE_RIGHT: '🖱 Right', MOUSE_MIDDLE: '🖱 Mid',
    MOUSE_SCROLL_UP: '🖱 Scroll↑', MOUSE_SCROLL_DOWN: '🖱 Scroll↓',
    MOUSE_SIDE1: '🖱 Side1', MOUSE_SIDE2: '🖱 Side2',
  };

  function handleAssignContextMenu(e, combo, keyId, macro) {
    e.preventDefault();
    e.stopPropagation();
    setAssignCtx({ combo, keyId, macro, x: e.clientX, y: e.clientY });
    setRenaming(null);
    setClearing(null);
  }

  function handleCtxRename() {
    if (!assignCtx) return;
    const { combo, keyId, macro } = assignCtx;
    setRenaming({ combo, keyId });
    setRenameVal(macro.label || '');
    setAssignCtx(null);
  }

  function commitRenameAssignment() {
    if (renaming && renameVal.trim()) {
      onRenameAssignment?.(renaming.combo, renaming.keyId, renameVal.trim());
    }
    setRenaming(null);
    setRenameVal('');
  }

  function cancelRename() {
    setRenaming(null);
    setRenameVal('');
  }

  function handleCtxDuplicate() {
    if (!assignCtx) return;
    onDuplicateFromContext?.(assignCtx.combo, assignCtx.keyId);
    setAssignCtx(null);
  }

  function handleCtxClear() {
    if (!assignCtx) return;
    setClearing({ combo: assignCtx.combo, keyId: assignCtx.keyId });
    setAssignCtx(null);
  }

  function confirmClear() {
    if (clearing) onClearAssignment?.(clearing.combo, clearing.keyId);
    setClearing(null);
  }

  const isRenaming = (combo, keyId) => renaming?.combo === combo && renaming?.keyId === keyId;
  const isClearing = (combo, keyId) => clearing?.combo === combo && clearing?.keyId === keyId;

  function renderItem({ combo, keyId, macro, hasDouble, doubleOnly }) {
    const meta = TYPE_META[macro.type] || { color: 'var(--text-muted)' };
    const displayKey = MOUSE_KEY_LABELS[keyId] || friendlyKeyName(keyId);
    const isSelected = selectedKey === keyId && combo === currentCombo;
    const isBareItem = combo === 'BARE';
    const typeName = TYPE_NAMES[macro.type] || macro.type;
    const displayLabel = macro.label || macro.data?.text || macro.data?.url || macro.data?.path || typeName;

    if (isClearing(combo, keyId)) {
      return (
        <div key={`${combo}::${keyId}`} className="sidebar-item sidebar-item-confirm">
          <span className="sidebar-confirm-text">Clear this key?</span>
          <button className="sidebar-confirm-yes" type="button" onClick={confirmClear}>Yes</button>
          <button className="sidebar-confirm-no" type="button" onClick={() => setClearing(null)}>No</button>
        </div>
      );
    }

    return (
      <div
        key={`${combo}::${keyId}`}
        className={`sidebar-item type-${macro.type}${isSelected ? ' sidebar-item-active' : ''}${isBareItem ? ' bare-item' : ''}`}
        onClick={() => onSelectAssignment(keyId, combo)}
        onContextMenu={e => handleAssignContextMenu(e, combo, keyId, macro)}
        title={`Edit ${isBareItem ? 'Bare' : combo}+${displayKey}`}
      >
        <span className="sidebar-key-badge" style={{ borderColor: meta.color + '55', color: meta.color }}>
          {displayKey}
        </span>
        <div className="sidebar-item-info">
          <div className="sidebar-item-label">
            {isRenaming(combo, keyId) ? (
              <input
                autoFocus
                className="sidebar-rename-input"
                value={renameVal}
                onChange={e => setRenameVal(e.target.value)}
                onKeyDown={e => { if (e.key === 'Enter') commitRenameAssignment(); if (e.key === 'Escape') cancelRename(); }}
                onBlur={cancelRename}
                onClick={e => e.stopPropagation()}
              />
            ) : (
              <>
                {displayLabel}
                {doubleOnly
                  ? <span className="sidebar-double-badge">×2 only</span>
                  : hasDouble && <span className="sidebar-double-badge">×2</span>
                }
              </>
            )}
          </div>
          <div className="sidebar-item-type">
            <span className="type-dot" style={{ background: meta.color }} />
            {typeName}
          </div>
        </div>
      </div>
    );
  }

  // ── Card for list view grid ──────────────────────────────────
  function renderCard({ combo, keyId, macro, hasDouble, doubleOnly }) {
    const meta = TYPE_META[macro.type] || { color: 'var(--text-muted)' };
    const displayKey = MOUSE_KEY_LABELS[keyId] || friendlyKeyName(keyId);
    const isSelected = selectedKey === keyId && combo === currentCombo;
    const comboLabel = combo === 'BARE' ? displayKey : combo + '+' + displayKey;
    const typeName = TYPE_NAMES[macro.type] || macro.type;
    const displayLabel = macro.label || macro.data?.text || macro.data?.url || macro.data?.path || typeName;

    // Preview line
    let preview = '';
    if (macro.type === 'text' || macro.type === 'expansion') {
      const raw = macro.data?.text || '';
      preview = raw.length > 40 ? raw.slice(0, 40) + '…' : raw;
    } else if (macro.type === 'macro') {
      const steps = macro.data?.steps || [];
      preview = `${steps.length} step${steps.length !== 1 ? 's' : ''}`;
    } else if (macro.type === 'hotkey') {
      preview = macro.data?.target || macro.label || '';
    } else if (macro.type === 'app') {
      preview = (macro.data?.path || '').split(/[/\\]/).pop() || '';
    } else if (macro.type === 'url') {
      preview = macro.data?.url || '';
    } else if (macro.type === 'folder') {
      preview = macro.data?.path || '';
    }

    if (isClearing(combo, keyId)) {
      return (
        <div key={`${combo}::${keyId}`} className="grid-card grid-card-confirm">
          <span className="sidebar-confirm-text">Clear this key?</span>
          <div className="sidebar-confirm-btns">
            <button className="sidebar-confirm-yes" type="button" onClick={confirmClear}>Yes</button>
            <button className="sidebar-confirm-no" type="button" onClick={() => setClearing(null)}>No</button>
          </div>
        </div>
      );
    }

    return (
      <div
        key={`${combo}::${keyId}`}
        className={`grid-card${isSelected ? ' grid-card--active' : ''}`}
        onClick={() => onSelectAssignment(keyId, combo)}
        onContextMenu={e => handleAssignContextMenu(e, combo, keyId, macro)}
      >
        <div className="grid-card-combo">
          {comboLabel}
          {doubleOnly
            ? <span className="sidebar-double-badge">×2 only</span>
            : hasDouble && <span className="sidebar-double-badge">×2</span>
          }
        </div>
        <div className="grid-card-label">
          {isRenaming(combo, keyId) ? (
            <input
              autoFocus
              className="sidebar-rename-input"
              value={renameVal}
              onChange={e => setRenameVal(e.target.value)}
              onKeyDown={e => { if (e.key === 'Enter') commitRenameAssignment(); if (e.key === 'Escape') cancelRename(); }}
              onBlur={cancelRename}
              onClick={e => e.stopPropagation()}
            />
          ) : displayLabel}
        </div>
        <div className="grid-card-bottom">
          <span className={`grid-card-type grid-card-type--${macro.type}`}>{typeName}</span>
          {preview && <span className="grid-card-preview" title={preview}>{preview}</span>}
        </div>
      </div>
    );
  }

  // ── Modifier bar for list view ──────────────────────────────
  const recordStartTime = useRef(0);

  useEffect(() => {
    if (isRecording) recordStartTime.current = Date.now();
  }, [isRecording]);

  function renderModifierBar() {
    const isBare = activeModifiers.includes('BARE');
    return (
      <div className="sidebar-modifier-bar">
        <div className="sidebar-modifier-keys">
          {MODIFIERS.map(mod => {
            const isActive = activeModifiers.includes(mod.id);
            return (
              <button
                key={mod.id}
                className={`sidebar-mod-btn${isActive ? ' active' : ''}`}
                style={isActive ? { '--mod-color': mod.color } : {}}
                onClick={isRecording ? undefined : () => onToggleModifier?.(mod.id)}
                disabled={isRecording}
                type="button"
              >
                {mod.label}
              </button>
            );
          })}
          <button
            className={`sidebar-mod-btn sidebar-mod-btn--bare${isBare ? ' active' : ''}`}
            style={isBare ? { '--mod-color': '#ff9040' } : {}}
            onClick={isRecording ? undefined : () => onToggleModifier?.('BARE')}
            disabled={isRecording}
            title={profileLinked ? 'Bare key assignments' : 'Bare key assignments (F-keys, numpad, nav keys)'}
            type="button"
          >
            Bare
          </button>
        </div>
        <div className="sidebar-modifier-right">
          {isRecording ? (
            <button
              className="sidebar-record-btn sidebar-record-btn--recording"
              onClick={() => {
                if (Date.now() - recordStartTime.current < 200) return;
                onStopRecord?.();
              }}
              type="button"
            >
              <span className="sidebar-record-dot" />
              Recording…
            </button>
          ) : (
            <button
              className="sidebar-record-btn"
              onMouseDown={() => onStartRecord?.()}
              type="button"
            >
              ⏺ Record
            </button>
          )}
        </div>
      </div>
    );
  }

  return (
    <aside className={`sidebar${listViewActive ? ' sidebar--expanded' : ''}`}>
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
        onUpdateProfileSettings={onUpdateProfileSettings}
        onExportProfile={onExportProfile}
        onImportProfile={onImportProfile}
        importPrompt={importPrompt}
        onImportProfileResolve={onImportProfileResolve}
        onImportPromptDismiss={onImportPromptDismiss}
      />

      <div className="sidebar-header">
        <span className="sidebar-title">Assignments</span>
        <span className="sidebar-count">{profileEntries.length}</span>
      </div>

      <div className="sidebar-filter-wrap">
        <input
          className="sidebar-filter-input"
          type="text"
          placeholder="Filter assignments…"
          value={assignFilter}
          onChange={e => setAssignFilter(e.target.value)}
          spellCheck={false}
        />
        {assignFilter && (
          <button className="sidebar-filter-clear" onClick={() => setAssignFilter('')} type="button">✕</button>
        )}
      </div>

      {listViewActive && renderModifierBar()}

      {/* Tabs only shown in classic (non-list) view */}
      {!listViewActive && (
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
      )}

      {listViewActive ? (
        /* ── Grid view — filtered by sidebarComboFilter (modifier bar clicks only) ── */
        (() => {
          const gridCombo = sidebarComboFilter || null;
          const gridFiltered = (gridCombo
            ? profileEntries.filter(e => e.combo === gridCombo)
            : profileEntries
          ).filter(matchesFilter);
          const gridGrouped = {};
          if (!gridCombo) {
            gridFiltered.forEach(e => {
              if (!gridGrouped[e.combo]) gridGrouped[e.combo] = [];
              gridGrouped[e.combo].push(e);
            });
          }
          const gridSortedCombos = Object.keys(gridGrouped).sort((a, b) => {
            if (a === 'BARE') return -1;
            if (b === 'BARE') return 1;
            if (a.length !== b.length) return a.length - b.length;
            return a.localeCompare(b);
          });

          return (
            <div className="sidebar-grid-wrap">
              {profileEntries.length === 0 ? (
                <div className="sidebar-empty sidebar-empty--grid">
                  <div className="sidebar-empty-icon">⌨</div>
                  <p>No assignments yet. Select a modifier above, then press <strong>Record</strong> to capture your first hotkey.</p>
                </div>
              ) : gridFiltered.length === 0 ? (
                <div className="sidebar-empty sidebar-empty--grid">
                  <p>No assignments on this layer yet</p>
                </div>
              ) : !gridCombo ? (
                <div className="sidebar-grid">
                  {gridSortedCombos.map(combo => (
                    <React.Fragment key={combo}>
                      <div className="sidebar-grid-group-header">
                        {combo === 'BARE' ? 'BARE KEYS' : combo}
                        <span className="sidebar-group-count">{gridGrouped[combo].length}</span>
                      </div>
                      {gridGrouped[combo].map(renderCard)}
                    </React.Fragment>
                  ))}
                </div>
              ) : (
                <div className="sidebar-grid">
                  <div className="sidebar-grid-group-header">
                    {gridCombo === 'BARE' ? 'BARE KEYS' : gridCombo}
                    <span className="sidebar-group-count">{gridFiltered.length}</span>
                  </div>
                  {gridFiltered.map(renderCard)}
                </div>
              )}
            </div>
          );
        })()
      ) : (
        /* ── Classic list view ──────────────────────────────── */
        <>
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
                  <div className="sidebar-grid-group-header">
                    {combo === 'BARE' ? 'BARE KEYS' : combo}
                    <span className="sidebar-group-count">{grouped[combo].length}</span>
                  </div>
                  {grouped[combo].map(renderItem)}
                </div>
              ))
            ) : (
              <>
                <div className="sidebar-grid-group-header">
                  {activeTab === 'BARE' ? 'BARE KEYS' : activeTab}
                  <span className="sidebar-group-count">{filtered.length}</span>
                </div>
                {filtered.map(renderItem)}
              </>
            )}
          </div>

          <div className="sidebar-footer">
            <div className="legend-item"><span className="legend-dot assigned" />Assigned</div>
            <div className="legend-item"><span className="legend-dot selected" />Selected</div>
            <div className="legend-item"><span className="legend-dot system-ld" />System Key</div>
          </div>
        </>
      )}
      {/* Assignment context menu */}
      {assignCtx && (
        <div
          ref={assignCtxRef}
          className="assign-ctx-menu"
          style={{ top: assignCtx.y, left: assignCtx.x }}
        >
          <button className="assign-ctx-item" type="button" onClick={handleCtxRename}>Rename</button>
          <button className="assign-ctx-item" type="button" onClick={handleCtxDuplicate}>Duplicate</button>
          {otherProfiles.length > 0 && (
            <>
              <div className="assign-ctx-divider" />
              <div className="assign-ctx-sub">
                <button className="assign-ctx-item" type="button">Copy to ▸</button>
                <div className="assign-ctx-submenu">
                  {otherProfiles.map(p => (
                    <button
                      key={p}
                      className="assign-ctx-item"
                      type="button"
                      onClick={() => {
                        onCopyToProfile?.(p, assignCtx.combo, assignCtx.keyId);
                        setAssignCtx(null);
                      }}
                    >
                      {p}
                    </button>
                  ))}
                </div>
              </div>
              <div className="assign-ctx-sub">
                <button className="assign-ctx-item" type="button">Move to ▸</button>
                <div className="assign-ctx-submenu">
                  {otherProfiles.map(p => (
                    <button
                      key={p}
                      className="assign-ctx-item"
                      type="button"
                      onClick={() => {
                        onMoveToProfile?.(p, assignCtx.combo, assignCtx.keyId);
                        setAssignCtx(null);
                      }}
                    >
                      {p}
                    </button>
                  ))}
                </div>
              </div>
            </>
          )}
          <div className="assign-ctx-divider" />
          <button className="assign-ctx-item assign-ctx-danger" type="button" onClick={handleCtxClear}>Clear</button>
        </div>
      )}
    </aside>
  );
}
