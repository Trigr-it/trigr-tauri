import React, { useState, useEffect, useRef, useCallback } from 'react';
import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, horizontalListSortingStrategy, useSortable, arrayMove } from '@dnd-kit/sortable';
import { CSS as DndCSS } from '@dnd-kit/utilities';
import './TitleBar.css';

// Extract filename from a full path without using Node's path module
function basename(p) {
  return p ? p.split(/[/\\]/).pop() : '';
}

// ── Sortable profile tab (extracted for @dnd-kit) ───────────────────────────

function SortableProfileTab({ profile, isActive, hasLink, isActiveGlob, isGlobal, isRenaming, renameValue, onRenameChange, onRenameCommit, onRenameCancel, onSelect, onDoubleClick, onGearClick, gearOpen, onContextMenu, profileSettings, measureRef }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: profile });
  const style = {
    transform: DndCSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.4 : 1,
  };

  const combinedRef = useCallback((el) => {
    setNodeRef(el);
    if (measureRef) measureRef(el);
  }, [setNodeRef, measureRef]);

  return (
    <div
      ref={combinedRef}
      style={style}
      className="profile-tab-group"
      onContextMenu={onContextMenu}
      {...attributes}
      {...listeners}
    >
      {isRenaming ? (
        <input
          autoFocus
          className="profile-add-input"
          value={renameValue}
          onChange={onRenameChange}
          onKeyDown={e => {
            if (e.key === 'Enter') onRenameCommit();
            if (e.key === 'Escape') onRenameCancel();
          }}
          onBlur={onRenameCommit}
        />
      ) : (
        <button
          className={`profile-tab${isActive ? ' active' : ''}${hasLink ? ' linked' : ''}${isActiveGlob ? ' global-active' : ''}`}
          onClick={onSelect}
          onDoubleClick={onDoubleClick}
          title={isGlobal ? (isActiveGlob ? 'Active global profile — currently the base profile when no app-specific profile is matched. Right-click to change.' : 'Global profile — active when no app-specific profile is in focus. Right-click to set as active base profile.') : undefined}
        >
          {hasLink && <span className="profile-app-icon">🖥</span>}
          {profile}
          {hasLink && <span className="profile-link-dot" title={basename(profileSettings[profile]?.linkedApp)} />}
          {isActiveGlob && <span className="profile-global-dot" title="Active global profile" />}
        </button>
      )}
      {!isRenaming && (
        <button
          className={`profile-gear-btn${gearOpen ? ' open' : ''}`}
          onClick={onGearClick}
          title="Profile settings"
          data-drag="false"
        >
          <svg width="11" height="11" viewBox="0 0 16 16" fill="none">
            <path d="M8 10a2 2 0 1 0 0-4 2 2 0 0 0 0 4Z" stroke="currentColor" strokeWidth="1.5"/>
            <path d="M8 1v1.5M8 13.5V15M1 8h1.5M13.5 8H15M2.93 2.93l1.06 1.06M12.01 12.01l1.06 1.06M2.93 13.07l1.06-1.06M12.01 3.99l1.06-1.06" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
          </svg>
        </button>
      )}
    </div>
  );
}

export default function TitleBar({
  activeProfile,
  profiles,
  onProfileChange,
  onAddProfile,
  onRenameProfile,
  macrosEnabled,
  onToggleMacros,
  profileSettings = {},
  onUpdateProfileSettings,
  theme = 'dark',
  onToggleTheme,
  onOpenSettings,
  settingsOpen = false,
  onReorderProfiles,
  onDuplicateProfile,
  onDeleteProfile,
  activeGlobalProfile = 'Default',
  onSetActiveGlobalProfile,
  activeArea = 'mapping',
  onAreaChange,
}) {
  const [addingProfile, setAddingProfile]   = useState(false);
  const [newProfileName, setNewProfileName] = useState('');
  // Name of the profile whose settings popover is open, plus its anchor position
  const [settingsFor, setSettingsFor]       = useState(null);
  const [popoverPos, setPopoverPos]         = useState({ top: 52, left: 0 });
  // Inline renaming state
  const [renamingProfile, setRenamingProfile] = useState(null);
  const [renameValue, setRenameValue]         = useState('');
  // @dnd-kit drag state
  const [activeDragId, setActiveDragId]     = useState(null);
  // Overflow state
  const [visibleCount, setVisibleCount]     = useState(Infinity);
  const [overflowOpen, setOverflowOpen]     = useState(false);
  const profileTabsRef                      = useRef(null);
  const tabElsRef                           = useRef({});
  const overflowRef                         = useRef(null);
  // Right-click context menu state
  const [contextMenu, setContextMenu]       = useState(null); // { profile, x, y }
  // Delete confirmation state
  const [deleteConfirm, setDeleteConfirm]   = useState(null); // profile name string
  const popoverRef                          = useRef(null);
  const contextMenuRef                      = useRef(null);

  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }));

  const handleMinimize = () => window.electronAPI?.minimize();
  const handleMaximize = () => window.electronAPI?.maximize();
  const handleClose    = () => window.electronAPI?.close();

  const handleAdd = (e) => {
    e.preventDefault();
    if (newProfileName.trim()) {
      onAddProfile(newProfileName.trim());
      setNewProfileName('');
      setAddingProfile(false);
    }
  };

  // Close popover on outside click
  useEffect(() => {
    if (!settingsFor) return;
    function onDown(e) {
      if (popoverRef.current && !popoverRef.current.contains(e.target)) {
        setSettingsFor(null);
      }
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [settingsFor]);

  function openSettings(e, profileName) {
    // Position the popover below the gear button
    const rect = e.currentTarget.getBoundingClientRect();
    setPopoverPos({ top: rect.bottom + 6, left: rect.left });
    setSettingsFor(prev => (prev === profileName ? null : profileName));
  }

  async function handleBrowse(profileName) {
    const filePath = await window.electronAPI?.browseForFile();
    if (filePath) onUpdateProfileSettings(profileName, { linkedApp: filePath });
  }

  function handleClear(profileName) {
    onUpdateProfileSettings(profileName, { linkedApp: null });
  }

  function startRename(profileName) {
    setSettingsFor(null);
    setRenamingProfile(profileName);
    setRenameValue(profileName);
  }

  function commitRename() {
    const trimmed = renameValue.trim();
    if (trimmed && trimmed !== renamingProfile) {
      onRenameProfile?.(renamingProfile, trimmed);
    }
    setRenamingProfile(null);
    setRenameValue('');
  }

  function cancelRename() {
    setRenamingProfile(null);
    setRenameValue('');
  }

  // Close context menu on outside click or Escape
  useEffect(() => {
    if (!contextMenu) return;
    function onDown(e) {
      if (contextMenuRef.current && !contextMenuRef.current.contains(e.target)) {
        setContextMenu(null);
      }
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

  // ── @dnd-kit drag handlers ────────────────────────────────
  function handleDndDragStart(event) {
    setActiveDragId(event.active.id);
  }

  function handleDndDragEnd(event) {
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

  // ── Overflow measurement ─────────────────────────────────
  useEffect(() => {
    const container = profileTabsRef.current;
    if (!container) return;
    const PILL_WIDTH = 80; // reserve space for overflow pill
    const GAP = 2;

    function measure() {
      const containerWidth = container.offsetWidth;
      if (containerWidth === 0) return; // not rendered yet
      const nonDefault = profiles.slice(1);
      let usedWidth = 0;
      let count = 0;
      for (const p of nonDefault) {
        const el = tabElsRef.current[p];
        if (!el || el.offsetWidth === 0) { count++; continue; } // not measured yet — assume fits
        const w = el.offsetWidth + GAP;
        // If this tab would overflow, check if we need pill space
        if (usedWidth + w > containerWidth - (count < nonDefault.length - 1 ? PILL_WIDTH : 0)) break;
        usedWidth += w;
        count++;
      }
      // If all fit, no need for pill reservation
      if (count >= nonDefault.length) setVisibleCount(Infinity);
      else setVisibleCount(count);
    }

    // Defer first measurement so the DOM has rendered tabs
    const raf = requestAnimationFrame(() => requestAnimationFrame(measure));
    const ro = new ResizeObserver(measure);
    ro.observe(container);
    return () => { cancelAnimationFrame(raf); ro.disconnect(); };
  }, [profiles]);

  // ── Close overflow dropdown on outside click ─────────────
  useEffect(() => {
    if (!overflowOpen) return;
    function onDown(e) {
      if (overflowRef.current && !overflowRef.current.contains(e.target)) {
        setOverflowOpen(false);
      }
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [overflowOpen]);

  // ── Context menu handlers ─────────────────────────────────
  function handleContextMenu(e, profileName) {
    e.preventDefault();
    setContextMenu({ profile: profileName, x: e.clientX, y: e.clientY });
  }

  function ctxSetActive() {
    onSetActiveGlobalProfile?.(contextMenu.profile);
    setContextMenu(null);
  }

  function ctxRename() {
    startRename(contextMenu.profile);
    setContextMenu(null);
  }

  function ctxDuplicate() {
    onDuplicateProfile?.(contextMenu.profile);
    setContextMenu(null);
  }

  function ctxDelete() {
    setDeleteConfirm(contextMenu.profile);
    setContextMenu(null);
  }

  function confirmDelete() {
    onDeleteProfile?.(deleteConfirm);
    setDeleteConfirm(null);
  }

  const linked = settingsFor ? profileSettings[settingsFor]?.linkedApp : null;

  return (
    <div className="titlebar" data-drag="true">
      <div className="titlebar-left">
        <div className="app-logo">
<span className="app-name">Trigr</span>
        </div>

        <div className="titlebar-divider" />

        {/* Area tabs — top-level navigation between Mapping and Text Expansion */}
        <div className="area-tabs" data-drag="false">
          <button
            className={`area-tab${activeArea === 'mapping' ? ' active' : ''}`}
            onClick={() => onAreaChange?.('mapping')}
            type="button"
          >
            Key Mapping
          </button>
          <button
            className={`area-tab${activeArea === 'expansions' ? ' active' : ''}`}
            onClick={() => onAreaChange?.('expansions')}
            type="button"
          >
            Text Expansion
          </button>
          <button
            className={`area-tab${activeArea === 'analytics' ? ' active' : ''}`}
            onClick={() => onAreaChange?.('analytics')}
            type="button"
          >
            Analytics
          </button>
        </div>

        {/* Profile tabs — only shown in Mapping area */}
        {/* Phase 3: Text Expansions may eventually have its own profile bar here */}
        {activeArea === 'mapping' && (() => {
          const nonDefault = profiles.slice(1);
          const sortableIds = nonDefault;
          const visibleProfiles = visibleCount === Infinity ? nonDefault : nonDefault.slice(0, visibleCount);
          const overflowProfiles = visibleCount === Infinity ? [] : nonDefault.slice(visibleCount);

          function renderTab(p, opts = {}) {
            const hasLink      = !!profileSettings[p]?.linkedApp;
            const isGlobal     = !hasLink;
            const isActiveGlob = isGlobal && p === activeGlobalProfile;
            const isRenaming   = renamingProfile === p;
            const canCtx       = !isRenaming;
            return (
              <SortableProfileTab
                key={p}
                profile={p}
                isActive={p === activeProfile}
                hasLink={hasLink}
                isActiveGlob={isActiveGlob}
                isGlobal={isGlobal}
                isRenaming={isRenaming}
                renameValue={renameValue}
                onRenameChange={e => setRenameValue(e.target.value)}
                onRenameCommit={commitRename}
                onRenameCancel={cancelRename}
                onSelect={() => { onProfileChange(p); setOverflowOpen(false); }}
                onDoubleClick={() => startRename(p)}
                onGearClick={e => openSettings(e, p)}
                gearOpen={settingsFor === p}
                onContextMenu={canCtx ? e => handleContextMenu(e, p) : undefined}
                profileSettings={profileSettings}
                measureRef={opts.measureRef}
              />
            );
          }

          // Default profile tab (not sortable, always first)
          const defaultHasLink = !!profileSettings['Default']?.linkedApp;
          const defaultIsGlobal = !defaultHasLink;
          const defaultIsActiveGlob = defaultIsGlobal && 'Default' === activeGlobalProfile;
          const defaultCanCtx = defaultIsGlobal && !defaultIsActiveGlob;

          return (
            <>
              <div className="titlebar-divider" />
              <DndContext sensors={sensors} onDragStart={handleDndDragStart} onDragEnd={handleDndDragEnd}>
                <SortableContext items={sortableIds} strategy={horizontalListSortingStrategy}>
                  <div className="profile-tabs" ref={profileTabsRef} data-drag="false">
                    {/* Default profile — always first, not draggable */}
                    <div className="profile-tab-group" onContextMenu={defaultCanCtx ? e => handleContextMenu(e, 'Default') : undefined}>
                      <button
                        className={`profile-tab${'Default' === activeProfile ? ' active' : ''}${defaultIsActiveGlob ? ' global-active' : ''}`}
                        onClick={() => onProfileChange('Default')}
                        title={defaultIsGlobal ? (defaultIsActiveGlob ? 'Active global profile' : 'Global profile — right-click to set as active') : undefined}
                      >
                        Default
                        {defaultIsActiveGlob && <span className="profile-global-dot" title="Active global profile" />}
                      </button>
                    </div>

                    {/* Visible non-Default tabs */}
                    {visibleProfiles.map(p => renderTab(p, {
                      measureRef: el => { if (el) tabElsRef.current[p] = el; },
                    }))}

                    {/* Overflow pill + dropdown */}
                    {overflowProfiles.length > 0 && (
                      <div className="profile-overflow-wrap" ref={overflowRef}>
                        <button
                          className="profile-overflow-pill"
                          onClick={() => setOverflowOpen(o => !o)}
                          type="button"
                        >
                          ▾ {overflowProfiles.length} more
                        </button>
                        {overflowOpen && (
                          <div className="profile-overflow-dropdown">
                            {overflowProfiles.map(p => {
                              const hasLink = !!profileSettings[p]?.linkedApp;
                              const isActive = p === activeProfile;
                              return (
                                <button
                                  key={p}
                                  className={`profile-overflow-item${isActive ? ' active' : ''}`}
                                  onClick={() => { onProfileChange(p); setOverflowOpen(false); }}
                                >
                                  {hasLink && <span className="profile-app-icon">🖥</span>}
                                  {p}
                                </button>
                              );
                            })}
                          </div>
                        )}
                      </div>
                    )}

                    {/* Measure hidden tabs for overflow — render off-screen to get widths */}
                    {overflowProfiles.map(p => (
                      <div key={`measure-${p}`} className="profile-tab-group profile-tab-measure" ref={el => { if (el) tabElsRef.current[p] = el; }}>
                        <button className="profile-tab" tabIndex={-1}>
                          {!!profileSettings[p]?.linkedApp && <span className="profile-app-icon">🖥</span>}
                          {p}
                        </button>
                      </div>
                    ))}

                    {/* Add profile button */}
                    {addingProfile ? (
                      <form onSubmit={handleAdd} className="profile-add-form" data-drag="false">
                        <input
                          autoFocus
                          value={newProfileName}
                          onChange={e => setNewProfileName(e.target.value)}
                          placeholder="Profile name..."
                          className="profile-add-input"
                          onBlur={() => { setAddingProfile(false); setNewProfileName(''); }}
                          onKeyDown={e => e.key === 'Escape' && setAddingProfile(false)}
                        />
                      </form>
                    ) : (
                      <button className="profile-tab add-tab" onClick={() => setAddingProfile(true)} data-drag="false">
                        <span>+</span>
                      </button>
                    )}
                  </div>
                </SortableContext>

                <DragOverlay>
                  {activeDragId ? (
                    <div className="profile-tab-group profile-tab-drag-ghost">
                      <button className="profile-tab active">{activeDragId}</button>
                    </div>
                  ) : null}
                </DragOverlay>
              </DndContext>
            </>
          );
        })()}
      </div>

      <div className="titlebar-right" data-drag="false">
        <button
          className={`macro-toggle ${macrosEnabled ? 'enabled' : 'disabled'}`}
          onClick={onToggleMacros}
          title={macrosEnabled ? 'Macros Active — Click to Disable' : 'Macros Paused — Click to Enable'}
        >
          <span className="toggle-dot" />
          {macrosEnabled ? 'ACTIVE' : 'PAUSED'}
        </button>

        <button
          className="theme-toggle-btn"
          onClick={onToggleTheme}
          title={theme === 'dark' ? 'Switch to Light Mode' : 'Switch to Dark Mode'}
          data-drag="false"
        >
          {theme === 'dark' ? (
            /* Sun — click to go light */
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none">
              <circle cx="12" cy="12" r="5" stroke="currentColor" strokeWidth="2"/>
              <line x1="12" y1="2"  x2="12" y2="5"  stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="12" y1="19" x2="12" y2="22" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="2"  y1="12" x2="5"  y2="12" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="19" y1="12" x2="22" y2="12" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="4.22"  y1="4.22"  x2="6.34"  y2="6.34"  stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="17.66" y1="17.66" x2="19.78" y2="19.78" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="19.78" y1="4.22"  x2="17.66" y2="6.34"  stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="6.34"  y1="17.66" x2="4.22"  y2="19.78" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
            </svg>
          ) : (
            /* Moon — click to go dark */
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none">
              <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/>
            </svg>
          )}
        </button>

        <button
          className={`tb-settings-btn${settingsOpen ? ' active' : ''}`}
          onClick={onOpenSettings}
          title="Settings"
          data-drag="false"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="3"/>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>
          </svg>
        </button>

        <div className="window-controls">
          <button className="wc-btn minimize" onClick={handleMinimize}>
            <svg width="10" height="2" viewBox="0 0 10 2"><rect width="10" height="2" rx="1" fill="currentColor"/></svg>
          </button>
          <button className="wc-btn maximize" onClick={handleMaximize}>
            <svg width="10" height="10" viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="1.5" stroke="currentColor" strokeWidth="1.5" fill="none"/></svg>
          </button>
          <button className="wc-btn close" onClick={handleClose}>
            <svg width="10" height="10" viewBox="0 0 10 10">
              <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
              <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
            </svg>
          </button>
        </div>
      </div>

      {/* Right-click context menu for profile tabs */}
      {contextMenu && (() => {
        const ctxIsDefault = contextMenu.profile === 'Default';
        const ctxHasLink   = !!profileSettings[contextMenu.profile]?.linkedApp;
        const ctxIsGlobal  = !ctxHasLink;
        const ctxIsActive  = contextMenu.profile === activeGlobalProfile;
        return (
          <div
            ref={contextMenuRef}
            className="profile-context-menu"
            style={{ top: contextMenu.y, left: contextMenu.x }}
            data-drag="false"
          >
            {ctxIsGlobal && !ctxIsActive && (
              <>
                <button className="pcm-item pcm-set-active" onClick={ctxSetActive}>
                  ◉ Set as Active Profile
                </button>
                <div className="pcm-divider" />
              </>
            )}
            {!ctxIsDefault && <button className="pcm-item" onClick={ctxRename}>Rename</button>}
            {!ctxIsDefault && <button className="pcm-item" onClick={ctxDuplicate}>Duplicate</button>}
            {!ctxIsDefault && <div className="pcm-divider" />}
            {!ctxIsDefault && <button className="pcm-item pcm-delete" onClick={ctxDelete}>Delete</button>}
          </div>
        );
      })()}

      {/* Delete confirmation dialog */}
      {deleteConfirm && (
        <div className="profile-delete-overlay" data-drag="false">
          <div className="profile-delete-dialog">
            <div className="pdd-title">Delete Profile</div>
            <p className="pdd-body">
              Delete <strong>{deleteConfirm}</strong>? All hotkeys in this profile will be permanently removed.
            </p>
            <div className="pdd-actions">
              <button className="pdd-cancel" onClick={() => setDeleteConfirm(null)}>Cancel</button>
              <button className="pdd-confirm" onClick={confirmDelete}>Delete</button>
            </div>
          </div>
        </div>
      )}

      {/* Profile settings popover — uses position:fixed to escape titlebar overflow */}
      {settingsFor && (
        <div
          ref={popoverRef}
          className="profile-settings-popover"
          style={{ top: popoverPos.top, left: popoverPos.left }}
          data-drag="false"
        >
          <div className="psp-title">{settingsFor}</div>
          <div className="psp-row">
            <span className="psp-label">Linked App</span>
            <span className="psp-value" title={linked || undefined}>
              {linked ? basename(linked) : <em className="psp-none">Not linked</em>}
            </span>
          </div>
          <div className="psp-actions">
            <button className="psp-browse-btn" onClick={() => handleBrowse(settingsFor)}>
              Browse…
            </button>
            {linked && (
              <button className="psp-clear-btn" onClick={() => handleClear(settingsFor)}>
                Clear
              </button>
            )}
          </div>
          <p className="psp-hint">
            When this app is focused, Trigr automatically switches to this profile.
          </p>
        </div>
      )}
    </div>
  );
}
