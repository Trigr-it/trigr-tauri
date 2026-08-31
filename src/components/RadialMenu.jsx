import React, { useState, useEffect, useCallback, useRef } from 'react';
import { Aperture } from 'lucide-react';
import RadialWheel from './RadialWheel';
import './RadialMenu.css';

// Seed the theme from the last session before first paint (shared cache key
// with the clipboard popup + Quick Search) so a lost payload can't leave a
// light-theme user with a dark wheel.
try {
  document.documentElement.setAttribute('data-theme', localStorage.getItem('trigr_overlay_theme') || 'dark');
} catch { /* storage unavailable — CSS :root default applies */ }

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
  // Live mirrors of the hover state so the hold-release handler (registered
  // once, below) reads the segment under the cursor at the exact moment the
  // hotkey is released rather than a stale closure value.
  const hoveredIndexRef = useRef(-1);
  const hoveredOuterIndexRef = useRef(-1);
  // One-shot fire guard (see fireItem). Declared here so both the data-reset
  // effect and fireItem can reach it.
  const firedRef = useRef(false);
  // Hold-to-select config, delivered with each open. The overlay holds
  // keyboard focus, so its own keyup listener (below) detects the launch-key
  // release — no backend/hook involvement. holdKey is a KeyboardEvent.code
  // (e.g. "KeyW"), the action segment of the radial hotkey combo.
  const holdToSelectRef = useRef(false);
  const holdKeyRef = useRef('');

  itemsRef.current = items;
  expandedFolderRef.current = expandedFolder;
  hoveredIndexRef.current = hoveredIndex;
  hoveredOuterIndexRef.current = hoveredOuterIndex;

  // ── Listen for data from Rust ──────────────────────────────────────────
  // One applier for the pushed radial-menu-data payload AND the self-heal
  // pull below, so both paths start the wheel in the same fresh state.
  const applyRadialData = useCallback((data) => {
    if (!data) return;
    const theme = data.theme || 'dark';
    document.documentElement.setAttribute('data-theme', theme);
    try { localStorage.setItem('trigr_overlay_theme', theme); } catch { /* ignore */ }
    setItems(data.items || []);
    setHoveredIndex(-1);
    setHoveredOuterIndex(-1);
    setExpandedFolder(null);
    setMissingNotice(false);
    setAnimKey(k => k + 1);
    firedRef.current = false; // fresh wheel — re-arm the one-shot fire guard
    holdToSelectRef.current = !!data.holdToSelect;
    holdKeyRef.current = data.holdKey || '';
  }, []);

  useEffect(() => {
    window.electronAPI?.onRadialMenuData(applyRadialData);

    // Close on window blur (clicking outside the window entirely)
    const onBlur = () => window.electronAPI?.closeRadialMenu();
    window.addEventListener('blur', onBlur);
    return () => {
      window.removeEventListener('blur', onBlur);
      if (missingTimer.current) clearTimeout(missingTimer.current);
    };
  }, [applyRadialData]);

  // ── Self-heal pull ─────────────────────────────────────────────────────
  // The pushed payload can be lost at cold start (lazy chunk not yet listening)
  // or on resume from webview_mem TrySuspend (IPC reconnect race) — the same
  // two windows the clipboard popup closed in v0.8.5. A lost payload here
  // meant an empty wheel AND no holdKey, so hold-to-select release never
  // fired for that open. Pull at mount (fill only while empty) and, forced,
  // on visibilitychange. webview_mem parks hidden windows, so that fires on
  // every show, not only after a suspend; the pull is idempotent.
  const selfHealPull = useCallback((force) => {
    window.electronAPI?.getRadialMenuData?.()
      .then((data) => {
        if (!data || !Array.isArray(data.items)) return;
        if (!force && itemsRef.current.length > 0) return;
        applyRadialData(data);
      })
      .catch(() => {});
  }, [applyRadialData]);

  useEffect(() => { selfHealPull(false); }, [selfHealPull]);

  useEffect(() => {
    const onVis = () => {
      if (document.visibilityState !== 'visible') return;
      selfHealPull(true);
    };
    document.addEventListener('visibilitychange', onVis);
    return () => document.removeEventListener('visibilitychange', onVis);
  }, [selfHealPull]);

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
    if (firedRef.current) return; // already fired this open — ignore repeats
    firedRef.current = true;
    const payload = { type: item.type, storageKey: item.storageKey, label: item.label };
    if (item.data?.text != null) payload.text = item.data.text;
    if (item.data?.html != null) payload.html = item.data.html;
    window.electronAPI?.executeRadialMenuItem(payload);
  }, []);

  // ── Hold-to-select: fire on launch-key release ─────────────────────────
  // The overlay holds keyboard focus while open (Rust set_focus on show —
  // that's also why the number-key / Esc nav below works), so we detect the
  // release directly as a DOM keyup here. No hook, no backend event: the hook
  // suppresses the launch key's KEYDOWN, but its KEYUP is delivered to this
  // focused window normally. On release we fire whatever the cursor is over —
  // a folder child, or a non-folder segment — else close (release over a gap,
  // the centre, or a folder itself is a cancel). Only active while the current
  // wheel was opened with hold-to-select on.
  useEffect(() => {
    const onKeyUp = (e) => {
      if (!holdToSelectRef.current) return;
      if (!holdKeyRef.current || e.code !== holdKeyRef.current) return;
      const expanded = expandedFolderRef.current;
      const outerIdx = hoveredOuterIndexRef.current;
      const innerIdx = hoveredIndexRef.current;
      if (expanded && outerIdx >= 0) {
        const folder = itemsRef.current.find(i => i?.id === expanded);
        if (folder?.children && outerIdx < folder.children.length) {
          fireItem(folder.children[outerIdx]);
          return;
        }
      }
      if (innerIdx >= 0) {
        const item = itemsRef.current[innerIdx];
        if (item && item.type !== 'folder') {
          fireItem(item);
          return;
        }
      }
      window.electronAPI?.closeRadialMenu();
    };
    window.addEventListener('keyup', onKeyUp);
    return () => window.removeEventListener('keyup', onKeyUp);
  }, [fireItem]);

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
            No actions on this wheel yet.<br />Open Keyfire, then Triggers → Radial to add some.
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
