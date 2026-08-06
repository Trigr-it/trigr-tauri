// Shared drag-to-move behaviour for the standalone overlay windows (Quick
// Search bar + clipboard popup). Wire the returned handlers onto a grip
// element: left-drag moves the window, double-click resets it to the default
// position. The position is persisted machine-locally by the backend
// (save_overlay_position) and re-applied on every show.
//
// Drag start uses a 3px movement threshold rather than firing startDragging()
// on mousedown: the native move loop startDragging enters swallows the mouse
// events a double-click needs, so an immediate start would make the
// double-click reset unreliable.
//
// Drag end has no DOM event (the native loop consumes the mouseup), so the
// save is driven by the window's onMoved stream: once armed by a drag, the
// last move event + 350ms of silence triggers the save. The armed flag keeps
// programmatic set_position calls (show-time placement, resets) from
// re-saving.
import { useCallback, useEffect, useRef } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';

export default function useOverlayDrag(name) {
  const armed = useRef(false);
  const timer = useRef(null);

  useEffect(() => {
    let unlisten = null;
    let disposed = false;
    getCurrentWindow().onMoved(() => {
      if (!armed.current) return;
      clearTimeout(timer.current);
      timer.current = setTimeout(() => {
        armed.current = false;
        window.electronAPI?.saveOverlayPosition(name);
      }, 350);
    }).then((u) => {
      if (disposed) u();
      else unlisten = u;
    });
    return () => {
      disposed = true;
      if (unlisten) unlisten();
      clearTimeout(timer.current);
    };
  }, [name]);

  const onGripMouseDown = useCallback((e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const sx = e.screenX;
    const sy = e.screenY;
    const onMove = (me) => {
      if (Math.abs(me.screenX - sx) + Math.abs(me.screenY - sy) < 3) return;
      cleanup();
      armed.current = true;
      getCurrentWindow().startDragging();
    };
    const onUp = () => cleanup();
    const cleanup = () => {
      window.removeEventListener('mousemove', onMove, true);
      window.removeEventListener('mouseup', onUp, true);
    };
    window.addEventListener('mousemove', onMove, true);
    window.addEventListener('mouseup', onUp, true);
  }, []);

  const onGripDoubleClick = useCallback(() => {
    armed.current = false;
    clearTimeout(timer.current);
    window.electronAPI?.resetOverlayPosition(name);
  }, [name]);

  return { onGripMouseDown, onGripDoubleClick };
}
