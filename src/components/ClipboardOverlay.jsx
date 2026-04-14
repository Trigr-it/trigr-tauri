import React, { useState, useEffect, useRef, useLayoutEffect, useMemo } from 'react';
import './ClipboardOverlay.css';
import ZoomableImage from './ZoomableImage';
import './ZoomableImage.css';

// ── Lazy image thumbnail loader ─────────────────────────────────────────────

function ImageThumb({ id, className, fallbackClass, zoomable }) {
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
      <div className={fallbackClass || 'co-thumb-ph'}>
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

// ── Overlay ─────────────────────────────────────────────────────────────────

export default function ClipboardOverlay() {
  const [items, setItems] = useState([]);
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [theme, setTheme] = useState('dark');
  const [search, setSearch] = useState('');
  const [filterTag, setFilterTag] = useState('All');
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState('');
  const rowRefs = useRef([]);
  const inputRef = useRef(null);

  // ── Data from Rust ────────────────────────────────────────────────────────

  useEffect(() => {
    window.electronAPI?.onClipboardOverlayData((data) => {
      const list = data?.items || [];
      setItems(list);
      setSelectedIndex(0);
      setSearch('');
      setFilterTag('All');
      if (data?.theme) setTheme(data.theme);
      setTimeout(() => inputRef.current?.focus(), 50);
    });
    return () => window.electronAPI?.removeAllListeners('clipboard-overlay-data');
  }, []);

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);

  // ── Filtering ─────────────────────────────────────────────────────────────

  const filtered = useMemo(() => {
    return items.filter(i => {
      if (search.trim() && !(i.preview || i.text_content || '').toLowerCase().includes(search.toLowerCase())) return false;
      if (filterTag !== 'All' && i.content_tag !== filterTag) return false;
      return true;
    });
  }, [items, search, filterTag]);

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

  useEffect(() => { setSelectedIndex(0); setEditing(false); }, [filtered.length]);

  // Cancel edit when selection changes
  useEffect(() => { setEditing(false); setEditText(''); }, [selectedIndex]);

  const selectedEntry = groupedFlat.find(e => e.type === 'item' && e.flatIndex === selectedIndex);
  const selected = selectedEntry?.item || null;

  // ── Keyboard nav ──────────────────────────────────────────────────────────

  useEffect(() => {
    function handleKeyDown(e) {
      // Don't intercept keys when editing text
      if (editing) {
        if (e.key === 'Escape') { e.preventDefault(); setEditing(false); setEditText(''); }
        return;
      }
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        setSelectedIndex(i => Math.min(i + 1, filtered.length - 1));
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        setSelectedIndex(i => Math.max(i - 1, 0));
      } else if (e.key === 'Enter') {
        e.preventDefault();
        if (selected) {
          window.electronAPI?.closeClipboardOverlay();
          window.electronAPI?.pasteClipboardItem(selected.id);
        }
      } else if (e.key === 'Escape') {
        e.preventDefault();
        window.electronAPI?.closeClipboardOverlay();
      }
    }
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [filtered, selectedIndex, selected, editing]);

  useLayoutEffect(() => {
    const el = rowRefs.current[selectedIndex];
    if (el) el.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
  }, [selectedIndex]);

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
    if (tag === 'Link') return <span className="co-row-icon">🔗</span>;
    if (tag === 'Email') return <span className="co-row-icon">✉️</span>;
    if (tag === 'Number') return <span className="co-row-icon co-row-icon-hash">#</span>;
    if (tag === 'Colour') {
      const c = parseColour(item.text_content || item.preview);
      return <span className="co-row-icon co-row-icon-dot" style={{ background: c || 'var(--text-muted)' }} />;
    }
    return <span className="co-row-icon">📄</span>;
  };

  // ── Inline edit ───────────────────────────────────────────────────────────

  const isTextEditable = selected && selected.content_type === 'text'
    && (selected.content_tag === 'Text' || selected.content_tag === 'Number');

  const handleStartEdit = () => {
    setEditing(true);
    setEditText(selected.text_content || selected.preview || '');
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
        <div className="co-panes">

        {/* ── LEFT: list pane ── */}
        <div className="co-left">
          <div className="co-left-search">
            <input
              ref={inputRef}
              className="co-search"
              placeholder="Search…"
              value={search}
              onChange={e => setSearch(e.target.value)}
              spellCheck={false}
            />
            <div className="co-tag-pills">
              {['All', 'Text', 'Link', 'Email', 'Colour', 'Number', 'Image'].map(tag => (
                <button
                  key={tag}
                  className={`co-tag-pill${filterTag === tag ? ' co-tag-active' : ''}`}
                  onClick={() => setFilterTag(tag)}
                  type="button"
                >{tag}</button>
              ))}
            </div>
          </div>
          <div className="co-left-list">
            {filtered.length === 0 ? (
              <div className="co-empty">{items.length === 0 ? 'No history' : 'No matches'}</div>
            ) : (
              groupedFlat.map((entry) => {
                if (entry.type === 'header') {
                  return <div key={`h-${entry.label}`} className="co-timeline-header">{entry.label === 'Pinned' ? '📌 Pinned' : entry.label}</div>;
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
                        <ImageThumb id={item.id} className="co-row-thumb" fallbackClass="co-row-thumb-ph" />
                        <div className="co-row-body">
                          <span className="co-row-text">{item.image_width}×{item.image_height}</span>
                          <span className="co-row-sub">
                            {item.source_app && <span className="co-row-app">{item.source_app}</span>}
                            <span className="co-row-time">{formatTime(item.timestamp)}</span>
                          </span>
                        </div>
                      </>
                    ) : (
                      <>
                        {rowIcon(item)}
                        <div className="co-row-body co-row-body-full">
                          <span className="co-row-text co-row-text-2">{(item.preview || item.text_content || '').slice(0, 160)}</span>
                          <span className="co-row-sub">
                            {item.source_app && <span className="co-row-app">{item.source_app}</span>}
                            <span className="co-row-time">{formatTime(item.timestamp)}</span>
                          </span>
                        </div>
                      </>
                    )}
                    {item.pinned && <span className="co-row-pin-badge">📌</span>}
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
                  <pre className="co-detail-text">{selected.text_content || selected.preview || ''}</pre>
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
        {/* ── Keyboard hints bar ── */}
        <div className="co-hints">
          <span>↑↓  Navigate</span>
          <span>↵  Paste</span>
          <span>Esc  Close</span>
        </div>
      </div>
    </div>
  );
}
