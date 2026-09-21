import React, { useMemo, useCallback, useState, useEffect } from 'react';
import { DndContext, PointerSensor, useSensor, useSensors } from '@dnd-kit/core';
import { SortableContext, arrayMove } from '@dnd-kit/sortable';
import {
  Clock, CalendarDays, CalendarRange, BatteryMedium, BatteryLow, BatteryCharging,
  Cpu, MemoryStick, Volume2, VolumeX, Keyboard, AppWindow, Layers,
} from 'lucide-react';
import { isLucideIcon, getLucideIconName, isSimpleIcon, getSimpleIconSlug, isCustomIcon, getCustomIconData, loadIconRenderers, getIconRenderers } from './iconUtils';
import { PILL, MAX_TIER, pillGeometry, pillTextWidths, layoutWordsOnArc, tierBases, normAngle } from './radialWidgets';
import './RadialWheel.css';

// ── Geometry constants ─────────────────────────────────────────────────────────

const CX = 210, CY = 210;
const INNER_R = 55, OUTER_R = 105;
const OUTER_INNER_R = 113, OUTER_OUTER_R = 163;
const CENTRE_R = 35;
const MAX_SLOTS = 8;
// Folder wedges extrude outward past the ring perimeter as the "nested items
// inside" signal (matches the radial inspo's pop-out behaviour). Icon
// positioning still uses the base outerR so all icons sit at the same radial
// distance — the extruded portion reads as empty headroom above the icon.
const FOLDER_EXTRUDE_PX = 10;
// Layered rings (radial polish, 2026-09-21): a plate disc sits behind the
// main wedge ring and a hub disc fills the centre hole, each one tone apart
// from the wedges. HUB_GAP is the sliver of plate between the wedges' inner
// edge and the hub; PLATE_PAD is the rim visible past the wedges' outer edge
// and matches it so the ring sits centred on the disc. The rim is far
// narrower than the folder extrusion (an active folder pokes past it, which
// reads as the pop-out it is) and the plate never grows under the outer
// ring: expanded children float outside it.
const HUB_GAP = 3;
const PLATE_PAD = HUB_GAP;
// Widget pills (Pro, 2026-09-21): pills hugging the plate rim, each centred
// on its own angle (radialWidgets.js: geometry + clearance rules shared with
// the editor). Body = a stroked arc (hairline outline under a plate-coloured
// body, round caps). Content = lucide glyph at the left end of the run +
// text laid along the arc WORD BY WORD (radialWidgets.js layoutWordsOnArc):
// each word is one straight, kerned block rotated to the tangent at its own
// centre, so the words follow the curve without any glyph being placed on
// its own. Two-row pills put the caption on the visually higher arc. Third
// approach after glyph-by-glyph textPath (never read smooth at this radius)
// and one straight block (broke on long labels), Rory 2026-09-21. Hidden
// while a folder's outer ring is open.
// Pill body shape. 'wedge' = an annular segment whose sides are radial lines
// through the wheel centre, like the main wedges (Rory 2026-09-21 test);
// 'capsule' = the stroked arc with round caps from the Halo reference.
const PILL_SHAPE = 'wedge';
const PILL_ICONS = {
  'clock': Clock,
  'calendar': CalendarDays,
  'week': CalendarRange,
  'battery': BatteryMedium,
  'battery-low': BatteryLow,
  'battery-charging': BatteryCharging,
  'cpu': Cpu,
  'ram': MemoryStick,
  'volume': Volume2,
  'volume-x': VolumeX,
  'keyboard': Keyboard,
  'app': AppWindow,
  'profile': Layers,
};

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
  scale = 1,                 // sizes the container (viewBox scaling), NEVER a CSS transform: a
                             // post-render scale() rasterises the SVG and re-scales the bitmap,
                             // which turns text on the pill arcs fuzzy-jagged at 125 %+ DPI
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
  pills = [],                // widget pills [{ id, slot, icon, label }] (radialWidgets.js resolvePills)
  onPillPointerDown,         // editor: (pill, event) — start dragging a pill to another slot
  onPillHandlePointerDown,   // editor: (pill, centre {x,y} in viewBox units, event) — end-cap drag = resize
  draggingPillId = null,     // editor: pill currently being dragged (styled)
}) {
  const isEditor = mode === 'editor';
  // Widget pill rings: inner radius of ring 0 and ring 1 for this pill set.
  const pillBases = tierBases(pills);
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

  // Sticky centre-label item. When the user's cursor passes from one wedge
  // to another there's a tiny no-hover gap; without this the centre label
  // flashes off-on-off-on. Holding the last item for ~80ms lets the gap
  // disappear into a single visible transition instead.
  const liveHovItem = (() => {
    if (hoveredOuterIndex >= 0 && expandedFolder) {
      const folder = items.find(i => i?.id === expandedFolder);
      return folder?.children?.[hoveredOuterIndex] || null;
    }
    if (hoveredIndex >= 0 && hoveredIndex < count) {
      return items[hoveredIndex] || null;
    }
    return null;
  })();
  const [stickyHovItem, setStickyHovItem] = useState(liveHovItem);
  useEffect(() => {
    if (liveHovItem) {
      setStickyHovItem(liveHovItem);
      return undefined;
    }
    const id = setTimeout(() => setStickyHovItem(null), 80);
    return () => clearTimeout(id);
  }, [liveHovItem]);

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
  // Hairline angular gap between adjacent wedges. A base donut fills the
  // entire ring outline behind the wedges; what shows through the gap IS the
  // separator. The donut is painted in --radial-hairline (a low-contrast line
  // colour), so the gap reads as a ~1px divider at the outer edge and tapers
  // toward the hub, matching the Halo reference. No per-wedge outlines.
  const WEDGE_GAP = 0.4;
  const RADIAL_GAP = 0;

  function renderWedge(w, isOuter = false) {
    const { index, startAngle, endAngle, bisector, item, isEmpty, folderId } = w;
    const iR = isOuter ? effectiveOuterInnerR : effectiveInnerR;
    const baseOR = isOuter ? effectiveOuterOuterR : effectiveOuterR;
    const isFolder = item?.type === 'folder';
    const isFolderExpanded = isFolder && expandedFolder === item?.id;

    const isHovered = isOuter
      ? hoveredOuterIndex === index
      : hoveredIndex === index;

    // Parent folder shows full hover styling (gradient + stroke + glow) when
    // ANY of its children is hovered, so the active context stays lit while
    // the user travels into the outer ring.
    const isChildHovered = !isOuter && isFolderExpanded && hoveredOuterIndex >= 0;

    // Folder wedges extrude outward whenever the folder is in an active
    // state: either directly hovered OR currently expanded (i.e. user is
    // working with its children). Idle folders sit flush.
    const oR = (!isOuter && isFolder && (isHovered || isFolderExpanded))
      ? baseOR + FOLDER_EXTRUDE_PX
      : baseOR;

    // Constant hairline gap on every wedge edge — no longer changes on
    // hover (the ring stays one continuous shape, only the wedge fill flips).
    const gapAngle = WEDGE_GAP;
    const gapR = RADIAL_GAP;
    const d = wedgePath(CX, CY, iR + gapR, oR - gapR, startAngle + gapAngle, endAngle - gapAngle);
    // Icon centring uses BASE outerR, not the extruded oR, so all wedge icons
    // align at the same radial distance regardless of folder/non-folder state.
    const midR = (iR + baseOR) / 2;
    const isMissing = item && item.exists === false;

    const meta = getItemMeta(item);

    // Content positions along bisector
    const [iconX, iconY] = polarToXY(CX, CY, midR, bisector);          // centred in wedge
    const [numX, numY] = polarToXY(CX, CY, iR + 10, bisector);        // inner edge

    // Folder child count badge position (uses base outerR — keeps badge in
    // the icon region rather than out in the extruded headroom).
    const [badgeX, badgeY] = polarToXY(CX, CY, baseOR - 14, bisector);

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
      isChildHovered && 'rw-wedge--child-hover',
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
            {/* Number key badge — editor only, and only on a wedge that has
                no icon or logo of its own (the fallback type glyph needs the
                extra identification; a real icon does not, Rory 2026-09-21). */}
            {isEditor && !isFolder && !isOuter
              && !item.appIcon && !isCustomIcon(item.icon) && !isSimpleIcon(item.icon) && !isLucideIcon(item.icon) && (
              <text
                x={numX} y={numY}
                className="rw-wedge-num"
                pointerEvents="none"
              >{numLabel(index)}</text>
            )}
            {/* Folder marker is now the +10px outward extrusion of the wedge
                itself (set via oR computation above). The previous gold-arc
                trim is removed — extrusion is sufficient signal and avoids
                visual noise. */}
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
      {/* Vertical accent gradient used for the hovered wedge fill. Top→bottom
          (--accent-highlight sheen, --accent-bright, --accent base) mirrors the
          primary-button hover recipe. The stops read CSS custom properties via
          the style attribute (stop-color / flood-color are presentation
          properties, so var() resolves) — that is what lets theme presets
          recolour the wheel: the overlay's RadialMenu.css mirror provides the
          Keyfire defaults and src/theme/runtime.js overrides them inline. */}
      <defs>
        <linearGradient id="rw-hover-grad" x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%"  style={{ stopColor: 'var(--accent-highlight)' }} stopOpacity="1" />
          <stop offset="8%"  style={{ stopColor: 'var(--accent-bright)' }} stopOpacity="1" />
          <stop offset="100%" style={{ stopColor: 'var(--accent)' }} stopOpacity="1" />
        </linearGradient>
        {/* Soft shadow under the plate: wide and diffuse so the disc lifts
            off the desktop instead of casting a hard edge. Opacity lives in
            CSS (.rw-plate-shadow) so the light half can run it lighter. The
            filter region is oversized to fit the blur radius. */}
        <filter id="rw-plate-shadow" x="-50%" y="-50%" width="200%" height="200%">
          <feDropShadow dx="0" dy="10" stdDeviation="14" className="rw-plate-shadow" />
        </filter>
        {/* Widget pills carry no shadow (side by side, one pill's shadow fell
            on its neighbour). If one is ever re-added it needs an explicit
            user-space filter region: a pill is a thin arc whose geometry box
            is a few px tall, and a percentage region clipped the stroke. */}
        {/* Per-wedge glow used on hover — matches the button's outer drop
            shadow (0 2px 10px rgba(--accent-rgb, 0.5)). */}
        <filter id="rw-wedge-glow" x="-30%" y="-30%" width="160%" height="160%">
          <feDropShadow dx="0" dy="2" stdDeviation="5" style={{ floodColor: 'var(--accent)' }} floodOpacity="0.55" />
        </filter>
      </defs>

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

      {/* Layered rings. Plate disc (one tone below the wedges, soft shadow,
          hairline rim) behind the main ring only; the expanded outer ring
          sits outside it. Hub disc (one tone below again, hairline edge)
          fills the centre hole and carries the hover label. Live mode
          animates both with the same expand-from-centre as the wedges. */}
      {(() => {
        const plateR = effectiveOuterR + PLATE_PAD;
        const hubR = Math.max(effectiveInnerR - HUB_GAP, 8);
        const plateStyle = !isEditor
          ? { animation: 'wedge-expand 0.18s cubic-bezier(0.2, 0.9, 0.3, 1.05) both' }
          : undefined;
        return (
          <g className="rw-rings" style={plateStyle}>
            <circle cx={CX} cy={CY} r={plateR} className="rw-plate" pointerEvents="none" />
            <circle cx={CX} cy={CY} r={hubR} className="rw-hub" pointerEvents="none" />
          </g>
        );
      })()}

      {/* Base donut sits BEHIND every wedge and is painted in the hairline
          colour: the WEDGE_GAP between wedges shows it through as the
          separator, so the ring reads as one continuous shape with thin
          dividers instead of eight independently outlined tiles. */}
      {(() => {
        const oR = effectiveOuterR;
        const iR = effectiveInnerR;
        const d = [
          `M ${CX + oR} ${CY}`,
          `A ${oR} ${oR} 0 1 1 ${CX - oR} ${CY}`,
          `A ${oR} ${oR} 0 1 1 ${CX + oR} ${CY}`,
          `M ${CX + iR} ${CY}`,
          `A ${iR} ${iR} 0 1 0 ${CX - iR} ${CY}`,
          `A ${iR} ${iR} 0 1 0 ${CX + iR} ${CY}`,
          'Z',
        ].join(' ');
        return (
          <path
            d={d}
            className="rw-base-donut"
            fillRule="evenodd"
            pointerEvents="none"
          />
        );
      })()}

      {/* Centre hover label — shows hovered item name + type.
          Capped at 2 lines × 14 chars so the pill always fits inside
          the wheel's inner ring (diameter ~110px). Long labels wrap on
          the nearest space before the cut point; longer-still gets an
          ellipsis. Uses `stickyHovItem` (managed by useEffect below) so
          the label stays visible for a short grace period when hover
          leaves — smooths cross-wedge transitions instead of flashing
          off in the inter-segment gaps. */}
      {(() => {
        const hovItem = stickyHovItem;
        if (!hovItem) return null;

        const label = hovItem.label || '';
        const showType = isEditor && (hovItem.type === 'folder' ? 'Folder' : (hovItem.assignType || hovItem.type || ''));

        const MAX_CHARS_PER_LINE = 13;
        const MAX_LABEL_CHARS = 26;
        const truncated = label.length > MAX_LABEL_CHARS
          ? label.slice(0, MAX_LABEL_CHARS - 1) + '\u2026'
          : label;
        let line1 = truncated;
        let line2 = '';
        if (truncated.length > MAX_CHARS_PER_LINE) {
          const breakAt = truncated.lastIndexOf(' ', MAX_CHARS_PER_LINE);
          if (breakAt > 0) {
            line1 = truncated.slice(0, breakAt);
            line2 = truncated.slice(breakAt + 1);
          } else {
            line1 = truncated.slice(0, MAX_CHARS_PER_LINE);
            line2 = truncated.slice(MAX_CHARS_PER_LINE);
          }
        }
        const isTwoLines = !!line2;
        const longestLine = Math.max(line1.length, line2.length);

        const LINE_H = 14;
        const TYPE_H = 13;
        const pillW = Math.min(Math.max(longestLine * 6.6 + 14, 60), 100);
        const textBlockH = (isTwoLines ? LINE_H * 2 : LINE_H) + (showType ? TYPE_H : 0);
        const pillH = textBlockH + 10;
        const topY = CY - textBlockH / 2;
        const line1Y = topY + LINE_H / 2;
        const line2Y = line1Y + LINE_H;
        const typeY = (isTwoLines ? line2Y : line1Y) + LINE_H / 2 + TYPE_H / 2;

        return (
          <g className="rw-centre-label">
            <rect
              x={CX - pillW / 2} y={CY - pillH / 2}
              width={pillW} height={pillH}
              rx={6}
              className="rw-centre-pill"
            />
            <text x={CX} y={line1Y} className="rw-centre-hover-name">{line1}</text>
            {isTwoLines && (
              <text x={CX} y={line2Y} className="rw-centre-hover-name">{line2}</text>
            )}
            {showType && (
              <text x={CX} y={typeY} className="rw-centre-hover-type">{showType}</text>
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

      {/* Widget pills around the plate. Unmounted while a folder is open:
          the outer ring grows into their band. Text runs left-to-right and
          upright in both hemispheres: the path is drawn clockwise across the
          top and counter-clockwise across the bottom, so "up" for the glyphs
          is always screen-up. */}
      {pills.length > 0 && outerWedges.length === 0 && (
        <g className="rw-pills" pointerEvents={isEditor ? 'auto' : 'none'}>
          {pills.map((p) => {
            if (!p.label || typeof p.angle !== 'number') return null;
            // pathHalfDeg = the stroked path; the round caps extend capDeg
            // beyond each end (halfDeg = pathHalfDeg + capDeg is the full
            // extent the clearance rules use).
            const { r, pathHalfDeg, capDeg, tall, h, iconLen } = pillGeometry(p, pillBases[p.tier || 0]);
            const halfDeg = pathHalfDeg;
            const angle = normAngle(p.angle);
            const bottom = angle > 0 && angle < 180;
            const sweep = bottom ? 0 : 1;
            const a0 = bottom ? angle + halfDeg : angle - halfDeg;
            const a1 = bottom ? angle - halfDeg : angle + halfDeg;
            const arc = (rad) => {
              const [sx, sy] = polarToXY(CX, CY, rad, a0);
              const [ex, ey] = polarToXY(CX, CY, rad, a1);
              return `M ${sx} ${sy} A ${rad} ${rad} 0 0 ${sweep} ${ex} ${ey}`;
            };
            const [x0, y0] = polarToXY(CX, CY, r, a0);
            const [x1, y1] = polarToXY(CX, CY, r, a1);
            const d = arc(r);
            const Icon = p.icon ? (PILL_ICONS[p.icon] || Clock) : null;
            // Word-by-word along the arc (Rory 2026-09-21, third approach):
            // each word is a straight, kerned block rotated to the tangent at
            // its own centre; the words follow the curve. Rows are centred on
            // the pill's angle (the icon does NOT shift them: it hangs off the
            // run's left end and the body is widened by the icon on both
            // sides in pillGeometry). Reading direction is increasing angle
            // across the top and decreasing across the bottom.
            const { textW, captionW } = pillTextWidths(p);
            const rowW = Math.max(textW, captionW);
            // Compact: icon + text centred as one group (text shifts half an
            // icon right). Otherwise text on the pill centre, body balanced
            // by the icon on both sides. Mirrors pillGeometry's contentLen.
            const grouped = !!p.compact && iconLen > 0;
            const contentW = grouped ? iconLen + rowW : rowW + 2 * iconLen;
            const dir = bottom ? -1 : 1;
            const pxToDeg = (px) => (px / r) * 180 / Math.PI;
            const textCentreAngle = grouped ? angle + dir * pxToDeg(iconLen / 2) : angle;
            const rowOff = PILL.ROW_OFFSET;
            // Caption row is the visually higher arc: outer radius across
            // the top, inner across the bottom (glyph "up" is screen-up).
            const captionR = bottom ? r - rowOff : r + rowOff;
            const valueR = bottom ? r + rowOff : r - rowOff;
            const captionText = tall ? String(p.caption).toLocaleUpperCase() : '';
            const valueWords = layoutWordsOnArc(p.label, tall ? valueR : r, textCentreAngle, dir, false);
            const captionWords = tall ? layoutWordsOnArc(captionText, captionR, textCentreAngle, dir, true) : [];
            const tangentRot = (a) => (bottom ? a - 90 : a + 90);
            // Icon: just left of the text run, at the pill's centre radius.
            const iconAngle = textCentreAngle - dir * pxToDeg(rowW / 2 + PILL.ICON_GAP + PILL.ICON / 2);
            const [iconX, iconY] = polarToXY(CX, CY, r, iconAngle);
            const isDragging = draggingPillId === p.id;
            return (
              <g
                key={p.id}
                className={`rw-pill${isDragging ? ' rw-pill--dragging' : ''}${tall ? ' rw-pill--tall' : ''}`}
                onPointerDown={isEditor && onPillPointerDown ? (e) => onPillPointerDown(p, e) : undefined}
              >
                {PILL_SHAPE === 'wedge' ? (
                  // Full extent (path span + what the round caps used to add)
                  // so the content padding and the clearance maths are the
                  // same for both shapes.
                  <path
                    d={wedgePath(CX, CY, r - h / 2, r + h / 2, angle - (halfDeg + capDeg), angle + (halfDeg + capDeg))}
                    className="rw-pill-wedge"
                  />
                ) : (
                  <>
                    <path d={d} className="rw-pill-outline" strokeWidth={h + 2} />
                    <path d={d} className="rw-pill-body" strokeWidth={h} />
                  </>
                )}
                {Icon && (
                  <g transform={`translate(${iconX} ${iconY}) rotate(${tangentRot(iconAngle)})`} pointerEvents="none">
                    <Icon
                      x={-PILL.ICON / 2} y={-PILL.ICON / 2}
                      width={PILL.ICON} height={PILL.ICON}
                      strokeWidth={2.2}
                      className="rw-pill-icon"
                      aria-hidden="true"
                    />
                  </g>
                )}
                {captionWords.map((wd, i) => {
                  const [wx, wy] = polarToXY(CX, CY, captionR, wd.angle);
                  return (
                    <text
                      key={`c${i}`}
                      className="rw-pill-caption"
                      textAnchor="middle"
                      dominantBaseline="central"
                      transform={`translate(${wx} ${wy}) rotate(${tangentRot(wd.angle)})`}
                      pointerEvents="none"
                    >{wd.text}</text>
                  );
                })}
                {valueWords.map((wd, i) => {
                  const [wx, wy] = polarToXY(CX, CY, tall ? valueR : r, wd.angle);
                  return (
                    <text
                      key={`v${i}`}
                      className="rw-pill-text"
                      textAnchor="middle"
                      dominantBaseline="central"
                      transform={`translate(${wx} ${wy}) rotate(${tangentRot(wd.angle)})`}
                      pointerEvents="none"
                    >{wd.text}</text>
                  );
                })}
                {/* Resize handles (editor). End caps: drag along the arc for
                    full <-> compact; a thin accent trim hugs each cap on
                    hover. Outside edge: drag away from the wheel for two
                    rows, toward it for one; a trim runs along that edge on
                    hover. RadialEditorView owns the pointer maths; it gets
                    the pill's centre and which handle was grabbed. */}
                {isEditor && onPillHandlePointerDown && (() => {
                  const [mx, my] = polarToXY(CX, CY, r, angle);
                  const capR = h / 2;
                  // Outward unit vector at each cap (along the arc, away
                  // from the body), as a polar heading in degrees.
                  const capTrim = (ex, ey, theta, outwardSign) => {
                    const heading = theta + 90 * outwardSign; // tangent direction, +90 = increasing angle
                    const R = capR + 1.5;
                    const [p1x, p1y] = [ex + R * Math.cos(deg2rad(heading - 90)), ey + R * Math.sin(deg2rad(heading - 90))];
                    const [p2x, p2y] = [ex + R * Math.cos(deg2rad(heading + 90)), ey + R * Math.sin(deg2rad(heading + 90))];
                    return `M ${p1x} ${p1y} A ${R} ${R} 0 0 1 ${p2x} ${p2y}`;
                  };
                  const sign0 = bottom ? 1 : -1; // cap at a0 points toward decreasing angle on top pills
                  const sign1 = -sign0;
                  const edgeR = r + capR;          // outside edge (away from the wheel)
                  const edgeInset = Math.min(halfDeg, capDeg * (r / edgeR));
                  const e0 = bottom ? a0 - edgeInset : a0 + edgeInset;
                  const e1 = bottom ? a1 + edgeInset : a1 - edgeInset;
                  const edgePath = (rad) => {
                    const [sx, sy] = polarToXY(CX, CY, rad, e0);
                    const [ex, ey] = polarToXY(CX, CY, rad, e1);
                    return `M ${sx} ${sy} A ${rad} ${rad} 0 0 ${sweep} ${ex} ${ey}`;
                  };
                  const down = (mode) => (e) => onPillHandlePointerDown(p, { x: mx, y: my }, e, mode);
                  if (PILL_SHAPE === 'wedge') {
                    // Wedge: the trims are the two radial sides and the
                    // outer arc, spanning the full extent.
                    const w0 = angle - (halfDeg + capDeg);
                    const w1 = angle + (halfDeg + capDeg);
                    const side = (a) => {
                      const [ix0, iy0] = polarToXY(CX, CY, r - capR - 1.5, a);
                      const [ox0, oy0] = polarToXY(CX, CY, r + capR + 1.5, a);
                      return `M ${ix0} ${iy0} L ${ox0} ${oy0}`;
                    };
                    const outer = (rad) => {
                      const [sx, sy] = polarToXY(CX, CY, rad, w0);
                      const [ex, ey] = polarToXY(CX, CY, rad, w1);
                      return `M ${sx} ${sy} A ${rad} ${rad} 0 0 1 ${ex} ${ey}`;
                    };
                    const [s0x, s0y] = polarToXY(CX, CY, r, w0);
                    const [s1x, s1y] = polarToXY(CX, CY, r, w1);
                    return (
                      <>
                        <path d={side(w0)} className="rw-pill-cap-trim" />
                        <path d={side(w1)} className="rw-pill-cap-trim" />
                        <path d={outer(edgeR + 1.5)} className="rw-pill-edge-trim" />
                        <circle cx={s0x} cy={s0y} r={capR} className="rw-pill-handle rw-pill-handle--cap" onPointerDown={down('width')} />
                        <circle cx={s1x} cy={s1y} r={capR} className="rw-pill-handle rw-pill-handle--cap" onPointerDown={down('width')} />
                        <path d={outer(edgeR - 2)} className="rw-pill-handle rw-pill-handle--edge" strokeWidth={10} onPointerDown={down('height')} />
                      </>
                    );
                  }
                  return (
                    <>
                      <path d={capTrim(x0, y0, a0, sign0)} className="rw-pill-cap-trim" />
                      <path d={capTrim(x1, y1, a1, sign1)} className="rw-pill-cap-trim" />
                      <path d={edgePath(edgeR + 1.5)} className="rw-pill-edge-trim" />
                      <circle cx={x0} cy={y0} r={capR} className="rw-pill-handle rw-pill-handle--cap" onPointerDown={down('width')} />
                      <circle cx={x1} cy={y1} r={capR} className="rw-pill-handle rw-pill-handle--cap" onPointerDown={down('width')} />
                      <path d={edgePath(edgeR - 2)} className="rw-pill-handle rw-pill-handle--edge" strokeWidth={10} onPointerDown={down('height')} />
                    </>
                  );
                })()}
              </g>
            );
          })}
          {/* Drop zones while a pill is being dragged (editor): one faint
              dashed band per ring at the dragged pill's height, the ring it
              currently sits in drawn stronger, so it is obvious a pill can
              be stacked above the others. */}
          {isEditor && draggingPillId && (() => {
            const dragged = pills.find(p => p.id === draggingPillId);
            if (!dragged) return null;
            const h = dragged.caption ? PILL.H_TALL : PILL.H;
            const donut = (inner, outer) => [
              `M ${CX + outer} ${CY}`,
              `A ${outer} ${outer} 0 1 1 ${CX - outer} ${CY}`,
              `A ${outer} ${outer} 0 1 1 ${CX + outer} ${CY}`,
              `M ${CX + inner} ${CY}`,
              `A ${inner} ${inner} 0 1 0 ${CX - inner} ${CY}`,
              `A ${inner} ${inner} 0 1 0 ${CX + inner} ${CY}`,
              'Z',
            ].join(' ');
            return (
              <g className="rw-pill-drop-zones" pointerEvents="none">
                {Array.from({ length: MAX_TIER + 1 }, (_, t) => (
                  <path
                    key={t}
                    d={donut(pillBases[t], pillBases[t] + h)}
                    fillRule="evenodd"
                    className={`rw-pill-drop-band${(dragged.tier || 0) === t ? ' is-active' : ''}`}
                  />
                ))}
              </g>
            );
          })()}
        </g>
      )}
    </svg>
  );

  // Wrap in DndContext for editor mode drag-to-reorder (skip if parent owns DndContext)
  if (isEditor && !externalDnd && (onReorder || onReorderChildren)) {
    return (
      <DndContext sensors={sensors} onDragEnd={handleDragEnd}>
        <SortableContext items={[...innerSortableIds, ...outerSortableIds]}>
          <div className="rw-container" style={scale !== 1 ? { width: Math.round(525 * scale), height: Math.round(525 * scale) } : undefined}>
            {svgContent}
          </div>
        </SortableContext>
      </DndContext>
    );
  }

  return (
    <div className="rw-container" style={scale !== 1 ? { width: Math.round(525 * scale), height: Math.round(525 * scale) } : undefined}>
      {svgContent}
    </div>
  );
}

export { MAX_SLOTS, CX, CY, INNER_R, OUTER_R, OUTER_INNER_R, OUTER_OUTER_R, polarToXY };
