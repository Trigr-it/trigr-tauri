import React, { useState, useRef, useCallback, useEffect, useLayoutEffect } from 'react';
import { Info } from 'lucide-react';
import RadialWheel, { CX, CY, MAX_SLOTS, OUTER_INNER_R, OUTER_OUTER_R, polarToXY } from './RadialWheel';
import { friendlyKeyName } from './keyboardLayout';
import './RadialEditorView.css';
import { SearchBar } from './SearchBar';

// Lazy — IconPicker drags in the full lucide-react + simple-icons libraries
// (~5.9MB of JS). Loading it on first picker open keeps that out of the main
// window's startup bundle. See iconUtils.jsx.
const IconPicker = React.lazy(() => import('./IconPicker'));

// Use the same radii as the live overlay (INNER_R=80, OUTER_R=130) for WYSIWYG.
// The editor scales the wheel up via CSS to fill more space.
const EDITOR_INNER_R = 55;
const EDITOR_OUTER_R = 105;


export default function RadialEditorView({
  radialMenuHotkey      = null,
  onSetRadialMenuHotkey,
  onClearRadialMenuHotkey,
  radialHoldToSelect    = false,
  onSetRadialHoldToSelect,
  radialMenuItems       = [],
  onAddRadialMenuItem,
  onRemoveRadialMenuItem,
  onReorderRadialMenuItems,
  onAddRadialMenuFolder,
  onAddChildToFolder,
  onRemoveChildFromFolder,
  onMoveItemToFolder,
  onMoveChildToMain,
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
  profiles              = [],
  activeProfile         = '',
  onCopyRadialSegmentToProfile,
  onForceOverwriteRadialSegment,
  hiddenTips            = [],
  onHideTip,
}) {
  const [capturingKey, setCapturingKey] = useState(false);
  const [capturedKey, setCapturedKey]   = useState(null);
  const [radialConflict, setRadialConflict] = useState(null);
  const setExpandedFolder = onExpandedFolderChange;
  const [hoveredIndex, setHoveredIndex] = useState(-1);
  const [hoveredOuter, setHoveredOuter] = useState(-1);

  // ── Right-click context menu ──────────────────────────────────────────
  const [ctxMenu, setCtxMenu] = useState(null); // { type, item, index, folderId?, child?, childIndex?, x, y }
  const ctxRef = useRef(null);
  // Tracks which Copy-to submenu is hovered. Replaces the CSS-only :hover
  // gate so a layout effect can flip the submenu when it would clip.
  const [hoveredCopySub, setHoveredCopySub] = useState(false);
  const copySubmenuRef = useRef(null);

  // ── Copy-to-profile overwrite confirmation ───────────────────────────
  const [copyConfirm, setCopyConfirm] = useState(null); // { targetProfile, index, existingLabel }
  const otherProfiles = profiles.filter(p => p !== activeProfile);

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

  // Clamp both popovers inside the viewport — both inherit raw clientX/clientY
  // from the original right-click, so they overflow when opened near an edge.
  useLayoutEffect(() => {
    if (!ctxMenu || !ctxRef.current) return;
    const el = ctxRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
    // Reset submenu hover so a stale state doesn't render off-screen when the
    // menu reopens in a different location.
    setHoveredCopySub(false);
  }, [ctxMenu]);

  // Flip the Copy-to submenu — shift up if bottom would clip, swap to the left
  // side if right would clip. Mirrors the macro step-type submenu fix.
  useLayoutEffect(() => {
    if (!ctxMenu || !hoveredCopySub || !copySubmenuRef.current) return;
    const sub = copySubmenuRef.current;
    sub.style.top = '';
    sub.style.left = '';
    sub.style.right = '';
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
  }, [hoveredCopySub, ctxMenu]);

  useLayoutEffect(() => {
    if (!iconPicker || !iconPickerRef.current) return;
    const el = iconPickerRef.current;
    const rect = el.getBoundingClientRect();
    const margin = 8;
    if (rect.right > window.innerWidth - margin) {
      el.style.left = `${Math.max(margin, window.innerWidth - rect.width - margin)}px`;
    }
    if (rect.bottom > window.innerHeight - margin) {
      el.style.top = `${Math.max(margin, window.innerHeight - rect.height - margin)}px`;
    }
  }, [iconPicker]);

  // ── Popover for interactive forms (rename input, picker) ──────────────
  const [popover, setPopover] = useState(null);

  // ── Wedge drag-to-swap state ──────────────────────────────────────────
  const [wedgeDragFrom, setWedgeDragFrom] = useState(-1);
  const [wedgeDragTo, setWedgeDragTo] = useState(-1);
  const [wedgeDragPos, setWedgeDragPos] = useState(null); // { x, y } for ghost
  const wedgeDragRef = useRef(null); // { fromIndex, startX, startY, active }
  // Keep a live ref to items so the drag callback never reads stale data
  const itemsRef = useRef(radialMenuItems);
  itemsRef.current = radialMenuItems;

  // Ref for expandedFolder so hit test always reads latest value
  const expandedFolderRef = useRef(expandedFolder);
  expandedFolderRef.current = expandedFolder;

  // Hit test returning { ring: 'inner'|'outer', index } or null
  const localHitTestFull = useCallback((clientX, clientY) => {
    if (!wheelRef?.current) return null;
    const rect = wheelRef.current.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const svgX = ((clientX - rect.left) / rect.width) * 420;
    const svgY = ((clientY - rect.top) / rect.height) * 420;
    const dx = svgX - CX, dy = svgY - CY;
    const dist = Math.sqrt(dx * dx + dy * dy);
    const step = 360 / MAX_SLOTS;
    let angle = Math.atan2(dy, dx) * (180 / Math.PI);
    angle = ((angle + 90 + step / 2) % 360 + 360) % 360;
    const idx = Math.floor(angle / step);
    if (dist >= EDITOR_INNER_R && dist <= EDITOR_OUTER_R) return { ring: 'inner', index: idx };

    // Outer ring: compute child index using same geometry as RadialWheel outerWedges
    if (dist >= OUTER_INNER_R && dist <= OUTER_OUTER_R && expandedFolderRef.current) {
      const items = itemsRef.current;
      const folderIdx = items.findIndex(i => i?.id === expandedFolderRef.current);
      if (folderIdx < 0) return null;
      const folder = items[folderIdx];
      if (!folder?.children) return null;
      const childCount = folder.children.length;
      const minArc = 22;
      const parentArc = step;
      const parentBisector = step * folderIdx - 90;
      const assignedCount = Math.max(childCount, 1);
      const assignedArc = Math.min(Math.max(parentArc, assignedCount * minArc), 160);
      const childWedge = assignedArc / assignedCount;
      const totalSlots = childCount + 1; // children + empty slot
      const totalArc = assignedArc + childWedge; // assigned arc + one empty slot
      const startAngle = parentBisector - assignedArc / 2;
      // Raw angle from atan2 (not the inner-ring-adjusted angle)
      let rawAngle = Math.atan2(dy, dx) * (180 / Math.PI);
      rawAngle = ((rawAngle % 360) + 360) % 360;
      let rel = rawAngle - ((startAngle % 360) + 360) % 360;
      if (rel < -180) rel += 360;
      if (rel > 180) rel -= 360;
      if (rel >= 0 && rel < totalArc) {
        const ci = Math.floor(rel / childWedge);
        if (ci >= 0 && ci < totalSlots) return { ring: 'outer', index: ci };
      }
      return null;
    }
    return null;
  }, [wheelRef]);

  // Simple inner-ring-only hit test for backward compat
  const localHitTest = useCallback((clientX, clientY) => {
    const hit = localHitTestFull(clientX, clientY);
    return hit?.ring === 'inner' ? hit.index : -1;
  }, [localHitTestFull]);

  const handleWedgePointerDown = useCallback((item, index, e) => {
    if (e.button !== 0) return;
    wedgeDragRef.current = { fromIndex: index, fromRing: 'inner', startX: e.clientX, startY: e.clientY, active: false };

    const onMove = (me) => {
      const ref = wedgeDragRef.current;
      if (!ref) return;
      if (!ref.active) {
        const dx = me.clientX - ref.startX, dy = me.clientY - ref.startY;
        if (Math.sqrt(dx * dx + dy * dy) < 5) return;
        ref.active = true;
        setWedgeDragFrom(ref.fromIndex);
        document.body.style.cursor = 'grabbing';
      }
      // Use inner-ring hit for visual target highlight
      setWedgeDragTo(localHitTest(me.clientX, me.clientY));
      setWedgeDragPos({ x: me.clientX, y: me.clientY });
    };

    const onUpFinal = (ue) => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUpFinal);
      const ref = wedgeDragRef.current;
      wedgeDragRef.current = null;
      if (ref?.active) {
        const hit = localHitTestFull(ue.clientX, ue.clientY);
        const items = itemsRef.current;
        const sourceItem = items[ref.fromIndex];

        if (hit && sourceItem) {
          if (hit.ring === 'inner' && hit.index !== ref.fromIndex) {
            // Inner ring: always swap positions (regardless of folder/non-folder)
            onSwapRadialMenuItems?.(ref.fromIndex, hit.index);
          } else if (hit.ring === 'outer') {
            // Outer ring: move item into the expanded folder as a child
            const efId = expandedFolderRef.current;
            const folderItem = efId ? items.find(i => i && i.id === efId) : null;
            if (sourceItem.type !== 'folder' && folderItem) {
              onMoveItemToFolder?.(ref.fromIndex, folderItem.id);
            }
          }
        }
      }
      setWedgeDragFrom(-1);
      setWedgeDragTo(-1);
      setWedgeDragPos(null);
      document.body.style.cursor = '';
    };

    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUpFinal);
  }, [localHitTest, localHitTestFull, onSwapRadialMenuItems, onMoveItemToFolder, expandedFolder]);

  // ── Drag child OUT of folder to main ring ──────────────────────────────
  const [childDragFrom, setChildDragFrom] = useState(null); // { folderId, childId, childLabel }
  const childDragRef = useRef(null);

  const handleChildPointerDown = useCallback((folderId, child, childIndex, e) => {
    if (e.button !== 0) return;
    childDragRef.current = { folderId, child, childIndex, startX: e.clientX, startY: e.clientY, active: false };

    const onMove = (me) => {
      const ref = childDragRef.current;
      if (!ref) return;
      if (!ref.active) {
        const dx = me.clientX - ref.startX, dy = me.clientY - ref.startY;
        if (Math.sqrt(dx * dx + dy * dy) < 5) return;
        ref.active = true;
        setChildDragFrom({ folderId: ref.folderId, childId: ref.child.id, childLabel: ref.child.label || '' });
        document.body.style.cursor = 'grabbing';
      }
      setWedgeDragTo(localHitTest(me.clientX, me.clientY));
      setWedgeDragPos({ x: me.clientX, y: me.clientY });
    };

    const onUpFinal = (ue) => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUpFinal);
      const ref = childDragRef.current;
      childDragRef.current = null;
      if (ref?.active) {
        const hit = localHitTestFull(ue.clientX, ue.clientY);
        if (hit?.ring === 'inner' && hit.index >= 0) {
          const items = itemsRef.current;
          const targetSlot = items[hit.index];
          // Drop onto empty main slot → move child out of folder
          if (!targetSlot) {
            onMoveChildToMain?.(ref.folderId, ref.child.id, hit.index);
          }
        } else if (hit?.ring === 'outer' && hit.index !== ref.childIndex) {
          // Drop onto another outer ring slot → swap children within folder
          const efId = expandedFolderRef.current;
          const items = itemsRef.current;
          const folder = efId ? items.find(i => i && i.id === efId) : null;
          if (folder?.children && hit.index < folder.children.length && hit.index !== ref.childIndex) {
            const newChildren = [...folder.children];
            const temp = newChildren[ref.childIndex];
            newChildren[ref.childIndex] = newChildren[hit.index];
            newChildren[hit.index] = temp;
            onReorderFolderChildren?.(efId, newChildren);
          }
        }
      }
      setChildDragFrom(null);
      setWedgeDragTo(-1);
      setWedgeDragPos(null);
      document.body.style.cursor = '';
    };

    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUpFinal);
  }, [localHitTest, localHitTestFull, onMoveChildToMain, onReorderFolderChildren]);

  // Derive drag ghost label
  const isDraggingMain = wedgeDragFrom >= 0;
  const isDraggingChild = childDragFrom != null;
  const dragGhostLabel = isDraggingMain && radialMenuItems[wedgeDragFrom]
    ? (radialMenuItems[wedgeDragFrom].label || radialMenuItems[wedgeDragFrom].storageKey?.split('::').pop() || 'Item')
    : isDraggingChild
      ? (childDragFrom.childLabel || 'Item')
      : null;
  const dragTargetIsFolder = wedgeDragTo >= 0 && radialMenuItems[wedgeDragTo]?.type === 'folder';
  const dragTargetIsEmpty = wedgeDragTo >= 0 && !radialMenuItems[wedgeDragTo];

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
              ref={el => el?.focus()}
              onBlur={() => { setCapturingKey(false); setCapturedKey(null); setRadialConflict(null); }}
              onKeyUp={e => { e.preventDefault(); e.stopPropagation(); }}
              onKeyDown={async e => {
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
                const result = await window.electronAPI?.checkHotkeyConflict(combo, 'radial');
                setRadialConflict(result?.conflict ? `Already used by ${result.conflictWith}. Pick a different one.` : null);
                setCapturedKey({ combo, label });
              }}
            >
              {capturedKey ? (
                <span className="rmp-captured">{capturedKey.label}</span>
              ) : (
                <span className="rmp-waiting">Press combo...</span>
              )}
              {capturedKey && !radialConflict && (
                <button className="rmp-save-btn" type="button" onMouseDown={e => e.preventDefault()} onClick={() => {
                  onSetRadialMenuHotkey?.(capturedKey.combo);
                  setCapturingKey(false);
                  setCapturedKey(null);
                  setRadialConflict(null);
                }}>Save</button>
              )}
              <button className="rmp-cancel-btn" type="button" onMouseDown={e => e.preventDefault()} onClick={() => { setCapturingKey(false); setCapturedKey(null); setRadialConflict(null); }}>&#10005;</button>
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
      {radialConflict && (
        <div className="rmp-conflict-warn">{radialConflict}</div>
      )}

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
                ? 'Click child to edit \u00b7 Drag child to main ring to remove from folder'
                : 'Click to edit \u00b7 Drag to reorder \u00b7 Drag onto folder to add \u00b7 Right-click for options'}
          </span>
        </div>
      )}

      {/* Hold-to-select toggle */}
      {radialMenuHotkey && (
        <div className="rev-holdselect-row">
          <div className="rev-holdselect-text">
            <span className="rev-holdselect-label">Hold to select</span>
            <span className="rev-holdselect-hint">
              Hold the hotkey, point at a segment, release to fire. When off, the wheel stays open to click.
            </span>
          </div>
          <button
            type="button"
            role="switch"
            aria-checked={!!radialHoldToSelect}
            className={`rev-holdselect-toggle${radialHoldToSelect ? ' on' : ''}`}
            onClick={() => onSetRadialHoldToSelect?.(!radialHoldToSelect)}
            title="Toggle hold-to-select mode"
          />
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
          {!hiddenTips.includes('radial-info') && (
            <div className="rev-tip">
              <Info size={14} strokeWidth={2} aria-hidden="true" />
              <span>Assign actions to the 8 segments, or create a folder by right clicking on the segment to nest actions.</span>
              <button type="button" className="rev-tip-close" title="Hide this tip (restore in Settings)" aria-label="Hide this tip" onClick={() => onHideTip?.('radial-info')}>&#10005;</button>
            </div>
          )}
          <div className="rev-editor" ref={wheelRef} onClick={() => { setPopover(null); setCtxMenu(null); }}>
            <RadialWheel
              mode="editor"
              externalDnd={true}
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
              onChildPointerDown={handleChildPointerDown}
            />

            {/* ── Drag ghost — follows cursor during wedge drag ── */}
            {wedgeDragPos && dragGhostLabel && (
              <div
                className={`rev-drag-ghost${dragTargetIsFolder ? ' rev-drag-ghost--folder' : ''}${isDraggingChild && dragTargetIsEmpty ? ' rev-drag-ghost--folder' : ''}`}
                style={{ left: wedgeDragPos.x, top: wedgeDragPos.y }}
              >
                {dragGhostLabel}
                {isDraggingMain && dragTargetIsFolder && <span className="rev-drag-ghost-hint">Drop into folder</span>}
                {isDraggingChild && dragTargetIsEmpty && <span className="rev-drag-ghost-hint">Drop to main ring</span>}
              </div>
            )}

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
                        <SearchBar
                          className="rmp-popover-search-bar compact"
                          placeholder="Search..."
                          value={popover.search || ''}
                          onChange={e => setPopover(p => ({ ...p, search: e.target.value }))}
                          onKeyDown={e => { e.stopPropagation(); if (e.key === 'Escape') setPopover(null); }}
                          autoFocus
                        />
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

            {/* ── Copy-to-profile overwrite confirmation ── */}
            {copyConfirm && (
              <>
                <div className="rmp-backdrop" onClick={() => setCopyConfirm(null)} />
                <div className="rmp-popover" style={{ left: '50%', top: '50%' }}>
                  <div className="rmp-popover-confirm-text">
                    Segment {copyConfirm.index + 1} on <strong>{copyConfirm.targetProfile}</strong> already has <strong>{copyConfirm.existingLabel}</strong>. Overwrite?
                  </div>
                  <div className="rmp-popover-btns">
                    <button type="button" className="rmp-popover-btn rmp-popover-danger" onClick={() => {
                      onForceOverwriteRadialSegment?.(copyConfirm.targetProfile, copyConfirm.index);
                      setCopyConfirm(null);
                    }}>Overwrite</button>
                    <button type="button" className="rmp-popover-btn" onClick={() => setCopyConfirm(null)}>Cancel</button>
                  </div>
                </div>
              </>
            )}
          </div>
          {!hiddenTips.includes('radial-hotkey') && (
            <div className="rev-tip rev-tip-prominent">
              <span className="rev-tip-badge">TIP</span>
              <span>
                Press{' '}
                {radialMenuHotkey.split('+').map((p, i, arr) => (
                  <React.Fragment key={i}>
                    <kbd className="rmp-kbd">{friendlyKeyName(p)}</kbd>
                    {i < arr.length - 1 && <span className="rmp-plus">+</span>}
                  </React.Fragment>
                ))}
                {' '}when this profile is active to launch your radial wheel. Even better, add hotkey to mouse side button!
              </span>
              <button type="button" className="rev-tip-close" title="Hide this tip (restore in Settings)" aria-label="Hide this tip" onClick={() => onHideTip?.('radial-hotkey')}>&#10005;</button>
            </div>
          )}
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
              {otherProfiles.length > 0 && (
                <>
                  <div className="assign-ctx-divider" />
                  <div
                    className="assign-ctx-sub"
                    onMouseEnter={() => setHoveredCopySub(true)}
                    onMouseLeave={() => setHoveredCopySub(false)}
                  >
                    <button className="assign-ctx-item" type="button">Copy to {'\u25b8'}</button>
                    {hoveredCopySub && (
                      <div className="assign-ctx-submenu" ref={copySubmenuRef}>
                        {otherProfiles.map(p => (
                          <button key={p} className="assign-ctx-item" type="button" onClick={() => {
                            const result = onCopyRadialSegmentToProfile?.(p, ctxMenu.index);
                            if (result?.conflict) {
                              setCopyConfirm({ targetProfile: p, index: ctxMenu.index, existingLabel: result.existingLabel });
                            }
                            setCtxMenu(null);
                          }}>{p}</button>
                        ))}
                      </div>
                    )}
                  </div>
                </>
              )}
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
              {otherProfiles.length > 0 && (
                <>
                  <div className="assign-ctx-divider" />
                  <div
                    className="assign-ctx-sub"
                    onMouseEnter={() => setHoveredCopySub(true)}
                    onMouseLeave={() => setHoveredCopySub(false)}
                  >
                    <button className="assign-ctx-item" type="button">Copy to {'\u25b8'}</button>
                    {hoveredCopySub && (
                      <div className="assign-ctx-submenu" ref={copySubmenuRef}>
                        {otherProfiles.map(p => (
                          <button key={p} className="assign-ctx-item" type="button" onClick={() => {
                            const result = onCopyRadialSegmentToProfile?.(p, ctxMenu.index);
                            if (result?.conflict) {
                              setCopyConfirm({ targetProfile: p, index: ctxMenu.index, existingLabel: result.existingLabel });
                            }
                            setCtxMenu(null);
                          }}>{p}</button>
                        ))}
                      </div>
                    )}
                  </div>
                </>
              )}
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
          <React.Suspense fallback={null}>
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
          </React.Suspense>
        </div>
      )}

    </div>
  );
}
