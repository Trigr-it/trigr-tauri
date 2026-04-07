import React, { useState, useEffect, useRef, useCallback } from 'react';
import './ClipboardPanel.css';

// ── Lazy image thumbnail ────────────────────────────────────────────────────

function ImageThumb({ id, className }) {
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
  return <img className={className} src={src} alt="" />;
}

const ALL_TAGS = ['All', 'Text', 'Image', 'Number', 'Link', 'Email', 'Colour'];

export default function ClipboardPanel() {
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
  const [selectedId, setSelectedId] = useState(null);
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState('');
  const ctxRef = useRef(null);
  const gridRef = useRef(null);

  const PER_PAGE = 50;

  const loadHistory = useCallback(async (p = 1, append = false) => {
    setLoading(true);
    try {
      const result = await window.electronAPI?.getClipboardHistory(p, PER_PAGE);
      if (result) {
        setItems(prev => append ? [...prev, ...result.items] : result.items);
        setTotal(result.total);
        setPage(p);
      }
    } catch (e) { /* ignore */ }
    setLoading(false);
  }, []);

  useEffect(() => {
    loadHistory(1);
    window.electronAPI?.getDistinctSourceApps?.().then(apps => {
      if (apps) setSourceApps(apps);
    });
  }, [loadHistory]);

  useEffect(() => {
    window.electronAPI?.onClipboardNewItem((item) => {
      setItems(prev => [item, ...prev]);
      setTotal(t => t + 1);
      if (item.source_app && !sourceApps.includes(item.source_app)) {
        setSourceApps(prev => [...prev, item.source_app].sort());
      }
    });
    return () => window.electronAPI?.removeAllListeners('clipboard-new-item');
  }, [sourceApps]);

  useEffect(() => {
    if (!ctxMenu) return;
    const handler = (e) => {
      if (ctxRef.current && !ctxRef.current.contains(e.target)) setCtxMenu(null);
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [ctxMenu]);

  // Cancel edit when selection changes
  useEffect(() => {
    setEditing(false);
    setEditText('');
  }, [selectedId]);

  // Escape key: cancel edit if editing, otherwise deselect
  useEffect(() => {
    function handleKeyDown(e) {
      if (e.key !== 'Escape') return;
      if (editing) {
        setEditing(false);
        setEditText('');
      } else if (selectedId !== null) {
        setSelectedId(null);
      }
    }
    document.addEventListener('keydown', handleKeyDown);
    return () => document.removeEventListener('keydown', handleKeyDown);
  }, [editing, selectedId]);

  const handlePaste = async (id) => { await window.electronAPI?.pasteClipboardItem(id); };

  const handleDelete = async (id) => {
    const ok = await window.electronAPI?.deleteClipboardItem(id);
    if (ok) {
      setItems(prev => prev.filter(i => i.id !== id));
      setTotal(t => t - 1);
      if (selectedId === id) setSelectedId(null);
    }
    setCtxMenu(null);
  };

  const handlePin = async (id, pinned) => {
    const ok = await window.electronAPI?.pinClipboardItem(id, !pinned);
    if (ok) { setItems(prev => prev.map(i => i.id === id ? { ...i, pinned: !pinned } : i)); }
    setCtxMenu(null);
  };

  const handleClearAll = async () => {
    const ok = await window.electronAPI?.clearClipboardHistory();
    if (ok) { setItems([]); setTotal(0); setSelectedId(null); }
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

  const filtered = items.filter(i => {
    if (search.trim() && !(i.preview || '').toLowerCase().includes(search.toLowerCase())) return false;
    if (filterApp && i.source_app !== filterApp) return false;
    if (filterTag !== 'All' && i.content_tag !== filterTag) return false;
    return true;
  });

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

  const isTextOnly = selected && selected.content_type === 'text' && (selected.content_tag === 'Text' || selected.content_tag === 'Number');

  return (
    <div className={`cbg-panel${selected ? ' cbg-panel-split' : ''}`}>
      {/* ── Left: toolbar + grid ── */}
      <div className="cbg-main">
        <div className="cbg-toolbar">
          <input
            className="cbg-search"
            placeholder="Search clipboard history…"
            value={search}
            onChange={e => setSearch(e.target.value)}
            spellCheck={false}
          />
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

        <div className="cbg-filters">
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
        </div>

        <div className="cbg-grid-wrap" ref={gridRef} onScroll={handleScroll}>
          {filtered.length === 0 ? (
            <div className="cbg-empty">
              {items.length === 0 ? 'No clipboard history yet — copy something to get started' : 'No results'}
            </div>
          ) : (
            <div className={`cbg-grid${selected ? ' cbg-grid-2col' : ''}`}>
              {filtered.map(item => {
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
                    {item.pinned && <span className="cbg-card-pin">📌</span>}

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
                          {isLink && <span className="cbg-link-icon">🔗 </span>}
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
          )}
          {loading && <div className="cbg-loading">Loading…</div>}
        </div>
      </div>

      {/* ── Right: detail pane ── */}
      {selected && (
        <>
          <div className="cbg-divider" />
          <div className="cbg-detail">
            <div className="cbg-detail-content">
              {selected.content_type === 'image' ? (
                <ImageThumb id={selected.id} className="cbg-detail-img" />
              ) : editing ? (
                <textarea
                  className="cbg-detail-textarea"
                  value={editText}
                  onChange={e => setEditText(e.target.value)}
                  autoFocus
                  spellCheck={false}
                />
              ) : (
                <pre className="cbg-detail-text">{selected.text_content || selected.preview || ''}</pre>
              )}
            </div>
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
                <button className="cbg-dbtn" onClick={() => handlePin(selected.id, selected.pinned)} type="button">
                  {selected.pinned ? '📌 Unpin' : '📌 Pin'}
                </button>
                {isTextOnly && !editing && (
                  <button className="cbg-dbtn" onClick={() => handleStartEdit(selected)} type="button">Edit</button>
                )}
                {editing && (
                  <>
                    <button className="cbg-dbtn cbg-dbtn-save" onClick={handleSaveEdit} type="button">Save</button>
                    <button className="cbg-dbtn" onClick={handleCancelEdit} type="button">Cancel</button>
                  </>
                )}
              </div>
              <div className="cbg-detail-actions-r">
                <button className="cbg-dbtn cbg-dbtn-del" onClick={() => handleDelete(selected.id)} type="button">Delete</button>
                <button className="cbg-dbtn cbg-dbtn-paste" onClick={() => handlePaste(selected.id)} type="button">Paste</button>
              </div>
            </div>
          </div>
        </>
      )}

      {ctxMenu && (
        <div ref={ctxRef} className="cbg-ctx" style={{ top: ctxMenu.y, left: ctxMenu.x }}>
          <button className="cbg-ctx-item" onClick={() => handlePin(ctxMenu.id, ctxMenu.pinned)} type="button">
            {ctxMenu.pinned ? 'Unpin' : 'Pin'}
          </button>
          <button className="cbg-ctx-item cbg-ctx-del" onClick={() => handleDelete(ctxMenu.id)} type="button">Delete</button>
        </div>
      )}
    </div>
  );
}
