import React, { useState, useRef, useCallback, useEffect, useLayoutEffect, useMemo } from 'react';
import {
  Info, ChevronDown, Pencil, Trash2, Plus, Check,
  Clock, CalendarDays, CalendarRange, BatteryMedium, Cpu, MemoryStick, Volume2, Keyboard, AppWindow, Layers,
} from 'lucide-react';
import {
  WIDGET_TYPES, WIDGET_GROUPS, ANGLE_PRESETS, MAX_WIDGETS, DEFAULT_ANGLE, TIME_ZONES, UTC_OFFSETS,
  normaliseWidgets, newWidgetId, resolvePills, hasLiveWidget, normaliseTimeZone, displayTimeZone,
  pillGeometry, pillFitsAt, freeAngle, normAngle, angleDiff, tierBases, MAX_TIER, PILL,
} from './radialWidgets';

const WIDGET_ICONS = {
  clock: Clock, date: CalendarDays, week: CalendarRange, battery: BatteryMedium,
  cpu: Cpu, ram: MemoryStick, volume: Volume2, locks: Keyboard, app: AppWindow, profile: Layers,
};

// Typeable time zone for extra clocks: native suggestions (datalist) over
// the IANA list plus UTC offsets; accepts "UTC+2" / "+05:30" too. Saves the
// canonical id only once the text resolves, so a half-typed name never lands
// in config; invalid text is flagged and left in the field.
function TimeZoneField({ id, value, onChange }) {
  const [text, setText] = useState(() => displayTimeZone(value));
  const lastValue = useRef(value);
  useEffect(() => {
    if (value !== lastValue.current) {
      lastValue.current = value;
      setText(displayTimeZone(value));
    }
  }, [value]);
  const resolved = text.trim() ? normaliseTimeZone(text) : '';
  const invalid = !!text.trim() && !resolved;
  const commit = (t) => {
    const next = t.trim() ? normaliseTimeZone(t) : '';
    if (next === null) return;
    if (next !== value) { lastValue.current = next; onChange(next); }
  };
  return (
    <label className="rev-widget-field">
      <span>Time zone</span>
      <input
        className={`rev-widget-select rev-widget-input${invalid ? ' is-invalid' : ''}`}
        type="text"
        list={id}
        placeholder="This PC"
        value={text}
        spellCheck={false}
        title={invalid ? 'Not a known time zone. Try a city (Europe/Paris) or an offset (UTC+2).' : 'City or UTC offset; leave empty for this PC'}
        onChange={e => { setText(e.target.value); commit(e.target.value); }}
        onBlur={e => { if (!e.target.value.trim()) commit(''); }}
      />
      <datalist id={id}>
        {UTC_OFFSETS.map(z => <option key={z} value={z} />)}
        {TIME_ZONES.map(z => <option key={z} value={z.replace(/_/g, ' ')} />)}
      </datalist>
    </label>
  );
}
import RadialWheel, { CX, CY, MAX_SLOTS, OUTER_INNER_R, OUTER_OUTER_R, polarToXY } from './RadialWheel';
import { friendlyKeyName } from './keyboardLayout';
import './RadialEditorView.css';
import { SearchBar } from './SearchBar';
import ColourPicker from './ColourPicker.jsx';

// Lazy — IconPicker drags in the full lucide-react + simple-icons libraries
// (~5.9MB of JS). Loading it on first picker open keeps that out of the main
// window's startup bundle. See iconUtils.jsx.
const IconPicker = React.lazy(() => import('./IconPicker'));

// Use the same radii as the live overlay (INNER_R=80, OUTER_R=130) for WYSIWYG.
// The editor scales the wheel up via CSS to fill more space.
const EDITOR_INNER_R = 55;
const EDITOR_OUTER_R = 105;


// ── Layout switcher (Pro, per-device wheels) ───────────────────────────────
// ONE selector: the layout it shows is both the one being edited and the one
// this device's radial hotkey opens (a pick persists as this machine's
// choice). Extra layouts sync with the config; the choice is machine-local.
// Free tier: one chip showing "Default" + PRO badge → upgrade prompt.
function RadialLayoutSwitcher({
  layouts = [], editingId = 'default', isPro = false,
  onSelect, onCreate, onRename, onDelete, onShowUpgrade,
}) {
  const [open, setOpen] = useState(false);
  const [renamingId, setRenamingId] = useState(null);
  const [confirmDeleteId, setConfirmDeleteId] = useState(null);
  const [creating, setCreating] = useState(false);
  const [draftName, setDraftName] = useState('');
  const rootRef = useRef(null);

  const all = [{ id: 'default', name: 'Default' }, ...layouts];
  const current = all.find(l => l.id === editingId) || all[0];

  const close = useCallback(() => {
    setOpen(false);
    setRenamingId(null);
    setConfirmDeleteId(null);
    setCreating(false);
    setDraftName('');
  }, []);

  useEffect(() => {
    if (!open) return;
    const onDown = (e) => {
      if (rootRef.current && !rootRef.current.contains(e.target)) close();
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open, close]);

  if (!isPro) {
    return (
      <button
        type="button"
        className="rev-layout-chip"
        onClick={() => onShowUpgrade?.('Radial layouts per device')}
        title="Pro: keep a different wheel on each device that shares this config"
      >
        <span className="rev-layout-chip-prefix">Active Radial</span>
        <span className="rev-layout-chip-name">Default</span>
        <ChevronDown size={14} />
      </button>
    );
  }

  const commitRename = () => {
    const n = draftName.trim();
    if (renamingId && n) onRename?.(renamingId, n);
    setRenamingId(null);
    setDraftName('');
  };
  const commitCreate = (duplicate) => {
    onCreate?.(draftName.trim(), duplicate);
    close();
  };

  return (
    <div className="rev-layout-switcher" ref={rootRef}>
      <button
        type="button"
        className={`rev-layout-chip${open ? ' is-open' : ''}`}
        onClick={() => (open ? close() : setOpen(true))}
        title="The radial layout this device opens and you are editing here"
      >
        <span className="rev-layout-chip-prefix">Active Radial</span>
        <span className="rev-layout-chip-name">{current.name}</span>
        <ChevronDown size={14} />
      </button>
      {open && (
        <div className="rev-layout-menu" role="menu">
          {all.map(l => {
            const isEditing = l.id === current.id;
            if (renamingId === l.id) {
              return (
                <div key={l.id} className="rev-layout-row is-editing">
                  <input
                    className="rev-layout-input"
                    autoFocus
                    value={draftName}
                    onChange={e => setDraftName(e.target.value)}
                    onKeyDown={e => {
                      if (e.key === 'Enter') commitRename();
                      if (e.key === 'Escape') { e.stopPropagation(); setRenamingId(null); setDraftName(''); }
                    }}
                    onBlur={commitRename}
                  />
                </div>
              );
            }
            if (confirmDeleteId === l.id) {
              return (
                <div key={l.id} className="rev-layout-row is-confirm">
                  <span className="rev-layout-row-name">Delete "{l.name}"?</span>
                  <button type="button" className="rev-layout-mini rev-layout-mini-danger" onClick={() => { onDelete?.(l.id); setConfirmDeleteId(null); }}>Delete</button>
                  <button type="button" className="rev-layout-mini" onClick={() => setConfirmDeleteId(null)}>Cancel</button>
                </div>
              );
            }
            return (
              <div
                key={l.id}
                className={`rev-layout-row${isEditing ? ' is-selected' : ''}`}
                role="menuitem"
                onClick={() => { onSelect?.(l.id); close(); }}
              >
                <span className="rev-layout-row-check">{isEditing && <Check size={12} />}</span>
                <span className="rev-layout-row-name">{l.name}</span>
                {l.id !== 'default' && (
                  <span className="rev-layout-row-actions">
                    <button type="button" className="rev-layout-icon-btn" title="Rename" onClick={e => { e.stopPropagation(); setDraftName(l.name); setRenamingId(l.id); }}><Pencil size={11} /></button>
                    <button type="button" className="rev-layout-icon-btn" title="Delete layout" onClick={e => { e.stopPropagation(); setConfirmDeleteId(l.id); }}><Trash2 size={11} /></button>
                  </span>
                )}
              </div>
            );
          })}
          <div className="rev-layout-menu-sep" />
          {creating ? (
            <div className="rev-layout-row is-editing">
              <input
                className="rev-layout-input"
                autoFocus
                placeholder="Layout name"
                value={draftName}
                onChange={e => setDraftName(e.target.value)}
                onKeyDown={e => {
                  if (e.key === 'Enter') commitCreate(false);
                  if (e.key === 'Escape') { e.stopPropagation(); setCreating(false); setDraftName(''); }
                }}
              />
              <button type="button" className="rev-layout-mini" onClick={() => commitCreate(false)} title="Start from an empty wheel">Blank</button>
              <button type="button" className="rev-layout-mini" onClick={() => commitCreate(true)} title={`Start from a copy of "${current.name}"`}>Copy current</button>
            </div>
          ) : (
            <div className="rev-layout-row rev-layout-row-new" role="menuitem" onClick={() => { setDraftName(''); setCreating(true); }}>
              <span className="rev-layout-row-check"><Plus size={12} /></span>
              <span className="rev-layout-row-name">New layout</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

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
  // Name of the global profile whose wheel the live overlay shows while
  // THIS (app-specific) profile's wheel is empty; null when not applicable.
  wheelFallbackProfile  = null,
  onCopyRadialSegmentToProfile,
  onForceOverwriteRadialSegment,
  hiddenTips            = [],
  onHideTip,
  radialLayouts         = [],
  editingRadialLayoutId = 'default',
  deviceRadialLayoutId  = 'default',
  onSelectRadialLayout,
  onCreateRadialLayout,
  onRenameRadialLayout,
  onDeleteRadialLayout,
  isPro                 = false,
  onShowUpgrade,
  // Widget pills (Pro): top-level config list, see radialWidgets.js.
  radialWidgets         = [],
  onSetRadialWidgets,
}) {
  const [capturingKey, setCapturingKey] = useState(false);

  // ── Widget pills preview ─────────────────────────────────────────────
  // The editor wheel shows the same pills the overlay will, resolved from
  // the live clock and the machine facts (battery, app, volume, locks, CPU,
  // RAM). Facts refresh every 2 s while a live widget is on, else every 30 s.
  const widgets = useMemo(() => normaliseWidgets(radialWidgets), [radialWidgets]);
  const [facts, setFacts] = useState(null);
  const [now, setNow] = useState(() => Date.now());
  const liveWidgets = hasLiveWidget(widgets);
  useEffect(() => {
    if (widgets.length === 0) return undefined;
    let cancelled = false;
    const sample = () => {
      window.electronAPI?.getRadialWidgetFacts?.()
        .then((f) => { if (!cancelled && f) setFacts(f); })
        .catch(() => {});
    };
    sample();
    const id = setInterval(() => { sample(); setNow(Date.now()); }, liveWidgets ? 2000 : 30000);
    return () => { cancelled = true; clearInterval(id); };
  }, [widgets.length, liveWidgets]);
  // Drag a pill to another slot: pointer angle around the wheel centre snaps
  // to the nearest slot no other widget holds; the pill previews there live
  // and the move commits on release.
  const [pillDrag, setPillDrag] = useState(null); // { id, slot }
  const [widgetsOpen, setWidgetsOpen] = useState(false); // drawer, session-only

  // ── Fit the wheel to the zone height ─────────────────────────────────
  // See .rev-editor in RadialEditorView.css. Measures the wheel zone and its
  // other children (tips, layouts bar) and scales the 525px wheel to fit.
  const zoneRef = useRef(null);
  const [wheelScale, setWheelScale] = useState(1);
  useEffect(() => {
    const zone = zoneRef.current;
    if (!zone || typeof ResizeObserver === 'undefined') return undefined;
    const WHEEL_PX = 525;
    const GAP_PX = 14; // .rev-wheel-zone gap
    const measure = () => {
      const kids = Array.from(zone.children);
      let others = 0;
      for (const k of kids) {
        if (k.classList.contains('rev-editor')) continue;
        others += k.offsetHeight;
      }
      const available = zone.clientHeight - others - GAP_PX * Math.max(0, kids.length - 1);
      const next = Math.max(0.5, Math.min(1, available / WHEEL_PX));
      setWheelScale(prev => (Math.abs(prev - next) > 0.005 ? next : prev));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(zone);
    for (const k of zone.children) ro.observe(k);
    return () => ro.disconnect();
  }, [radialMenuHotkey, hiddenTips, isPro]);
  // Drag an end cap to resize, on two axes. Along the arc (toward / away
  // from the pill's centre) switches full <-> compact; away from / toward
  // the WHEEL centre switches one row <-> two rows (tall). The dominant
  // axis past a 10px threshold wins; previews live, commits on release.
  const [pillResize, setPillResize] = useState(null); // { id, mode, width, rows, width0, rows0, start, centre, wheel }
  const pillResizeRef = useRef(null);
  pillResizeRef.current = pillResize;
  const handlePillHandlePointerDown = useCallback((pill, centreVb, e, mode = 'width') => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const el = wheelRef?.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const k = rect.width / 420;
    const centre = { x: rect.left + centreVb.x * k, y: rect.top + centreVb.y * k };
    const wheel = { x: rect.left + rect.width / 2, y: rect.top + rect.height / 2 };
    const w = widgets.find(x => x.id === pill.id);
    const width = w?.width === 'compact' ? 'compact' : 'full';
    const rows = w?.rows === 2 ? 2 : 1;
    setPillResize({ id: pill.id, mode, width, rows, width0: width, rows0: rows, start: { x: e.clientX, y: e.clientY }, centre, wheel });
  }, [wheelRef, widgets]);
  useEffect(() => {
    if (!pillResize) return undefined;
    const dist = (a, b) => Math.hypot(a.x - b.x, a.y - b.y);
    const onMove = (e) => {
      const rs = pillResizeRef.current;
      if (!rs) return;
      const here = { x: e.clientX, y: e.clientY };
      const along = dist(here, rs.centre) - dist(rs.start, rs.centre);   // + = away from the pill centre
      const radial = dist(here, rs.wheel) - dist(rs.start, rs.wheel);    // + = away from the wheel
      // Each handle drives ONE axis: caps change width only, the outside
      // edge changes rows only; neither ever touches the other setting.
      let width = rs.width0;
      let rows = rs.rows0;
      if (rs.mode === 'height') {
        if (radial > 10) rows = 2;
        else if (radial < -10) rows = 1;
      } else if (Math.abs(along) > 10) {
        width = along < 0 ? 'compact' : 'full';
      }
      if (width !== rs.width || rows !== rs.rows) setPillResize({ ...rs, width, rows });
    };
    const onUp = () => {
      const rs = pillResizeRef.current;
      if (rs) {
        const current = widgets.find(w => w.id === rs.id);
        if (current && (current.width !== rs.width || current.rows !== rs.rows)) updateWidget(rs.id, { width: rs.width, rows: rs.rows });
      }
      setPillResize(null);
    };
    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
    document.addEventListener('pointercancel', onUp);
    return () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      document.removeEventListener('pointercancel', onUp);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [!!pillResize, widgets]);
  const pillDragRef = useRef(null);
  pillDragRef.current = pillDrag;
  const handlePillPointerDown = useCallback((pill, e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    setPillDrag({ id: pill.id, angle: pill.angle, tier: pill.tier || 0 });
  }, []);
  useEffect(() => {
    if (!pillDrag) return undefined;
    const onMove = (e) => {
      const el = wheelRef?.current;
      const drag = pillDragRef.current;
      if (!el || !drag) return;
      const rect = el.getBoundingClientRect();
      const cx = rect.left + rect.width / 2;
      const cy = rect.top + rect.height / 2;
      // Snapped to 15-degree steps so pills land on tidy, repeatable spots.
      const raw = Math.atan2(e.clientY - cy, e.clientX - cx) * 180 / Math.PI; // SVG degrees
      const angle = normAngle(Math.round(raw / 15) * 15);
      // Clearance: the pill follows the pointer while it keeps MIN_GAP_PX
      // from every other pill, and stops against a neighbour otherwise.
      const me = pillsRef.current.find(p => p.id === drag.id);
      if (!me) return;
      // Ring from the pointer's distance to the wheel centre (viewBox units):
      // past the outer ring's inner edge (less half the gap) = ring 1.
      const k = rect.width / 420;
      const dist = Math.hypot(e.clientX - cx, e.clientY - cy) / k;
      const bases = tierBases(pillsRef.current);
      const tier = Math.min(MAX_TIER, dist >= bases[1] - PILL.TIER_GAP / 2 ? 1 : 0);
      const geom = pillGeometry(me, bases[tier]);
      const others = pillsRef.current
        .filter(p => p.id !== drag.id && (p.tier || 0) === tier)
        .map(p => ({ angle: p.angle, geom: pillGeometry(p, bases[tier]) }));
      if (!pillFitsAt(angle, geom, others)) return;
      if (angleDiff(angle, drag.angle) > 0.25 || tier !== drag.tier) setPillDrag({ id: drag.id, angle, tier });
    };
    const onUp = () => {
      const drag = pillDragRef.current;
      if (drag) {
        const current = widgets.find(w => w.id === drag.id);
        if (current && (angleDiff(current.angle, drag.angle) > 0.25 || (current.tier || 0) !== drag.tier)) {
          updateWidget(drag.id, { angle: Math.round(drag.angle * 10) / 10, tier: drag.tier });
        }
      }
      setPillDrag(null);
    };
    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
    document.addEventListener('pointercancel', onUp);
    return () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      document.removeEventListener('pointercancel', onUp);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [!!pillDrag, widgets]);
  const pills = useMemo(() => {
    if (!isPro) return [];
    let list = widgets;
    if (pillDrag) list = list.map(w => (w.id === pillDrag.id ? { ...w, angle: pillDrag.angle, tier: pillDrag.tier } : w));
    if (pillResize) list = list.map(w => (w.id === pillResize.id ? { ...w, width: pillResize.width, rows: pillResize.rows } : w));
    return resolvePills(list, facts, now, { showMissing: true, profile: activeProfile });
  }, [widgets, facts, now, isPro, pillDrag, pillResize, activeProfile]);
  // Resolved pills with their current (possibly previewed) angles, for the
  // drag handlers' clearance checks.
  const pillsRef = useRef([]);
  pillsRef.current = pills;
  // Where a NEW pill of `type` would be seated: the preset nearest its
  // default angle where it clears every existing pill; null = no room.
  const seatFor = (type) => {
    const probe = resolvePills([{ id: '__probe', type, angle: DEFAULT_ANGLE[type] ?? -90 }], facts, now, { showMissing: true, profile: activeProfile })[0];
    const bases = tierBases(pills);
    for (let tier = 0; tier <= MAX_TIER; tier++) {
      const geom = pillGeometry(probe || { label: 'CPU 100%' }, bases[tier]);
      const others = pills.filter(p => (p.tier || 0) === tier).map(p => ({ angle: p.angle, geom: pillGeometry(p, bases[tier]) }));
      const angle = freeAngle(geom, others, DEFAULT_ANGLE[type] ?? -90);
      if (angle != null) return { angle, tier };
    }
    return null;
  };
  // Drawer cards: one collapsible row per widget entry (plus one "off" row
  // per unused type). Only one row's settings are open at a time.
  const [expandedWidgetId, setExpandedWidgetId] = useState(null);
  const roomLeft = widgets.length < MAX_WIDGETS;
  const addWidget = (type) => {
    if (!isPro) { onShowUpgrade?.('Radial widgets'); return; }
    if (!roomLeft) return;
    const seat = seatFor(type);
    if (!seat) return;
    const id = newWidgetId(type, widgets);
    onSetRadialWidgets?.([...widgets, { id, type, angle: seat.angle, tier: seat.tier }]);
    setExpandedWidgetId(id);
  };
  const removeWidget = (id) => {
    onSetRadialWidgets?.(widgets.filter(w => w.id !== id));
    if (expandedWidgetId === id) setExpandedWidgetId(null);
  };
  const updateWidget = (id, patch) => {
    let next = widgets.map(w => (w.id === id ? { ...w, ...patch } : w));
    // Anything that can change a pill's width may push neighbours apart at
    // render time (resolvePills -> resolveOverlaps). Write those pushed
    // angles back so the stored config matches what is on screen.
    const reflows = ['width', 'rows', 'showIcon', 'label', 'format', 'mode', 'hour12', 'timeZone', 'angle', 'tier'];
    if (Object.keys(patch).some(k => reflows.includes(k))) {
      const resolved = resolvePills(next, facts, now, { showMissing: true, profile: activeProfile });
      const byId = new Map(resolved.map(p => [p.id, p.angle]));
      next = next.map(w => {
        const a = byId.get(w.id);
        return typeof a === 'number' && angleDiff(a, w.angle) > 0.05 ? { ...w, angle: a } : w;
      });
    }
    onSetRadialWidgets?.(next);
  };
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

      {/* Empty app-profile wheel: its own row between the hold-to-select row
          and the wheel. Inside .rev-wheel-zone (a centred column) a second
          tip pushed the fixed-size wheel up over the toggle row. */}
      {radialMenuHotkey && wheelFallbackProfile && radialMenuItems.every((item) => !item) && (
        <div className="rev-fallback-row">
          <div className="rev-tip">
            <Info size={14} strokeWidth={2} aria-hidden="true" />
            <span>This wheel is empty, so the {wheelFallbackProfile} wheel shows in this app. Add an action here to use a wheel of its own.</span>
          </div>
        </div>
      )}

      {/* Wheel editor area */}
      {!radialMenuHotkey ? (
        <div className="rev-empty-state">
          <span className="rmp-empty-icon">{'\u25ce'}</span>
          <p>Set a hotkey above to enable the radial menu.</p>
        </div>
      ) : (
        <div className="rev-wheel-zone" ref={zoneRef}>
          {!hiddenTips.includes('radial-info') && (
            <div className="rev-tip">
              <Info size={14} strokeWidth={2} aria-hidden="true" />
              <span>Assign actions to the 8 segments, or create a folder by right clicking on the segment to nest actions.</span>
              <button type="button" className="rev-tip-close" title="Hide this tip (restore in Settings)" aria-label="Hide this tip" onClick={() => onHideTip?.('radial-info')}>&#10005;</button>
            </div>
          )}
          <div
            className="rev-editor"
            ref={wheelRef}
            style={{
              // Both axes: localHitTestFull maps pointer -> viewBox through
              // this rect, so it must be the scaled square the wheel paints.
              // RadialWheel sizes its container to the same square (viewBox
              // scaling, no CSS transform), so the two always agree.
              width: Math.round(525 * wheelScale),
              height: Math.round(525 * wheelScale),
            }}
            onClick={() => { setPopover(null); setCtxMenu(null); }}
          >
            <RadialWheel
              mode="editor"
              externalDnd={true}
              items={radialMenuItems}
              scale={wheelScale}
              pills={pills}
              onPillPointerDown={handlePillPointerDown}
              onPillHandlePointerDown={handlePillHandlePointerDown}
              draggingPillId={pillDrag?.id || null}
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
                      // Unassigned library entries are hidden here, mirroring
                      // the sidebar's radial-mode exclusion — a stored
                      // reference would dangle once the entry is bound.
                      if (key.includes('::UNASSIGNED::')) continue;
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
          {isPro && !hiddenTips.includes('radial-layouts') && (
            <div className="rev-tip rev-layouts-tip">
              <Info size={14} strokeWidth={2} aria-hidden="true" />
              <span>Pick a layout to make it this device's active radial and edit it here. Other devices sharing your config keep their own.</span>
              <button type="button" className="rev-tip-close" title="Hide this tip (restore in Settings)" aria-label="Hide this tip" onClick={() => onHideTip?.('radial-layouts')}>&#10005;</button>
            </div>
          )}
          {/* Wheel layouts (Pro): which arrangement this device fires. Sits
              under the wheel, apart from the hotkey / hold settings above. */}
          <div className="rev-layouts-bar">
            <div className="rev-layouts-text">
              <span className="rev-layouts-label">
                Wheel layouts
                {!isPro && <span className="pro-badge">PRO</span>}
              </span>
              <span className="rev-layouts-hint">
                {isPro
                  ? 'The wheel this device opens. Same actions, a different arrangement per device.'
                  : 'A different wheel on each device that shares your config.'}
              </span>
            </div>
            <RadialLayoutSwitcher
              layouts={radialLayouts}
              editingId={editingRadialLayoutId}
              isPro={isPro}
              onSelect={onSelectRadialLayout}
              onCreate={onCreateRadialLayout}
              onRename={onRenameRadialLayout}
              onDelete={onDeleteRadialLayout}
              onShowUpgrade={onShowUpgrade}
            />
          </div>

        </div>
      )}

      {/* Widget pills (Pro): a collapsible drawer on the panel's left edge,
          i.e. sticking off the profile sidebar, with a vertical "Widgets"
          tab. Lives in the drawer rather than under the wheel: a bar there
          overflowed the panel on short windows and overlapped the legend. */}
      {radialMenuHotkey && (
        <div className={`rev-widgets-drawer${widgetsOpen ? ' is-open' : ''}`}>
          <div className="rev-widgets-panel" aria-hidden={!widgetsOpen}>
            <div className="rev-widgets-head">
              <span className="rev-layouts-label">
                Widgets
                {!isPro && <span className="pro-badge">PRO</span>}
              </span>
              <span className="rev-layouts-hint">
                {isPro
                  ? 'Information pills around the wheel. Drag one to move it. Drag an end cap along the arc to shrink or widen it, or away from the wheel for two rows. They step aside while a folder is open.'
                  : 'Clock, date, battery, CPU, memory and more, as pills around your wheel.'}
              </span>
            </div>
            {WIDGET_GROUPS.map((group) => (
              <div key={group.id} className="rev-widget-group">
                <div className="rev-widget-group-title">{group.label}</div>
                {WIDGET_TYPES.filter(m => m.group === group.id).map((meta) => {
                  const entries = widgets.filter(w => w.type === meta.type);
                  const Icon = WIDGET_ICONS[meta.type] || Clock;
                  const rows = entries.length > 0 ? entries : [null];
                  return (
                    <React.Fragment key={meta.type}>
                      {rows.map((entry, i) => {
                        const on = isPro && !!entry;
                        const rowId = entry ? entry.id : `off-${meta.type}`;
                        const expanded = on && expandedWidgetId === rowId;
                        const canAdd = isPro ? (roomLeft && seatFor(meta.type) != null) : true;
                        const name = meta.multi && entries.length > 1 ? `${meta.label} ${i + 1}` : meta.label;
                        return (
                          <div key={rowId} className={`rev-widget-card${on ? ' is-on' : ''}${expanded ? ' is-open' : ''}`}>
                            <div className="rev-widget-card-head">
                              <Icon size={14} strokeWidth={2} aria-hidden="true" />
                              {on ? (
                                <button
                                  type="button"
                                  className="rev-widget-card-name rev-widget-card-expand"
                                  aria-expanded={expanded}
                                  onClick={() => setExpandedWidgetId(expanded ? null : rowId)}
                                  title={expanded ? 'Hide settings' : 'Show settings'}
                                >
                                  <span>{name}</span>
                                  <ChevronDown size={13} strokeWidth={2} aria-hidden="true" className="rev-widget-card-chevron" />
                                </button>
                              ) : (
                                <span className="rev-widget-card-name">{name}</span>
                              )}
                              <button
                                type="button"
                                role="switch"
                                aria-checked={on}
                                aria-label={`${name} pill`}
                                className={`rev-holdselect-toggle${on ? ' on' : ''}`}
                                disabled={!on && !canAdd}
                                onClick={() => (on ? removeWidget(entry.id) : addWidget(meta.type))}
                                title={on
                                  ? `Remove the ${name.toLowerCase()} pill`
                                  : canAdd ? `Add a ${meta.label.toLowerCase()} pill` : 'No room left around the wheel'}
                              />
                            </div>
                            {!on && meta.hint && <span className="rev-widget-card-hint">{meta.hint}</span>}
                            {expanded && (
                              <div className="rev-widget-card-opts">
                                <label className="rev-widget-field">
                                  <span>Position</span>
                                  {(() => {
                                    const me = pills.find(p => p.id === entry.id);
                                    const bases = tierBases(pills);
                                    const tier = entry.tier || 0;
                                    const geom = pillGeometry(me || { label: 'CPU 100%' }, bases[tier]);
                                    const others = pills
                                      .filter(p => p.id !== entry.id && (p.tier || 0) === tier)
                                      .map(p => ({ angle: p.angle, geom: pillGeometry(p, bases[tier]) }));
                                    const preset = ANGLE_PRESETS.find(a => angleDiff(a.angle, entry.angle) < 0.5);
                                    return (
                                      <select
                                        className="rev-widget-select"
                                        value={preset ? preset.id : 'custom'}
                                        onChange={e => {
                                          const a = ANGLE_PRESETS.find(x => x.id === e.target.value);
                                          if (a) updateWidget(entry.id, { angle: a.angle });
                                        }}
                                      >
                                        {!preset && <option value="custom" disabled>Custom (dragged)</option>}
                                        {ANGLE_PRESETS.map(a => (
                                          <option key={a.id} value={a.id} disabled={!pillFitsAt(a.angle, geom, others)}>{a.label}</option>
                                        ))}
                                      </select>
                                    );
                                  })()}
                                </label>
                                <label className="rev-widget-field">
                                  <span>Ring</span>
                                  <select
                                    className="rev-widget-select"
                                    value={entry.tier === 1 ? '1' : '0'}
                                    onChange={e => updateWidget(entry.id, { tier: e.target.value === '1' ? 1 : 0 })}
                                  >
                                    <option value="0">Inner (next to the wheel)</option>
                                    <option value="1">Outer (stacked above)</option>
                                  </select>
                                </label>
                                <label className="rev-widget-field">
                                  <span>Width</span>
                                  <select
                                    className="rev-widget-select"
                                    value={entry.width === 'compact' ? 'compact' : 'full'}
                                    onChange={e => updateWidget(entry.id, { width: e.target.value })}
                                  >
                                    <option value="full">Full (word + value)</option>
                                    <option value="compact">Compact (value only)</option>
                                  </select>
                                </label>
                                <label className="rev-widget-field">
                                  <span>Rows</span>
                                  <select
                                    className="rev-widget-select"
                                    value={entry.rows === 2 ? '2' : '1'}
                                    onChange={e => updateWidget(entry.id, { rows: e.target.value === '2' ? 2 : 1 })}
                                  >
                                    <option value="1">One row</option>
                                    <option value="2">Two rows (caption above value)</option>
                                  </select>
                                </label>
                                <label className="rev-widget-field">
                                  <span>Icon</span>
                                  <select
                                    className="rev-widget-select"
                                    value={entry.showIcon === false ? 'hidden' : 'shown'}
                                    onChange={e => updateWidget(entry.id, { showIcon: e.target.value === 'shown' })}
                                  >
                                    <option value="shown">Shown</option>
                                    <option value="hidden">Hidden</option>
                                  </select>
                                </label>
                                {meta.type === 'clock' && (
                                  <>
                                    <label className="rev-widget-field">
                                      <span>Format</span>
                                      <select
                                        className="rev-widget-select"
                                        value={entry.hour12 ? '12' : '24'}
                                        onChange={e => updateWidget(entry.id, { hour12: e.target.value === '12' })}
                                      >
                                        <option value="24">24-hour</option>
                                        <option value="12">12-hour</option>
                                      </select>
                                    </label>
                                    <TimeZoneField
                                      id={`tz-${entry.id}`}
                                      value={entry.timeZone || ''}
                                      onChange={tz => updateWidget(entry.id, { timeZone: tz || undefined })}
                                    />
                                    <label className="rev-widget-field">
                                      <span>Label</span>
                                      <input
                                        className="rev-widget-select rev-widget-input"
                                        type="text"
                                        maxLength={8}
                                        placeholder="e.g. NYC"
                                        value={entry.label || ''}
                                        onChange={e => updateWidget(entry.id, { label: e.target.value })}
                                      />
                                    </label>
                                  </>
                                )}
                                {meta.type === 'date' && (
                                  <label className="rev-widget-field">
                                    <span>Format</span>
                                    <select
                                      className="rev-widget-select"
                                      value={entry.format === 'long' ? 'long' : 'short'}
                                      onChange={e => updateWidget(entry.id, { format: e.target.value })}
                                    >
                                      <option value="short">Short (Mon 21 Sep)</option>
                                      <option value="long">Long (Monday 21 September)</option>
                                    </select>
                                  </label>
                                )}
                                {meta.type === 'ram' && (
                                  <label className="rev-widget-field">
                                    <span>Show</span>
                                    <select
                                      className="rev-widget-select"
                                      value={entry.mode === 'gb' ? 'gb' : 'pct'}
                                      onChange={e => updateWidget(entry.id, { mode: e.target.value })}
                                    >
                                      <option value="pct">Percent used</option>
                                      <option value="gb">Used / total GB</option>
                                    </select>
                                  </label>
                                )}
                              </div>
                            )}
                          </div>
                        );
                      })}
                      {meta.multi && isPro && entries.length > 0 && roomLeft && seatFor(meta.type) != null && (
                        <button type="button" className="rev-widget-add" onClick={() => addWidget(meta.type)}>
                          <Plus size={12} strokeWidth={2} aria-hidden="true" />
                          <span>Add another {meta.label.toLowerCase()}</span>
                        </button>
                      )}
                    </React.Fragment>
                  );
                })}
              </div>
            ))}
          </div>
          <button
            type="button"
            className="rev-widgets-tab"
            aria-expanded={widgetsOpen}
            onClick={() => setWidgetsOpen(o => !o)}
            title={widgetsOpen ? 'Hide widgets' : 'Show widgets'}
          >
            <span>Widgets</span>
            {!isPro && <span className="pro-badge">PRO</span>}
          </button>
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
                const item = ctxMenu.item;
                setCtxMenu(null);
                // Folder removal already confirms via its popover; plain
                // segments removed here instantly with no undo.
                const what = item?.label ? `"${item.label}"` : 'this segment';
                if (!window.confirm(`Remove ${what} from the wheel?`)) return;
                onRemoveRadialMenuItem?.(item.id);
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
          {/* Colour picker row — uses the shared component so the palette
              and custom-colour picker stay identical to Text Expansions and
              Search Templates. Radial uses '' (empty string) as its "default
              type colour" sentinel, so pass noneHex="" to preserve the wire. */}
          <div className="rev-color-row">
            <span className="rev-color-label">Colour</span>
            <ColourPicker
              value={iconPicker.currentColor || ''}
              noneHex=""
              onChange={(c) => {
                if (iconPicker.childId) {
                  onSetRadialChildIcon?.(iconPicker.folderId, iconPicker.childId, undefined, c);
                } else {
                  onSetRadialMenuItemIcon?.(iconPicker.itemId, undefined, c);
                }
                setIconPicker(p => ({ ...p, currentColor: c }));
              }}
            />
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
