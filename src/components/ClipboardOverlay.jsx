import React, { useState, useEffect, useRef, useLayoutEffect, useMemo, useCallback } from 'react';
import { listen } from '@tauri-apps/api/event';
import { Type, Link2, Mail, Hash, ExternalLink, Pin, GripVertical, ArrowLeftRight } from 'lucide-react';
import './ClipboardOverlay.css';
import ZoomableImage from './ZoomableImage';
import './ZoomableImage.css';
import { SearchBar } from './SearchBar';
import useOverlayDrag from './useOverlayDrag';
import { useAppIcon } from './appIconCache';
import { applyCached as applyCachedTheme } from '../theme/runtime';

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
    parts.push(<mark key={idx} className="co-hl">{text.slice(idx, idx + n.length)}</mark>);
    i = idx + n.length;
  }
  return parts;
}

// Incremental list rendering. The popup used to mount every history row (500
// items = ~5,100 DOM nodes and ~700 <img> thumbnails) and keep them alive while
// hidden. Only the first ROW_CHUNK timeline entries are rendered; more are
// appended as the user scrolls or arrows past the rendered range, and the
// limit collapses back when the window is parked (visibilityState 'hidden'),
// so a hidden popup holds ~60 rows, not the whole history.
const ROW_CHUNK = 60;

// Rows per backend page (2026-09-15). The popup used to receive 500 rows on
// every open and filter them in memory; on a long-retention history that was
// a ~600 ms page pass plus a multi-megabyte payload per open. Now an open
// costs ONE 50-row page, search + tag filters run on the writer thread (the
// same decrypt-and-scan the main panel uses, which also matches full body
// text past the preview boundary), and scrolling or arrowing past the end
// appends the next page. Must match CLIPBOARD_POPUP_PAGE_ROWS in lib.rs.
const PAGE_ROWS = 50;
const SEARCH_DEBOUNCE_MS = 150;

// ── Lazy image thumbnail loader ─────────────────────────────────────────────

function ImageThumb({ id, thumbB64, className, fallbackClass, zoomable }) {
  // v0.8.4: inline WebP thumb bypasses the viewport-lazy IntersectionObserver
  // path entirely — the payload already carries the data, no need to defer.
  // Legacy rows without a backfilled thumb keep the lazy full-res fetch.
  const [src, setSrc] = useState(thumbB64 ? `data:image/webp;base64,${thumbB64}` : null);
  const holderRef = useRef(null);
  useEffect(() => {
    if (thumbB64) { setSrc(`data:image/webp;base64,${thumbB64}`); return; }
    setSrc(null);
    let cancelled = false;
    let requested = false;
    const load = () => {
      if (requested || cancelled) return;
      requested = true;
      window.electronAPI?.getClipboardImage(id).then(b64 => {
        if (!cancelled && b64) setSrc(`data:image/png;base64,${b64}`);
      }).catch(() => {});
    };
    const el = holderRef.current;
    if (!el) { load(); return () => { cancelled = true; }; }
    const obs = new IntersectionObserver((entries) => {
      if (entries.some(e => e.isIntersecting)) { load(); obs.disconnect(); }
    });
    obs.observe(el);
    return () => { cancelled = true; obs.disconnect(); };
  }, [id, thumbB64]);
  if (!src) {
    return (
      <div ref={holderRef} className={fallbackClass || 'co-thumb-ph'}>
        <svg width="20" height="20" viewBox="0 0 32 32" fill="none" stroke="currentColor" strokeWidth="1.2">
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

// Icon-or-text badge — matches the ClipboardPanel pattern. Text fallback
// keeps the existing co-row-app class so pill styling doesn't drift.
function SourceAppBadge({ name, path }) {
  const icon = useAppIcon(name, path);
  if (!name) return null;
  if (icon) {
    return (
      <span className="co-row-app co-row-app-icon" title={name}>
        <img src={icon} width="12" height="12" alt="" draggable={false} />
      </span>
    );
  }
  return <span className="co-row-app">{name}</span>;
}

// ── Timeline grouping ───────────────────────────────────────────────────────

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

// ── Type icons (matches SearchOverlay's TYPE_META pattern) ──────────────────

const TYPE_ICONS = {
  Text:   { Icon: Type,   color: 'var(--blue-400)' },
  Link:   { Icon: Link2,  color: 'var(--yellow-400)' },
  Email:  { Icon: Mail,   color: 'var(--violet-400)' },
  Number: { Icon: Hash,   color: 'var(--text-secondary)' },
};

// ── Overlay ─────────────────────────────────────────────────────────────────

export default function ClipboardOverlay() {
  const { onGripPointerDown, onGripDoubleClick } = useOverlayDrag('clipboard');
  // List-left / preview-right by default; persisted machine-locally like the
  // window position.
  const [swapSides, setSwapSides] = useState(
    () => localStorage.getItem('trigr_clip_swap_sides') === '1'
  );
  const toggleSwapSides = () => setSwapSides(prev => {
    const next = !prev;
    localStorage.setItem('trigr_clip_swap_sides', next ? '1' : '0');
    return next;
  });
  const [items, setItems] = useState([]);
  // Backend paging state for the current query (search + tag): `total` is the
  // writer's match count, `page` the last page appended.
  const [total, setTotal] = useState(0);
  const pageRef = useRef(1);
  const loadingRef = useRef(false);
  // Request sequence: a response older than the latest request is dropped
  // (fast typing, or a search superseding an in-flight page append).
  const reqSeqRef = useRef(0);
  // Set when a show is under way and the Rust push is expected to supply
  // page 1; the query effect skips its own fetch for the default query while
  // this is true so an open costs one writer round trip, not two.
  const awaitingPushRef = useRef(false);
  // True for the render right after a page APPEND, so the "list changed →
  // selection back to the top" effect leaves the user's place alone.
  const appendPendingRef = useRef(false);
  const [renderLimit, setRenderLimit] = useState(ROW_CHUNK);
  const [selectedIndex, setSelectedIndex] = useState(0);
  // Theme is painted by src/theme/runtime.js (cached snapshot before React
  // mounts, live 'theme-changed' broadcasts, re-applied on every show in the
  // wake handler below). The payload's `theme` field is no longer read.
  const [search, setSearch] = useState('');
  const [filterTag, setFilterTag] = useState('All');
  // Live mirrors so the push handler / pager read the current query without
  // re-subscribing.
  const searchRef = useRef('');
  const filterTagRef = useRef('All');
  useEffect(() => { searchRef.current = search; }, [search]);
  useEffect(() => { filterTagRef.current = filterTag; }, [filterTag]);
  const isDefaultQuery = () => !searchRef.current.trim() && filterTagRef.current === 'All';
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState('');
  const rowRefs = useRef([]);
  const inputRef = useRef(null);

  // ── Data from Rust ────────────────────────────────────────────────────────

  // Data arrives asynchronously AFTER the window is already visible (Rust
  // fetches history off the processor thread for snappiness). It must NOT
  // reset search/selection: with the NOACTIVATE hook-routing path the user
  // may already be typing into search while the fetch runs, and wiping the
  // box when the payload lands would eat their input.
  // The wake handler below arms a deferred self-heal pull on every show; a
  // push landing first cancels it so a normal open costs ONE history fetch
  // on the clipboard writer thread instead of two (v0.8.13).
  const wakePullTimer = useRef(null);
  // performance.now() of the last hidden->visible transition; lets the push
  // handler report how long the visible popup waited for its list.
  const shownAtRef = useRef(0);
  useEffect(() => {
    window.electronAPI?.onClipboardOverlayData((data) => {
      // Rust never pushes a timed-out payload, but guard anyway: a timeout is
      // not an empty history and must not blank the list.
      if (data?.timed_out) return;
      if (wakePullTimer.current) {
        clearTimeout(wakePullTimer.current);
        wakePullTimer.current = null;
      }
      awaitingPushRef.current = false;
      // The push is always the DEFAULT page 1. If the user has already typed
      // a search (keys are routed from the first moment after the hotkey)
      // the query effect owns the list and this payload is stale for it.
      if (!isDefaultQuery()) return;
      const list = data?.items || [];
      // Perf breadcrumb (2026-09-15): only the slow case reaches the log. A
      // list that lands more than 300 ms after the popup became visible is
      // exactly the "takes a second to show the items" report.
      if (shownAtRef.current) {
        const waited = Math.round(performance.now() - shownAtRef.current);
        if (waited > 300) {
          window.electronAPI?.logPerf?.(`clipboard popup: pushed list landed ${waited}ms after visible (${list.length} rows, had ${itemsLenRef.current})`);
        }
      }
      reqSeqRef.current += 1; // supersede any in-flight default-page pull
      pageRef.current = 1;
      setItems(list);
      setTotal(typeof data?.total === 'number' ? data.total : list.length);
    });
    return () => window.electronAPI?.removeAllListeners('clipboard-overlay-data');
  }, []);

  // ── Self-heal pull ────────────────────────────────────────────────────────
  // The pushed clipboard-overlay-data event is fire-and-forget with two known
  // loss windows: (1) cold start — this lazy chunk may not have registered
  // its listener when the first show emits; (2) resume from webview_mem
  // TrySuspend — the resume/IPC-reconnect race can drop events emitted right
  // after Resume() (the same race that exempted the countdown window from
  // suspension). Either loss left the popup blank + default-dark until closed
  // and reopened. So the overlay pulls its own data whenever the push may
  // have been missed: at mount, on a reset event that finds no items, and on
  // the visibilitychange fired by resume_for_show's SetIsVisible(true) after
  // a suspend. A non-forced pull never clobbers a fresher push (items only
  // fill in while the list is empty); the wake pull is forced — same DB, and
  // the pushed payload for that show may be the thing that got lost.
  const itemsLenRef = useRef(0);
  useEffect(() => { itemsLenRef.current = items.length; }, [items]);

  // Backend page loader for the CURRENT query. page 1 replaces the list,
  // later pages append. Filters mirror the main panel's vocabulary: tag
  // 'All' = no tag filter; empty search = default timeline. promoteStarred
  // stays off so starred rows keep their place in the timeline (popup rule).
  const loadHistory = useCallback((page, append) => {
    const seq = ++reqSeqRef.current;
    loadingRef.current = true;
    const filters = {
      tagFilter: filterTagRef.current !== 'All' ? filterTagRef.current : null,
      search: searchRef.current.trim() || null,
    };
    return window.electronAPI?.getClipboardHistory?.(page, PAGE_ROWS, filters)
      .then((data) => {
        if (seq !== reqSeqRef.current) return; // superseded by a newer request
        // A writer-thread timeout comes back flagged (v0.8.13). It is not an
        // empty history: a forced pull used to overwrite a good pushed list
        // with nothing here — one candidate for the blank-popup report.
        if (data?.timed_out) return;
        const list = data?.items || [];
        pageRef.current = page;
        if (append) appendPendingRef.current = true;
        setItems(prev => (append ? [...prev, ...list] : list));
        setTotal(typeof data?.total === 'number' ? data.total : list.length);
      })
      .catch(() => {})
      .finally(() => { if (seq === reqSeqRef.current) loadingRef.current = false; });
  }, []);

  // Self-heal: a non-forced pull only fills an empty popup; a forced one
  // reloads page 1 of the current query.
  const selfHealPull = useCallback((force) => {
    if (!force && itemsLenRef.current > 0) return;
    awaitingPushRef.current = false;
    loadHistory(1, false);
  }, [loadHistory]);

  // (No separate mount pull: the query effect below fires once at mount for
  // the default query, which fills the hidden popup ahead of its first show.)

  // Query changes (typed search, tag pill, cleared search) reload page 1
  // from the writer. Search is debounced so a fast typist fires one scan,
  // not one per keystroke; tag changes are immediate. The default query
  // right after a show is skipped while the Rust push is expected: the
  // reset event clears search/tag on every open, and without this guard
  // that clear would race the push with a second identical fetch.
  useEffect(() => {
    const defaultQuery = !search.trim() && filterTag === 'All';
    if (defaultQuery && awaitingPushRef.current) return;
    const timer = setTimeout(() => loadHistory(1, false), search.trim() ? SEARCH_DEBOUNCE_MS : 0);
    return () => clearTimeout(timer);
  }, [search, filterTag, loadHistory]);

  // Append the next page when the user scrolls or arrows past what is loaded.
  const hasMore = items.length < total;
  const loadNextPage = useCallback(() => {
    if (loadingRef.current || itemsLenRef.current >= total) return;
    loadHistory(pageRef.current + 1, true);
  }, [loadHistory, total]);

  // Wake / park hook. webview_mem parks every hidden window with
  // SetIsVisible(false), so visibilityState is truthful here and this fires
  // hidden->visible on EVERY show (before v0.8.11 it fired only after a
  // TrySuspend resume). The forced re-pull still covers the suspend/IPC-
  // reconnect race that can drop the show path's reset + data emits, but it
  // is DEFERRED (v0.8.13): Rust pushes the same 500-row page for every show
  // and both requests queued on the single clipboard writer thread, each
  // decrypting 500 previews + thumbnails. The pull now waits WAKE_PULL_MS and
  // the push handler cancels it, so the pull only runs when the push was
  // actually lost (or the writer is slow enough that a retry is welcome).
  // The hidden branch shrinks the mounted list back to one chunk.
  const WAKE_PULL_MS = 1200;
  useEffect(() => {
    const onVis = () => {
      if (document.visibilityState !== 'visible') {
        setRenderLimit(ROW_CHUNK);
        if (wakePullTimer.current) {
          clearTimeout(wakePullTimer.current);
          wakePullTimer.current = null;
        }
        return;
      }
      applyCachedTheme();
      shownAtRef.current = performance.now();
      awaitingPushRef.current = true;
      if (wakePullTimer.current) clearTimeout(wakePullTimer.current);
      wakePullTimer.current = setTimeout(() => {
        wakePullTimer.current = null;
        // The push did not arrive within WAKE_PULL_MS: this is the case the
        // perf review wants counted. If the popup was empty the user saw a
        // blank list until this pull returns.
        window.electronAPI?.logPerf?.(`clipboard popup: no pushed list within ${WAKE_PULL_MS}ms of visible, self-heal pull running (had ${itemsLenRef.current} rows)`);
        selfHealPull(true);
      }, WAKE_PULL_MS);
      setSelectedIndex(0);
      setSearch('');
      setFilterTag('All');
      setEditing(false);
      setEditText('');
      setTimeout(() => inputRef.current?.focus(), 50);
    };
    document.addEventListener('visibilitychange', onVis);
    return () => document.removeEventListener('visibilitychange', onVis);
  }, [selfHealPull]);

  // Show-time reset — emitted by Rust at the top of BOTH show paths (normal
  // + fill-in mode), before any routed keystrokes, so every open starts with
  // a clean search box and selection regardless of when the data payload
  // arrives.
  useEffect(() => {
    const unlistenPromise = listen('clipboard-overlay-reset', () => {
      // Rust pushes page 1 right after this; let it own the default query.
      awaitingPushRef.current = true;
      setSelectedIndex(0);
      setSearch('');
      setFilterTag('All');
      setEditing(false);
      setEditText('');
      setTimeout(() => inputRef.current?.focus(), 50);
      // Reset arrived but the popup holds nothing — the data payload for a
      // previous show (or this one) was likely dropped. Pull instead of
      // waiting on a push that may never come.
      if (itemsLenRef.current === 0) selfHealPull(false);
    });
    return () => { unlistenPromise.then(fn => fn()); };
  }, [selfHealPull]);

  // ── Filtering ─────────────────────────────────────────────────────────────

  // Search + tag filtering moved to the writer (2026-09-15): `items` already
  // IS the filtered page set for the current query, so no client-side pass.
  // Kept under the old name so the render / key-routing code below is
  // unchanged. Search-inside-images (Pro + setting) and full-body text hits
  // are decided by search_history in clipboard.rs, same as the main panel.
  const filtered = items;

  const groupedFlat = useMemo(() => {
    const groups = groupByTimeline(filtered);
    const result = [];
    let idx = 0;
    for (const [label, groupItems] of groups) {
      result.push({ type: 'header', label });
      for (const item of groupItems) {
        result.push({ type: 'item', item, flatIndex: idx++ });
      }
    }
    return result;
  }, [filtered]);

  useEffect(() => {
    // A page append grows the list under the user's selection: keep it.
    // Every other change (new query result, delete) restarts at the top.
    if (appendPendingRef.current) { appendPendingRef.current = false; return; }
    setSelectedIndex(0); setEditing(false); setRenderLimit(ROW_CHUNK);
  }, [filtered.length]);

  // Keep the selected row mounted when arrowing past the rendered range, and
  // fetch the next backend page when the selection nears the loaded end.
  useEffect(() => {
    const pos = groupedFlat.findIndex(e => e.type === 'item' && e.flatIndex === selectedIndex);
    if (pos >= 0) setRenderLimit(l => (pos + 8 > l ? pos + ROW_CHUNK : l));
    if (hasMore && selectedIndex >= items.length - 8) loadNextPage();
  }, [selectedIndex, groupedFlat, hasMore, items.length, loadNextPage]);

  // Append the next chunk as the list nears its rendered end; once every
  // loaded row is rendered, ask the writer for the next page.
  const onListScroll = useCallback((e) => {
    const el = e.currentTarget;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 240) {
      setRenderLimit(l => (l < groupedFlat.length ? l + ROW_CHUNK : l));
      if (renderLimit >= groupedFlat.length && hasMore) loadNextPage();
    }
  }, [groupedFlat.length, renderLimit, hasMore, loadNextPage]);

  // Cancel edit when selection changes
  useEffect(() => { setEditing(false); setEditText(''); }, [selectedIndex]);

  const selectedEntry = groupedFlat.find(e => e.type === 'item' && e.flatIndex === selectedIndex);
  const selected = selectedEntry?.item || null;

  // Lazy-fetch text_content + html_content on selection for text rows.
  // The list payload no longer ships text_content (dropped in the clipboard
  // perf patch); overlay Paste button uses pasteClipboardItem(id) so the
  // backend handles that path race-free, but the detail pane display and
  // the "Paste plain" button need the full text.
  useEffect(() => {
    if (!selected || selected.content_type !== 'text') return;
    if (selected.text_content != null) return;
    const id = selected.id;
    let cancelled = false;
    window.electronAPI?.getClipboardItemTextFull?.(id).then(full => {
      if (cancelled || !full) return;
      const text = full.text_content ?? '';
      const html = full.html_content ?? null;
      setItems(prev => prev.map(it => it.id === id
        ? { ...it, text_content: text, html_content: html }
        : it));
    }).catch(() => {});
    return () => { cancelled = true; };
  }, [selected]);

  // ── LL hook keyboard routing (WS_EX_NOACTIVATE path) ─────────────────────
  // When the overlay is open it never steals OS focus (WS_EX_NOACTIVATE).
  // The Rust LL hook intercepts keystrokes and emits 'clipboard-overlay-key'
  // with { vk, shift } so navigation and search still work without DOM focus.
  //
  // Refs give the one-time listener current values without re-registering on
  // every keystroke. The previous `[filtered, selected, editing]` deps re-ran
  // the effect on every search change; `listen()`/unlisten are async and the
  // new subscription could finish before the old one tore down, so multiple
  // handlers ended up alive and each keystroke wrote N copies of itself into
  // the search box (bug: 'the' became 'tthhee'). See
  // [[feedback_tauri_listener_registration_race]].
  const editingRef = useRef(editing);
  const selectedRef = useRef(selected);
  const filteredRef = useRef(filtered);
  useEffect(() => { editingRef.current = editing; }, [editing]);
  useEffect(() => { selectedRef.current = selected; }, [selected]);
  useEffect(() => { filteredRef.current = filtered; }, [filtered]);

  useEffect(() => {
    function vkToChar(vk, shift) {
      if (vk >= 65 && vk <= 90) return shift ? String.fromCharCode(vk) : String.fromCharCode(vk + 32);
      if (vk >= 48 && vk <= 57) return String.fromCharCode(vk);
      if (vk === 32) return ' ';
      return null;
    }

    function handleHookKey({ payload }) {
      const { vk, shift } = payload || {};
      if (editingRef.current) {
        if (vk === 27) { setEditing(false); setEditText(''); }
        return;
      }
      if (vk === 27) { window.electronAPI?.closeClipboardOverlay(); return; }
      if (vk === 13) {
        const sel = selectedRef.current;
        if (sel) { window.electronAPI?.closeClipboardOverlay(); window.electronAPI?.pasteClipboardItem(sel.id); }
        return;
      }
      if (vk === 40) { setSelectedIndex(i => Math.min(i + 1, filteredRef.current.length - 1)); return; }
      if (vk === 38) { setSelectedIndex(i => Math.max(i - 1, 0)); return; }
      if (vk === 8)  { setSearch(q => q.slice(0, -1)); return; }
      const ch = vkToChar(vk, shift);
      if (ch !== null) setSearch(q => q + ch);
    }

    const unlistenPromise = listen('clipboard-overlay-key', handleHookKey);
    return () => { unlistenPromise.then(fn => fn()); };
  }, []);

  // DOM keyboard fallback for fill-in mode. When the popup is opened from
  // the fill-in webview it's activated (real OS focus, not NOACTIVATE) so
  // its own DOM handles keystrokes — the LL-hook routing above is skipped
  // in Rust for that mode. In normal mode this listener also fires when
  // the SearchBar input is focused, but that's fine: arrow/Enter/Escape
  // handling matches the LL-hook path and search-input typing is owned by
  // the SearchBar's onChange, so both paths agree without conflicting.
  useEffect(() => {
    function onKeyDown(e) {
      if (editingRef.current) {
        if (e.key === 'Escape') { setEditing(false); setEditText(''); e.preventDefault(); }
        return;
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        window.electronAPI?.closeClipboardOverlay();
        return;
      }
      if (e.key === 'Enter') {
        e.preventDefault();
        const sel = selectedRef.current;
        if (sel) {
          window.electronAPI?.pasteClipboardItem(sel.id);
        } else {
          window.electronAPI?.closeClipboardOverlay();
        }
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex(i => Math.min(i + 1, filteredRef.current.length - 1));
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex(i => Math.max(i - 1, 0));
        return;
      }
    }
    document.addEventListener('keydown', onKeyDown);
    return () => document.removeEventListener('keydown', onKeyDown);
  }, []);

  useLayoutEffect(() => {
    const el = rowRefs.current[selectedIndex];
    if (el) el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [selectedIndex, renderLimit]);

  // ── Resize window to panel ────────────────────────────────────────────────

  // ── Helpers ───────────────────────────────────────────────────────────────

  const formatTime = (ts) => {
    try {
      const d = new Date(ts);
      const diff = Date.now() - d.getTime();
      if (diff < 60000) return 'Just now';
      if (diff < 3600000) return `${Math.floor(diff / 60000)}m`;
      if (diff < 86400000) return `${Math.floor(diff / 3600000)}h`;
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

  // ── Helpers ────────────────────────────────────────────────────────────────

  const parseColour = (text) => {
    if (!text) return null;
    const t = text.trim();
    if (t.startsWith('#') && t.length >= 4 && t.length <= 7 && /^#[0-9a-fA-F]+$/.test(t)) return t;
    if (t.startsWith('rgb')) return t;
    return null;
  };

  const rowIcon = (item) => {
    const tag = item.content_tag || 'Text';
    if (tag === 'Colour') {
      const c = parseColour(item.preview);
      return <span className="co-row-icon"><span className="co-row-icon-dot" style={{ background: c || 'var(--text-muted)' }} /></span>;
    }
    const meta = TYPE_ICONS[tag] || TYPE_ICONS.Text;
    const MetaIcon = meta.Icon;
    return (
      <span className="co-row-icon" style={{ color: meta.color }}>
        <MetaIcon size={14} strokeWidth={1.75} />
      </span>
    );
  };

  // ── Inline edit ───────────────────────────────────────────────────────────

  const isTextEditable = selected && selected.content_type === 'text';

  const handleStartEdit = async () => {
    setEditing(true);
    if (selected.text_content != null) {
      setEditText(selected.text_content);
      return;
    }
    setEditText(selected.preview || '');
    try {
      const full = await window.electronAPI?.getClipboardItemTextFull?.(selected.id);
      const text = full?.text_content;
      if (text != null) {
        setEditText(text);
        setItems(prev => prev.map(it => it.id === selected.id
          ? { ...it, text_content: text, html_content: full?.html_content ?? null }
          : it));
      }
    } catch (_) {}
  };

  const handleSaveEdit = async () => {
    if (!selected) return;
    const newTag = await window.electronAPI?.updateClipboardItem(selected.id, editText);
    if (newTag) {
      const newPreview = editText.length > 200 ? editText.slice(0, 200) + '…' : editText;
      setItems(prev => prev.map(it =>
        it.id === selected.id
          ? { ...it, text_content: editText, preview: newPreview, content_tag: newTag }
          : it
      ));
    }
    setEditing(false);
  };

  // ── Render ────────────────────────────────────────────────────────────────

  return (
    <div className="co-root">
      <div className="co-panel">
        <div className={`co-panes${swapSides ? ' co-swapped' : ''}`}>

        {/* ── LEFT: list pane ── */}
        <div className="co-left">
          <div className="co-input-row">
            <span
              className="co-grip"
              title="Drag to move · Double-click to reset position"
              onPointerDown={onGripPointerDown}
              onDoubleClick={onGripDoubleClick}
              aria-hidden="true"
            >
              <GripVertical size={14} strokeWidth={1.75} />
            </span>
            <SearchBar
              ref={inputRef}
              className="co-search-bar"
              placeholder="Search clipboard…"
              value={search}
              onChange={e => setSearch(e.target.value)}
            />
            <button
              className="co-swap-btn"
              type="button"
              title="Swap list and preview sides"
              onClick={toggleSwapSides}
            >
              <ArrowLeftRight size={13} strokeWidth={1.75} />
            </button>
          </div>
          <div className="co-tag-pills">
            {['All', 'Text', 'Image', 'Number', 'Link', 'Email', 'Colour'].map(tag => (
              <button
                key={tag}
                className={`co-tag-pill${filterTag === tag ? ' co-tag-active' : ''}`}
                onClick={() => setFilterTag(tag)}
                type="button"
              >{tag}</button>
            ))}
          </div>
          <div className="co-left-list" onScroll={onListScroll}>
            {filtered.length === 0 ? (
              <div className="co-empty">{search.trim() || filterTag !== 'All' ? 'No matches' : 'No history'}</div>
            ) : (
              groupedFlat.slice(0, renderLimit).map((entry) => {
                if (entry.type === 'header') {
                  return <div key={`h-${entry.label}`} className="co-timeline-header">{entry.label}</div>;
                }
                const { item, flatIndex: i } = entry;
                const isImage = item.content_type === 'image';
                return (
                  <div
                    key={item.id}
                    ref={el => (rowRefs.current[i] = el)}
                    className={`co-row${i === selectedIndex ? ' co-row-sel' : ''}${item.pinned ? ' co-row-pin' : ''}`}
                    onMouseEnter={() => setSelectedIndex(i)}
                    onClick={() => {
                      window.electronAPI?.closeClipboardOverlay();
                      window.electronAPI?.pasteClipboardItem(item.id);
                    }}
                  >
                    {isImage ? (
                      <>
                        <ImageThumb id={item.id} thumbB64={item.thumb_b64} className="co-row-thumb" fallbackClass="co-row-thumb-ph" />
                        <div className="co-row-body">
                          <span className="co-row-text">{item.image_width}×{item.image_height}</span>
                          <span className="co-row-sub">
                            {item.source_app && <SourceAppBadge name={item.source_app} path={item.source_app_path} />}
                            <span className="co-row-time">{formatTime(item.timestamp)}</span>
                          </span>
                        </div>
                      </>
                    ) : (
                      <>
                        {rowIcon(item)}
                        <div className="co-row-body co-row-body-full">
                          <span className="co-row-text co-row-text-2">{highlightMatches((item.preview || '').slice(0, 160), search.trim())}</span>
                          <span className="co-row-sub">
                            {item.source_app && <SourceAppBadge name={item.source_app} path={item.source_app_path} />}
                            <span className="co-row-time">{formatTime(item.timestamp)}</span>
                          </span>
                        </div>
                      </>
                    )}
                    {item.pinned && (
                      <span className="co-row-pin-badge" aria-label="Pinned">
                        <Pin size={11} strokeWidth={2} fill="currentColor" />
                      </span>
                    )}
                  </div>
                );
              })
            )}
          </div>
        </div>

        {/* ── DIVIDER ── */}
        <div className="co-divider" />

        {/* ── RIGHT: detail pane ── */}
        <div className="co-right">
          {selected ? (
            <div className="co-detail">
              <div className="co-detail-content">
                {selected.content_type === 'image' ? (
                  <div className="co-detail-img-wrap">
                    <ImageThumb id={selected.id} className="co-detail-img" fallbackClass="co-detail-img-ph" zoomable />
                  </div>
                ) : editing ? (
                  <textarea
                    className="co-detail-textarea"
                    value={editText}
                    onChange={e => setEditText(e.target.value)}
                    autoFocus
                    spellCheck={false}
                  />
                ) : (
                  <pre className="co-detail-text">{highlightMatches(selected.text_content || selected.preview || '', search.trim())}</pre>
                )}
              </div>
              <div className="co-detail-meta">
                {selected.source_app && (
                  <>
                    <span className="co-meta-label">Source App</span>
                    <span className="co-meta-value">{selected.source_app}</span>
                  </>
                )}
                <span className="co-meta-label">Type</span>
                <span className="co-meta-value">{selected.content_tag || 'Text'}</span>
                {selected.content_type === 'image' && (
                  <>
                    <span className="co-meta-label">Dimensions</span>
                    <span className="co-meta-value">{selected.image_width} × {selected.image_height} px</span>
                  </>
                )}
                <span className="co-meta-label">Captured</span>
                <span className="co-meta-value">{formatFullTime(selected.timestamp)}</span>
                {selected.content_type !== 'image' && (
                  <>
                    <span className="co-meta-label">Characters</span>
                    <span className="co-meta-value">{(selected.text_content || selected.preview || '').length}</span>
                  </>
                )}
              </div>
              <div className="co-detail-actions">
                <div className="co-detail-actions-l">
                  <button className="co-btn co-btn-pin" type="button"
                    onClick={e => { e.stopPropagation(); window.electronAPI?.pinClipboardItem(selected.id, !selected.pinned); setItems(prev => prev.map(it => it.id === selected.id ? { ...it, pinned: !it.pinned } : it)); }}
                  >{selected.pinned ? 'Unpin' : 'Pin'}</button>
                  {isTextEditable && !editing && (
                    <button className="co-btn" type="button" onClick={e => { e.stopPropagation(); handleStartEdit(); }}>Edit</button>
                  )}
                  {editing && (
                    <>
                      <button className="co-btn co-btn-paste" type="button" onClick={e => { e.stopPropagation(); handleSaveEdit(); }}>Save</button>
                      <button className="co-btn" type="button" onClick={e => { e.stopPropagation(); setEditing(false); setEditText(''); }}>Cancel</button>
                    </>
                  )}
                </div>
                <div className="co-detail-actions-r">
                  <button className="co-btn co-btn-del" type="button"
                    onClick={e => { e.stopPropagation(); window.electronAPI?.deleteClipboardItem(selected.id); setItems(prev => prev.filter(it => it.id !== selected.id)); }}
                  >Delete</button>
                  {selected.has_html && (
                    <button
                      className="co-btn"
                      type="button"
                      title="Paste without formatting"
                      onClick={async e => {
                        e.stopPropagation();
                        // Race-safe: fetch full text before pasting plain. If
                        // fetch fails, fall back to preview (better than empty).
                        let text = selected.text_content;
                        if (text == null) {
                          try {
                            const full = await window.electronAPI?.getClipboardItemTextFull?.(selected.id);
                            text = full?.text_content ?? selected.preview ?? '';
                          } catch (_) { text = selected.preview ?? ''; }
                        }
                        window.electronAPI?.closeClipboardOverlay();
                        window.electronAPI?.pasteText(text, selected.id);
                      }}
                    >Paste plain</button>
                  )}
                  <button className="co-btn co-btn-paste" type="button"
                    onClick={e => { e.stopPropagation(); window.electronAPI?.closeClipboardOverlay(); window.electronAPI?.pasteClipboardItem(selected.id); }}
                  >Paste</button>
                </div>
              </div>
            </div>
          ) : (
            <div className="co-detail-empty">Select an item to preview</div>
          )}
        </div>

        </div>
      </div>
    </div>
  );
}
