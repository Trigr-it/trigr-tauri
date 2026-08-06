// Shared drag-to-move behaviour for the standalone overlay windows (Quick
// Search bar + clipboard popup). Wire the returned handlers onto a grip
// element: left-drag moves the window, double-click resets it to the default
// position. The position is persisted machine-locally by the backend
// (save_overlay_position) and re-applied on every show.
//
// The drag is a MANUAL move (pointer capture + setPosition), NOT the native
// window.startDragging(): the clipboard popup is a WS_EX_NOACTIVATE window
// and the WM_NCLBUTTONDOWN modal move loop startDragging relies on doesn't
// move no-activate windows. Manual moves also keep DOM pointerup, so the
// save fires exactly at drag end with no debounce.
//
// Deltas use screenX/screenY (absolute screen coords, unaffected by the
// window moving underneath the cursor) scaled by devicePixelRatio to match
// the physical units outerPosition/setPosition speak. A 3px threshold keeps
// the double-click reset reliable and stops zero-pixel "drags" from saving.
import { useCallback } from 'react';
import { getCurrentWindow, PhysicalPosition } from '@tauri-apps/api/window';

export default function useOverlayDrag(name) {
  const onGripPointerDown = useCallback((e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const grip = e.currentTarget;
    const win = getCurrentWindow();
    const startSX = e.screenX;
    const startSY = e.screenY;
    const scale = window.devicePixelRatio || 1;
    let startPos = null;
    let moved = false;
    win.outerPosition().then((p) => { startPos = p; }).catch(() => {});
    try { grip.setPointerCapture(e.pointerId); } catch { /* non-fatal */ }

    const onMove = (me) => {
      if (!startPos) return;
      const dxl = me.screenX - startSX;
      const dyl = me.screenY - startSY;
      if (!moved && Math.abs(dxl) + Math.abs(dyl) < 3) return;
      moved = true;
      win.setPosition(new PhysicalPosition(
        Math.round(startPos.x + dxl * scale),
        Math.round(startPos.y + dyl * scale),
      )).catch(() => {});
    };
    const onUp = () => {
      grip.removeEventListener('pointermove', onMove);
      grip.removeEventListener('pointerup', onUp);
      grip.removeEventListener('pointercancel', onUp);
      if (moved) window.electronAPI?.saveOverlayPosition(name);
    };
    grip.addEventListener('pointermove', onMove);
    grip.addEventListener('pointerup', onUp);
    grip.addEventListener('pointercancel', onUp);
  }, [name]);

  const onGripDoubleClick = useCallback(() => {
    window.electronAPI?.resetOverlayPosition(name);
  }, [name]);

  return { onGripPointerDown, onGripDoubleClick };
}
