import React, { useState, useEffect, useLayoutEffect, useRef, useCallback } from 'react';
import ReactDOM from 'react-dom';
import { Pin, PinOff, Link2, Maximize2, Clipboard } from 'lucide-react';
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

// ── Timeline grouping ──────────────────────────────────────────────────────

function groupByTimeline(items) {
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterdayStart = new Date(todayStart); yesterdayStart.setDate(todayStart.getDate() - 1);
  const weekStart = new Date(todayStart); weekStart.setDate(todayStart.getDate() - todayStart.getDay());
  const monthStart = new Date(now.getFullYear(), now.getMonth(), 1);

  const groups = { Pinned: [], Today: [], Yesterday: [], 'This Week': [], 'This Month': [], Older: [] };

  for (const item of items) {
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

export default function ClipboardPanel({ previewWidth = 480, onChangePreviewWidth, onCreateExpansion, clipboardPasteHotkey = 'Ctrl+Shift+V', hiddenTips = [], onHideTip }) {
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
  // Date sidebar state. selectedDate: 'all' (timeline grouping) | 'pinned'
  // (all pinned items) | 'YYYY-MM-DD' (single local date).
  const [dateBuckets, setDateBuckets] = useState({ dates: [], pinned_count: 0 });
  const [selectedDate, setSelectedDate] = useState('all');
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
  const ctxRef = useRef(null);
  const gridRef = useRef(null);

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
        dateFilter: f.date === 'all' ? null : f.date,
        appFilter: f.app || null,
        tagFilter: f.tag && f.tag !== 'All' ? f.tag : null,
        search: f.search?.trim() || null,
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

  const loadDateBuckets = useCallback(() => {
    window.electronAPI?.getClipboardDateBuckets?.().then(b => {
      if (b) setDateBuckets(b);
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
    }, search ? 200 : 0);
    return () => clearTimeout(timer);
    // loadHistory is stable; we re-fire on any filter change.
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
      // Functional setter so the closure doesn't capture a stale sourceApps —
      // lets this effect run once on mount instead of re-registering the
      // listener every time sourceApps mutates (which raced with the async
      // listen() registration and produced duplicate visual rows on copy).
      if (item.source_app) {
        setSourceApps(prev => prev.includes(item.source_app) ? prev : [...prev, item.source_app].sort());
      }
      // Update sidebar bucket counts for the new item's date.
      setDateBuckets(prev => {
        const key = itemLocalDateKey(item);
        if (!key) return prev;
        const existing = prev.dates.find(d => d.date === key);
        const dates = existing
          ? prev.dates.map(d => d.date === key ? { ...d, count: d.count + 1 } : d)
          : [{ date: key, count: 1 }, ...prev.dates].sort((a, b) => b.date.localeCompare(a.date));
        return { ...prev, dates };
      });
    });
    return () => window.electronAPI?.removeAllListeners('clipboard-new-item');
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
      } else if (selectedId !== null) {
        setSelectedId(null);
      }
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [editing, selectedId, lightboxOpen]);

  // Copy a history item back onto the system clipboard. The user is expected to
  // switch to their target app and paste with Ctrl+V themselves — the in-place
  // paste path can't reliably focus the right window from this panel (WebView2
  // owns input here), and the popup overlay (Ctrl+Shift+V) remains the fast path.
  const handleCopy = async (id) => {
    await window.electronAPI?.copyClipboardItem(id);
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
    }
    setCtxMenu(null);
  };

  const handlePin = async (id, pinned) => {
    const ok = await window.electronAPI?.pinClipboardItem(id, !pinned);
    if (ok) {
      setItems(prev => prev.map(i => i.id === id ? { ...i, pinned: !pinned } : i));
      loadDateBuckets();
    }
    setCtxMenu(null);
  };

  const handleClearAll = async () => {
    const ok = await window.electronAPI?.clearClipboardHistory();
    if (ok) {
      setItems([]); setTotal(0); setSelectedId(null);
      setDateBuckets({ dates: [], pinned_count: 0 });
      setSelectedDate('all');
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
    if (selectedDate === 'pinned' && !i.pinned) return false;
    if (selectedDate !== 'all' && selectedDate !== 'pinned' && itemLocalDateKey(i) !== selectedDate) return false;
    if (filterApp && i.source_app !== filterApp) return false;
    if (filterTag !== 'All' && i.content_tag !== filterTag) return false;
    if (search.trim() && !(i.preview || '').toLowerCase().includes(search.toLowerCase())) return false;
    return true;
  });

  // Timeline grouping only when 'all' is selected. For a single date or pinned
  // view a flat list under a single header reads cleaner than re-bucketing.
  const grouped = selectedDate === 'all'
    ? groupByTimeline(filtered)
    : (() => {
        const label = selectedDate === 'pinned'
          ? 'Pinned'
          : formatDateSidebarLabel(selectedDate, todayKey);
        return filtered.length > 0 ? [[label, filtered]] : [];
      })();

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
                  picks a single date or Pinned (total gets overwritten by each
                  filtered backend query). dates excludes pinned rows by
                  design, so add pinned_count back in to get the true total. */}
              <span className="cbg-date-count">{dateBuckets.dates.reduce((s, d) => s + d.count, 0) + dateBuckets.pinned_count}</span>
            </button>
            {dateBuckets.pinned_count > 0 && (
              <button
                type="button"
                className={`cbg-date-row cbg-date-row-pinned${selectedDate === 'pinned' ? ' cbg-date-row-active' : ''}`}
                onClick={() => setSelectedDate('pinned')}
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
      <div className={`cbg-main${selected ? ' cbg-main-split' : ''}`}>
        <div className="cbg-grid-wrap" ref={gridRef} onScroll={handleScroll}>
          {filtered.length === 0 ? (
            <div className="cbg-empty">
              {items.length === 0 ? 'No clipboard history yet — copy something to get started' : 'No results'}
            </div>
          ) : (
            grouped.map(([label, groupItems]) => (
              <div key={label} className="cbg-timeline-group">
                <div className="cbg-timeline-header">
                  {label === 'Pinned' && (
                    <span className="cbg-timeline-icon">
                      <Pin size={10} strokeWidth={2} fill="currentColor" />
                    </span>
                  )}
                  <span className="cbg-timeline-name">{label}</span>
                  <span className="cbg-timeline-count">{groupItems.length}</span>
                  <span className="cbg-timeline-rule" />
                </div>
                <div className={`cbg-grid${selected ? ' cbg-grid-2col' : ''}`}>
                  {groupItems.map(item => {
                    const isImage = item.content_type === 'image';
                    const tag = item.content_tag || 'Text';
                    const colourVal = tag === 'Colour' ? parseColour(item.text_content || item.preview) : null;
                    const isLink = tag === 'Link';
                    const isSel = item.id === selectedId;

                    return (
                      <div
                        key={item.id}
                        className={`cbg-card${isImage ? ' cbg-card-img' : ' cbg-card-text'}${isSel ? ' cbg-card-sel' : ''}`}
                        onClick={() => setSelectedId(isSel ? null : item.id)}
                        onContextMenu={e => {
                          e.preventDefault();
                          setCtxMenu({ id: item.id, x: e.clientX, y: e.clientY, pinned: item.pinned });
                        }}
                      >
                        <span className={`cbg-tag cbg-tag-${tag.toLowerCase()}`}>{tag}</span>
                        {item.pinned && (
                          <span className="cbg-card-pin" aria-label="Pinned">
                            <Pin size={11} strokeWidth={2} fill="currentColor" />
                          </span>
                        )}

                        {isImage ? (
                          <>
                            <ImageThumb id={item.id} className="cbg-card-image" />
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
                              {(item.preview || item.text_content || '').slice(0, 400)}
                            </div>
                            <div className="cbg-card-meta">
                              {item.source_app && <span className="cbg-source-badge">{item.source_app}</span>}
                              <span className="cbg-card-time">{formatTime(item.timestamp)}</span>
                            </div>
                          </>
                        )}
                      </div>
                    );
                  })}
                </div>
              </div>
            ))
          )}
          {loading && <div className="cbg-loading">Loading…</div>}
        </div>

        {/* ── Right: detail pane — shares the body row with the grid ── */}
        {selected && (
        <>
          <div className="cbg-divider" />
          <div className="cbg-detail" style={{ width: `${effectivePreviewWidth}px` }}>
            <div
              className="cbg-detail-resize"
              onMouseDown={startResize}
              title="Drag to resize"
              role="separator"
              aria-label="Resize preview pane"
            />
            <button
              className="cbg-detail-close"
              onClick={() => { setSelectedId(null); setEditing(false); }}
              title="Close preview"
              type="button"
            >✕</button>
            <div className="cbg-detail-content">
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
                  >
                    {ocrLoading
                      ? 'Extracting…'
                      : (ocrText !== null && !ocrError ? 'Re-extract text' : 'Extract text')}
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
              <span className="cbg-meta-label">Captured</span>
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
                  <button className="cbg-dbtn cbg-dbtn-copy" onClick={() => handleCopy(selected.id)} type="button">Copy</button>
                </div>
              )}
            </div>
          </div>
        </>
        )}
      </div>
      </div>{/* /cbg-body */}

      {ctxMenu && (
        <div ref={ctxRef} className="cbg-ctx" style={{ top: ctxMenu.y, left: ctxMenu.x }}>
          <button className="cbg-ctx-item" onClick={() => handlePin(ctxMenu.id, ctxMenu.pinned)} type="button">
            {ctxMenu.pinned ? 'Unpin' : 'Pin'}
          </button>
          <button className="cbg-ctx-item cbg-ctx-del" onClick={() => handleDelete(ctxMenu.id)} type="button">Delete</button>
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
