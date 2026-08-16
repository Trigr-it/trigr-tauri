import React, { useState, useEffect, useLayoutEffect, useRef, useMemo } from 'react';
import ReactDOM from 'react-dom';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors, useDraggable } from '@dnd-kit/core';
import { SortableContext, verticalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import { GripVertical, Link, Keyboard, Zap, Disc, Plus, Download, Users } from 'lucide-react';
import './Sidebar.css';
import { SearchBar } from './SearchBar';
import { friendlyKeyName } from './keyboardLayout';
import { useModalKeyboard } from '../hooks/useModalKeyboard';

const TYPE_META = {
  text:      { color: '#64b4ff' },
  expansion: { color: '#a070ff' },
  hotkey:    { color: '#c864ff' },
  app:       { color: '#50c878' },
  url:       { color: '#ffc832' },
  macro:     { color: '#ff783c' },
  folder:    { color: '#40c8a0' },
};

const TYPE_NAMES = {
  text: 'Text', expansion: 'Expansion', hotkey: 'Hotkey', app: 'App',
  url: 'URL', macro: 'Macro', folder: 'Folder',
};

// ── Sortable profile row ────────────────────────────────────────────────────

function SortableProfileRow({ profile, isActive, isFallback, hasLink, linkedAppName, linkedWindowTitle, onSelect, onDoubleClick, onContextMenu }) {
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
      <div className="profile-drag-handle" {...attributes} {...listeners} aria-label="Drag to reorder">
        <GripVertical size={12} strokeWidth={1.75} />
      </div>
      <span className="profile-row-name">
        {isFallback && <span className="profile-fallback-dot" />}
        {profile}
      </span>
      {hasLink && (
        <span className="profile-row-link" title={linkedAppName + (linkedWindowTitle ? ` (title: ${linkedWindowTitle})` : '')} aria-label="Linked to app">
          <Link size={11} strokeWidth={1.75} />
        </span>
      )}
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
  isPro = false, onShowUpgrade,
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
  const [linkPickerMode, setLinkPickerMode] = useState('link'); // 'link' | 'change'
  const [linkPickerCurrentApp, setLinkPickerCurrentApp] = useState(null); // shown in Change mode
  const [linkWindowList, setLinkWindowList] = useState([]);
  const [linkSelectedExe, setLinkSelectedExe] = useState(null);
  const [linkWindowTitle, setLinkWindowTitle] = useState('');
  const [linkDropdownOpen, setLinkDropdownOpen] = useState(false);
  const linkDropdownRef = useRef(null);
  const linkDropdownPortalRef = useRef(null);
  const pickAppBtnRef = useRef(null);
  const [linkDropdownPos, setLinkDropdownPos] = useState(null);
  const linkModalPanelRef = useRef(null);
  const importPromptRef = useRef(null);

  function closeLinkPicker() {
    setLinkPicker(null);
    setLinkPickerMode('link');
    setLinkPickerCurrentApp(null);
    setLinkSelectedExe(null);
    setLinkWindowTitle('');
    setLinkDropdownOpen(false);
  }

  useModalKeyboard(linkModalPanelRef, closeLinkPicker, { enabled: !!linkPicker });

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

  // Clamp the profile right-click menu inside the viewport — raw clientX/clientY
  // overflow when right-clicking near the sidebar's bottom or right edge.
  useLayoutEffect(() => {
    if (!contextMenu || !ctxRef.current) return;
    const el = ctxRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
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

  // Flip the portal'd link-to-app dropdown above the trigger button when its
  // default below-button position would clip the viewport. Remeasures on
  // linkWindowList load so the placeholder→real-rows height change is caught.
  useLayoutEffect(() => {
    if (!linkDropdownOpen || !linkDropdownPos || !linkDropdownPortalRef.current) return;
    const el = linkDropdownPortalRef.current;
    el.style.top = `${linkDropdownPos.top}px`;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.bottom > window.innerHeight - margin) {
      const flipped = linkDropdownPos.btnTop - rect.height - 4;
      el.style.top = `${Math.max(margin, flipped)}px`;
    }
  }, [linkDropdownOpen, linkDropdownPos, linkWindowList]);

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
    const linkedWinTitle = profileSettings[p]?.linkedWindowTitle;
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
          linkedWindowTitle={linkedWinTitle || ''}
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
      {/* Header — always visible. Two-line gold-accented header treatment so
          it reads as a substantive UI area, not a muted category divider.
          Testers didn't realise profiles existed as a feature under the old
          all-caps "PROFILES" label — the gold stripe + tinted band + display
          typography now anchor the concept visually. Line 1: small gold
          "PROFILE" label + chevron. Line 2: active profile name in the
          display font, bright gold. Line 3 (conditional): muted "editing: X"
          note when the user is previewing a different profile. */}
      <div
        className={`profile-accordion-header${isExpanded ? ' profile-accordion-header-open' : ''}`}
        onClick={() => setIsExpanded(v => !v)}
        title="Switch between profiles, or right-click a profile for options"
        role="button"
        aria-expanded={isExpanded}
      >
        <div className="profile-accordion-header-top">
          <Users size={12} strokeWidth={2} className="profile-accordion-icon" />
          <span className="profile-accordion-label">PROFILE</span>
          <span className="profile-accordion-switch">
            {isExpanded ? 'Close' : 'Switch'}
            <span className="profile-accordion-chevron">{isExpanded ? '▴' : '▾'}</span>
          </span>
        </div>
        {/* activeProfile follows the currently-firing profile — updated by
            both the foreground watcher's profile-switched event (auto-switch
            when the user tabs to an app-linked profile) AND manual sidebar
            clicks. activeGlobalProfile is the USER'S PERSISTED FALLBACK
            (config setting, changed only via right-click → Set as global),
            so it's shown as a secondary "fallback:" note when the firing
            profile is something else. */}
        <div className="profile-accordion-header-name" title={`Active profile: ${activeProfile}`}>
          {activeProfile}
        </div>
        {!isSameProfile && (
          <div className="profile-accordion-header-fallback" title={`Global fallback profile: ${activeGlobalProfile}`}>
            fallback: <span>{activeGlobalProfile}</span>
          </div>
        )}
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

          {/* Add / Import profile actions */}
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
            <div className="profile-actions-row">
              <button
                className="profile-action-btn profile-action-btn--primary"
                type="button"
                onClick={() => setIsAdding(true)}
              >
                <Plus size={14} strokeWidth={2.25} />
                <span>New Profile</span>
              </button>
              <button
                className="profile-action-btn profile-action-btn--secondary"
                type="button"
                onClick={() => onImportProfile?.()}
              >
                <Download size={14} strokeWidth={2.25} />
                <span>Import Profile</span>
              </button>
            </div>
          )}
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
                if (!isPro) { onShowUpgrade?.('App-specific profiles'); setContextMenu(null); return; }
                setLinkPicker(contextMenu.profile);
                setLinkPickerMode('link');
                setLinkPickerCurrentApp(null);
                setLinkSelectedExe(null);
                setLinkWindowTitle('');
                setContextMenu(null);
              }}>
                Link to App <span className="pro-badge">PRO</span>
              </button>
            )}
            {!isStatic && (
              <>
                <button className="profile-ctx-item" onClick={() => {
                  if (!isPro) { onShowUpgrade?.('App-specific profiles'); setContextMenu(null); return; }
                  const settings = profileSettings[contextMenu.profile] || {};
                  setLinkPicker(contextMenu.profile);
                  setLinkPickerMode('change');
                  setLinkPickerCurrentApp(settings.linkedApp || null);
                  setLinkSelectedExe(settings.linkedApp || null);
                  setLinkWindowTitle(settings.linkedWindowTitle || '');
                  setContextMenu(null);
                }}>
                  Change App
                </button>
                <button className="profile-ctx-item" onClick={() => {
                  onUpdateProfileSettings?.(contextMenu.profile, { linkedApp: null, linkedWindowTitle: null });
                  setContextMenu(null);
                }}>
                  Unlink App
                </button>
              </>
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

      {/* Link to App picker modal */}
      {linkPicker && (
        <div
          className="modal-overlay profile-link-modal-overlay"
          role="dialog"
          aria-modal="true"
          aria-labelledby="profile-link-modal-title"
          onClick={closeLinkPicker}
        >
          <div
            className="modal-panel profile-link-modal"
            ref={linkModalPanelRef}
            onClick={e => e.stopPropagation()}
          >
            <div className="profile-link-modal-header">
              <h2 className="profile-link-modal-title" id="profile-link-modal-title">
                {linkPickerMode === 'change' ? 'Change linked app' : 'Link profile to app'}
              </h2>
              <button
                className="profile-link-modal-close"
                type="button"
                onClick={closeLinkPicker}
                aria-label="Close"
              >
                <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
                  <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
                  <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
                </svg>
              </button>
            </div>
            <p className="profile-link-modal-sub">
              Profile <strong>"{linkPicker}"</strong> will switch on automatically when the linked app gains focus.
            </p>
            {linkPickerMode === 'change' && linkPickerCurrentApp && (
              <div className="profile-link-modal-current">
                Currently linked to <strong>{linkPickerCurrentApp}</strong>
              </div>
            )}
            <p className="profile-link-modal-hint">Make sure the app is running, then use Pick App below or browse for the app directly.</p>
            <div className="profile-link-modal-row" ref={linkDropdownRef}>
              {linkSelectedExe ? (
                <span className="pick-window-badge">
                  {linkSelectedExe}
                  <button className="pick-window-badge-clear" type="button" onClick={() => setLinkSelectedExe(null)} aria-label="Clear selection">✕</button>
                </span>
              ) : (
                <>
                  <button className="browse-btn" ref={pickAppBtnRef} type="button" onClick={async () => {
                    const rowEl = linkDropdownRef.current;
                    if (rowEl) {
                      const rect = rowEl.getBoundingClientRect();
                      setLinkDropdownPos({ top: rect.bottom + 4, left: rect.left, width: rect.width, btnTop: rect.top });
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
                      console.error('[Keyfire] list_open_windows failed:', e);
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
            {linkSelectedExe && (
              <div className="profile-link-modal-title-row">
                <label className="profile-link-modal-label">Window title contains (optional)</label>
                <input
                  className="profile-link-modal-title-input"
                  type="text"
                  placeholder="e.g. Inbox"
                  value={linkWindowTitle}
                  onChange={e => setLinkWindowTitle(e.target.value)}
                />
              </div>
            )}
            <div className="profile-link-modal-actions">
              <button className="profile-link-modal-cancel" type="button" onClick={closeLinkPicker}>
                Cancel
              </button>
              <button
                className="profile-link-modal-confirm"
                type="button"
                disabled={!linkSelectedExe}
                onClick={() => {
                  if (!linkSelectedExe) return;
                  onUpdateProfileSettings?.(linkPicker, {
                    linkedApp: linkSelectedExe,
                    linkedWindowTitle: linkWindowTitle.trim() || null,
                  });
                  closeLinkPicker();
                }}
              >
                {linkPickerMode === 'change' ? 'Save' : 'Confirm'}
              </button>
            </div>
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

// ── Draggable wrapper for sidebar cards/items in radial mode ────────────────

function DraggableCardWrap({ id, storageKey, isUsed, enabled, children }) {
  const { attributes, listeners, setNodeRef, isDragging } = useDraggable({
    id: `sidebar-${id}`,
    data: { kind: 'library-card', storageKey },
    disabled: !enabled || isUsed,
  });

  return (
    <div
      ref={setNodeRef}
      className={`${isDragging ? 'is-dragging' : ''}${isUsed ? ' is-used' : ''}`}
      {...(enabled && !isUsed ? { ...listeners, ...attributes } : {})}
      style={{ touchAction: 'none' }}
    >
      {children}
    </div>
  );
}

function DraggableFolderCard({ id, folderName, children }) {
  const { attributes, listeners, setNodeRef, isDragging } = useDraggable({
    id: `folder-${id}`,
    data: { kind: 'library-folder', folderName },
  });

  return (
    <div
      ref={setNodeRef}
      className={isDragging ? 'is-dragging' : ''}
      {...listeners}
      {...attributes}
      style={{ touchAction: 'none' }}
    >
      {children}
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
  // Radial mode props
  activeView = 'keyboard',
  radialMenuItems = [],
  // Pro gating
  isPro = false,
  onShowUpgrade,
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
      if (parts[parts.length - 1] === 'hold') continue;
      const baseKey = k;
      seen.add(baseKey);
      entries.push({
        combo:      parts[1] || '',
        keyId:      parts[2] || '',
        macro:      v,
        hasDouble:  !!assignments[baseKey + '::double'],
        hasHold:    !!assignments[baseKey + '::hold'],
        doubleOnly: false,
        holdOnly:   false,
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
      seen.add(baseKey);
      entries.push({
        combo:      parts[1] || '',
        keyId:      parts[2] || '',
        macro:      v,
        hasDouble:  true,
        hasHold:    !!assignments[baseKey + '::hold'],
        doubleOnly: true,
        holdOnly:   false,
      });
    }
    // Third pass: collect hold-only entries (no single, no double)
    for (const [k, v] of Object.entries(assignments)) {
      if (!k.startsWith(activeProfile + '::')) continue;
      if (k.includes('::EXPANSION::')) continue;
      const parts = k.split('::');
      if (parts[parts.length - 1] !== 'hold') continue;
      const baseKey = parts.slice(0, -1).join('::');
      if (seen.has(baseKey)) continue; // already listed via single/double entry
      entries.push({
        combo:      parts[1] || '',
        keyId:      parts[2] || '',
        macro:      v,
        hasDouble:  false,
        hasHold:    true,
        doubleOnly: false,
        holdOnly:   true,
      });
    }
    return entries;
  })();

  const otherProfiles = (profiles || []).filter(p => p !== activeProfile);

  // Assignments only appear under the canvas they belong to: mouse view
  // lists mouse triggers, keyboard view lists keyboard triggers. Other
  // views (radial) keep the full set.
  const viewEntries = activeView === 'mouse'
    ? profileEntries.filter(e => e.keyId.startsWith('MOUSE_'))
    : activeView === 'keyboard'
      ? profileEntries.filter(e => !e.keyId.startsWith('MOUSE_'))
      : profileEntries;

  const comboSource = viewEntries;
  const combos = [...new Set(comboSource.map(e => e.combo))].sort((a, b) => {
    if (a.length !== b.length) return a.length - b.length;
    return a.localeCompare(b);
  });

  const [activeTab, setActiveTab] = useState('All');
  const [assignFilter, setAssignFilter] = useState('');
  const [assignSort, setAssignSort] = useState(() =>
    localStorage.getItem('trigr.assignmentSort') || 'key-asc'
  );

  // ── Radial mode state ──
  const isRadialMode = activeView === 'radial';
  const [newFolderName, setNewFolderName] = useState(null);

  const radialUsedKeys = useMemo(() => {
    const s = new Set();
    radialMenuItems.forEach(i => {
      if (!i) return;
      if (i.storageKey) s.add(i.storageKey);
      if (i.children) i.children.forEach(c => { if (c && c.storageKey) s.add(c.storageKey); });
    });
    return s;
  }, [radialMenuItems]);

  // ── Assignment context menu + inline actions ──
  const [assignCtx, setAssignCtx] = useState(null); // { combo, keyId, macro, x, y }
  const [renaming, setRenaming] = useState(null); // { combo, keyId }
  const [renameVal, setRenameVal] = useState('');
  const [clearing, setClearing] = useState(null); // { combo, keyId }
  const assignCtxRef = useRef(null);
  // Which assignment-submenu is hovered ('copy' | 'move' | null). Replaces the
  // old CSS-only :hover trigger so a layout effect can flip the submenu when
  // its default left:100%/top:-4px position would clip the viewport.
  const [hoveredAssignSub, setHoveredAssignSub] = useState(null);
  const assignCtxSubmenuRef = useRef(null);

  useEffect(() => {
    if (!assignCtx) return;
    function onDown(e) {
      if (assignCtxRef.current && !assignCtxRef.current.contains(e.target)) setAssignCtx(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [assignCtx]);

  // Clamp both right-click context menus inside the viewport. Right-clicks
  // near the bottom-right corner of the sidebar would otherwise overflow.
  useLayoutEffect(() => {
    if (!assignCtx || !assignCtxRef.current) return;
    const el = assignCtxRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
    // Reset hover state so a stale submenu doesn't render off-screen when the
    // user re-opens the menu in a different location.
    setHoveredAssignSub(null);
  }, [assignCtx]);

  // Flip the active assignment submenu — shift up if bottom would clip, swap
  // to the left side if right would clip. Mirrors the macro step-type submenu
  // fix exactly.
  useLayoutEffect(() => {
    if (!assignCtx || !hoveredAssignSub || !assignCtxSubmenuRef.current) return;
    const sub = assignCtxSubmenuRef.current;
    sub.style.top = '';
    sub.style.left = '';
    sub.style.right = '';
    sub.style.marginLeft = '';
    sub.style.marginRight = '';
    const rect = sub.getBoundingClientRect();
    const margin = 8;
    const bottomOverflow = rect.bottom - (window.innerHeight - margin);
    if (bottomOverflow > 0) {
      let shift = bottomOverflow;
      const newTop = rect.top - shift;
      if (newTop < margin) shift -= (margin - newTop);
      sub.style.top = `${-4 - shift}px`;
    }
    if (rect.right > window.innerWidth - margin) {
      sub.style.left = 'auto';
      sub.style.right = '100%';
    }
  }, [hoveredAssignSub, assignCtx]);

  useEffect(() => {
    setActiveTab('All');
  }, [activeProfile]);

  useEffect(() => {
    setActiveTab(sidebarComboFilter || 'All');
  }, [sidebarComboFilter]);

  // Only show modifier-layer tabs for combos that actually have assignments.
  // Empty layers (including BARE when no bare keys are assigned) stay hidden so
  // the tab bar doesn't overflow with noise.
  const tabs = ['All', ...combos];

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

  // View-scoped entries (mouse view → mouse only, keyboard view → keyboard only)
  const viewFiltered = viewEntries;

  const filtered = sortEntries((activeTab === 'All'
    ? viewFiltered
    : viewFiltered.filter(e => e.combo === activeTab)
  ).filter(matchesFilter));

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
    MOUSE_LEFT: 'Mouse Left', MOUSE_RIGHT: 'Mouse Right', MOUSE_MIDDLE: 'Mouse Mid',
    MOUSE_SCROLL_UP: 'Mouse Scroll ↑', MOUSE_SCROLL_DOWN: 'Mouse Scroll ↓',
    MOUSE_SIDE1: 'Mouse Side1', MOUSE_SIDE2: 'Mouse Side2',
  };

  function sortEntries(arr) {
    const a = [...arr];
    // numeric: true = natural sort, so F2 comes before F12 (plain
    // localeCompare is lexicographic and ordered F1, F12, F2...).
    const nat = (x, y) => x.localeCompare(y, undefined, { numeric: true, sensitivity: 'base' });
    switch (assignSort) {
      case 'key-desc':  return a.sort((x, y) => nat(friendlyKeyName(y.keyId), friendlyKeyName(x.keyId)));
      case 'name-asc':  return a.sort((x, y) => nat(x.macro?.label || x.macro?.data?.text || x.macro?.data?.url || x.macro?.data?.path || '', y.macro?.label || y.macro?.data?.text || y.macro?.data?.url || y.macro?.data?.path || ''));
      case 'name-desc': return a.sort((x, y) => nat(y.macro?.label || y.macro?.data?.text || y.macro?.data?.url || y.macro?.data?.path || '', x.macro?.label || x.macro?.data?.text || x.macro?.data?.url || x.macro?.data?.path || ''));
      case 'type':      return a.sort((x, y) => (x.macro?.type || '').localeCompare(y.macro?.type || ''));
      default:          return a.sort((x, y) => nat(friendlyKeyName(x.keyId), friendlyKeyName(y.keyId))); // key-asc
    }
  }

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

  function renderItem({ combo, keyId, macro, hasDouble, doubleOnly, hasHold, holdOnly }) {
    const meta = TYPE_META[macro.type] || { color: 'var(--text-muted)' };
    const displayKey = MOUSE_KEY_LABELS[keyId] || friendlyKeyName(keyId);
    const isSelected = selectedKey === keyId && combo === currentCombo;
    const isBareItem = combo === 'BARE';
    const typeName = TYPE_NAMES[macro.type] || macro.type;
    const displayLabel = macro.label || macro.data?.text || macro.data?.url || macro.data?.path || typeName;

    if (isClearing(combo, keyId)) {
      return (
        <div key={`${combo}::${keyId}`} className="sidebar-item sidebar-item-confirm">
          <span className="sidebar-confirm-text">Delete this key?</span>
          <button className="sidebar-confirm-yes" type="button" onClick={confirmClear}>Yes</button>
          <button className="sidebar-confirm-no" type="button" onClick={() => setClearing(null)}>No</button>
        </div>
      );
    }

    const storageKey = `${activeProfile}::${combo}::${keyId}`;
    const isUsedInRadial = radialUsedKeys.has(storageKey);

    const itemEl = (
      <div
        key={`${combo}::${keyId}`}
        className={`sidebar-item type-${macro.type}${isSelected ? ' sidebar-item-active' : ''}${isBareItem ? ' bare-item' : ''}`}
        onClick={() => onSelectAssignment(keyId, combo)}
        onContextMenu={e => handleAssignContextMenu(e, combo, keyId, macro)}
        title={`Edit ${isBareItem ? 'Bare' : combo}+${displayKey}`}
      >
        <div className="sidebar-key-stack">
          <span className="sidebar-key-badge" style={{ borderColor: meta.color + '55', color: meta.color }}>
            {displayKey}
          </span>
          {(hasDouble || hasHold) && (
            <span className="sidebar-mode-chip-row">
              {hasDouble && (
                <span
                  className={`sidebar-mode-chip${doubleOnly ? ' sidebar-mode-chip-only' : ''}`}
                  title={doubleOnly ? 'Double-press only (no single-press action)' : 'Double-press also mapped'}
                >×2</span>
              )}
              {hasHold && (
                <span
                  className={`sidebar-mode-chip${holdOnly ? ' sidebar-mode-chip-only' : ''}`}
                  title={holdOnly ? 'Hold only (no single-press action)' : 'Hold also mapped'}
                >⏱</span>
              )}
            </span>
          )}
        </div>
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
              displayLabel
            )}
          </div>
          <div className="sidebar-item-type">
            <span className="type-dot" style={{ background: meta.color }} />
            {typeName}
          </div>
        </div>
      </div>
    );

    if (isRadialMode) {
      return (
        <DraggableCardWrap
          key={`${combo}::${keyId}`}
          id={`${combo}::${keyId}`}
          storageKey={storageKey}
          isUsed={isUsedInRadial}
          enabled={true}
        >
          {itemEl}
        </DraggableCardWrap>
      );
    }

    return itemEl;
  }

  // ── Card for list view grid ──────────────────────────────────
  function renderCard({ combo, keyId, macro, hasDouble, doubleOnly, hasHold, holdOnly }) {
    const meta = TYPE_META[macro.type] || { color: 'var(--text-muted)' };
    const displayKey = MOUSE_KEY_LABELS[keyId] || friendlyKeyName(keyId);
    const isSelected = selectedKey === keyId && combo === currentCombo;
    const comboLabel = combo === 'BARE' ? displayKey : combo + '+' + displayKey;
    const typeName = TYPE_NAMES[macro.type] || macro.type;
    const displayLabel = macro.label || macro.data?.text || macro.data?.url || macro.data?.path || typeName;

    // Preview line
    let preview = '';
    if (macro.type === 'text') {
      const raw = macro.data?.text || '';
      preview = raw.length > 40 ? raw.slice(0, 40) + '…' : raw;
    } else if (macro.type === 'expansion') {
      const trig = macro.data?.trigger || '';
      preview = trig ? `Fires :${trig}` : 'No expansion selected';
    } else if (macro.type === 'macro') {
      const steps = macro.data?.steps || [];
      preview = `${steps.length} step${steps.length !== 1 ? 's' : ''}`;
    } else if (macro.type === 'hotkey') {
      preview = macro.data?.target || macro.label || '';
    } else if (macro.type === 'app') {
      preview = macro.data?.kind === 'aumid'
        ? (macro.data?.appName || 'Installed app')
        : ((macro.data?.path || '').split(/[/\\]/).pop() || '');
    } else if (macro.type === 'url') {
      preview = macro.data?.url || '';
    } else if (macro.type === 'folder') {
      preview = macro.data?.path || '';
    }
    // Don't echo the title: when the item has no custom label, displayLabel
    // falls back to the same data field the preview shows (url, path, target,
    // text), so the card would print it twice. The startsWith case covers
    // truncated text previews ("Lorem ipsum…" under a "Lorem ipsum dolor…"
    // label).
    if (preview === displayLabel ||
        (preview.endsWith('…') && displayLabel.startsWith(preview.slice(0, -1)))) {
      preview = '';
    }

    if (isClearing(combo, keyId)) {
      return (
        <div key={`${combo}::${keyId}`} className="grid-card grid-card-confirm">
          <span className="sidebar-confirm-text">Delete this key?</span>
          <div className="sidebar-confirm-btns">
            <button className="sidebar-confirm-yes" type="button" onClick={confirmClear}>Yes</button>
            <button className="sidebar-confirm-no" type="button" onClick={() => setClearing(null)}>No</button>
          </div>
        </div>
      );
    }

    const storageKey = `${activeProfile}::${combo}::${keyId}`;
    const isUsedInRadial = radialUsedKeys.has(storageKey);

    const cardEl = (
      <div
        key={`${combo}::${keyId}`}
        className={`grid-card${isSelected ? ' grid-card--active' : ''}`}
        onClick={() => onSelectAssignment(keyId, combo)}
        onContextMenu={e => handleAssignContextMenu(e, combo, keyId, macro)}
      >
        {/* Same per-item anatomy as the sidebar rows: coloured key badge +
            mode chips, not accent text. Keep these two renderers visually
            in sync. */}
        <div className="grid-card-combo">
          <span className="sidebar-key-badge" style={{ borderColor: meta.color + '55', color: meta.color }}>
            {comboLabel}
          </span>
          {(hasDouble || hasHold) && (
            <span className="sidebar-mode-chip-row">
              {hasDouble && (
                <span
                  className={`sidebar-mode-chip${doubleOnly ? ' sidebar-mode-chip-only' : ''}`}
                  title={doubleOnly ? 'Double-press only (no single-press action)' : 'Double-press also mapped'}
                >×2</span>
              )}
              {hasHold && (
                <span
                  className={`sidebar-mode-chip${holdOnly ? ' sidebar-mode-chip-only' : ''}`}
                  title={holdOnly ? 'Hold only (no single-press action)' : 'Hold also mapped'}
                >⏱</span>
              )}
            </span>
          )}
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
          <span className="sidebar-item-type">
            <span className="type-dot" style={{ background: meta.color }} />
            {typeName}
          </span>
          {preview && <span className="grid-card-preview" title={preview}>{preview}</span>}
        </div>
      </div>
    );

    if (isRadialMode) {
      return (
        <DraggableCardWrap
          key={`${combo}::${keyId}`}
          id={`card-${combo}::${keyId}`}
          storageKey={storageKey}
          isUsed={isUsedInRadial}
          enabled={true}
        >
          {cardEl}
        </DraggableCardWrap>
      );
    }

    return cardEl;
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
        {!isRadialMode && (
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
              <Disc size={12} strokeWidth={2} fill="currentColor" style={{ marginRight: 4, verticalAlign: -1 }} /> Record
            </button>
          )}
        </div>
        )}
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
        isPro={isPro}
        onShowUpgrade={onShowUpgrade}
      />

      <div className="sidebar-header">
        <span className="sidebar-title">Assignments</span>
        <span className="sidebar-count">{comboSource.length}</span>
      </div>

      <div className="sidebar-filter-wrap">
        <div className="sidebar-filter-input-wrap">
          <SearchBar
            className="sidebar-filter-bar"
            placeholder="Filter assignments…"
            value={assignFilter}
            onChange={e => setAssignFilter(e.target.value)}
          />
          {assignFilter && (
            <button className="sidebar-filter-clear" onClick={() => setAssignFilter('')} type="button">✕</button>
          )}
        </div>
        <select
          className="sidebar-sort-select"
          value={assignSort}
          onChange={e => {
            setAssignSort(e.target.value);
            localStorage.setItem('trigr.assignmentSort', e.target.value);
          }}
          title="Sort assignments"
        >
          <option value="key-asc">Key A→Z</option>
          <option value="key-desc">Key Z→A</option>
          <option value="name-asc">Name A→Z</option>
          <option value="name-desc">Name Z→A</option>
          <option value="type">Type</option>
        </select>
      </div>

      {listViewActive && renderModifierBar()}

      {/* Add folder button — radial mode only */}
      {isRadialMode && (
        <div className="sidebar-add-folder-wrap">
          {newFolderName === null ? (
            <button
              className="sidebar-add-folder-btn"
              type="button"
              onClick={() => setNewFolderName('')}
            >
              + New Folder
            </button>
          ) : (
            <div className="sidebar-new-folder-row">
              <input
                className="sidebar-new-folder-input"
                placeholder="Folder name"
                value={newFolderName}
                onChange={e => setNewFolderName(e.target.value)}
                onKeyDown={e => {
                  e.stopPropagation();
                  if (e.key === 'Escape') setNewFolderName(null);
                }}
                autoFocus
              />
              <DraggableFolderCard
                id="new-folder-drag"
                folderName={newFolderName || 'New folder'}
              >
                <div className="sidebar-folder-drag-card">
                  <span className="sidebar-folder-drag-icon">{'\u25c9'}</span>
                  <span className="sidebar-folder-drag-label">{newFolderName || 'New folder'}</span>
                </div>
              </DraggableFolderCard>
            </div>
          )}
        </div>
      )}

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
          const gridFiltered = sortEntries((gridCombo
            ? viewEntries.filter(e => e.combo === gridCombo)
            : viewEntries
          ).filter(matchesFilter));
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
              {viewEntries.length === 0 ? (
                <div className="sidebar-empty sidebar-empty--grid">
                  <div className="sidebar-empty-icon" aria-hidden="true"><Keyboard size={28} strokeWidth={1.5} /></div>
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
            {viewEntries.length === 0 && activeTab !== 'BARE' ? (
              <div className="sidebar-empty">
                <div className="sidebar-empty-icon" aria-hidden="true"><Keyboard size={28} strokeWidth={1.5} /></div>
                <p>Select modifiers above the keyboard, then click a key to assign a hotkey</p>
              </div>
            ) : filtered.length === 0 ? (
              <div className="sidebar-empty">
                {activeTab === 'BARE' ? (
                  <>
                    <div className="sidebar-empty-icon" aria-hidden="true"><Zap size={28} strokeWidth={1.5} /></div>
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
              <div
                className="assign-ctx-sub"
                onMouseEnter={() => setHoveredAssignSub('copy')}
                onMouseLeave={() => setHoveredAssignSub(prev => prev === 'copy' ? null : prev)}
              >
                <button className="assign-ctx-item" type="button">Copy to ▸</button>
                {hoveredAssignSub === 'copy' && (
                  <div className="assign-ctx-submenu" ref={assignCtxSubmenuRef}>
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
                )}
              </div>
              <div
                className="assign-ctx-sub"
                onMouseEnter={() => setHoveredAssignSub('move')}
                onMouseLeave={() => setHoveredAssignSub(prev => prev === 'move' ? null : prev)}
              >
                <button className="assign-ctx-item" type="button">Move to ▸</button>
                {hoveredAssignSub === 'move' && (
                  <div className="assign-ctx-submenu" ref={assignCtxSubmenuRef}>
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
                )}
              </div>
            </>
          )}
          <div className="assign-ctx-divider" />
          <button className="assign-ctx-item assign-ctx-danger" type="button" onClick={handleCtxClear}>Delete</button>
        </div>
      )}
    </aside>
  );
}
