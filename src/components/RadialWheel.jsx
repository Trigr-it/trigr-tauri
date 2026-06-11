import React, { useMemo, useCallback, useState, useEffect } from 'react';
import { DndContext, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, arrayMove } from '@dnd-kit/sortable';
import { isLucideIcon, getLucideIconName, isSimpleIcon, getSimpleIconSlug, isCustomIcon, getCustomIconData, loadIconRenderers, getIconRenderers } from './iconUtils';
import './RadialWheel.css';

// ── Geometry constants ─────────────────────────────────────────────────────────

const CX = 210, CY = 210;
const INNER_R = 55, OUTER_R = 105;
const OUTER_INNER_R = 113, OUTER_OUTER_R = 163;
const CENTRE_R = 35;
const MAX_SLOTS = 8;

// ── Type icons (matches SearchOverlay TYPE_META) ───────────────────────────────

// Type-identification colours — theme-invariant, matches SearchOverlay TYPE_META
const TYPE_META = {
  text:       { icon: '\u2726', color: '#64b4ff' },
  hotkey:     { icon: '\u2328', color: '#c864ff' },
  app:        { icon: '\u2b21', color: '#50c878' },
  url:        { icon: '\u2295', color: '#ffc832' },
  folder:     { icon: '\u2b22', color: '#40c8a0' },
  macro:      { icon: '\u25c8', color: '#ff783c' },
  expansion:  { icon: '\u21a9', color: '#ffc832' },
  autocorrect:{ icon: '\u270f', color: '#aaaaaa' },
};

const FOLDER_ICON = '\u25c9'; // fisheye circle — not emoji per requirement
const FOLDER_COLOR = 'var(--accent)';

// ── Helpers ────────────────────────────────────────────────────────────────────

function deg2rad(d) { return d * Math.PI / 180; }

function polarToXY(cx, cy, r, angleDeg) {
  const rad = deg2rad(angleDeg);
  return [cx + r * Math.cos(rad), cy + r * Math.sin(rad)];
}

/** Build an SVG arc path along a circle at radius r between two angles. */
function arcPath(cx, cy, r, startAngle, endAngle) {
  const span = endAngle - startAngle;
  const largeArc = Math.abs(span) > 180 ? 1 : 0;
  const [x1, y1] = polarToXY(cx, cy, r, startAngle);
  const [x2, y2] = polarToXY(cx, cy, r, endAngle);
  return `M ${x1} ${y1} A ${r} ${r} 0 ${largeArc} 1 ${x2} ${y2}`;
}

/** Build an SVG path for a wedge (pie slice) between two radii and two angles. */
function wedgePath(cx, cy, innerR, outerR, startAngle, endAngle) {
  const span = endAngle - startAngle;
  const largeArc = Math.abs(span) > 180 ? 1 : 0;

  const [ix1, iy1] = polarToXY(cx, cy, innerR, startAngle);
  const [ix2, iy2] = polarToXY(cx, cy, innerR, endAngle);
  const [ox1, oy1] = polarToXY(cx, cy, outerR, startAngle);
  const [ox2, oy2] = polarToXY(cx, cy, outerR, endAngle);

  return [
    `M ${ix1} ${iy1}`,
    `A ${innerR} ${innerR} 0 ${largeArc} 1 ${ix2} ${iy2}`,
    `L ${ox2} ${oy2}`,
    `A ${outerR} ${outerR} 0 ${largeArc} 0 ${ox1} ${oy1}`,
    'Z',
  ].join(' ');
}

function getItemMeta(item) {
  if (!item) return { icon: '+', color: 'var(--text-muted)' };
  if (item.type === 'folder') return { icon: FOLDER_ICON, color: FOLDER_COLOR };
  if (item.type === 'expansion' || item.type === 'autocorrect')
    return TYPE_META[item.type] || { icon: '?', color: 'var(--text-muted)' };
  return TYPE_META[item.assignType] || TYPE_META[item.type] || { icon: '\u25c8', color: 'var(--text-muted)' };
}

function numLabel(i) {
  if (i < MAX_SLOTS) return String(i + 1);
  return '';
}

// ── Component ──────────────────────────────────────────────────────────────────

export default function RadialWheel({
  mode = 'live',             // 'live' | 'editor'
  items = [],
  expandedFolder = null,
  hoveredIndex = -1,
  hoveredOuterIndex = -1,
  dropTargetIndex = -1,      // inner wedge index highlighted as drop target
  dropTargetOuterIndex = -1, // outer wedge index highlighted as drop target
  onHoverInner,
  onHoverOuter,
  onItemClick,
  onEmptyWedgeClick,
  onFolderChildClick,
  onEmptyChildWedgeClick,
  onBackgroundClick,
  onReorder,
  onReorderChildren,
  scale = 1,
  externalDnd = false,       // true = parent owns DndContext, skip internal wrapper
  onItemContextMenu,         // (item, index, event) — right-click on filled inner wedge
  onChildContextMenu,        // (folderId, child, childIndex, event) — right-click on filled outer wedge
  onEmptyWedgeContextMenu,   // (index, event) — right-click on empty inner wedge
  onWedgePointerDown,        // (item, index, event) — pointerdown on filled inner wedge (for drag)
  onChildPointerDown,        // (folderId, item, index, event) — pointerdown on filled outer wedge (for drag out)
  dragFromIndex = -1,        // inner wedge index being dragged (dim visual)
  selectedIndex = -1,        // inner wedge index currently selected for editing
  innerRadius,               // override INNER_R (editor uses smaller value to reduce centre gap)
  outerRadius,               // override OUTER_R (editor shifts ring inward)
}) {
  const isEditor = mode === 'editor';
  const effectiveInnerR = innerRadius != null ? innerRadius : INNER_R;
  const effectiveOuterR = outerRadius != null ? outerRadius : OUTER_R;
  // Outer ring (folder children) radii — starts just outside inner ring, same wedge height
  const wedgeHeight = effectiveOuterR - effectiveInnerR;
  const effectiveOuterInnerR = effectiveOuterR + 8;
  const effectiveOuterOuterR = effectiveOuterInnerR + wedgeHeight;
  const count = Math.min(items.length, MAX_SLOTS);
  const angleStep = 360 / MAX_SLOTS;

  // dnd-kit sensor — 5px distance threshold to distinguish click from drag
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } })
  );

  // Icon libraries load on demand: only wheels that actually contain a
  // lucide:/simple: icon pull in the heavy renderer chunk (see iconUtils.jsx).
  // appIcon takes render priority over item.icon, so it doesn't trigger a load.
  const needsRenderers = useMemo(() => {
    const needs = (it) => !!it && (
      (!it.appIcon && (isLucideIcon(it.icon) || isSimpleIcon(it.icon))) ||
      (it.children || []).some(needs)
    );
    return items.some(needs);
  }, [items]);
  const [renderers, setRenderers] = useState(getIconRenderers);
  useEffect(() => {
    if (!needsRenderers || renderers) return;
    let cancelled = false;
    loadIconRenderers().then((mod) => { if (!cancelled) setRenderers(mod); });
    return () => { cancelled = true; };
  }, [needsRenderers, renderers]);

  // ── Compute inner wedges ─────────────────────────────────────────────
  const innerWedges = useMemo(() => {
    const wedges = [];
    // Always use MAX_SLOTS for layout so wedge sizes are consistent
    // between editor and live overlay. Empty slots render in editor only.
    const slotCount = MAX_SLOTS;
    if (count === 0 && !isEditor) return wedges;
    const step = 360 / slotCount;
    for (let i = 0; i < slotCount; i++) {
      const startAngle = step * i - 90 - step / 2;
      const endAngle = startAngle + step;
      const bisector = (startAngle + endAngle) / 2;
      const item = i < count ? items[i] : null;
      const isEmpty = !item;
      // In live mode, skip empty slots entirely (don't render blank wedges)
      if (isEmpty && !isEditor) continue;
      wedges.push({ index: i, startAngle, endAngle, bisector, item, isEmpty });
    }
    return wedges;
  }, [items, count, isEditor]);

  // ── Compute outer wedges (expanded folder children) ──────────────────
  const outerWedges = useMemo(() => {
    if (!expandedFolder) return [];
    const folderIdx = items.findIndex(i => i?.id === expandedFolder);
    if (folderIdx < 0) return [];
    const folder = items[folderIdx];
    if (folder.type !== 'folder' || !folder.children) return [];

    const children = folder.children;
    const childCount = children.length;
    // Always use MAX_SLOTS for parent slot geometry (matches inner ring layout)
    const slotStep = 360 / MAX_SLOTS;
    const parentStart = slotStep * folderIdx - 90 - slotStep / 2;
    const parentEnd = parentStart + slotStep;
    const parentBisector = (parentStart + parentEnd) / 2;
    const parentArc = slotStep;

    const minArcPerChild = 22;
    const hasEmpty = isEditor && childCount < 8;
    // Centre the assigned children on the parent; empty slot goes at the end
    const assignedCount = Math.max(childCount, 1);
    const desiredArc = Math.max(parentArc, assignedCount * minArcPerChild);
    const assignedArc = Math.min(desiredArc, 160);
    const childWedgeAngle = assignedArc / assignedCount;
    const assignedStart = parentBisector - assignedArc / 2;

    const wedges = [];
    // Assigned children — centred on parent
    for (let i = 0; i < childCount; i++) {
      const s = assignedStart + childWedgeAngle * i;
      const e = s + childWedgeAngle;
      wedges.push({
        index: i, startAngle: s, endAngle: e, bisector: (s + e) / 2,
        item: children[i], isEmpty: false, folderId: expandedFolder,
      });
    }
    // Empty "+" slot at the end (editor only, if room)
    if (hasEmpty) {
      const emptyStart = childCount > 0
        ? assignedStart + childWedgeAngle * childCount
        : assignedStart;
      const emptyEnd = emptyStart + childWedgeAngle;
      wedges.push({
        index: childCount, startAngle: emptyStart, endAngle: emptyEnd,
        bisector: (emptyStart + emptyEnd) / 2,
        item: null, isEmpty: true, folderId: expandedFolder,
      });
    }
    return wedges;
  }, [items, expandedFolder, count, isEditor]);

  // ── dnd-kit handlers ─────────────────────────────────────────────────
  const handleDragEnd = useCallback((event) => {
    const { active, over } = event;
    if (!over || active.id === over.id) return;

    // Check if this is a top-level reorder or a child reorder
    const activeIsChild = String(active.id).startsWith('child-');
    if (activeIsChild && onReorderChildren && expandedFolder) {
      const folder = items.find(i => i?.id === expandedFolder);
      if (!folder?.children) return;
      const oldIdx = folder.children.findIndex(c => `child-${c.id}` === active.id);
      const newIdx = folder.children.findIndex(c => `child-${c.id}` === over.id);
      if (oldIdx >= 0 && newIdx >= 0) {
        onReorderChildren(expandedFolder, arrayMove(folder.children, oldIdx, newIdx));
      }
    } else if (!activeIsChild && onReorder) {
      const oldIdx = items.findIndex(i => i?.id === active.id);
      const newIdx = items.findIndex(i => i?.id === over.id);
      if (oldIdx >= 0 && newIdx >= 0) {
        onReorder(arrayMove([...items], oldIdx, newIdx));
      }
    }
  }, [items, expandedFolder, onReorder, onReorderChildren]);

  // ── Sortable IDs for dnd-kit ─────────────────────────────────────────
  const innerSortableIds = useMemo(
    () => items.filter(i => i?.id).map(i => i.id),
    [items]
  );
  const outerSortableIds = useMemo(() => {
    if (!expandedFolder) return [];
    const folder = items.find(i => i?.id === expandedFolder);
    return (folder?.children || []).filter(c => c?.id).map(c => `child-${c.id}`);
  }, [items, expandedFolder]);

  // ── Render a single wedge ────────────────────────────────────────────
  const WEDGE_GAP = 1.2; // degrees inset from each edge
  const RADIAL_GAP = 2;  // pixels inset from inner/outer edges

  function renderWedge(w, isOuter = false) {
    const { index, startAngle, endAngle, bisector, item, isEmpty, folderId } = w;
    const iR = isOuter ? effectiveOuterInnerR : effectiveInnerR;
    const oR = isOuter ? effectiveOuterOuterR : effectiveOuterR;

    const isHovered = isOuter
      ? hoveredOuterIndex === index
      : hoveredIndex === index;

    // Gap inset: shrink wedge by WEDGE_GAP degrees on each side + RADIAL_GAP pixels on inner/outer
    // On hover: expand back to full size
    const gapAngle = isHovered ? 0 : WEDGE_GAP;
    const gapR = isHovered ? 0 : RADIAL_GAP;
    const d = wedgePath(CX, CY, iR + gapR, oR - gapR, startAngle + gapAngle, endAngle - gapAngle);
    const midR = (iR + oR) / 2;
    const isFolder = item?.type === 'folder';
    const isFolderExpanded = isFolder && expandedFolder === item?.id;
    const isMissing = item && item.exists === false;

    const meta = getItemMeta(item);

    // Content positions along bisector
    const [iconX, iconY] = polarToXY(CX, CY, midR, bisector);          // centred in wedge
    const [numX, numY] = polarToXY(CX, CY, iR + 10, bisector);        // inner edge

    // Folder child count badge position
    const [badgeX, badgeY] = polarToXY(CX, CY, oR - 14, bisector);

    // Text rotation — align with wedge angle, flip bottom half for readability
    const rawAngle = bisector + 90; // perpendicular to radius = along arc
    // If text would be upside-down (pointing left), flip 180°
    const textAngle = (bisector > 0 && bisector < 180) ? rawAngle + 180 : rawAngle;

    const isDropTarget = isOuter ? dropTargetOuterIndex === index : dropTargetIndex === index;
    const isDragSource = !isOuter && dragFromIndex === index;
    // Empty wedges included: clicking one opens the editor for that slot, so
    // the wheel should keep showing which slot is being assigned.
    const isSelected = !isOuter && selectedIndex === index;

    const classNames = [
      'rw-wedge',
      isHovered && 'rw-wedge--hovered',
      isFolder && 'rw-wedge--folder',
      isFolderExpanded && 'rw-wedge--folder-expanded',
      isMissing && 'rw-wedge--missing',
      isEmpty && 'rw-wedge--empty',
      isOuter && 'rw-wedge--outer',
      isDropTarget && 'rw-wedge--drop-target',
      isDragSource && 'rw-wedge--drag-source',
      isSelected && 'rw-wedge--selected',
    ].filter(Boolean).join(' ');

    const handleClick = (e) => {
      e.stopPropagation();
      if (isEmpty && isEditor) {
        if (isOuter && folderId) {
          onEmptyChildWedgeClick?.(folderId, index);
        } else {
          onEmptyWedgeClick?.(index);
        }
      } else if (item) {
        if (isOuter && folderId) {
          onFolderChildClick?.(folderId, item, index);
        } else {
          onItemClick?.(item, index);
        }
      }
    };

    const handleCtxMenu = (e) => {
      e.preventDefault();
      e.stopPropagation();
      if (isEmpty && !isOuter) {
        onEmptyWedgeContextMenu?.(index, e);
        return;
      }
      if (!item) return;
      if (isOuter && folderId) {
        onChildContextMenu?.(folderId, item, index, e);
      } else {
        onItemContextMenu?.(item, index, e);
      }
    };

    const handlePtrDown = (e) => {
      if (e.button !== 0 || isEmpty || !item || !isEditor) return;
      if (isOuter && folderId) {
        onChildPointerDown?.(folderId, item, index, e);
      } else if (!isOuter) {
        onWedgePointerDown?.(item, index, e);
      }
    };

    // Full-size path for hover hit area (invisible)
    const dFull = wedgePath(CX, CY, iR, oR, startAngle, endAngle);

    // Animation: slide outward from centre along bisector (live mode only)
    const animStyle = !isEditor ? {
      animation: `wedge-expand 0.18s cubic-bezier(0.2, 0.9, 0.3, 1.05) both`,
    } : undefined;

    return (
      <g key={`${isOuter ? 'outer' : 'inner'}-${index}`} className={classNames} style={animStyle}>
        {/* Invisible hit area — covers full wedge including gaps */}
        <path
          d={dFull}
          fill="transparent"
          stroke="none"
          onClick={handleClick}
          onContextMenu={isEditor ? handleCtxMenu : undefined}
          onPointerDown={isEditor ? handlePtrDown : undefined}
          onMouseEnter={() => isOuter ? onHoverOuter?.(index) : onHoverInner?.(index)}
          onMouseLeave={() => isOuter ? onHoverOuter?.(-1) : onHoverInner?.(-1)}
        />
        {/* Visible wedge path (inset with gap, expands on hover) */}
        <path
          d={d}
          className="rw-wedge-path"
          pointerEvents="none"
        />
        {/* Wedge content */}
        {isEmpty && isEditor && (
          <text
            x={polarToXY(CX, CY, midR, bisector)[0]}
            y={polarToXY(CX, CY, midR, bisector)[1]}
            className="rw-wedge-plus"
            onClick={handleClick}
            pointerEvents="none"
          >+</text>
        )}
        {item && (() => {
          const iconColor = item.iconColor || meta.color;
          const iconScale = isHovered ? 1.12 : 1;
          return (
          <>
            {/* Icon — priority: appIcon > custom image > Simple Icon > Lucide > unicode */}
            <g style={{ transform: `scale(${iconScale})`, transformOrigin: `${iconX}px ${iconY}px`, transition: 'transform 0.12s cubic-bezier(0.2, 0, 0, 1)' }}>
            {item.appIcon ? (
              <image
                href={item.appIcon}
                x={iconX - 14} y={iconY - 14}
                width={28} height={28}
                pointerEvents="none"
                style={{ imageRendering: 'auto' }}
              />
            ) : isCustomIcon(item.icon) ? (
              <image
                href={getCustomIconData(item.icon)}
                x={iconX - 14} y={iconY - 14}
                width={28} height={28}
                pointerEvents="none"
                preserveAspectRatio="xMidYMid meet"
                style={{ imageRendering: 'auto' }}
              />
            ) : isSimpleIcon(item.icon) ? (
              <svg
                x={iconX - 10} y={iconY - 10}
                width={20} height={20}
                viewBox="0 0 24 24"
                pointerEvents="none"
              >
                {renderers ? renderers.renderSimpleIcon(getSimpleIconSlug(item.icon), 24, iconColor !== 'currentColor' ? iconColor : undefined) : null}
              </svg>
            ) : isLucideIcon(item.icon) ? (
              <svg
                x={iconX - 12} y={iconY - 12}
                width={24} height={24}
                viewBox="0 0 24 24"
                pointerEvents="none"
              >
                {renderers ? renderers.renderLucideIcon(getLucideIconName(item.icon), 24, iconColor, false) : null}
              </svg>
            ) : (
              <text
                x={iconX} y={iconY}
                className="rw-wedge-icon"
                style={{ fill: iconColor }}
                pointerEvents="none"
              >{meta.icon}</text>
            )}
            </g>
            {/* Number key badge — editor only */}
            {isEditor && !isFolder && !isOuter && (
              <text
                x={numX} y={numY}
                className="rw-wedge-num"
                pointerEvents="none"
              >{numLabel(index)}</text>
            )}
            {/* Folder trim — gold arc along outer edge. Radius tracks gapR so
                the band hugs the wedge edge in both rest and hover (grown)
                states: band outer edge lands flush with the wedge's 1px
                outline at oR - gapR. */}
            {isFolder && (
              <path
                d={arcPath(CX, CY, oR - gapR - 2, startAngle + gapAngle + 1, endAngle - gapAngle - 1)}
                className="rw-folder-trim"
                pointerEvents="none"
              />
            )}
          </>
        );})()}
      </g>
    );
  }

  // ── Main render ──────────────────────────────────────────────────────
  const svgContent = (
    <svg
      viewBox="0 0 420 420"
      className={`rw-svg${isEditor ? ' rw-svg--editor' : ''}`}
      xmlns="http://www.w3.org/2000/svg"
    >
      {/* Transparent backdrop for click-outside handling */}
      <rect
        x="0" y="0" width="420" height="420"
        fill="transparent"
        pointerEvents="all"
        onClick={(e) => {
          e.stopPropagation();
          onBackgroundClick?.();
        }}
      />

      {/* Centre hover label — shows hovered item name + type */}
      {(() => {
        let hovItem = null;
        if (hoveredOuterIndex >= 0 && expandedFolder) {
          const folder = items.find(i => i?.id === expandedFolder);
          hovItem = folder?.children?.[hoveredOuterIndex];
        } else if (hoveredIndex >= 0 && hoveredIndex < count) {
          hovItem = items[hoveredIndex];
        }
        if (!hovItem) return null;
        const label = hovItem.label || '';
        const showType = isEditor && (hovItem.type === 'folder' ? 'Folder' : (hovItem.assignType || hovItem.type || ''));
        const pillW = Math.max(label.length * 7.5, 60);
        const pillH = showType ? 34 : 22;
        return (
          <g className="rw-centre-label">
            <rect
              x={CX - pillW / 2} y={CY - pillH / 2}
              width={pillW} height={pillH}
              rx={6}
              className="rw-centre-pill"
            />
            <text x={CX} y={showType ? CY - 5 : CY} className="rw-centre-hover-name">{
              label.length > 20 ? label.slice(0, 19) + '\u2026' : label
            }</text>
            {showType && (
              <text x={CX} y={CY + 11} className="rw-centre-hover-type">{showType}</text>
            )}
          </g>
        );
      })()}

      {/* Inner ring wedges */}
      {innerWedges.map(w => renderWedge(w, false))}

      {/* Outer ring wedges (expanded folder children) — key forces remount on folder switch */}
      {outerWedges.length > 0 && (
        <g className="rw-outer-ring" key={expandedFolder}>
          {outerWedges.map(w => renderWedge(w, true))}
        </g>
      )}
    </svg>
  );

  // Wrap in DndContext for editor mode drag-to-reorder (skip if parent owns DndContext)
  if (isEditor && !externalDnd && (onReorder || onReorderChildren)) {
    return (
      <DndContext sensors={sensors} onDragEnd={handleDragEnd}>
        <SortableContext items={[...innerSortableIds, ...outerSortableIds]}>
          <div className="rw-container" style={scale !== 1 ? { transform: `scale(${scale})`, transformOrigin: 'center center' } : undefined}>
            {svgContent}
          </div>
        </SortableContext>
      </DndContext>
    );
  }

  return (
    <div className="rw-container" style={scale !== 1 ? { transform: `scale(${scale})`, transformOrigin: 'center center' } : undefined}>
      {svgContent}
    </div>
  );
}

export { MAX_SLOTS, CX, CY, INNER_R, OUTER_R, OUTER_INNER_R, OUTER_OUTER_R, polarToXY };
