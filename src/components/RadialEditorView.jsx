import React, { useState, useRef, useCallback, useEffect } from 'react';
import RadialWheel, { CX, CY, MAX_SLOTS, polarToXY } from './RadialWheel';
import IconPicker from './IconPicker';
import { friendlyKeyName } from './keyboardLayout';
import './RadialEditorView.css';

const EDITOR_INNER_R = 60;  // inner edge close to centre, minimal dead space
const EDITOR_OUTER_R = 130; // outer edge — keeps wedge height at 70 (same as before)


export default function RadialEditorView({
  radialMenuHotkey      = null,
  onSetRadialMenuHotkey,
  onClearRadialMenuHotkey,
  radialMenuItems       = [],
  onAddRadialMenuItem,
  onRemoveRadialMenuItem,
  onReorderRadialMenuItems,
  onAddRadialMenuFolder,
  onAddChildToFolder,
  onRemoveChildFromFolder,
  onReorderFolderChildren,
  onRenameFolder,
  onRenameRadialMenuItem,
  onRenameChildInFolder,
  onSwapRadialMenuItems,
  onCreateRadialAction,
  selectedRadialSegment = null,
  onSelectRadialSegment,
  onSelectRadialChild,
  onSetRadialMenuItemIcon,
  onSetRadialChildIcon,
  assignments           = {},
  dropTargetOuterIndex  = -1,
  expandedFolder        = null,
  onExpandedFolderChange,
  dropTargetIndex       = -1,
  rejectIndex           = -1,
  wheelRef,
  usedKeys,
}) {
  const [capturingKey, setCapturingKey] = useState(false);
  const [capturedKey, setCapturedKey]   = useState(null);
  const setExpandedFolder = onExpandedFolderChange;
  const [hoveredIndex, setHoveredIndex] = useState(-1);
  const [hoveredOuter, setHoveredOuter] = useState(-1);

  // ── Right-click context menu ──────────────────────────────────────────
  const [ctxMenu, setCtxMenu] = useState(null); // { type, item, index, folderId?, child?, childIndex?, x, y }
  const ctxRef = useRef(null);

  // ── Icon picker panel (fixed position, outside SVG) ─────────────────
  const [iconPicker, setIconPicker] = useState(null); // { itemId, folderId?, childId?, currentIcon, currentColor, x, y }
  const iconPickerRef = useRef(null);

  useEffect(() => {
    if (!ctxMenu) return;
    function onDown(e) {
      if (ctxRef.current && !ctxRef.current.contains(e.target)) setCtxMenu(null);
    }
    function onKey(e) { if (e.key === 'Escape') setCtxMenu(null); }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => { document.removeEventListener('mousedown', onDown); document.removeEventListener('keydown', onKey); };
  }, [ctxMenu]);

  useEffect(() => {
    if (!iconPicker) return;
    function onDown(e) {
      if (iconPickerRef.current && !iconPickerRef.current.contains(e.target)) setIconPicker(null);
    }
    function onKey(e) { if (e.key === 'Escape') setIconPicker(null); }
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => { document.removeEventListener('mousedown', onDown); document.removeEventListener('keydown', onKey); };
  }, [iconPicker]);

  // ── Popover for interactive forms (rename input, picker) ──────────────
  const [popover, setPopover] = useState(null);

  // ── Wedge drag-to-swap state ──────────────────────────────────────────
  const [wedgeDragFrom, setWedgeDragFrom] = useState(-1);
  const [wedgeDragTo, setWedgeDragTo] = useState(-1);
  const wedgeDragRef = useRef(null); // { fromIndex, startX, startY, active }

  const localHitTest = useCallback((clientX, clientY) => {
    if (!wheelRef?.current) return -1;
    const rect = wheelRef.current.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return -1;
    const svgX = ((clientX - rect.left) / rect.width) * 620;
    const svgY = ((clientY - rect.top) / rect.height) * 620;
    const dx = svgX - CX, dy = svgY - CY;
    const dist = Math.sqrt(dx * dx + dy * dy);
    if (dist < EDITOR_INNER_R || dist > EDITOR_OUTER_R) return -1;
    let angle = Math.atan2(dy, dx) * (180 / Math.PI);
    angle = ((angle + 90) % 360 + 360) % 360;
    return Math.floor(angle / (360 / MAX_SLOTS));
  }, [wheelRef]);

  const handleWedgePointerDown = useCallback((item, index, e) => {
    if (e.button !== 0) return;
    wedgeDragRef.current = { fromIndex: index, startX: e.clientX, startY: e.clientY, active: false };

    const onMove = (me) => {
      const ref = wedgeDragRef.current;
      if (!ref) return;
      if (!ref.active) {
        const dx = me.clientX - ref.startX, dy = me.clientY - ref.startY;
        if (Math.sqrt(dx * dx + dy * dy) < 5) return;
        ref.active = true;
        setWedgeDragFrom(ref.fromIndex);
      }
      setWedgeDragTo(localHitTest(me.clientX, me.clientY));
    };

    const onUp = () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      const ref = wedgeDragRef.current;
      wedgeDragRef.current = null;
      if (ref?.active) {
        const to = localHitTest(arguments[0]?.clientX ?? 0, arguments[0]?.clientY ?? 0);
        // Use the last known target from state — React may not have flushed yet,
        // so read from the ref-tracked value via a microtask.
      }
      setWedgeDragFrom(-1);
      setWedgeDragTo(-1);
    };

    // Use a single pointerup that captures the final position
    const onUpFinal = (ue) => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUpFinal);
      const ref = wedgeDragRef.current;
      wedgeDragRef.current = null;
      if (ref?.active) {
        const target = localHitTest(ue.clientX, ue.clientY);
        if (target >= 0 && target !== ref.fromIndex) {
          onSwapRadialMenuItems?.(ref.fromIndex, target);
        }
      }
      setWedgeDragFrom(-1);
      setWedgeDragTo(-1);
    };

    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUpFinal);
  }, [localHitTest, onSwapRadialMenuItems]);

  // Effective drop target: combine library drops (parent prop) with wedge drag
  const effectiveDropTarget = wedgeDragFrom >= 0 ? wedgeDragTo : dropTargetIndex;

  // ── Context menu handlers ─────────────────────────────────────────────
  const handleItemContextMenu = useCallback((item, index, e) => {
    setCtxMenu({ type: item.type === 'folder' ? 'folder' : 'filled', item, index, x: e.clientX, y: e.clientY });
    setPopover(null);
  }, []);

  const handleChildContextMenu = useCallback((folderId, child, childIndex, e) => {
    setCtxMenu({ type: 'childFilled', folderId, child, childIndex, x: e.clientX, y: e.clientY });
    setPopover(null);
  }, []);

  // Open rename popover (positioned at wedge centre)
  function openRenamePopover(type, data) {
    setCtxMenu(null);
    const midR = (EDITOR_INNER_R + EDITOR_OUTER_R) / 2;
    const step = 360 / MAX_SLOTS;
    const bisector = step * (data.index ?? 0) - 90;
    const [px, py] = polarToXY(CX, CY, midR, bisector);
    setPopover({ ...data, type, subType: 'rename', x: px, y: py });
  }

  return (
    <div className="rev-panel">
      {/* Hotkey capture strip */}
      <div className="rev-header">
        <span className="rev-title">Radial Menu</span>
        <div className="rev-hotkey-ctrl">
          {capturingKey ? (
            <div
              className="rmp-capture"
              tabIndex={0}
              autoFocus
              onBlur={() => { setCapturingKey(false); setCapturedKey(null); }}
              onKeyUp={e => { e.preventDefault(); e.stopPropagation(); }}
              onKeyDown={e => {
                e.preventDefault();
                e.stopPropagation();
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
                setCapturedKey({ combo, label });
              }}
            >
              {capturedKey ? (
                <span className="rmp-captured">{capturedKey.label}</span>
              ) : (
                <span className="rmp-waiting">Press combo...</span>
              )}
              {capturedKey && (
                <button className="rmp-save-btn" type="button" onMouseDown={e => e.preventDefault()} onClick={() => {
                  onSetRadialMenuHotkey?.(capturedKey.combo);
                  setCapturingKey(false);
                  setCapturedKey(null);
                }}>Save</button>
              )}
              <button className="rmp-cancel-btn" type="button" onMouseDown={e => e.preventDefault()} onClick={() => { setCapturingKey(false); setCapturedKey(null); }}>&#10005;</button>
            </div>
          ) : radialMenuHotkey ? (
            <>
              <span className="rmp-hotkey-badge">
                {radialMenuHotkey.split('+').map((p, i, arr) => (
                  <React.Fragment key={i}>
                    <kbd className="rmp-kbd">{friendlyKeyName(p)}</kbd>
                    {i < arr.length - 1 && <span className="rmp-plus">+</span>}
                  </React.Fragment>
                ))}
              </span>
              <button className="rmp-action-btn" type="button" onClick={() => setCapturingKey(true)}>Change</button>
              <button className="rmp-action-btn rmp-action-danger" type="button" onClick={() => onClearRadialMenuHotkey?.()} title="Remove radial menu hotkey">Remove</button>
            </>
          ) : (
            <button className="rmp-action-btn" type="button" onClick={() => setCapturingKey(true)}>Set hotkey</button>
          )}
        </div>
      </div>

      {/* Stats bar */}
      {radialMenuHotkey && (
        <div className="rev-stats">
          <span className="rev-stat">
            <span className="rev-stat-value">{radialMenuItems.filter(Boolean).length}</span>
            <span className="rev-stat-label">of {MAX_SLOTS} segments</span>
          </span>
          <span className="rev-stat-sep" />
          <span className="rev-stat-hint">
            {selectedRadialSegment != null
              ? 'Edit action in the panel on the right'
              : expandedFolder
                ? 'Click a child segment to edit, or click background to collapse'
                : 'Click a segment to edit \u00b7 Drag to reorder \u00b7 Right-click for options'}
          </span>
        </div>
      )}

      {/* Wheel editor area */}
      {!radialMenuHotkey ? (
        <div className="rev-empty-state">
          <span className="rmp-empty-icon">{'\u25ce'}</span>
          <p>Set a hotkey above to enable the radial menu.</p>
        </div>
      ) : (
        <div className="rev-wheel-zone">
          <div className="rev-editor" ref={wheelRef} onClick={() => { setPopover(null); setCtxMenu(null); }}>
            <RadialWheel
              mode="editor"
              externalDnd={true}
              innerRadius={EDITOR_INNER_R}
              outerRadius={EDITOR_OUTER_R}
              items={radialMenuItems}
              expandedFolder={expandedFolder}
              hoveredIndex={hoveredIndex}
              hoveredOuterIndex={hoveredOuter}
              dropTargetIndex={effectiveDropTarget}
              dropTargetOuterIndex={dropTargetOuterIndex}
              dragFromIndex={wedgeDragFrom}
              selectedIndex={selectedRadialSegment != null ? selectedRadialSegment : -1}
              onHoverInner={setHoveredIndex}
              onHoverOuter={setHoveredOuter}
              onItemClick={(item, index) => {
                if (item.type === 'folder') {
                  setExpandedFolder(prev => prev === item.id ? null : item.id);
                  setPopover(null);
                  setCtxMenu(null);
                } else {
                  // Select segment for editing in MacroPanel
                  onSelectRadialSegment?.(index);
                  setPopover(null);
                  setCtxMenu(null);
                }
              }}
              onItemContextMenu={handleItemContextMenu}
              onEmptyWedgeContextMenu={(index, e) => {
                setCtxMenu({ type: 'empty', index, x: e.clientX, y: e.clientY });
                setPopover(null);
              }}
              onChildContextMenu={handleChildContextMenu}
              onWedgePointerDown={handleWedgePointerDown}
              onEmptyWedgeClick={(index) => {
                onSelectRadialSegment?.(index);
                setPopover(null);
                setCtxMenu(null);
              }}
              onFolderChildClick={(folderId, child, childIndex) => {
                // Left-click on filled child: open in MacroPanel for editing
                onSelectRadialChild?.(folderId, childIndex);
                setPopover(null);
                setCtxMenu(null);
              }}
              onEmptyChildWedgeClick={(folderId, childIndex) => {
                // Left-click on empty child: open MacroPanel for new action
                onSelectRadialChild?.(folderId, childIndex);
                setPopover(null);
                setCtxMenu(null);
              }}
              onBackgroundClick={() => {
                if (popover) {
                  setPopover(null);
                } else if (ctxMenu) {
                  setCtxMenu(null);
                } else if (expandedFolder) {
                  setExpandedFolder(null);
                }
              }}
              onReorder={onReorderRadialMenuItems}
              onReorderChildren={onReorderFolderChildren}
            />


            {/* ── Popover: interactive forms (rename input, picker) ── */}
            {popover && (
              <>
                <div className="rmp-backdrop" onClick={(e) => { e.stopPropagation(); setPopover(null); }} />
                <div
                  className="rmp-popover"
                  style={popover.x != null ? { left: `${popover.x}px`, top: `${popover.y}px` } : { left: '50%', top: '50%' }}
                  onClick={e => e.stopPropagation()}
                >
                  {/* Rename input — regular item */}
                  {popover.type === 'filled' && popover.subType === 'rename' && (
                    <div className="rmp-popover-folder-form">
                      <input
                        className="form-input rmp-popover-input"
                        value={popover.name || ''}
                        placeholder="Segment name"
                        onChange={e => setPopover(p => ({ ...p, name: e.target.value }))}
                        onKeyDown={e => {
                          e.stopPropagation();
                          if (e.key === 'Escape') setPopover(null);
                          if (e.key === 'Enter') { onRenameRadialMenuItem?.(popover.item.id, (popover.name || '').trim()); setPopover(null); }
                        }}
                        autoFocus
                      />
                      <div className="rmp-popover-btns">
                        <button type="button" className="rmp-popover-btn" onClick={() => { onRenameRadialMenuItem?.(popover.item.id, (popover.name || '').trim()); setPopover(null); }}>Save</button>
                        <button type="button" className="rmp-popover-btn" onClick={() => setPopover(null)}>Cancel</button>
                      </div>
                    </div>
                  )}

                  {/* Rename input — child item */}
                  {popover.type === 'childFilled' && popover.subType === 'rename' && (
                    <div className="rmp-popover-folder-form">
                      <input
                        className="form-input rmp-popover-input"
                        value={popover.name || ''}
                        placeholder="Segment name"
                        onChange={e => setPopover(p => ({ ...p, name: e.target.value }))}
                        onKeyDown={e => {
                          e.stopPropagation();
                          if (e.key === 'Escape') setPopover(null);
                          if (e.key === 'Enter') { onRenameChildInFolder?.(popover.folderId, popover.child.id, (popover.name || '').trim()); setPopover(null); }
                        }}
                        autoFocus
                      />
                      <div className="rmp-popover-btns">
                        <button type="button" className="rmp-popover-btn" onClick={() => { onRenameChildInFolder?.(popover.folderId, popover.child.id, (popover.name || '').trim()); setPopover(null); }}>Save</button>
                        <button type="button" className="rmp-popover-btn" onClick={() => setPopover(null)}>Cancel</button>
                      </div>
                    </div>
                  )}

                  {/* Rename input — folder */}
                  {popover.type === 'folder' && popover.subType === 'rename' && (
                    <div className="rmp-popover-folder-form">
                      <input
                        className="form-input rmp-popover-input"
                        value={popover.name || ''}
                        onChange={e => setPopover(p => ({ ...p, name: e.target.value }))}
                        onKeyDown={e => {
                          e.stopPropagation();
                          if (e.key === 'Escape') setPopover(null);
                          if (e.key === 'Enter' && popover.name?.trim()) { onRenameFolder?.(popover.item.id, popover.name.trim()); setPopover(null); }
                        }}
                        autoFocus
                      />
                      <div className="rmp-popover-btns">
                        <button type="button" className="rmp-popover-btn" onClick={() => { if (popover.name?.trim()) { onRenameFolder?.(popover.item.id, popover.name.trim()); setPopover(null); } }}>Save</button>
                        <button type="button" className="rmp-popover-btn" onClick={() => setPopover(null)}>Cancel</button>
                      </div>
                    </div>
                  )}

                  {/* Folder: confirm remove */}
                  {popover.type === 'folder' && popover.subType === 'confirmRemove' && (
                    <div className="rmp-popover-confirm">
                      <p className="rmp-popover-confirm-text">Remove folder and {popover.item.children?.length || 0} children?</p>
                      <div className="rmp-popover-btns">
                        <button type="button" className="rmp-popover-btn rmp-popover-danger" onClick={() => { onRemoveRadialMenuItem?.(popover.item.id); setPopover(null); setExpandedFolder(null); }}>Remove</button>
                        <button type="button" className="rmp-popover-btn" onClick={() => setPopover(null)}>Cancel</button>
                      </div>
                    </div>
                  )}

                  {/* Folder: add child picker */}
                  {popover.type === 'folder' && popover.subType === 'addChild' && (() => {
                    const q = (popover.search || '').toLowerCase();
                    const picks = [];
                    for (const [key, val] of Object.entries(assignments)) {
                      if (usedKeys.has(key)) continue;
                      if (key.startsWith('GLOBAL::AUTOCORRECT::')) continue;
                      const lbl = val.label || key.split('::').pop() || '';
                      if (q && !lbl.toLowerCase().includes(q) && !key.toLowerCase().includes(q)) continue;
                      picks.push({ key, label: lbl });
                    }
                    picks.sort((a, b) => a.label.localeCompare(b.label));
                    return (
                      <div className="rmp-popover-picker">
                        <input className="form-input rmp-popover-search" placeholder="Search..." value={popover.search || ''}
                          onChange={e => setPopover(p => ({ ...p, search: e.target.value }))}
                          onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') setPopover(null); }} autoFocus />
                        <div className="rmp-popover-list">
                          {picks.length === 0 && <div className="rmp-popover-empty">No matching items</div>}
                          {picks.slice(0, 40).map(p => (
                            <button key={p.key} type="button" className="rmp-popover-pick" onClick={() => { onAddChildToFolder?.(popover.item.id, p.key, null); setPopover(null); }}>{p.label}</button>
                          ))}
                        </div>
                      </div>
                    );
                  })()}

                </div>
              </>
            )}
          </div>
        </div>
      )}

      {/* Legend footer */}
      {radialMenuHotkey && (
        <div className="rev-legend">
          <div className="rev-legend-item">
            <span className="rev-legend-dot rev-legend-dot--empty" />
            <span>Empty</span>
          </div>
          <div className="rev-legend-item">
            <span className="rev-legend-dot rev-legend-dot--assigned" />
            <span>Assigned</span>
          </div>
          <div className="rev-legend-item">
            <span className="rev-legend-dot rev-legend-dot--selected" />
            <span>Selected</span>
          </div>
          <div className="rev-legend-item">
            <span className="rev-legend-dot rev-legend-dot--folder" />
            <span>Folder</span>
          </div>
        </div>
      )}

      {/* ── Right-click context menu (fixed position, outside SVG) ── */}
      {ctxMenu && (
        <div ref={ctxRef} className="assign-ctx-menu" style={{ top: ctxMenu.y, left: ctxMenu.x }}>
          {/* Regular filled item */}
          {ctxMenu.type === 'filled' && (
            <>
              <button className="assign-ctx-item" type="button" onClick={() => {
                openRenamePopover('filled', { item: ctxMenu.item, index: ctxMenu.index, name: ctxMenu.item.label || '' });
              }}>Rename</button>
              <button className="assign-ctx-item" type="button" onClick={() => {
                setIconPicker({ itemId: ctxMenu.item.id, currentIcon: ctxMenu.item.icon || '', currentColor: ctxMenu.item.iconColor || '', x: ctxMenu.x, y: ctxMenu.y });
                setCtxMenu(null);
              }}>Change icon</button>
              <div className="assign-ctx-divider" />
              <button className="assign-ctx-item assign-ctx-danger" type="button" onClick={() => {
                onRemoveRadialMenuItem?.(ctxMenu.item.id);
                setCtxMenu(null);
              }}>Remove</button>
            </>
          )}

          {/* Folder item */}
          {ctxMenu.type === 'folder' && (
            <>
              <button className="assign-ctx-item" type="button" onClick={() => {
                setCtxMenu(null);
                const midR = (EDITOR_INNER_R + EDITOR_OUTER_R) / 2;
                const step = 360 / MAX_SLOTS;
                const bisector = step * ctxMenu.index - 90;
                const [px, py] = polarToXY(CX, CY, midR, bisector);
                setPopover({ type: 'folder', subType: 'addChild', item: ctxMenu.item, index: ctxMenu.index, search: '', x: px, y: py });
              }}>Add child</button>
              <button className="assign-ctx-item" type="button" onClick={() => {
                openRenamePopover('folder', { item: ctxMenu.item, index: ctxMenu.index, name: ctxMenu.item.label || '' });
              }}>Rename</button>
              <button className="assign-ctx-item" type="button" onClick={() => {
                setIconPicker({ itemId: ctxMenu.item.id, currentIcon: ctxMenu.item.icon || '', currentColor: ctxMenu.item.iconColor || '', x: ctxMenu.x, y: ctxMenu.y });
                setCtxMenu(null);
              }}>Change icon</button>
              <div className="assign-ctx-divider" />
              <button className="assign-ctx-item assign-ctx-danger" type="button" onClick={() => {
                setCtxMenu(null);
                const midR = (EDITOR_INNER_R + EDITOR_OUTER_R) / 2;
                const step = 360 / MAX_SLOTS;
                const bisector = step * ctxMenu.index - 90;
                const [px, py] = polarToXY(CX, CY, midR, bisector);
                setPopover({ type: 'folder', subType: 'confirmRemove', item: ctxMenu.item, x: px, y: py });
              }}>Remove</button>
            </>
          )}

          {/* Folder child item */}
          {ctxMenu.type === 'childFilled' && (
            <>
              <button className="assign-ctx-item" type="button" onClick={() => {
                setCtxMenu(null);
                setPopover({ type: 'childFilled', subType: 'rename', folderId: ctxMenu.folderId, child: ctxMenu.child, childIndex: ctxMenu.childIndex, name: ctxMenu.child.label || '', x: null, y: null });
              }}>Rename</button>
              <button className="assign-ctx-item" type="button" onClick={() => {
                setIconPicker({ folderId: ctxMenu.folderId, childId: ctxMenu.child.id, currentIcon: ctxMenu.child.icon || '', currentColor: ctxMenu.child.iconColor || '', x: ctxMenu.x, y: ctxMenu.y });
                setCtxMenu(null);
              }}>Change icon</button>
              <div className="assign-ctx-divider" />
              <button className="assign-ctx-item assign-ctx-danger" type="button" onClick={() => {
                onRemoveChildFromFolder?.(ctxMenu.folderId, ctxMenu.child.id);
                setCtxMenu(null);
              }}>Remove</button>
            </>
          )}

          {/* Empty wedge */}
          {ctxMenu.type === 'empty' && (
            <button className="assign-ctx-item" type="button" onClick={() => {
              onAddRadialMenuFolder?.('New folder', ctxMenu.index);
              setCtxMenu(null);
            }}>Add folder</button>
          )}
        </div>
      )}
      {/* ── Icon picker panel (fixed position) ── */}
      {iconPicker && (
        <div ref={iconPickerRef} className="rev-icon-picker-panel" style={{ top: iconPicker.y, left: iconPicker.x }}>
          {/* Colour picker row */}
          <div className="rev-color-row">
            <span className="rev-color-label">Colour</span>
            {['', '#64b4ff', '#c864ff', '#50c878', '#ffc832', '#ff783c', '#40c8a0', '#ff6b6b', '#f0ede8', '#e8a020'].map(c => (
              <button
                key={c}
                className={`rev-color-swatch${(iconPicker.currentColor || '') === c ? ' active' : ''}`}
                style={c ? { background: c } : {}}
                title={c || 'Default (type colour)'}
                type="button"
                onClick={() => {
                  if (iconPicker.childId) {
                    onSetRadialChildIcon?.(iconPicker.folderId, iconPicker.childId, undefined, c);
                  } else {
                    onSetRadialMenuItemIcon?.(iconPicker.itemId, undefined, c);
                  }
                  setIconPicker(p => ({ ...p, currentColor: c }));
                }}
              >
                {!c && <span className="rev-color-swatch-auto">A</span>}
              </button>
            ))}
          </div>
          {/* Icon grid */}
          <IconPicker
            currentIcon={iconPicker.currentIcon}
            onSelect={(iconName) => {
              if (iconPicker.childId) {
                onSetRadialChildIcon?.(iconPicker.folderId, iconPicker.childId, iconName, undefined);
              } else {
                onSetRadialMenuItemIcon?.(iconPicker.itemId, iconName, undefined);
              }
              setIconPicker(null);
            }}
            onClose={() => setIconPicker(null)}
          />
        </div>
      )}
    </div>
  );
}
