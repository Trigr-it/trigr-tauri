import React, { useState, useEffect, useCallback, useRef } from 'react';
import RadialWheel from './RadialWheel';
import './RadialMenu.css';

export default function RadialMenu() {
  const [items, setItems] = useState([]);
  const [hoveredIndex, setHoveredIndex] = useState(-1);
  const [hoveredOuterIndex, setHoveredOuterIndex] = useState(-1);
  const [expandedFolder, setExpandedFolder] = useState(null);
  const [animKey, setAnimKey] = useState(0);

  const itemsRef = useRef([]);
  const expandedFolderRef = useRef(null);

  itemsRef.current = items;
  expandedFolderRef.current = expandedFolder;

  // ── Listen for data from Rust ──────────────────────────────────────────
  useEffect(() => {
    window.electronAPI?.onRadialMenuData((data) => {
      document.documentElement.setAttribute('data-theme', data.theme || 'dark');
      setItems(data.items || []);
      setHoveredIndex(-1);
      setHoveredOuterIndex(-1);
      setExpandedFolder(null);
      setAnimKey(k => k + 1);
    });
  }, []);

  // ── Fire an item ───────────────────────────────────────────────────────
  const fireItem = useCallback((item) => {
    if (!item || !item.exists) return;
    const payload = { type: item.type, storageKey: item.storageKey, label: item.label };
    if (item.data?.text != null) payload.text = item.data.text;
    if (item.data?.html != null) payload.html = item.data.html;
    window.electronAPI?.executeRadialMenuItem(payload);
  }, []);

  // ── Hover handlers — highlight only, no auto-fire ─────────────────────
  const handleHoverInner = useCallback((idx) => {
    setHoveredIndex(idx);
  }, []);

  const handleHoverOuter = useCallback((idx) => {
    setHoveredOuterIndex(idx);
  }, []);

  // ── Keyboard navigation ────────────────────────────────────────────────
  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape') {
        if (expandedFolder) {
          setExpandedFolder(null);
          setHoveredOuterIndex(-1);
        } else {
          window.electronAPI?.closeRadialMenu();
        }
        return;
      }
      const num = parseInt(e.key, 10);
      const idx = num >= 1 && num <= 8 ? num - 1 : -1;
      if (idx < 0) return;

      if (expandedFolder) {
        const folder = items.find(i => i?.id === expandedFolder);
        if (folder?.children && idx < folder.children.length) {
          fireItem(folder.children[idx]);
        }
      } else if (idx < items.length && items[idx]) {
        const item = items[idx];
        if (item.type === 'folder') {
          setExpandedFolder(item.id);
          setHoveredOuterIndex(-1);
        } else {
          fireItem(item);
        }
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [items, expandedFolder, fireItem]);

  // ── Click handlers ─────────────────────────────────────────────────────
  const handleItemClick = useCallback((item) => {
    if (item.type === 'folder') {
      setExpandedFolder(prev => prev === item.id ? null : item.id);
      setHoveredOuterIndex(-1);
    } else {
      fireItem(item);
    }
  }, [fireItem]);

  const handleFolderChildClick = useCallback((_folderId, child) => {
    fireItem(child);
  }, [fireItem]);

  // Click outside any segment — always close
  const handleBackgroundClick = useCallback(() => {
    window.electronAPI?.closeRadialMenu();
  }, []);

  // ── Empty state ────────────────────────────────────────────────────────
  if (items.length === 0) {
    return (
      <div className="radial-root">
        <div className="radial-empty">
          <span className="radial-empty-icon">{'\u25ce'}</span>
          <span className="radial-empty-text">
            No items added.<br />Configure in Settings.
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="radial-root">
      <RadialWheel
        key={animKey}
        mode="live"
        items={items}
        expandedFolder={expandedFolder}
        hoveredIndex={hoveredIndex}
        hoveredOuterIndex={hoveredOuterIndex}
        onHoverInner={handleHoverInner}
        onHoverOuter={handleHoverOuter}
        onItemClick={handleItemClick}
        onFolderChildClick={handleFolderChildClick}
        onBackgroundClick={handleBackgroundClick}
      />
    </div>
  );
}
