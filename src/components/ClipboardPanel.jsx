import React, { useState, useEffect, useLayoutEffect, useRef, useCallback } from 'react';
import ReactDOM from 'react-dom';
import { Pin, PinOff, Bookmark, Folder, ChevronDown, ChevronRight, Link2, Maximize2, Clipboard, Square, Columns2, LayoutGrid } from 'lucide-react';
import { DndContext, PointerSensor, useSensor, useSensors, useDroppable } from '@dnd-kit/core';
import { SortableContext, arrayMove, useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { friendlyKeyName } from './keyboardLayout';
import './ClipboardPanel.css';
import ZoomableImage from './ZoomableImage';
import './ZoomableImage.css';
import { SearchBar } from './SearchBar';
import { findPresetIconForUrl } from '../utils/presetIcons';

// ── Lazy image thumbnail ────────────────────────────────────────────────────

function ImageThumb({ id, className, zoomable }) {
  const [src, setSrc] = useState(null);
  useEffect(() => {
    let cancelled = false;
    window.electronAPI?.getClipboardImage(id).then(b64 => {
      if (!cancelled && b64) setSrc(`data:image/png;base64,${b64}`);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [id]);
  if (!src) {
    return (
      <div className="cbg-img-ph">
        <svg width="28" height="28" viewBox="0 0 32 32" fill="none" stroke="currentColor" strokeWidth="1">
          <rect x="2" y="4" width="28" height="24" rx="3"/>
          <circle cx="10" cy="12" r="3"/>
          <path d="M2 24l8-8 4 4 6-6 10 10"/>
        </svg>
      </div>
    );
  }
  if (zoomable) return <ZoomableImage src={src} className={className} />;
  return <img className={className} src={src} alt="" />;
}

const ALL_TAGS = ['All', 'Text', 'Image', 'Number', 'Link', 'Email', 'Colour'];

// ── Local-date helpers (used by the date-bucket sidebar) ──────────────────
// SQLite returns `DATE(timestamp, 'localtime')` as 'YYYY-MM-DD'. We mirror
// that format on the JS side so item.timestamp → localDateKey can be matched
// against a sidebar selection without timezone drift.
// Per [[feedback_sqlite_localtime_pattern]] both sides must use local time.

function localDateKeyFromDate(d) {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

function itemLocalDateKey(item) {
  try { return localDateKeyFromDate(new Date(item.timestamp)); }
  catch { return ''; }
}

function formatDateSidebarLabel(dateKey, todayKey) {
  if (dateKey === todayKey) return 'Today';
  // Compute yesterday's key from today's key (string math via Date)
  const t = new Date(todayKey + 'T00:00');
  const y = new Date(t); y.setDate(t.getDate() - 1);
  if (dateKey === localDateKeyFromDate(y)) return 'Yesterday';
  const d = new Date(dateKey + 'T00:00');
  const month = d.toLocaleString(undefined, { month: 'short' });
  const day = d.getDate();
  if (d.getFullYear() !== t.getFullYear()) return `${day} ${month} ${d.getFullYear()}`;
  return `${day} ${month}`;
}

// ── Sortable card wrapper (Saved + Pinned tiers in Main UI) ───────────────
// Drag-to-reorder via dnd-kit. PointerSensor 5px activation distance keeps
// click-to-select snappy — only a real drag triggers reorder. Listeners go
// on the card root so the whole tile is the drag handle (no separate gutter).
function SortableCardWrap({ sortableId, className, onClick, onContextMenu, dataId, children }) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: sortableId });
  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : 1,
    zIndex: isDragging ? 20 : undefined,
  };
  return (
    <div
      ref={setNodeRef}
      style={style}
      {...attributes}
      {...listeners}
      className={className}
      data-clip-id={dataId}
      onClick={onClick}
      onContextMenu={onContextMenu}
    >
      {children}
    </div>
  );
}

// ── Folder drop zone (Saved section headers + empty-folder areas) ─────────
// Generic droppable wrapper for dnd-kit. Dropping a saved card on one of
// these moves the card into the folder encoded in dropId ("folderdrop-root",
// "folderdrop-<id>", "folderdrop-<id>-empty") — see handleDragEnd.
function FolderDropZone({ dropId, className, onClick, onContextMenu, ariaExpanded, title, children }) {
  const { isOver, setNodeRef } = useDroppable({ id: dropId });
  return (
    <div
      ref={setNodeRef}
      className={`${className}${isOver ? ' cbg-drop-over' : ''}`}
      onClick={onClick}
      onContextMenu={onContextMenu}
      title={title}
      // Collapse headers are real toggle controls: focusable, Enter/Space
      // activated, state exposed via aria-expanded.
      role={onClick ? 'button' : undefined}
      tabIndex={onClick ? 0 : undefined}
      aria-expanded={ariaExpanded}
      onKeyDown={onClick ? (e) => {
        // stopPropagation keeps the document-level Enter-to-copy shortcut
        // from also firing while a header has focus.
        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); onClick(); }
      } : undefined}
    >
      {children}
    </div>
  );
}

// ── Search-term highlighting ────────────────────────────────────────────────
// Wraps every case-insensitive occurrence of `needle` in <mark> so search
// results show WHY they matched. Returns the plain string when no search is
// active or nothing matches (zero render cost on the common path).
function highlightMatches(text, needle) {
  if (!needle) return text;
  const lower = text.toLowerCase();
  const n = needle.toLowerCase();
  if (!lower.includes(n)) return text;
  const parts = [];
  let i = 0;
  for (;;) {
    const idx = lower.indexOf(n, i);
    if (idx < 0) { parts.push(text.slice(i)); break; }
    if (idx > i) parts.push(text.slice(i, idx));
    parts.push(<mark key={idx} className="cbg-hl">{text.slice(idx, idx + n.length)}</mark>);
    i = idx + n.length;
  }
  return parts;
}

// ── Timeline grouping ──────────────────────────────────────────────────────

function groupByTimeline(items) {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterdayStart = new Date(todayStart); yesterdayStart.setDate(todayStart.getDate() - 1);
  const weekStart = new Date(todayStart); weekStart.setDate(todayStart.getDate() - todayStart.getDay());
  const monthStart = new Date(now.getFullYear(), now.getMonth(), 1);

  // Saved (internally still `starred`) sits above Pinned. An item that's BOTH
  // saved and pinned shows only under Saved (the higher tier) — the popup,
  // which ignores starred, still treats it as pinned via its own ORDER BY.
  // The Saved group holds root AND foldered items; the render splits them.
  const groups = { Saved: [], Pinned: [], Today: [], Yesterday: [], 'This Week': [], 'This Month': [], Older: [] };

  for (const item of items) {
    if (item.starred) { groups.Saved.push(item); continue; }
    if (item.pinned) { groups.Pinned.push(item); continue; }
    const d = new Date(item.timestamp);
    if (d >= todayStart) groups.Today.push(item);
    else if (d >= yesterdayStart) groups.Yesterday.push(item);
    else if (d >= weekStart) groups['This Week'].push(item);
    else if (d >= monthStart) groups['This Month'].push(item);
    else groups.Older.push(item);
  }

  return Object.entries(groups).filter(([, arr]) => arr.length > 0);
}

function formatStorageSize(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

// ── Colour parser/formatter helpers (shared by ColourPane) ──────────────────
// Handles #RRGGBB, #RGB, rgb(...), rgba(...) — all formats classified as
// 'Colour' by clipboard.rs:auto_tag(). Returns null if the input is anything else.
function parseColourValue(input) {
  if (!input) return null;
  const t = input.trim();
  let m = /^#([0-9a-fA-F]{6})(?:[0-9a-fA-F]{2})?$/.exec(t);
  if (m) {
    const hex = m[1].toLowerCase();
    return {
      r: parseInt(hex.substring(0, 2), 16),
      g: parseInt(hex.substring(2, 4), 16),
      b: parseInt(hex.substring(4, 6), 16),
    };
  }
  m = /^#([0-9a-fA-F]{3})$/.exec(t);
  if (m) {
    const h = m[1].toLowerCase();
    return {
      r: parseInt(h[0] + h[0], 16),
      g: parseInt(h[1] + h[1], 16),
      b: parseInt(h[2] + h[2], 16),
    };
  }
  m = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)(?:\s*,\s*[\d.]+)?\s*\)$/i.exec(t);
  if (m) {
    return { r: +m[1], g: +m[2], b: +m[3] };
  }
  return null;
}

function rgbToHexUpper(r, g, b) {
  return '#' + [r, g, b].map(v => v.toString(16).padStart(2, '0')).join('').toUpperCase();
}

function rgbToHsl(r, g, b) {
  r /= 255; g /= 255; b /= 255;
  const max = Math.max(r, g, b), min = Math.min(r, g, b);
  let h = 0, s = 0; const l = (max + min) / 2;
  if (max !== min) {
    const d = max - min;
    s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
    if (max === r)      h = (g - b) / d + (g < b ? 6 : 0);
    else if (max === g) h = (b - r) / d + 2;
    else                h = (r - g) / d + 4;
    h /= 6;
  }
  return { h: Math.round(h * 360), s: Math.round(s * 100), l: Math.round(l * 100) };
}

const safeCopy = async (text) => { try { await navigator.clipboard?.writeText(text); } catch {} };

// ── Type-specific preview panes (Stage E) ───────────────────────────────────

function LinkPane({ url, onCopyToast }) {
  const iconFile = findPresetIconForUrl(url);
  return (
    <div className="cbg-pane-link">
      <div className="cbg-pane-link-head">
        {iconFile
          ? <img className="cbg-pane-icon" src={`/preset-icons/${iconFile}`} alt="" draggable={false} onError={e => { e.currentTarget.style.display = 'none'; }} />
          : <span className="cbg-pane-icon-fallback" aria-hidden="true"><Link2 size={16} strokeWidth={1.75} /></span>
        }
        <div className="cbg-pane-link-url" title={url}>{url}</div>
      </div>
      <div className="cbg-pane-actions">
        <button className="cbg-dbtn cbg-dbtn-paste" type="button" onClick={() => window.electronAPI?.openExternal(url)}>Open</button>
        <button className="cbg-dbtn" type="button" onClick={() => { safeCopy(url); onCopyToast?.('url'); }}>Copy</button>
      </div>
    </div>
  );
}

function EmailPane({ email, onCopyToast }) {
  return (
    <div className="cbg-pane-email">
      <div className="cbg-pane-email-addr" title={email}>{email}</div>
      <div className="cbg-pane-actions">
        <button className="cbg-dbtn cbg-dbtn-paste" type="button" onClick={() => window.electronAPI?.openExternal(`mailto:${email}`)}>Mailto</button>
        <button className="cbg-dbtn" type="button" onClick={() => { safeCopy(email); onCopyToast?.('email'); }}>Copy</button>
      </div>
    </div>
  );
}

function ColourPane({ value }) {
  const parsed = parseColourValue(value);
  if (!parsed) {
    // Unparseable input — fall back to plain text view so we never lose content.
    return <pre className="cbg-detail-text">{value}</pre>;
  }
  const hex = rgbToHexUpper(parsed.r, parsed.g, parsed.b);
  const rgb = `rgb(${parsed.r}, ${parsed.g}, ${parsed.b})`;
  const { h, s, l } = rgbToHsl(parsed.r, parsed.g, parsed.b);
  const hsl = `hsl(${h}, ${s}%, ${l}%)`;
  return (
    <div className="cbg-pane-colour">
      <div className="cbg-pane-colour-swatch" style={{ background: hex }} aria-label={`Colour swatch ${hex}`} />
      <div className="cbg-pane-colour-rows">
        {[
          { label: 'Hex', value: hex },
          { label: 'RGB', value: rgb },
          { label: 'HSL', value: hsl },
        ].map(row => (
          <div className="cbg-pane-colour-row" key={row.label}>
            <span className="cbg-pane-colour-label">{row.label}</span>
            <code className="cbg-pane-colour-value">{row.value}</code>
            <button className="cbg-dbtn" type="button" onClick={() => safeCopy(row.value)}>Copy</button>
          </div>
        ))}
      </div>
    </div>
  );
}

// Reflow OCR text into continuous paragraphs: blank-line paragraph breaks
// survive, single line breaks join with a space, and hyphenated line-end word
// splits rejoin ("exam-\nple" -> "example").
function reflowParagraphs(text) {
  return text
    .split(/\n{2,}/)
    .map(p => p.replace(/-\n([a-z])/g, '$1').replace(/\n/g, ' '))
    .join('\n\n');
}

export default function ClipboardPanel({ previewWidth = 480, onChangePreviewWidth, onCreateExpansion, clipboardPasteHotkey = 'Ctrl+Shift+V', hiddenTips = [], onHideTip, columnMode = 'auto', onChangeColumnMode, isPro = false, onShowUpgrade }) {
  const [items, setItems] = useState([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(false);
  const [search, setSearch] = useState('');
  const [ctxMenu, setCtxMenu] = useState(null);
  const [clearConfirm, setClearConfirm] = useState(false);
  const [sourceApps, setSourceApps] = useState([]);
  const [filterApp, setFilterApp] = useState('');
  const [filterTag, setFilterTag] = useState('All');
  // Date sidebar state. selectedDate: 'all' (timeline grouping) | 'starred'
  // (all saved items) | 'pinned' (all pinned items) | 'folder-<id>' (one
  // Saved folder) | 'YYYY-MM-DD' (single local date). Internal value stays
  // 'starred' — only UI strings say "Saved".
  const [dateBuckets, setDateBuckets] = useState({ dates: [], pinned_count: 0, starred_count: 0 });
  const [selectedDate, setSelectedDate] = useState('all');
  // Saved folders — [{ id, name, count }] from the backend, count = saved
  // items in the folder. Refreshed after every folder/item mutation.
  const [folders, setFolders] = useState([]);
  const [creatingFolder, setCreatingFolder] = useState(false);
  const [newFolderName, setNewFolderName] = useState('');
  const [renamingFolderId, setRenamingFolderId] = useState(null);
  const [renameText, setRenameText] = useState('');
  // Collapsed section keys — click any group header to hide/show its items.
  // One set holds string section labels ('Saved', 'Pinned', 'Today', ...) and
  // numeric folder ids; the types can't collide through JSON round-trips.
  // Persisted in localStorage (UI-only preference, like trigr_list_view).
  const [collapsedSections, setCollapsedSections] = useState(() => {
    try { return new Set(JSON.parse(localStorage.getItem('trigr_clip_sections_collapsed') || '[]')); }
    catch { return new Set(); }
  });
  const toggleSectionCollapsed = (key) => {
    setCollapsedSections(prev => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key); else next.add(key);
      try { localStorage.setItem('trigr_clip_sections_collapsed', JSON.stringify([...next])); } catch {}
      return next;
    });
  };
  // Re-derived once a minute so the "Today" / "Yesterday" labels refresh on
  // midnight rollover without requiring the user to reopen the panel.
  const [todayKey, setTodayKey] = useState(() => localDateKeyFromDate(new Date()));
  const [selectedId, setSelectedId] = useState(null);
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState('');
  const [storageSize, setStorageSize] = useState(null);
  // Live width during drag — null when not resizing. Persisted width comes from props.
  const [dragWidth, setDragWidth] = useState(null);
  const effectivePreviewWidth = dragWidth ?? previewWidth;
  // Image-pane state — OCR result, loading/error, dominant colours, zoom toggle
  const [ocrText, setOcrText]         = useState(null);
  const [ocrLoading, setOcrLoading]   = useState(false);
  const [ocrError, setOcrError]       = useState(null);
  const [imageColors, setImageColors] = useState([]);
  // Lightbox state — replaces the previous in-pane fit/zoom toggle. Click the
  // image in the detail pane to open a full-screen overlay with ZoomableImage
  // (wheel zoom + drag pan). Close on ESC, backdrop click, or X.
  const [lightboxOpen, setLightboxOpen] = useState(false);
  const [lightboxSrc, setLightboxSrc] = useState(null);
  const [copyToast, setCopyToast]     = useState(null); // for swatch hex-copy feedback
  // Ctrl+click multi-selection (bulk save / move / delete). Independent of
  // selectedId — the detail pane keeps single-item semantics.
  const [multiSel, setMultiSel] = useState(new Set());
  // Bottom-centre feedback toast for quick actions (double-click copy etc.)
  const [actionToast, setActionToast] = useState(null);
  // Most recent clipboard-new-item id — drives the card arrival animation.
  const [newItemId, setNewItemId] = useState(null);
  // 'starred' | 'pinned' while a card drag is live. Saved drags light up every
  // folder drop zone; any drag relaxes the collapse-wrapper overflow clipping.
  const [dragState, setDragState] = useState(null);
  const ctxRef = useRef(null);
  const gridRef = useRef(null);
  const toastTimerRef = useRef(null);
  // Spring-loaded folders: hovering a drag over a collapsed folder header for
  // 600ms auto-expands it (Explorer-style).
  const springRef = useRef({ key: null, timer: null });

  // Resize-handle drag tracking
  const startResize = useCallback((e) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = previewWidth;
    let lastW = startW;
    const onMove = (ev) => {
      // Handle is on the LEFT edge — moving left grows the pane (further from grid).
      const delta = startX - ev.clientX;
      lastW = Math.max(320, Math.min(1200, startW + delta));
      setDragWidth(lastW);
    };
    const onUp = () => {
      document.removeEventListener('mousemove', onMove);
      document.removeEventListener('mouseup', onUp);
      setDragWidth(null);
      onChangePreviewWidth?.(lastW);
    };
    document.addEventListener('mousemove', onMove);
    document.addEventListener('mouseup', onUp);
  }, [previewWidth, onChangePreviewWidth]);

  const PER_PAGE = 50;

  // Filter refs so pagination (handleScroll) can read the current toolbar
  // filter values without re-creating loadHistory whenever they change.
  const filtersRef = useRef({ date: 'all', app: '', tag: 'All', search: '' });

  const loadHistory = useCallback(async (p = 1, append = false, overrideFilters) => {
    setLoading(true);
    try {
      const f = overrideFilters || filtersRef.current;
      const filters = {
        // A folder view fetches ALL saved rows ('starred' backend keyword) and
        // narrows to the folder client-side — the backend filter vocabulary
        // stays untouched and every foldered row is saved by definition.
        dateFilter: f.date === 'all' ? null : (f.date.startsWith('folder-') ? 'starred' : f.date),
        appFilter: f.app || null,
        tagFilter: f.tag && f.tag !== 'All' ? f.tag : null,
        search: f.search?.trim() || null,
        // Main UI: promote starred items above pinned. Popup omits this flag.
        promoteStarred: true,
      };
      const result = await window.electronAPI?.getClipboardHistory(p, PER_PAGE, filters);
      if (result) {
        setItems(prev => append ? [...prev, ...result.items] : result.items);
        setTotal(result.total);
        setPage(p);
      }
    } catch (e) { /* ignore */ }
    setLoading(false);
  }, []);

  // Filter-aware: when the app/tag toolbar filter is set, the sidebar date
  // counts (and the Pinned count) reflect items matching that filter. Empty
  // dates drop out via SQL GROUP BY, so the sidebar hides dates with 0 hits
  // for the current filter automatically.
  const loadDateBuckets = useCallback((overrideFilters) => {
    const f = overrideFilters || filtersRef.current;
    const filters = {
      appFilter: f.app || null,
      tagFilter: f.tag && f.tag !== 'All' ? f.tag : null,
    };
    window.electronAPI?.getClipboardDateBuckets?.(filters).then(b => {
      if (b) setDateBuckets(b);
    });
  }, []);

  const loadFolders = useCallback(() => {
    window.electronAPI?.getClipboardFolders?.().then(f => {
      if (Array.isArray(f)) setFolders(f);
    });
  }, []);

  useEffect(() => {
    // One-time mount side effects (source apps, storage size, date buckets).
    // History loading is owned by the selectedDate effect below — it fires on
    // mount for the initial 'all' view AND on every sidebar bucket change.
    window.electronAPI?.getDistinctSourceApps?.().then(apps => {
      if (apps) setSourceApps(apps);
    });
    window.electronAPI?.getClipboardStorageSize?.().then(size => {
      if (size != null) setStorageSize(size);
    });
    loadDateBuckets();
    loadFolders();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Reload from the backend whenever ANY toolbar filter changes — sidebar
  // bucket, app dropdown, tag pill, or search input. Without backend filtering
  // the client-side checks would only see the loaded page, missing matches in
  // unscrolled history. Search input is debounced ~200ms so each keystroke
  // doesn't fire its own SQL query.
  useEffect(() => {
    const timer = setTimeout(() => {
      const next = { date: selectedDate, app: filterApp, tag: filterTag, search };
      filtersRef.current = next;
      loadHistory(1, false, next);
      // Sidebar bucket counts mirror the toolbar filters so dates with 0 hits
      // for the current app/tag drop out. Search isn't applied to buckets
      // (decrypt-and-scan would be expensive per keystroke).
      loadDateBuckets(next);
    }, search ? 200 : 0);
    return () => clearTimeout(timer);
    // loadHistory / loadDateBuckets are stable; we re-fire on any filter change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedDate, filterApp, filterTag, search]);

  // Midnight rollover: tick every minute, refresh buckets + Today label on
  // local date change. Counts otherwise stay accurate via the new-item handler.
  useEffect(() => {
    const interval = setInterval(() => {
      const k = localDateKeyFromDate(new Date());
      setTodayKey(prev => {
        if (prev !== k) {
          loadDateBuckets();
          return k;
        }
        return prev;
      });
    }, 60000);
    return () => clearInterval(interval);
  }, [loadDateBuckets]);

  useEffect(() => {
    window.electronAPI?.onClipboardNewItem((item) => {
      setItems(prev => [item, ...prev]);
      setTotal(t => t + 1);
      // Arrival animation — cleared after the keyframe finishes so reloads
      // and re-renders never re-trigger it.
      setNewItemId(item.id);
      setTimeout(() => setNewItemId(prev => prev === item.id ? null : prev), 700);
      // Functional setter so the closure doesn't capture a stale sourceApps —
      // lets this effect run once on mount instead of re-registering the
      // listener every time sourceApps mutates (which raced with the async
      // listen() registration and produced duplicate visual rows on copy).
      if (item.source_app) {
        setSourceApps(prev => prev.includes(item.source_app) ? prev : [...prev, item.source_app].sort());
      }
      // Refresh sidebar bucket counts. Re-fetch (rather than incrementing
      // locally) because the new item may or may not match the active app/tag
      // filter — a single SQL query is the simplest way to stay correct under
      // both filtered and unfiltered views.
      loadDateBuckets();
    });
    return () => window.electronAPI?.removeAllListeners('clipboard-new-item');
    // loadDateBuckets is a stable useCallback; deps must stay `[]` so the
    // listener registers exactly once per [[feedback_tauri_listener_registration_race]].
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Promote-on-use: a row was copied from the panel or pasted from the popup,
  // so its timestamp moved to now. Mirror the reorder locally — saved/pinned
  // items just take the new timestamp (their sections have their own order),
  // timeline items float to the front of the non-tier block.
  useEffect(() => {
    window.electronAPI?.onClipboardItemTouched?.(({ id, timestamp }) => {
      setItems(prev => {
        const item = prev.find(i => i.id === id);
        if (!item) return prev;
        const updated = { ...item, timestamp };
        if (item.starred || item.pinned) {
          return prev.map(i => (i.id === id ? updated : i));
        }
        const rest = prev.filter(i => i.id !== id);
        const firstLoose = rest.findIndex(i => !i.starred && !i.pinned);
        const at = firstLoose < 0 ? rest.length : firstLoose;
        return [...rest.slice(0, at), updated, ...rest.slice(at)];
      });
      // The item may have jumped date buckets (e.g. Older → Today).
      loadDateBuckets();
    });
    return () => window.electronAPI?.removeAllListeners('clipboard-item-touched');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Auto-OCR (Pro): the worker finished OCR on a row — fetch its ocr_text
  // and merge it into local state so search-inside-images picks it up
  // immediately without a full history reload.
  useEffect(() => {
    window.electronAPI?.onClipboardItemOcred?.(({ id, has_text }) => {
      if (!has_text) return;
      window.electronAPI?.getClipboardOcrText?.(id).then(text => {
        if (!text) return;
        setItems(prev => prev.map(i => i.id === id ? { ...i, ocr_text: text } : i));
      }).catch(() => {});
    });
    return () => window.electronAPI?.removeAllListeners('clipboard-item-ocred');
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!ctxMenu) return;
    const handler = (e) => {
      if (ctxRef.current && !ctxRef.current.contains(e.target)) setCtxMenu(null);
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [ctxMenu]);

  // Clamp the right-click context menu inside the viewport — raw clientX /
  // clientY overflow when right-clicking near the edge of the panel.
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
  }, [ctxMenu]);

  // Cancel edit when selection changes
  useEffect(() => {
    setEditing(false);
    setEditText('');
  }, [selectedId]);

  // Escape key: close lightbox first if open, then cancel edit, then deselect.
  useEffect(() => {
    function handleKeyDown(e) {
      if (e.key !== 'Escape') return;
      if (lightboxOpen) {
        setLightboxOpen(false);
        setLightboxSrc(null);
        e.stopPropagation();
        return;
      }
      if (editing) {
        setEditing(false);
        setEditText('');
      } else if (multiSel.size > 0) {
        setMultiSel(new Set());
      } else if (selectedId !== null) {
        setSelectedId(null);
      }
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [editing, selectedId, lightboxOpen, multiSel]);

  // ── Keyboard navigation over the card grid ─────────────────────────────
  // Arrows move the selection (Left/Right = document order, Up/Down =
  // geometric nearest card so multi-column grids and section boundaries
  // behave), Enter copies, Delete deletes (the multi-selection if one is
  // active). The listener registers once; per-render values reach it through
  // keyNavRef, which is populated AFTER the handlers are declared (see the
  // assignment further down) to avoid a temporal-dead-zone crash per
  // [[feedback_usecallback_dep_array_tdz]].
  const keyNavRef = useRef(null);
  useEffect(() => {
    const onKey = (e) => {
      const isCtrl = e.ctrlKey || e.metaKey;
      const handledPlain = ['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Enter', 'Delete'].includes(e.key);
      const handledCtrl = isCtrl && (e.key === 'c' || e.key === 'C' || e.key === 'a' || e.key === 'A');
      if (!handledPlain && !handledCtrl) return;
      const t = e.target;
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.tagName === 'SELECT' || t.isContentEditable)) return;
      const ctx = keyNavRef.current;
      if (!ctx) return;

      // Ctrl+C copies the selected card — but never hijack a real text
      // selection (e.g. copying part of the preview pane's text).
      if (isCtrl && (e.key === 'c' || e.key === 'C')) {
        const sel = window.getSelection();
        if (sel && !sel.isCollapsed) return;
        if (ctx.selectedId != null) { e.preventDefault(); ctx.copy(ctx.selectedId); }
        return;
      }
      // Ctrl+A multi-selects every visible card (respects filters + collapse).
      if (isCtrl && (e.key === 'a' || e.key === 'A')) {
        const ids = Array.from(gridRef.current?.querySelectorAll('[data-clip-id]') || [])
          .filter(el => el.getBoundingClientRect().height > 0)
          .map(el => Number(el.dataset.clipId));
        if (ids.length) { e.preventDefault(); ctx.selectAll(ids); }
        return;
      }

      if (e.key === 'Enter') {
        if (ctx.selectedId != null) { e.preventDefault(); ctx.copy(ctx.selectedId); }
        return;
      }
      if (e.key === 'Delete') {
        if (ctx.multiSel.size > 0) { e.preventDefault(); ctx.bulkDel([...ctx.multiSel]); }
        else if (ctx.selectedId != null) { e.preventDefault(); ctx.del(ctx.selectedId); }
        return;
      }

      // Visible cards only — collapsed sections keep their cards mounted at
      // zero height, so filter by rendered size.
      const els = Array.from(gridRef.current?.querySelectorAll('[data-clip-id]') || [])
        .filter(el => el.getBoundingClientRect().height > 0);
      if (!els.length) return;
      e.preventDefault();

      const cur = ctx.selectedId != null
        ? els.find(el => Number(el.dataset.clipId) === ctx.selectedId)
        : null;
      if (!cur) {
        const first = els[0];
        ctx.select(Number(first.dataset.clipId));
        first.scrollIntoView({ block: 'nearest' });
        return;
      }

      let target = null;
      if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
        const idx = els.indexOf(cur);
        target = els[idx + (e.key === 'ArrowRight' ? 1 : -1)] || null;
      } else {
        const cr = cur.getBoundingClientRect();
        const cx = cr.left + cr.width / 2;
        const cy = cr.top + cr.height / 2;
        let bestScore = Infinity;
        for (const el of els) {
          if (el === cur) continue;
          const r = el.getBoundingClientRect();
          const dx = (r.left + r.width / 2) - cx;
          const dy = (r.top + r.height / 2) - cy;
          if (e.key === 'ArrowDown' ? dy <= 4 : dy >= -4) continue;
          // Prefer the same column: vertical distance + doubled lateral drift.
          const score = Math.abs(dy) + Math.abs(dx) * 2;
          if (score < bestScore) { bestScore = score; target = el; }
        }
      }
      if (target) {
        ctx.select(Number(target.dataset.clipId));
        target.scrollIntoView({ block: 'nearest' });
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, []);

  // Copy a history item back onto the system clipboard. The user is expected to
  // switch to their target app and paste with Ctrl+V themselves — the in-place
  // paste path can't reliably focus the right window from this panel (WebView2
  // owns input here), and the popup overlay (Ctrl+Shift+V) remains the fast path.
  const handleCopy = async (id) => {
    await window.electronAPI?.copyClipboardItem(id);
  };

  const showActionToast = (msg) => {
    clearTimeout(toastTimerRef.current);
    setActionToast(msg);
    toastTimerRef.current = setTimeout(() => setActionToast(null), 1400);
  };

  // Quick-copy path — double-click on a card, Enter with a card selected, or
  // the Copy item on the right-click menu. The toast confirms the action
  // since copying has no other visible effect in the panel.
  const handleCopyWithToast = async (id) => {
    await handleCopy(id);
    showActionToast('Copied to clipboard');
    setCtxMenu(null);
  };

  // ── Bulk actions (Ctrl+click multi-selection) ───────────────────────────

  const bulkDelete = async (ids) => {
    for (const id of ids) {
      await window.electronAPI?.deleteClipboardItem(id);
    }
    const gone = new Set(ids);
    setItems(prev => prev.filter(i => !gone.has(i.id)));
    setTotal(t => Math.max(0, t - ids.length));
    if (gone.has(selectedId)) setSelectedId(null);
    setMultiSel(new Set());
    loadDateBuckets();
    loadFolders();
    setCtxMenu(null);
    showActionToast(`Deleted ${ids.length} items`);
  };

  const bulkSave = async (ids) => {
    for (const id of ids) {
      await window.electronAPI?.starClipboardItem(id, true);
    }
    const set = new Set(ids);
    setItems(prev => prev.map(i => set.has(i.id) ? { ...i, starred: true } : i));
    setMultiSel(new Set());
    loadDateBuckets();
    setCtxMenu(null);
    showActionToast(`Saved ${ids.length} items`);
  };

  const bulkMove = async (ids, folderId) => {
    for (const id of ids) {
      await window.electronAPI?.moveClipboardItemToFolder?.(id, folderId);
    }
    const set = new Set(ids);
    setItems(prev => prev.map(i => set.has(i.id)
      ? { ...i, folder_id: folderId, starred: folderId != null ? true : i.starred }
      : i));
    setMultiSel(new Set());
    loadFolders();
    loadDateBuckets();
    setCtxMenu(null);
    showActionToast(`Moved ${ids.length} items`);
  };

  // Legacy paste-with-Ctrl+V path retained for the transform pills below
  // (lowercase / UPPERCASE / Trimmed / Plain) — same WebView2 focus caveats apply,
  // tracked as follow-up.
  const handlePasteText = async (text, sourceId) => {
    if (!text) return;
    await window.electronAPI?.pasteText(text, sourceId ?? null);
    if (sourceId) {
      setItems(prev => prev.map(it => it.id === sourceId ? { ...it, paste_count: (it.paste_count || 0) + 1 } : it));
    }
  };

  // Plain transform — replaces fancy unicode (curly quotes, em-dash, ellipsis, NBSP) with
  // ASCII equivalents and strips zero-width characters. Useful for pasting copy from
  // word processors and web pages that smuggled in non-ASCII characters.
  const toPlainAscii = (s) => (s || '')
    .replace(/[‘’‚‛]/g, "'")     // curly singles → '
    .replace(/[“”„‟]/g, '"')     // curly doubles → "
    .replace(/[–—]/g, '-')                  // en/em dash → -
    .replace(/…/g, '...')                        // ellipsis → ...
    .replace(/ /g, ' ')                          // NBSP → space
    .replace(/[​-‍﻿]/g, '');           // zero-width chars → removed

  // Smart-action detection — strict matches against the trimmed full text.
  const detectSmartAction = (rawText) => {
    const t = (rawText || '').trim();
    if (!t) return { kind: null };
    if (/^https?:\/\/\S+$/.test(t)) return { kind: 'url', value: t };
    if (/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(t)) return { kind: 'email', value: t };
    // Phone: only digits/space/-/+/() characters, AND at least 7 digits total
    if (/^[\d\s\-+()]+$/.test(t)) {
      const digits = t.replace(/\D/g, '');
      if (digits.length >= 7) return { kind: 'phone', value: t, digits };
    }
    return { kind: null };
  };

  // Counts row helpers
  const countWords = (s) => (s || '').trim() ? (s.trim().match(/\S+/g) || []).length : 0;
  const countLines = (s) => (s || '').length === 0 ? 0 : (s.match(/\n/g) || []).length + 1;

  // ── Image preview helpers ──
  const rgbToHex = (r, g, b) =>
    '#' + [r, g, b].map(v => v.toString(16).padStart(2, '0')).join('').toUpperCase();

  const copyHexToClipboard = async (hex) => {
    try { await navigator.clipboard?.writeText(hex); setCopyToast(hex); setTimeout(() => setCopyToast(null), 1200); } catch {}
  };

  const handleRunOcr = async () => {
    if (!selectedId) return;
    // Pro gate: OCR (manual + auto + search-inside-images) is a Pro feature.
    // Route free users to the upgrade modal instead of hitting the backend,
    // which returns OCR_PRO_REQUIRED as belt-and-braces.
    if (!isPro) {
      onShowUpgrade?.('Text extraction from images (OCR)');
      return;
    }
    setOcrLoading(true);
    setOcrError(null);
    try {
      const text = await window.electronAPI?.ocrClipboardImage(selectedId);
      const value = text || '';
      setOcrText(value);
      // Mirror the cached value back into the items list so re-selecting the
      // same image in this session shows the text without re-running OCR
      // (Rust already persisted it via set_ocr_text).
      setItems(prev => prev.map(i => i.id === selectedId ? { ...i, ocr_text: value } : i));
    } catch (e) {
      // Sentinel from the backend Pro gate — surface the upgrade modal even
      // if we somehow reached the invoke (setting-drift, race with a licence
      // change mid-click).
      if (e === 'OCR_PRO_REQUIRED' || (typeof e === 'string' && e.includes('OCR_PRO_REQUIRED'))) {
        onShowUpgrade?.('Text extraction from images (OCR)');
        setOcrLoading(false);
        return;
      }
      setOcrError(typeof e === 'string' ? e : (e?.message || 'OCR failed'));
    } finally {
      setOcrLoading(false);
    }
  };

  // Reset image-pane state whenever selection changes. If the newly selected
  // image has cached OCR text from a previous extraction, restore it so the
  // user sees the text immediately (no need to click Extract again).
  // Use ?? not || so an empty-string OCR result (image with no readable text)
  // still counts as cached and doesn't prompt a re-extract.
  useEffect(() => {
    const sel = items.find(i => i.id === selectedId);
    setOcrText(sel?.ocr_text ?? null);
    setOcrError(null);
    setOcrLoading(false);
    setImageColors([]);
    setLightboxOpen(false);
    setLightboxSrc(null);
  }, [selectedId]);

  // Fetch dominant colours when an image is selected.
  useEffect(() => {
    const sel = items.find(i => i.id === selectedId);
    if (!sel || sel.content_type !== 'image') return;
    let cancelled = false;
    window.electronAPI?.getClipboardImageColors?.(selectedId).then(cols => {
      if (!cancelled && Array.isArray(cols)) setImageColors(cols);
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [selectedId, items]);

  const handleDelete = async (id) => {
    const ok = await window.electronAPI?.deleteClipboardItem(id);
    if (ok) {
      setItems(prev => prev.filter(i => i.id !== id));
      setTotal(t => t - 1);
      if (selectedId === id) setSelectedId(null);
      loadDateBuckets();
      loadFolders();
    }
    setCtxMenu(null);
  };

  const handlePin = async (id, pinned) => {
    const ok = await window.electronAPI?.pinClipboardItem(id, !pinned);
    if (ok) {
      // Unpin also clears pinned_order server-side; mirror that locally so the
      // next reload doesn't show stale rank state.
      setItems(prev => prev.map(i => i.id === id
        ? { ...i, pinned: !pinned, pinned_order: pinned ? null : i.pinned_order }
        : i));
      loadDateBuckets();
    }
    setCtxMenu(null);
  };

  const handleStar = async (id, starred) => {
    const ok = await window.electronAPI?.starClipboardItem(id, !starred);
    if (ok) {
      // Un-saving clears the rank AND folder assignment server-side; mirror
      // both locally so the next reload doesn't show stale state.
      setItems(prev => prev.map(i => i.id === id
        ? { ...i, starred: !starred, starred_order: starred ? null : i.starred_order, folder_id: starred ? null : i.folder_id }
        : i));
      loadDateBuckets();
      loadFolders();
    }
    setCtxMenu(null);
  };

  // ── Saved folder handlers ──────────────────────────────────────────────

  const handleCreateFolder = async () => {
    const name = newFolderName.trim();
    if (name) {
      const id = await window.electronAPI?.createClipboardFolder?.(name);
      if (id != null) loadFolders();
    }
    setCreatingFolder(false);
    setNewFolderName('');
  };

  const commitFolderRename = async () => {
    const name = renameText.trim();
    if (renamingFolderId != null && name) {
      const ok = await window.electronAPI?.renameClipboardFolder?.(renamingFolderId, name);
      if (ok) setFolders(prev => prev.map(f => f.id === renamingFolderId ? { ...f, name } : f));
    }
    setRenamingFolderId(null);
    setRenameText('');
  };

  // Deleting a folder moves its items back to the Saved root (backend does the
  // same transactionally) — folder deletion never deletes clipboard content.
  const handleDeleteFolder = async (id) => {
    const ok = await window.electronAPI?.deleteClipboardFolder?.(id);
    if (ok) {
      setFolders(prev => prev.filter(f => f.id !== id));
      setItems(prev => prev.map(i => i.folder_id === id ? { ...i, folder_id: null } : i));
      if (selectedDate === `folder-${id}`) setSelectedDate('all');
    }
    setCtxMenu(null);
  };

  const handleMoveToFolder = async (id, folderId) => {
    const ok = await window.electronAPI?.moveClipboardItemToFolder?.(id, folderId);
    if (ok) {
      // Moving into a folder also saves the item backend-side (a folder is by
      // definition inside the Saved tier) — mirror that locally.
      setItems(prev => prev.map(i => i.id === id
        ? { ...i, folder_id: folderId, starred: folderId != null ? true : i.starred }
        : i));
      loadFolders();
      loadDateBuckets();
    }
    setCtxMenu(null);
  };

  // Populated every render, read by the keyboard-nav document listener above.
  // Lives after the handler declarations — see the TDZ note at the effect.
  keyNavRef.current = {
    selectedId, multiSel,
    copy: handleCopyWithToast,
    del: handleDelete,
    bulkDel: bulkDelete,
    select: (id) => setSelectedId(id),
    selectAll: (ids) => setMultiSel(new Set(ids)),
  };

  // dnd-kit sensor mirrors the RadialWheel pattern — 5px activation distance
  // so a click on a card still selects the item (no accidental drags).
  const dndSensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 5 } })
  );

  // ── Drag lifecycle ──────────────────────────────────────────────────────
  // dragState drives two affordances while a card is in flight: saved drags
  // light up every folder drop zone (cbg-drop-eligible), and any drag relaxes
  // the collapse-wrapper overflow clipping so the card ghost isn't cut off at
  // its section boundary.

  const clearSpring = () => {
    clearTimeout(springRef.current.timer);
    springRef.current = { key: null, timer: null };
  };

  const handleDragStart = (event) => {
    const id = String(event.active.id);
    setDragState(id.startsWith('starred-') ? 'starred' : id.startsWith('pinned-') ? 'pinned' : null);
  };

  // Spring-loaded folders: linger over a collapsed folder's header for 600ms
  // mid-drag and it opens, so a card can be placed INSIDE without a separate
  // expand step first. Dropping on the closed header still works regardless.
  const handleDragOver = (event) => {
    const overId = event.over ? String(event.over.id) : null;
    const m = overId && /^folderdrop-(\d+)/.exec(overId);
    const fid = m ? Number(m[1]) : null;
    if (springRef.current.key === fid) return;
    clearSpring();
    springRef.current.key = fid;
    if (fid != null && collapsedSections.has(fid)) {
      springRef.current.timer = setTimeout(() => {
        setCollapsedSections(prev => {
          const next = new Set(prev);
          next.delete(fid);
          try { localStorage.setItem('trigr_clip_sections_collapsed', JSON.stringify([...next])); } catch {}
          return next;
        });
      }, 600);
    }
  };

  const handleDragCancel = () => {
    setDragState(null);
    clearSpring();
  };

  // Drag-end handler. Three outcomes:
  //   1. Saved card dropped on a folder drop zone (header / empty area /
  //      Saved-root header) → move it into that folder (or back to root).
  //   2. Saved card dropped on a card in a DIFFERENT folder group → join that
  //      card's folder — the intuitive "drop it next to those" gesture.
  //   3. Same-group drop → reorder, as before. Cross-tier (saved↔pinned)
  //      drags stay ignored — tier changes go via the Save/Pin toggles.
  // id format "starred-<n>" / "pinned-<n>" encodes the tier; drop zones use
  // "folderdrop-root" / "folderdrop-<id>" / "folderdrop-<id>-empty".
  const handleDragEnd = useCallback(async (event) => {
    setDragState(null);
    clearTimeout(springRef.current.timer);
    springRef.current = { key: null, timer: null };
    const { active, over } = event;
    if (!over || active.id === over.id) return;
    const [aTier, aIdStr] = String(active.id).split('-');
    const aItemId = Number(aIdStr);

    const moveTo = async (folderId) => {
      const item = items.find(i => i.id === aItemId);
      if (!item || (item.folder_id ?? null) === folderId) return;
      const ok = await window.electronAPI?.moveClipboardItemToFolder?.(aItemId, folderId);
      if (ok) {
        setItems(prev => prev.map(i => i.id === aItemId ? { ...i, folder_id: folderId } : i));
        loadFolders();
      }
    };

    const dropMatch = /^folderdrop-(root|\d+)/.exec(String(over.id));
    if (dropMatch) {
      if (aTier !== 'starred') return; // only saved cards file into folders
      await moveTo(dropMatch[1] === 'root' ? null : Number(dropMatch[1]));
      return;
    }

    const [oTier, oIdStr] = String(over.id).split('-');
    if (aTier !== oTier) return; // cross-tier drag not supported
    const tier = aTier;
    if (tier !== 'starred' && tier !== 'pinned') return;
    const oItemId = Number(oIdStr);

    if (tier === 'starred') {
      const activeItem = items.find(i => i.id === aItemId);
      const overItem = items.find(i => i.id === oItemId);
      if (!activeItem || !overItem) return;
      const aFolder = activeItem.folder_id ?? null;
      const oFolder = overItem.folder_id ?? null;
      if (aFolder !== oFolder) {
        await moveTo(oFolder);
        return;
      }
    }

    // Same-group reorder. Saved groups are scoped per folder — ranks are only
    // rewritten for the dragged group's ids, so relative order inside every
    // other folder is untouched (rank collisions across folders don't matter,
    // the UI always groups by folder before ordering).
    const activeFolder = items.find(i => i.id === aItemId)?.folder_id ?? null;
    const groupItems = items.filter(i => tier === 'starred'
      ? (i.starred && (i.folder_id ?? null) === activeFolder)
      : (i.pinned && !i.starred));
    const oldIdx = groupItems.findIndex(i => i.id === aItemId);
    const newIdx = groupItems.findIndex(i => i.id === oItemId);
    if (oldIdx < 0 || newIdx < 0) return;
    const reorderedIds = arrayMove(groupItems, oldIdx, newIdx).map(i => i.id);

    // Rebuild items state: reordered group first within its tier, then the
    // tier's other items, then the rest. Display order inside each folder
    // group is what matters; new ranks land in *_order on next reload.
    setItems(prev => {
      const byId = new Map(prev.map(i => [i.id, i]));
      const groupSet = new Set(reorderedIds);
      const starredIds = tier === 'starred'
        ? [...reorderedIds, ...prev.filter(i => i.starred && !groupSet.has(i.id)).map(i => i.id)]
        : prev.filter(i => i.starred).map(i => i.id);
      const pinnedIds = tier === 'pinned'
        ? reorderedIds
        : prev.filter(i => !i.starred && i.pinned).map(i => i.id);
      const restIds = prev.filter(i => !i.starred && !i.pinned).map(i => i.id);
      return [...starredIds, ...pinnedIds, ...restIds]
        .map(id => byId.get(id)).filter(Boolean);
    });

    if (tier === 'starred') {
      await window.electronAPI?.reorderClipboardStarred?.(reorderedIds);
    } else {
      await window.electronAPI?.reorderClipboardPinned?.(reorderedIds);
    }
  }, [items, loadFolders]);

  const handleClearAll = async () => {
    const ok = await window.electronAPI?.clearClipboardHistory();
    if (ok) {
      // Pinned + starred items survive the clear, so reload from the backend
      // rather than zeroing local state (which would lie about what's left).
      setSelectedId(null);
      setSelectedDate('all');
      const next = { date: 'all', app: filterApp, tag: filterTag, search };
      filtersRef.current = next;
      loadHistory(1, false, next);
      loadDateBuckets(next);
      // Refresh storage size so the toolbar reflects post-VACUUM file size,
      // not the stale value cached when the panel mounted.
      const size = await window.electronAPI?.getClipboardStorageSize?.();
      if (size != null) setStorageSize(size);
    }
    setClearConfirm(false);
  };

  const handleScroll = () => {
    const el = gridRef.current;
    if (!el || loading) return;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 80) {
      if (items.length < total) loadHistory(page + 1, true);
    }
  };

  const handleStartEdit = (item) => {
    setEditing(true);
    setEditText(item.text_content || item.preview || '');
  };

  const handleSaveEdit = async () => {
    if (!selectedId) return;
    const newTag = await window.electronAPI?.updateClipboardItem(selectedId, editText);
    if (newTag) {
      const newPreview = editText.length > 200 ? editText.slice(0, 200) + '…' : editText;
      setItems(prev => prev.map(i =>
        i.id === selectedId
          ? { ...i, text_content: editText, preview: newPreview, content_tag: newTag }
          : i
      ));
    }
    setEditing(false);
  };

  const handleCancelEdit = () => {
    setEditing(false);
    setEditText('');
  };

  // Save the edited text as a brand-new clipboard history entry, leaving the
  // original row untouched. The Rust copy_text command writes to the system
  // clipboard without suppressing the listener, so the clipboard watcher picks
  // it up and creates a new DB row + emits clipboard-new-item.
  const handleSaveAsNew = async () => {
    if (!editText) return;
    await window.electronAPI?.copyText(editText);
    setEditing(false);
    setEditText('');
  };

  // Filtering happens on the backend (so it covers the whole history, not
  // just the loaded page), but every check is mirrored here as a defensive
  // layer for items arriving via the clipboard-new-item event between
  // backend reloads. The duplicate cost is negligible vs. a flash of a
  // wrong-bucket item in a filtered view.
  const filtered = items.filter(i => {
    if (selectedDate.startsWith('folder-')) {
      if (String(i.folder_id ?? '') !== selectedDate.slice('folder-'.length)) return false;
    } else {
      if (selectedDate === 'pinned' && !i.pinned) return false;
      if (selectedDate === 'starred' && !i.starred) return false;
      if (selectedDate !== 'all' && selectedDate !== 'pinned' && selectedDate !== 'starred' && itemLocalDateKey(i) !== selectedDate) return false;
    }
    if (filterApp && i.source_app !== filterApp) return false;
    if (filterTag !== 'All' && i.content_tag !== filterTag) return false;
    if (search.trim()) {
      const needle = search.toLowerCase();
      const inPreview = (i.preview || '').toLowerCase().includes(needle);
      // Search-inside-images: match against cached OCR text on image rows.
      // Backend enforces the Pro + setting gate on paginated queries; this
      // JS layer just needs to not filter OCR hits back out. Legacy rows
      // (before auto-OCR) simply have ocr_text = null and won't match.
      const inOcr = (i.ocr_text || '').toLowerCase().includes(needle);
      if (!inPreview && !inOcr) return false;
    }
    return true;
  });

  // Timeline grouping only when 'all' is selected. For a single date / pinned /
  // folder view a flat list under a single header reads cleaner than re-bucketing.
  // The Saved sidebar view keeps the 'Saved' label so it renders with the same
  // root + folder sub-header layout as the All view.
  let grouped = selectedDate === 'all'
    ? groupByTimeline(filtered)
    : (() => {
        const label = selectedDate === 'pinned'
          ? 'Pinned'
          : selectedDate === 'starred'
            ? 'Saved'
            : selectedDate.startsWith('folder-')
              ? (folders.find(f => `folder-${f.id}` === selectedDate)?.name || 'Folder')
              : formatDateSidebarLabel(selectedDate, todayKey);
        return filtered.length > 0 ? [[label, filtered]] : [];
      })();

  // The Saved section must render even with zero saved items when folders
  // exist — otherwise the folders (and the New Folder tile) would vanish.
  const savedSectionVisible = selectedDate === 'all' || selectedDate === 'starred';
  if (savedSectionVisible && folders.length > 0 && !grouped.some(([l]) => l === 'Saved')) {
    grouped = [['Saved', []], ...grouped];
  }

  const selected = items.find(i => i.id === selectedId) || null;

  const formatTime = (ts) => {
    try {
      const d = new Date(ts);
      const diff = Date.now() - d.getTime();
      if (diff < 60000) return 'Just now';
      if (diff < 3600000) return `${Math.floor(diff / 60000)}m ago`;
      if (diff < 86400000) return `${Math.floor(diff / 3600000)}h ago`;
      return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
    } catch { return ''; }
  };

  const formatFullTime = (ts) => {
    try {
      return new Date(ts).toLocaleString(undefined, {
        year: 'numeric', month: 'short', day: 'numeric',
        hour: '2-digit', minute: '2-digit', second: '2-digit',
      });
    } catch { return ''; }
  };

  const parseColour = (text) => {
    if (!text) return null;
    const t = text.trim();
    if (t.startsWith('#') && t.length >= 4 && t.length <= 7) return t;
    if (t.startsWith('rgb')) return t;
    return null;
  };

  const isTextOnly = selected && selected.content_type === 'text';

  return (
    <div className="cbg-panel">
      {/* ── Header (mode pills + actions) — mirrors te-header.
           Only one pill today ("Clipboard") but the structure leaves room for
           future siblings like Pinned, Snippets etc. without restructuring. */}
      <div className="cbg-header">
        <div className="cbg-mode-tabs">
          <button className="cbg-mode-tab active" type="button"><Clipboard size={12} fill="currentColor" strokeWidth={1} className="cbg-mode-tab-icon" aria-hidden="true" /> Clipboard Manager</button>
        </div>
        {/* How-to tip — same gold TIP treatment as the other panel headers.
            Hidden when the popup hotkey has been removed in Settings. */}
        {clipboardPasteHotkey && !hiddenTips.includes('clipboard') && (
          <div className="cbg-tip">
            <span className="cbg-tip-badge">TIP</span>
            <span>
              Press{' '}
              {clipboardPasteHotkey.split('+').map((p, i, arr) => (
                <React.Fragment key={i}>
                  <kbd className="cbg-tip-kbd">{friendlyKeyName(p)}</kbd>
                  {i < arr.length - 1 && <span className="cbg-tip-plus">+</span>}
                </React.Fragment>
              ))}
              {' '}in any app to open the Clipboard Manager and paste a saved item right where you are.
              {' '}Right-click any item here to copy it back to your clipboard.
            </span>
            <button type="button" className="cbg-tip-close" title="Hide this tip (restore in Settings)" aria-label="Hide this tip" onClick={() => onHideTip?.('clipboard')}>&#10005;</button>
          </div>
        )}
        <div className="cbg-header-right">
          {storageSize != null && storageSize > 0 && (
            <span className="cbg-storage-size">{formatStorageSize(storageSize)}</span>
          )}
          {clearConfirm ? (
            <div className="cbg-clear-confirm">
              <span>Clear?</span>
              <button className="cbg-clear-yes" onClick={handleClearAll} type="button">Yes</button>
              <button className="cbg-clear-no" onClick={() => setClearConfirm(false)} type="button">No</button>
            </div>
          ) : (
            <button className="cbg-clear-btn" onClick={() => setClearConfirm(true)} type="button" disabled={items.length === 0}>
              Clear All
            </button>
          )}
        </div>
      </div>

      {/* ── Filter toolbar — app filter, tag pills, search ── */}
      <div className="cbg-toolbar">
        <select className="cbg-app-filter" value={filterApp} onChange={e => setFilterApp(e.target.value)}>
          <option value="">All Apps</option>
          {sourceApps.map(app => <option key={app} value={app}>{app}</option>)}
        </select>
        <div className="cbg-tag-pills">
          {ALL_TAGS.map(tag => (
            <button key={tag} className={`cbg-tag-pill${filterTag === tag ? ' cbg-tag-active' : ''}`}
              onClick={() => setFilterTag(tag)} type="button">{tag}</button>
          ))}
        </div>
        <SearchBar
          className="cbg-search-bar"
          placeholder="Search clipboard history…"
          value={search}
          onChange={e => setSearch(e.target.value)}
        />
        <div className="cbg-col-toggle" role="group" aria-label="Card layout">
          <button
            type="button"
            className={`cbg-col-btn${columnMode === 'one' ? ' active' : ''}`}
            onClick={() => onChangeColumnMode?.('one')}
            title="Single column"
            aria-label="Single column"
            aria-pressed={columnMode === 'one'}
          >
            <Square size={14} strokeWidth={2} />
          </button>
          <button
            type="button"
            className={`cbg-col-btn${columnMode === 'two' ? ' active' : ''}`}
            onClick={() => onChangeColumnMode?.('two')}
            title="Two columns"
            aria-label="Two columns"
            aria-pressed={columnMode === 'two'}
          >
            <Columns2 size={14} strokeWidth={2} />
          </button>
          <button
            type="button"
            className={`cbg-col-btn${columnMode === 'auto' ? ' active' : ''}`}
            onClick={() => onChangeColumnMode?.('auto')}
            title="Auto layout"
            aria-label="Auto layout"
            aria-pressed={columnMode === 'auto'}
          >
            <LayoutGrid size={14} strokeWidth={2} />
          </button>
        </div>
      </div>

      {/* ── Body: date sidebar + grid (+ preview when an item is selected) ── */}
      <div className="cbg-body">
        <div className="cbg-date-sidebar">
          <div className="cbg-date-sidebar-list">
            <button
              type="button"
              className={`cbg-date-row${selectedDate === 'all' ? ' cbg-date-row-active' : ''}`}
              onClick={() => setSelectedDate('all')}
            >
              <span className="cbg-date-row-name">All</span>
              {/* Derived from dateBuckets so it stays constant when the user
                  picks a single date / Pinned / Saved (total gets overwritten
                  by each filtered backend query). dates excludes pinned + saved
                  rows by design, so add both counts back in for the true total. */}
              <span className="cbg-date-count">{dateBuckets.dates.reduce((s, d) => s + d.count, 0) + dateBuckets.pinned_count + (dateBuckets.starred_count || 0)}</span>
            </button>
            {((dateBuckets.starred_count || 0) > 0 || folders.length > 0) && (
              <button
                type="button"
                className={`cbg-date-row cbg-date-row-starred${selectedDate === 'starred' ? ' cbg-date-row-active' : ''}`}
                onClick={() => setSelectedDate('starred')}
                title="Saved items never expire and can be organised into folders."
              >
                <span className="cbg-date-row-icon"><Bookmark size={10} strokeWidth={2} fill="currentColor" /></span>
                <span className="cbg-date-row-name">Saved</span>
                <span className="cbg-date-count">{dateBuckets.starred_count || 0}</span>
              </button>
            )}
            {folders.map(f => (
              <button
                key={f.id}
                type="button"
                className={`cbg-date-row cbg-date-row-folder${selectedDate === `folder-${f.id}` ? ' cbg-date-row-active' : ''}`}
                onClick={() => setSelectedDate(`folder-${f.id}`)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setCtxMenu({ folder: true, folderId: f.id, name: f.name, x: e.clientX, y: e.clientY });
                }}
              >
                <span className="cbg-date-row-icon"><Folder size={10} strokeWidth={2} /></span>
                <span className="cbg-date-row-name">{f.name}</span>
                <span className="cbg-date-count">{f.count}</span>
              </button>
            ))}
            {/* Add Folder — mirrors the sidebar Add Category pattern in
                TextExpansions (gold primary button ↔ inline name input). */}
            {creatingFolder ? (
              <form
                className="cbg-folder-add-form"
                onSubmit={(e) => { e.preventDefault(); handleCreateFolder(); }}
              >
                <input
                  autoFocus
                  className="cbg-folder-add-input"
                  value={newFolderName}
                  onChange={e => setNewFolderName(e.target.value)}
                  placeholder="Folder name…"
                  onBlur={handleCreateFolder}
                  onKeyDown={e => {
                    if (e.key === 'Escape') { e.stopPropagation(); setCreatingFolder(false); setNewFolderName(''); }
                  }}
                />
              </form>
            ) : (
              <button
                type="button"
                className="cbg-folder-new-btn"
                title={isPro ? 'Create a folder in the Saved section' : 'Saved folders are a Pro feature'}
                onClick={() => {
                  if (!isPro) { onShowUpgrade?.('Saved folders'); return; }
                  setCreatingFolder(true);
                }}
              >
                + Add Folder
                {!isPro && <span className="cbg-pro-chip">PRO</span>}
              </button>
            )}
            {dateBuckets.pinned_count > 0 && (
              <button
                type="button"
                className={`cbg-date-row cbg-date-row-pinned${selectedDate === 'pinned' ? ' cbg-date-row-active' : ''}`}
                onClick={() => setSelectedDate('pinned')}
                title="Pinned items never expire and appear at the top of the quick paste popup."
              >
                <span className="cbg-date-row-icon"><Pin size={10} strokeWidth={2} fill="currentColor" /></span>
                <span className="cbg-date-row-name">Pinned</span>
                <span className="cbg-date-count">{dateBuckets.pinned_count}</span>
              </button>
            )}
            {dateBuckets.dates.length > 0 && <div className="cbg-date-divider" />}
            {dateBuckets.dates.map(b => (
              <button
                key={b.date}
                type="button"
                className={`cbg-date-row${selectedDate === b.date ? ' cbg-date-row-active' : ''}`}
                onClick={() => setSelectedDate(b.date)}
              >
                <span className="cbg-date-row-name">{formatDateSidebarLabel(b.date, todayKey)}</span>
                <span className="cbg-date-count">{b.count}</span>
              </button>
            ))}
          </div>
        </div>
      <div className="cbg-main cbg-main-split">
        <div className={`cbg-grid-wrap${dragState ? ' cbg-dragging' : ''}`} ref={gridRef} onScroll={handleScroll}>
          {filtered.length === 0 && grouped.length === 0 ? (
            <div className="cbg-empty">
              {items.length === 0 ? 'No clipboard history yet. Copy something to get started.' : 'No results'}
            </div>
          ) : (
            <DndContext
              sensors={dndSensors}
              onDragStart={handleDragStart}
              onDragOver={handleDragOver}
              onDragCancel={handleDragCancel}
              onDragEnd={handleDragEnd}
            >
              {grouped.map(([label, groupItems]) => {
                // Tier checks are scoped to the views that produce tier labels
                // so a user folder named "Saved" or "Pinned" can't hijack the
                // tier rendering when viewed via its own sidebar row.
                const isSavedTier = label === 'Saved' && savedSectionVisible;
                const isPinnedTier = label === 'Pinned' && (selectedDate === 'all' || selectedDate === 'pinned');
                const isFolderView = selectedDate.startsWith('folder-');
                const sortable = isSavedTier || isPinnedTier;
                const tierPrefix = isSavedTier ? 'starred' : 'pinned';
                const sortableIds = sortable
                  ? groupItems.map(i => `${tierPrefix}-${i.id}`)
                  : null;

                const renderCard = (item) => {
                  const isImage = item.content_type === 'image';
                  const tag = item.content_tag || 'Text';
                  const colourVal = tag === 'Colour' ? parseColour(item.text_content || item.preview) : null;
                  const isLink = tag === 'Link';
                  const isSel = item.id === selectedId;
                  const isMulti = multiSel.has(item.id);
                  // "in image" chip: appears when the active search matched the
                  // row's OCR text but NOT its preview — helps the user
                  // understand why a screenshot appeared for a text query.
                  // Trust the backend hint (search_source) when present, and
                  // recompute locally for defensive JS-side matches.
                  const needle = search.trim().toLowerCase();
                  const isOcrMatch = isImage && needle.length > 0 && (
                    item.search_source === 'ocr' ||
                    (!(item.preview || '').toLowerCase().includes(needle) &&
                      (item.ocr_text || '').toLowerCase().includes(needle))
                  );
                  const className = `cbg-card${isImage ? ' cbg-card-img' : ' cbg-card-text'}${isSel ? ' cbg-card-sel' : ''}${isMulti ? ' cbg-card-multisel' : ''}${item.id === newItemId ? ' cbg-card-arrive' : ''}${sortable ? ' cbg-card-sortable' : ''}`;
                  const onClick = (e) => {
                    // Ctrl/Cmd+click builds a multi-selection for bulk actions
                    // without disturbing the detail-pane selection.
                    if (e.ctrlKey || e.metaKey) {
                      setMultiSel(prev => {
                        const next = new Set(prev);
                        if (next.has(item.id)) next.delete(item.id); else next.add(item.id);
                        return next;
                      });
                      return;
                    }
                    if (multiSel.size > 0) setMultiSel(new Set());
                    setSelectedId(isSel ? null : item.id);
                  };
                  const onContextMenu = e => {
                    e.preventDefault();
                    // Right-click inside an active multi-selection targets the
                    // whole selection; outside it, collapse back to single.
                    const multi = multiSel.has(item.id) && multiSel.size > 1;
                    if (!multi && multiSel.size > 0) setMultiSel(new Set());
                    setCtxMenu({
                      id: item.id, x: e.clientX, y: e.clientY,
                      pinned: item.pinned, starred: item.starred, folderId: item.folder_id ?? null,
                      multi, ids: multi ? [...multiSel] : null,
                    });
                  };

                  const inner = (
                    <>
                      <span className={`cbg-tag cbg-tag-${tag.toLowerCase()}`}>{tag}</span>
                      {(item.starred || item.pinned) && (
                        <span className="cbg-card-badges" aria-hidden="true">
                          {item.starred && (
                            <span className="cbg-card-star" aria-label="Saved">
                              <Bookmark size={11} strokeWidth={2} fill="currentColor" />
                            </span>
                          )}
                          {item.pinned && (
                            <span className="cbg-card-pin" aria-label="Pinned">
                              <Pin size={11} strokeWidth={2} fill="currentColor" />
                            </span>
                          )}
                        </span>
                      )}

                      {isImage ? (
                        <>
                          <ImageThumb id={item.id} className="cbg-card-image" />
                          {isOcrMatch && (
                            <span className="cbg-ocr-match-chip" aria-label="Match found in image text">
                              in image
                            </span>
                          )}
                          <div className="cbg-card-img-overlay">
                            {item.source_app && <span className="cbg-source-badge">{item.source_app}</span>}
                            <span className="cbg-overlay-right">{item.image_width}×{item.image_height} · {formatTime(item.timestamp)}</span>
                          </div>
                        </>
                      ) : colourVal ? (
                        <>
                          <div className="cbg-colour-swatch" style={{ background: colourVal }} />
                          <div className="cbg-card-body cbg-colour-value">{item.text_content || item.preview || ''}</div>
                          <div className="cbg-card-meta">
                            {item.source_app && <span className="cbg-source-badge">{item.source_app}</span>}
                            <span className="cbg-card-time">{formatTime(item.timestamp)}</span>
                          </div>
                        </>
                      ) : (
                        <>
                          <div className="cbg-card-body">
                            {isLink && (
                              <span className="cbg-link-icon" aria-hidden="true">
                                <Link2 size={12} strokeWidth={1.75} style={{ verticalAlign: -2, marginRight: 4 }} />
                              </span>
                            )}
                            {highlightMatches((item.text_content || item.preview || '').slice(0, 1000), search.trim())}
                          </div>
                          <div className="cbg-card-meta">
                            {item.source_app && <span className="cbg-source-badge">{item.source_app}</span>}
                            <span className="cbg-card-time">{formatTime(item.timestamp)}</span>
                          </div>
                        </>
                      )}
                    </>
                  );

                  if (sortable) {
                    return (
                      <SortableCardWrap
                        key={item.id}
                        sortableId={`${tierPrefix}-${item.id}`}
                        className={className}
                        onClick={onClick}
                        onContextMenu={onContextMenu}
                        dataId={item.id}
                      >
                        {inner}
                      </SortableCardWrap>
                    );
                  }
                  return (
                    <div
                      key={item.id}
                      className={className}
                      data-clip-id={item.id}
                      onClick={onClick}
                      onContextMenu={onContextMenu}
                    >
                      {inner}
                    </div>
                  );
                };

                // ── Saved section: New Folder tile + root items, then one
                // sub-group per folder. Headers and empty areas are drop zones
                // so saved cards can be dragged between root and folders.
                if (isSavedTier) {
                  const rootItems = groupItems.filter(i => i.folder_id == null);
                  const byFolder = new Map();
                  for (const it of groupItems) {
                    if (it.folder_id != null) {
                      if (!byFolder.has(it.folder_id)) byFolder.set(it.folder_id, []);
                      byFolder.get(it.folder_id).push(it);
                    }
                  }
                  const savedCollapsed = collapsedSections.has('Saved');
                  return (
                    <div key={label} className="cbg-timeline-group">
                      <FolderDropZone
                        dropId="folderdrop-root"
                        className={`cbg-timeline-header cbg-collapsible${dragState === 'starred' ? ' cbg-drop-eligible' : ''}`}
                        onClick={() => toggleSectionCollapsed('Saved')}
                        ariaExpanded={!savedCollapsed}
                        title="Saved items never expire and can be organised into folders. Click to collapse or expand."
                      >
                        <span className="cbg-folder-caret" aria-hidden="true">
                          {savedCollapsed
                            ? <ChevronRight size={11} strokeWidth={2.25} />
                            : <ChevronDown size={11} strokeWidth={2.25} />}
                        </span>
                        <span className="cbg-timeline-icon cbg-timeline-icon-star">
                          <Bookmark size={10} strokeWidth={2} fill="currentColor" />
                        </span>
                        <span className="cbg-timeline-name">Saved</span>
                        <span className="cbg-timeline-count">{groupItems.length}</span>
                        <span className="cbg-timeline-rule" />
                      </FolderDropZone>
                      <div className={`cbg-collapse-wrap${savedCollapsed ? ' cbg-collapsed' : ''}`}>
                      <div className="cbg-collapse-inner">
                      {/* Skip the root grid entirely when empty — its padding
                          would otherwise leave a dead gap between the Saved
                          header and the first folder header. */}
                      {rootItems.length > 0 && (
                        <SortableContext items={rootItems.map(i => `starred-${i.id}`)}>
                          <div className="cbg-grid cbg-grid-2col" data-cols={columnMode}>
                            {rootItems.map(renderCard)}
                          </div>
                        </SortableContext>
                      )}
                      {folders.map(f => {
                        const fItems = byFolder.get(f.id) || [];
                        const isCollapsed = collapsedSections.has(f.id);
                        return (
                          <div key={f.id} className="cbg-folder-group">
                            {/* Header click toggles collapse. It stays a drop
                                target while collapsed so cards can still be
                                dragged into a closed folder. */}
                            <FolderDropZone
                              dropId={`folderdrop-${f.id}`}
                              className={`cbg-timeline-header cbg-folder-header${dragState === 'starred' ? ' cbg-drop-eligible' : ''}`}
                              onClick={() => toggleSectionCollapsed(f.id)}
                              ariaExpanded={!isCollapsed}
                              onContextMenu={(e) => {
                                e.preventDefault();
                                setCtxMenu({ folder: true, folderId: f.id, name: f.name, x: e.clientX, y: e.clientY });
                              }}
                            >
                              <span className="cbg-folder-caret" aria-hidden="true">
                                {isCollapsed
                                  ? <ChevronRight size={11} strokeWidth={2.25} />
                                  : <ChevronDown size={11} strokeWidth={2.25} />}
                              </span>
                              <span className="cbg-timeline-icon">
                                <Folder size={10} strokeWidth={2} fill="currentColor" />
                              </span>
                              {renamingFolderId === f.id ? (
                                <input
                                  className="cbg-folder-rename-input"
                                  autoFocus
                                  value={renameText}
                                  onChange={e => setRenameText(e.target.value)}
                                  onKeyDown={e => {
                                    if (e.key === 'Enter') commitFolderRename();
                                    if (e.key === 'Escape') { e.stopPropagation(); setRenamingFolderId(null); setRenameText(''); }
                                  }}
                                  onBlur={commitFolderRename}
                                  onClick={e => e.stopPropagation()}
                                />
                              ) : (
                                <span
                                  className="cbg-timeline-name"
                                  title="Double-click to rename"
                                  onClick={e => e.stopPropagation()}
                                  onDoubleClick={(e) => {
                                    e.stopPropagation();
                                    setRenamingFolderId(f.id);
                                    setRenameText(f.name);
                                  }}
                                >{f.name}</span>
                              )}
                              <span className="cbg-timeline-count">{fItems.length}</span>
                              <span className="cbg-timeline-rule" />
                            </FolderDropZone>
                            <div className={`cbg-collapse-wrap${isCollapsed ? ' cbg-collapsed' : ''}`}>
                            <div className="cbg-collapse-inner">
                            {fItems.length > 0 ? (
                              <SortableContext items={fItems.map(i => `starred-${i.id}`)}>
                                <div className="cbg-grid cbg-grid-2col" data-cols={columnMode}>
                                  {fItems.map(renderCard)}
                                </div>
                              </SortableContext>
                            ) : (
                              <FolderDropZone
                                dropId={`folderdrop-${f.id}-empty`}
                                className={`cbg-folder-empty${dragState === 'starred' ? ' cbg-drop-eligible' : ''}`}
                              >
                                Drag saved items here, or right-click an item and choose Move to Folder
                              </FolderDropZone>
                            )}
                            </div>
                            </div>
                          </div>
                        );
                      })}
                      </div>
                      </div>
                    </div>
                  );
                }

                const gridContent = (
                  <div className="cbg-grid cbg-grid-2col" data-cols={columnMode}>
                    {groupItems.map(renderCard)}
                  </div>
                );

                // Every header in the stacked 'all' timeline collapses on
                // click. Single-bucket views (one date / Pinned / a folder)
                // keep their lone header static — collapsing the only section
                // on screen would just leave a blank page.
                const collapsible = selectedDate === 'all';
                const isCollapsed = collapsible && collapsedSections.has(label);
                return (
                  <div key={label} className="cbg-timeline-group">
                    <div
                      className={`cbg-timeline-header${collapsible ? ' cbg-collapsible' : ''}`}
                      onClick={collapsible ? () => toggleSectionCollapsed(label) : undefined}
                      role={collapsible ? 'button' : undefined}
                      tabIndex={collapsible ? 0 : undefined}
                      aria-expanded={collapsible ? !isCollapsed : undefined}
                      onKeyDown={collapsible ? (e) => {
                        if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); e.stopPropagation(); toggleSectionCollapsed(label); }
                      } : undefined}
                      title={isPinnedTier ? 'Pinned items never expire and appear at the top of the quick paste popup.' : undefined}
                    >
                      {collapsible && (
                        <span className="cbg-folder-caret" aria-hidden="true">
                          {isCollapsed
                            ? <ChevronRight size={11} strokeWidth={2.25} />
                            : <ChevronDown size={11} strokeWidth={2.25} />}
                        </span>
                      )}
                      {isFolderView && (
                        <span className="cbg-timeline-icon">
                          <Folder size={10} strokeWidth={2} fill="currentColor" />
                        </span>
                      )}
                      {isPinnedTier && (
                        <span className="cbg-timeline-icon">
                          <Pin size={10} strokeWidth={2} fill="currentColor" />
                        </span>
                      )}
                      <span className="cbg-timeline-name">{label}</span>
                      <span className="cbg-timeline-count">{groupItems.length}</span>
                      <span className="cbg-timeline-rule" />
                    </div>
                    <div className={`cbg-collapse-wrap${isCollapsed ? ' cbg-collapsed' : ''}`}>
                    <div className="cbg-collapse-inner">
                    {sortable ? (
                      <SortableContext items={sortableIds}>
                        {gridContent}
                      </SortableContext>
                    ) : gridContent}
                    </div>
                    </div>
                  </div>
                );
              })}
            </DndContext>
          )}
          {loading && <div className="cbg-loading">Loading…</div>}
        </div>

        {/* ── Right: detail pane — always visible, mirroring the Text
             Expansions editor pane. Resizable via the left-edge handle;
             shows a muted placeholder until an item is selected. ── */}
        <div className="cbg-divider" />
        <div className="cbg-detail" style={{ width: `${effectivePreviewWidth}px` }}>
          <div
            className="cbg-detail-resize"
            onMouseDown={startResize}
            title="Drag to resize"
            role="separator"
            aria-label="Resize preview pane"
          />
          {!selected ? (
            <div className="cbg-detail-empty">
              <Clipboard size={22} strokeWidth={1.5} aria-hidden="true" />
              <span>Select an item to preview it here</span>
            </div>
          ) : (
          <>
            <button
              className="cbg-detail-close"
              onClick={() => { setSelectedId(null); setEditing(false); }}
              title="Clear selection"
              type="button"
            >✕</button>
            <div className={`cbg-detail-content${selected.content_type === 'image' ? ' cbg-detail-content-img' : ''}`}>
              {selected.content_type === 'image' ? (
                <div
                  className="cbg-detail-img-wrap"
                  onClick={async () => {
                    // Fetch the full image once and hand to the lightbox.
                    const b64 = await window.electronAPI?.getClipboardImage(selected.id);
                    if (b64) {
                      setLightboxSrc(`data:image/png;base64,${b64}`);
                      setLightboxOpen(true);
                    }
                  }}
                  title="Click to open full image"
                >
                  <ImageThumb id={selected.id} className="cbg-detail-img" zoomable={false} />
                  <span className="cbg-detail-img-expand" aria-hidden="true">
                    <Maximize2 size={14} strokeWidth={2} />
                  </span>
                </div>
              ) : editing ? (
                <textarea
                  className="cbg-detail-textarea"
                  value={editText}
                  onChange={e => setEditText(e.target.value)}
                  autoFocus
                  spellCheck={false}
                />
              ) : selected.content_tag === 'Link' ? (
                <LinkPane url={selected.text_content || selected.preview || ''} />
              ) : selected.content_tag === 'Email' ? (
                <EmailPane email={selected.text_content || selected.preview || ''} />
              ) : selected.content_tag === 'Colour' ? (
                <ColourPane value={selected.text_content || selected.preview || ''} />
              ) : (
                <pre className="cbg-detail-text" style={{ fontSize: (() => {
                  const len = (selected.text_content || selected.preview || '').length;
                  if (len < 60) return '1.6rem';
                  if (len < 200) return '1.25rem';
                  return '1.0rem';
                })() }}>{selected.text_content || selected.preview || ''}</pre>
              )}
            </div>

            {/* ── Text-type extras (counts, transforms, smart actions) ──    */}
            {/* Skip when a custom pane is rendering (Link/Email/Colour) — those */}
            {/* panes already provide tag-appropriate buttons.                    */}
            {selected.content_type === 'text' && !editing
              && !['Link', 'Email', 'Colour'].includes(selected.content_tag)
              && (() => {
              const fullText = selected.text_content || selected.preview || '';
              const words = countWords(fullText);
              const chars = fullText.length;
              const lines = countLines(fullText);
              const smart = detectSmartAction(fullText);
              return (
                <>
                  <div className="cbg-detail-counts">
                    <span>{words} word{words === 1 ? '' : 's'}</span>
                    <span className="cbg-counts-sep">·</span>
                    <span>{chars} char{chars === 1 ? '' : 's'}</span>
                    <span className="cbg-counts-sep">·</span>
                    <span>{lines} line{lines === 1 ? '' : 's'}</span>
                  </div>
                  {fullText.trim() && (
                    <div className="cbg-transform-pills">
                      <button className="cbg-tpill" type="button" onClick={() => handlePasteText(fullText.toLowerCase(), selected.id)}>lowercase</button>
                      <button className="cbg-tpill" type="button" onClick={() => handlePasteText(fullText.toUpperCase(), selected.id)}>UPPERCASE</button>
                      <button className="cbg-tpill" type="button" onClick={() => handlePasteText(fullText.trim(), selected.id)}>Trimmed</button>
                      <button className="cbg-tpill" type="button" onClick={() => handlePasteText(toPlainAscii(fullText), selected.id)}>Plain</button>
                    </div>
                  )}
                  {smart.kind && (
                    <div className="cbg-smart-actions">
                      {smart.kind === 'url' && (
                        <button className="cbg-dbtn" type="button" onClick={() => window.electronAPI?.openExternal(smart.value)}>Open</button>
                      )}
                      {smart.kind === 'email' && (
                        <button className="cbg-dbtn" type="button" onClick={() => window.electronAPI?.openExternal(`mailto:${smart.value}`)}>Email</button>
                      )}
                      {smart.kind === 'phone' && (
                        <button className="cbg-dbtn" type="button" onClick={() => window.electronAPI?.openExternal(`tel:${smart.digits}`)}>Call</button>
                      )}
                      <button
                        className="cbg-dbtn"
                        type="button"
                        onClick={() => { try { navigator.clipboard?.writeText(smart.value); } catch {} }}
                      >Copy</button>
                    </div>
                  )}
                </>
              );
            })()}

            {/* ── Image-type extras (OCR, colours, save) ── */}
            {selected.content_type === 'image' && (
              <>
                <div className="cbg-image-actions">
                  <button
                    className="cbg-dbtn"
                    type="button"
                    onClick={handleRunOcr}
                    disabled={ocrLoading}
                    title={isPro ? 'Run OCR on this image' : 'OCR text extraction is a Pro feature'}
                  >
                    {ocrLoading
                      ? 'Extracting…'
                      : (ocrText !== null && !ocrError ? 'Re-extract text' : 'Extract text')}
                    {!isPro && <span className="cbg-pro-chip" style={{ marginLeft: 6 }}>PRO</span>}
                  </button>
                  <button
                    className="cbg-dbtn"
                    type="button"
                    onClick={() => window.electronAPI?.saveClipboardImageAs(selected.id, 'png')}
                  >Save as PNG</button>
                  <button
                    className="cbg-dbtn"
                    type="button"
                    onClick={() => window.electronAPI?.saveClipboardImageAs(selected.id, 'jpg')}
                  >Save as JPG</button>
                </div>
                {ocrError && (
                  <div className="cbg-ocr-error">OCR not available on this system.</div>
                )}
                {ocrText !== null && !ocrError && (
                  <div className="cbg-ocr-result">
                    <div className="cbg-ocr-text">{ocrText.trim() || '(no text detected)'}</div>
                  </div>
                )}
                {ocrText !== null && !ocrError && ocrText.trim() && (
                  <div className="cbg-ocr-copy-actions">
                    <button
                      className="cbg-dbtn"
                      type="button"
                      title="Keeps each line break from the image"
                      onClick={async () => { try { await window.electronAPI?.copyText(ocrText); setCopyToast('ocr-shown'); setTimeout(() => setCopyToast(null), 1200); } catch {} }}
                    >{copyToast === 'ocr-shown' ? 'Copied!' : 'Copy as shown'}</button>
                    <button
                      className="cbg-dbtn"
                      type="button"
                      title="Joins lines into continuous paragraphs"
                      onClick={async () => { try { await window.electronAPI?.copyText(reflowParagraphs(ocrText)); setCopyToast('ocr-para'); setTimeout(() => setCopyToast(null), 1200); } catch {} }}
                    >{copyToast === 'ocr-para' ? 'Copied!' : 'Copy as paragraphs'}</button>
                  </div>
                )}
                {imageColors.length > 0 && (
                  <div className="cbg-color-swatches">
                    {imageColors.map((rgb, idx) => {
                      const hex = rgbToHex(rgb[0], rgb[1], rgb[2]);
                      return (
                        <button
                          key={idx}
                          type="button"
                          className="cbg-color-swatch-btn"
                          style={{ background: hex }}
                          onClick={() => copyHexToClipboard(hex)}
                          title={`${hex} — click to copy`}
                        >
                          {copyToast === hex && <span className="cbg-swatch-copied">✓</span>}
                        </button>
                      );
                    })}
                  </div>
                )}
              </>
            )}

            <div className="cbg-detail-meta">
              {selected.source_app && (
                <>
                  <span className="cbg-meta-label">Source</span>
                  <span className="cbg-meta-value">{selected.source_app}</span>
                </>
              )}
              {/* "Last copied", not "Captured" — promote-on-use rewrites this
                  timestamp whenever the item is copied or pasted again, and
                  for never-reused items it's the original copy time anyway. */}
              <span className="cbg-meta-label">Last copied</span>
              <span className="cbg-meta-value">{formatFullTime(selected.timestamp)}</span>
              {selected.content_type === 'image' ? (
                <>
                  <span className="cbg-meta-label">Size</span>
                  <span className="cbg-meta-value">{selected.image_width} × {selected.image_height} px</span>
                </>
              ) : (
                <>
                  <span className="cbg-meta-label">Characters</span>
                  <span className="cbg-meta-value">{(selected.text_content || selected.preview || '').length}</span>
                </>
              )}
            </div>
            <div className="cbg-detail-actions">
              <div className="cbg-detail-actions-l">
                <button className="cbg-dbtn cbg-dbtn-icon" onClick={() => handleStar(selected.id, selected.starred)} type="button">
                  <Bookmark size={13} strokeWidth={1.75} fill={selected.starred ? 'currentColor' : 'none'} />
                  {selected.starred ? ' Unsave' : ' Save'}
                </button>
                <button className="cbg-dbtn cbg-dbtn-icon" onClick={() => handlePin(selected.id, selected.pinned)} type="button">
                  {selected.pinned ? (
                    <><PinOff size={13} strokeWidth={1.75} /> Unpin</>
                  ) : (
                    <><Pin size={13} strokeWidth={1.75} /> Pin</>
                  )}
                </button>
                {isTextOnly && !editing && (
                  <button className="cbg-dbtn" onClick={() => handleStartEdit(selected)} type="button">Edit</button>
                )}
                {isTextOnly && !editing && onCreateExpansion && (
                  <button
                    className="cbg-dbtn cbg-dbtn-create-expansion"
                    onClick={() => onCreateExpansion(selected.text_content || selected.preview || '')}
                    type="button"
                    title="Save this clip as a text expansion"
                  >Create Expansion</button>
                )}
                {editing && (
                  <>
                    <button className="cbg-dbtn cbg-dbtn-save" onClick={handleSaveEdit} type="button" title="Overwrite this clip with the edited text">Save</button>
                    <button className="cbg-dbtn" onClick={handleSaveAsNew} type="button" title="Create a new clipboard entry with the edited text, leaving this one untouched">Save as New</button>
                    <button className="cbg-dbtn" onClick={handleCancelEdit} type="button">Cancel</button>
                  </>
                )}
                {(selected.paste_count || 0) > 0 && (
                  <span className="cbg-paste-count">
                    Pasted {selected.paste_count} time{selected.paste_count === 1 ? '' : 's'}
                  </span>
                )}
              </div>
              {!editing && (
                <div className="cbg-detail-actions-r">
                  <button className="cbg-dbtn cbg-dbtn-del" onClick={() => handleDelete(selected.id)} type="button">Delete</button>
                  {selected.has_html && selected.content_type === 'text' && (
                    <button
                      className="cbg-dbtn"
                      type="button"
                      title="Copy without formatting"
                      onClick={async () => {
                        await window.electronAPI?.copyText(selected.text_content || selected.preview || '');
                        showActionToast('Copied as plain text');
                      }}
                    >Copy plain</button>
                  )}
                  <button className="cbg-dbtn cbg-dbtn-copy" onClick={() => handleCopyWithToast(selected.id)} type="button">Copy</button>
                </div>
              )}
            </div>
          </>
          )}
        </div>
      </div>
      </div>{/* /cbg-body */}

      {ctxMenu && (
        <div ref={ctxRef} className="cbg-ctx" style={{ top: ctxMenu.y, left: ctxMenu.x }}>
          {ctxMenu.folder ? (
            <>
              <button
                className="cbg-ctx-item"
                onClick={() => {
                  setRenamingFolderId(ctxMenu.folderId);
                  setRenameText(ctxMenu.name || '');
                  setCtxMenu(null);
                }}
                type="button"
              >Rename</button>
              <button className="cbg-ctx-item cbg-ctx-del" onClick={() => handleDeleteFolder(ctxMenu.folderId)} type="button">
                Delete Folder
              </button>
            </>
          ) : ctxMenu.multi ? (
            <>
              {/* Bulk menu — right-click landed inside a Ctrl+click selection. */}
              <button className="cbg-ctx-item" onClick={() => bulkSave(ctxMenu.ids)} type="button">
                Save {ctxMenu.ids.length} items
              </button>
              {folders.length > 0 && (
                <>
                  <button
                    className="cbg-ctx-item"
                    onClick={(e) => { e.stopPropagation(); setCtxMenu(m => ({ ...m, moveOpen: !m.moveOpen })); }}
                    type="button"
                  >Move {ctxMenu.ids.length} to Folder…</button>
                  {ctxMenu.moveOpen && (
                    <div className="cbg-ctx-sub">
                      <button className="cbg-ctx-item" onClick={() => bulkMove(ctxMenu.ids, null)} type="button">
                        Saved (no folder)
                      </button>
                      {folders.map(f => (
                        <button key={f.id} className="cbg-ctx-item" onClick={() => bulkMove(ctxMenu.ids, f.id)} type="button">
                          {f.name}
                        </button>
                      ))}
                    </div>
                  )}
                </>
              )}
              <button className="cbg-ctx-item cbg-ctx-del" onClick={() => bulkDelete(ctxMenu.ids)} type="button">
                Delete {ctxMenu.ids.length} items
              </button>
            </>
          ) : (
            <>
              <button className="cbg-ctx-item" onClick={() => handleCopyWithToast(ctxMenu.id)} type="button">
                Copy
              </button>
              <button className="cbg-ctx-item" onClick={() => handleStar(ctxMenu.id, ctxMenu.starred)} type="button">
                {ctxMenu.starred ? 'Unsave' : 'Save'}
              </button>
              {ctxMenu.starred && folders.length > 0 && (
                <>
                  <button
                    className="cbg-ctx-item"
                    onClick={(e) => { e.stopPropagation(); setCtxMenu(m => ({ ...m, moveOpen: !m.moveOpen })); }}
                    type="button"
                  >Move to Folder…</button>
                  {ctxMenu.moveOpen && (
                    <div className="cbg-ctx-sub">
                      {ctxMenu.folderId != null && (
                        <button className="cbg-ctx-item" onClick={() => handleMoveToFolder(ctxMenu.id, null)} type="button">
                          Saved (no folder)
                        </button>
                      )}
                      {folders.filter(f => f.id !== ctxMenu.folderId).map(f => (
                        <button key={f.id} className="cbg-ctx-item" onClick={() => handleMoveToFolder(ctxMenu.id, f.id)} type="button">
                          {f.name}
                        </button>
                      ))}
                    </div>
                  )}
                </>
              )}
              <button className="cbg-ctx-item" onClick={() => handlePin(ctxMenu.id, ctxMenu.pinned)} type="button">
                {ctxMenu.pinned ? 'Unpin' : 'Pin'}
              </button>
              <button className="cbg-ctx-item cbg-ctx-del" onClick={() => handleDelete(ctxMenu.id)} type="button">Delete</button>
            </>
          )}
        </div>
      )}

      {actionToast && (
        <div className="cbg-action-toast" role="status">{actionToast}</div>
      )}
      {/* Persistent hint while a multi-selection is active — makes the bulk
          right-click actions discoverable. The transient toast wins the slot. */}
      {!actionToast && multiSel.size > 0 && (
        <div className="cbg-action-toast cbg-selection-bar" role="status">
          {multiSel.size} selected · right-click for actions · Esc to clear
        </div>
      )}

      {lightboxOpen && lightboxSrc && ReactDOM.createPortal(
        <div
          className="cbg-lightbox"
          onClick={() => { setLightboxOpen(false); setLightboxSrc(null); }}
          role="dialog"
          aria-label="Image preview"
        >
          <button
            className="cbg-lightbox-close"
            onClick={(e) => { e.stopPropagation(); setLightboxOpen(false); setLightboxSrc(null); }}
            type="button"
            title="Close (Esc)"
          >✕</button>
          <div className="cbg-lightbox-stage" onClick={e => e.stopPropagation()}>
            <ZoomableImage src={lightboxSrc} className="cbg-lightbox-img" />
          </div>
          <div className="cbg-lightbox-hint">Scroll to zoom · Drag to pan · Esc / click outside to close</div>
        </div>,
        document.body
      )}
    </div>
  );
}
