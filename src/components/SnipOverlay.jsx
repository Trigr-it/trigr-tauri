// Drag-select region picker overlay. Fullscreen transparent window covering
// the entire virtual desktop (all monitors). Mouse-down starts a drag,
// mouse-move draws a live rect + coord readout, mouse-up commits and hides.
// Right-click or ESC cancels. Rust sizes and positions this window to
// (SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN)
// before showing, and passes origin coords via the `snip-overlay-shown`
// event so we can convert overlay-local pixel coords into screen coords
// the BitBlt executor consumes.
//
// Reusable — any feature that needs the user to pick a screen rect calls
// `showSnipOverlay` and listens for `region-snip-result` / `region-snip-cancelled`.
// Do NOT put step-specific logic in here.

import React, { useEffect, useRef, useState } from 'react';
import './SnipOverlay.css';

export default function SnipOverlay() {
  // (originX, originY) = top-left of the virtual desktop in screen coords.
  // Overlay-local pixels are in (0, 0)..(width, height); add the origin to
  // convert to screen coords for the emit.
  const [origin, setOrigin] = useState({ x: 0, y: 0 });
  // (physW, physH) = the virtual desktop's PHYSICAL pixel dimensions the
  // BitBlt executor reads at. WebView2's window.innerWidth/Height are CSS
  // pixels and don't map 1:1 to physical pixels — observed 4080×2527 CSS
  // on a 1920×1080 physical screen at 100% scale — so we track the physical
  // size separately and scale mouse coords before emitting.
  const [physSize, setPhysSize] = useState({ w: 0, h: 0 });
  const [dragging, setDragging] = useState(false);
  const [rect, setRect] = useState(null); // { x, y, w, h } in overlay-local CSS px
  const [cursorPos, setCursorPos] = useState({ x: 0, y: 0 });

  // Refs for the mouse handlers so they can read latest state without
  // re-registering on every drag frame.
  const originRef = useRef(origin);
  originRef.current = origin;
  const physRef = useRef(physSize);
  physRef.current = physSize;
  const draggingRef = useRef(false);
  const startRef = useRef({ x: 0, y: 0 });

  // Two-path population of origin + physical size:
  //   (a) pull once on mount via invoke — always lands, even for the first
  //       show where the async emit races the React listener registration
  //       (observed empirically: phys stayed 0×0 with only the emit path).
  //   (b) listen for the show event too, so subsequent shows on a live
  //       overlay still refresh state (multi-monitor hotplug during a
  //       session would change the virtual desktop bounds).
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const cfg = await window.electronAPI?.getSnipOverlayConfig?.();
        if (cancelled || !cfg) return;
        setOrigin({ x: cfg.originX || 0, y: cfg.originY || 0 });
        setPhysSize({ w: cfg.width || 0, h: cfg.height || 0 });
      } catch (_) { /* fall back to the event / window.screen path */ }
    })();
    if (window.electronAPI?.onSnipOverlayShown) {
      window.electronAPI.onSnipOverlayShown((payload) => {
        setOrigin({ x: payload?.originX || 0, y: payload?.originY || 0 });
        setPhysSize({ w: payload?.width || 0, h: payload?.height || 0 });
        setDragging(false);
        setRect(null);
        draggingRef.current = false;
      });
    }
    return () => { cancelled = true; };
  }, []);

  // Compute the CSS-to-physical scale factor. Prefer the payload size the
  // Rust show command delivered (matches GetSystemMetrics virtual-desktop
  // dims exactly). Fall back to window.screen.width/height when the show
  // event hasn't arrived yet (Tauri listener race). Falls further back to
  // 1.0 which is the "everything's already 1:1" case.
  const scaleFor = (physDim, cssDim) => {
    if (physDim > 0 && cssDim > 0) return physDim / cssDim;
    return 1;
  };
  const computeScale = () => {
    const iw = window.innerWidth || 0;
    const ih = window.innerHeight || 0;
    const p = physRef.current;
    if (p.w > 0 && p.h > 0) {
      return { sx: scaleFor(p.w, iw), sy: scaleFor(p.h, ih) };
    }
    // Fallback: window.screen reports the primary monitor's physical size
    // even when the show payload hasn't landed yet.
    const sw = window.screen?.width || 0;
    const sh = window.screen?.height || 0;
    if (sw > 0 && sh > 0) {
      return { sx: scaleFor(sw, iw), sy: scaleFor(sh, ih) };
    }
    return { sx: 1, sy: 1 };
  };

  const cancel = () => {
    setDragging(false);
    setRect(null);
    draggingRef.current = false;
    window.electronAPI?.emitSnipCancelled?.();
  };

  useEffect(() => {
    const onKey = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        cancel();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  const onMouseDown = (e) => {
    // Only left button starts a drag. Right button cancels.
    if (e.button === 2) {
      e.preventDefault();
      cancel();
      return;
    }
    if (e.button !== 0) return;
    e.preventDefault();
    startRef.current = { x: e.clientX, y: e.clientY };
    draggingRef.current = true;
    setDragging(true);
    setRect({ x: e.clientX, y: e.clientY, w: 0, h: 0 });
  };

  const onMouseMove = (e) => {
    setCursorPos({ x: e.clientX, y: e.clientY });
    if (!draggingRef.current) return;
    const s = startRef.current;
    const x = Math.min(s.x, e.clientX);
    const y = Math.min(s.y, e.clientY);
    const w = Math.abs(e.clientX - s.x);
    const h = Math.abs(e.clientY - s.y);
    setRect({ x, y, w, h });
  };

  const onMouseUp = (e) => {
    if (!draggingRef.current) return;
    if (e.button !== 0) return;
    e.preventDefault();
    draggingRef.current = false;
    setDragging(false);
    // Convert overlay-local CSS pixels to physical screen pixels, then add
    // the virtual-desktop origin. The scale factor accounts for WebView2
    // reporting an inflated window.innerWidth vs the actual physical window
    // size (observed 4080 CSS on a 1920 physical primary monitor).
    const o = originRef.current;
    const s = startRef.current;
    const { sx, sy } = computeScale();
    const cssX = Math.min(s.x, e.clientX);
    const cssY = Math.min(s.y, e.clientY);
    const cssW = Math.abs(e.clientX - s.x);
    const cssH = Math.abs(e.clientY - s.y);
    const x = Math.round(cssX * sx) + o.x;
    const y = Math.round(cssY * sy) + o.y;
    const w = Math.round(cssW * sx);
    const h = Math.round(cssH * sy);
    // Degenerate rect (accidental single click, no drag) → cancel rather
    // than commit a 0-area region that would OCR fruitlessly. Same 4×4
    // floor as the executor's guard.
    if (w < 4 || h < 4) {
      cancel();
      return;
    }
    window.electronAPI?.emitSnipResult?.({ x, y, w, h });
  };

  // Suppress the browser context menu so right-click serves as cancel
  // rather than surfacing the WebView2 menu.
  const onContextMenu = (e) => {
    e.preventDefault();
    cancel();
  };

  const showCoords = dragging && rect;
  const { sx: dispSx, sy: dispSy } = computeScale();
  const readout = showCoords
    ? `${Math.round(rect.w * dispSx)} × ${Math.round(rect.h * dispSy)}   at (${Math.round(rect.x * dispSx) + origin.x}, ${Math.round(rect.y * dispSy) + origin.y})`
    : 'Click and drag to select a region.  Right-click or ESC to cancel.';

  // Readout position: follows cursor with a small offset, flipped to the
  // left / above the cursor when it would otherwise clip off the viewport.
  const READOUT_W = 320;
  const READOUT_H = 34;
  const OFFSET = 16;
  let rx = cursorPos.x + OFFSET;
  let ry = cursorPos.y + OFFSET;
  if (rx + READOUT_W > window.innerWidth - 4) rx = cursorPos.x - READOUT_W - OFFSET;
  if (ry + READOUT_H > window.innerHeight - 4) ry = cursorPos.y - READOUT_H - OFFSET;

  return (
    <div
      className="snip-overlay-root"
      onMouseDown={onMouseDown}
      onMouseMove={onMouseMove}
      onMouseUp={onMouseUp}
      onContextMenu={onContextMenu}
    >
      {/* Dim background covers the whole overlay. The drag rect punches a
          "cut-out" via the SVG mask below so the selected area shows the
          screen underneath at full brightness. */}
      <svg className="snip-overlay-svg" width="100%" height="100%">
        <defs>
          <mask id="snip-cutout">
            <rect x="0" y="0" width="100%" height="100%" fill="white" />
            {rect && rect.w > 0 && rect.h > 0 && (
              <rect x={rect.x} y={rect.y} width={rect.w} height={rect.h} fill="black" />
            )}
          </mask>
        </defs>
        <rect
          x="0"
          y="0"
          width="100%"
          height="100%"
          fill="rgba(0, 0, 0, 0.35)"
          mask="url(#snip-cutout)"
        />
        {rect && rect.w > 0 && rect.h > 0 && (
          <rect
            x={rect.x}
            y={rect.y}
            width={rect.w}
            height={rect.h}
            fill="none"
            stroke="#e8a020"
            strokeWidth="1.5"
            shapeRendering="crispEdges"
          />
        )}
      </svg>
      <div
        className="snip-overlay-readout"
        style={{ left: rx, top: ry, width: READOUT_W, height: READOUT_H }}
      >
        {readout}
      </div>
    </div>
  );
}
