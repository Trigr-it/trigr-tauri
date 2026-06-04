import React, { useState, useEffect, useCallback, useRef } from 'react';
import { Aperture } from 'lucide-react';
import RadialWheel from './RadialWheel';
import './RadialMenu.css';

export default function RadialMenu() {
  const [items, setItems] = useState([]);
  const [hoveredIndex, setHoveredIndex] = useState(-1);
  const [hoveredOuterIndex, setHoveredOuterIndex] = useState(-1);
  const [expandedFolder, setExpandedFolder] = useState(null);
  const [animKey, setAnimKey] = useState(0);
  const [missingNotice, setMissingNotice] = useState(false);
  const missingTimer = useRef(null);

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
      setMissingNotice(false);
      setAnimKey(k => k + 1);
    });

    // Close on window blur (clicking outside the window entirely)
    const onBlur = () => window.electronAPI?.closeRadialMenu();
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('blur', onBlur);
      if (missingTimer.current) clearTimeout(missingTimer.current);
    };
  }, []);

  // ── Fire an item ───────────────────────────────────────────────────────
  const fireItem = useCallback((item) => {
    if (!item) return;
    if (!item.exists) {
      // The linked source was renamed or deleted — the wedge renders faded
      // (40% opacity, easy to miss) and firing would be a silent no-op.
      // Tell the user instead; keep the wheel open so they can read it.
      setMissingNotice(true);
      if (missingTimer.current) clearTimeout(missingTimer.current);
      missingTimer.current = setTimeout(() => setMissingNotice(false), 2500);
      return;
    }
    const payload = { type: item.type, storageKey: item.storageKey, label: item.label };
    if (item.data?.text != null) payload.text = item.data.text;
    if (item.data?.html != null) payload.html = item.data.html;
    window.electronAPI?.executeRadialMenuItem(payload);
  }, []);

  // ── Hover handlers with folder auto-expand/collapse ───────────────────
  const expandTimer = useRef(null);
  const collapseTimer = useRef(null);

  const handleHoverInner = useCallback((idx) => {
    setHoveredIndex(idx);
    if (expandTimer.current) { clearTimeout(expandTimer.current); expandTimer.current = null; }
    if (collapseTimer.current) { clearTimeout(collapseTimer.current); collapseTimer.current = null; }

    if (idx >= 0) {
      const item = itemsRef.current[idx];
      if (item?.type === 'folder' && expandedFolderRef.current !== item.id) {
        expandTimer.current = setTimeout(() => { setExpandedFolder(item.id); }, 50);
      } else if (item && item.type !== 'folder' && expandedFolderRef.current) {
        collapseTimer.current = setTimeout(() => { setExpandedFolder(null); }, 150);
      }
    } else if (expandedFolderRef.current) {
      collapseTimer.current = setTimeout(() => { setExpandedFolder(null); }, 150);
    }
  }, []);

  const handleHoverOuter = useCallback((idx) => {
    setHoveredOuterIndex(idx);
    if (idx >= 0) {
      if (collapseTimer.current) { clearTimeout(collapseTimer.current); collapseTimer.current = null; }
    } else if (expandedFolderRef.current) {
      collapseTimer.current = setTimeout(() => { setExpandedFolder(null); }, 150);
    }
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
          <span className="radial-empty-icon" aria-hidden="true"><Aperture size={42} strokeWidth={1.5} /></span>
          <span className="radial-empty-text">
            No items added.<br />Configure in Settings.
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="radial-root" onClick={handleBackgroundClick}>
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
      {missingNotice && (
        <div className="radial-missing-notice">
          This action's source was renamed or deleted.<br />
          Re-link it in the Radial editor.
        </div>
      )}
    </div>
  );
}
