import React, { useState, useEffect, useCallback, useRef } from 'react';
import RadialWheel from './RadialWheel';
import './RadialMenu.css';

export default function RadialMenu() {
  const [items, setItems] = useState([]);
  const [hoveredIndex, setHoveredIndex] = useState(-1);
  const [hoveredOuterIndex, setHoveredOuterIndex] = useState(-1);
  const [expandedFolder, setExpandedFolder] = useState(null);

  // Refs for hold-to-select — event listeners need latest values
  const hoveredIndexRef = useRef(-1);
  const hoveredOuterIndexRef = useRef(-1);
  const expandedFolderRef = useRef(null);
  const itemsRef = useRef([]);

  hoveredIndexRef.current = hoveredIndex;
  hoveredOuterIndexRef.current = hoveredOuterIndex;
  expandedFolderRef.current = expandedFolder;
  itemsRef.current = items;

  // Track when the overlay was shown — distinguish quick tap from hold
  const showTimeRef = useRef(0);

  // ── Listen for data from Rust ──────────────────────────────────────────
  useEffect(() => {
    window.electronAPI?.onRadialMenuData((data) => {
      document.documentElement.setAttribute('data-theme', data.theme || 'dark');
      setItems(data.items || []);
      setHoveredIndex(-1);
      setHoveredOuterIndex(-1);
      setExpandedFolder(null);
      showTimeRef.current = Date.now();
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

  // ── Hold-to-select: fire hovered item on hotkey release ────────────────
  useEffect(() => {
    window.electronAPI?.onRadialMenuKeyReleased?.(() => {
      // Quick tap (< 250ms): leave the wheel open for click selection
      if (Date.now() - showTimeRef.current < 250) return;

      const idx = hoveredIndexRef.current;
      const outerIdx = hoveredOuterIndexRef.current;
      const folder = expandedFolderRef.current;
      const currentItems = itemsRef.current;

      // If hovering an outer-ring child (folder expanded)
      if (folder && outerIdx >= 0) {
        const folderItem = currentItems.find(i => i.id === folder);
        if (folderItem?.children && outerIdx < folderItem.children.length) {
          fireItem(folderItem.children[outerIdx]);
          return;
        }
      }

      // If hovering an inner-ring item
      if (idx >= 0 && idx < currentItems.length) {
        const item = currentItems[idx];
        if (item.type === 'folder') {
          // Expand the folder instead of firing
          setExpandedFolder(prev => prev === item.id ? null : item.id);
          setHoveredOuterIndex(-1);
          return;
        }
        fireItem(item);
        return;
      }

      // Nothing hovered — close
      window.electronAPI?.closeRadialMenu();
    });
  }, [fireItem]);

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
      const idx = e.key === '0' ? 9 : (num >= 1 && num <= 9 ? num - 1 : -1);
      if (idx < 0) return;

      if (expandedFolder) {
        const folder = items.find(i => i.id === expandedFolder);
        if (folder?.children && idx < folder.children.length) {
          fireItem(folder.children[idx]);
        }
      } else if (idx < items.length) {
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

  const handleBackgroundClick = useCallback(() => {
    if (expandedFolder) {
      setExpandedFolder(null);
      setHoveredOuterIndex(-1);
    } else {
      window.electronAPI?.closeRadialMenu();
    }
  }, [expandedFolder]);

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
        mode="live"
        items={items}
        expandedFolder={expandedFolder}
        hoveredIndex={hoveredIndex}
        hoveredOuterIndex={hoveredOuterIndex}
        onHoverInner={setHoveredIndex}
        onHoverOuter={setHoveredOuterIndex}
        onItemClick={handleItemClick}
        onFolderChildClick={handleFolderChildClick}
        onBackgroundClick={handleBackgroundClick}
      />
    </div>
  );
}
