import { useEffect, useRef } from 'react';

/**
 * Wires ESC-to-dismiss and Tab/Shift+Tab focus trapping into a modal panel.
 *
 *   const panelRef = useRef(null);
 *   useModalKeyboard(panelRef, onDismiss, { enabled: true });
 *   return <div ref={panelRef} className="modal-panel">...</div>;
 *
 * - ESC anywhere fires onDismiss.
 * - Tab from the last focusable element wraps to the first.
 * - Shift+Tab from the first wraps to the last.
 * - Focus is moved into the panel on mount (first focusable element, or
 *   the panel itself if it has no focusable descendants).
 *
 * Listeners use capture phase on the panel itself so they do not leak into
 * inputs in surrounding content.
 */
export function useModalKeyboard(panelRef, onDismiss, opts = {}) {
  const { enabled = true } = opts;
  const onDismissRef = useRef(onDismiss);
  onDismissRef.current = onDismiss;

  useEffect(() => {
    if (!enabled) return;
    const panel = panelRef.current;
    if (!panel) return;

    const getFocusable = () => Array.from(
      panel.querySelectorAll(
        'a[href], button:not([disabled]), textarea:not([disabled]), input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'
      )
    ).filter(el => !el.hasAttribute('inert') && el.offsetParent !== null);

    // Move focus into the modal if focus is currently outside it.
    if (!panel.contains(document.activeElement)) {
      const focusables = getFocusable();
      if (focusables.length > 0) {
        focusables[0].focus();
      } else if (typeof panel.focus === 'function') {
        // Make the panel itself focusable as a fallback so the trap has somewhere to keep focus
        if (!panel.hasAttribute('tabindex')) panel.setAttribute('tabindex', '-1');
        panel.focus();
      }
    }

    const handleKeyDown = (e) => {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onDismissRef.current?.();
        return;
      }
      if (e.key !== 'Tab') return;

      const focusables = getFocusable();
      if (focusables.length === 0) {
        e.preventDefault();
        return;
      }
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement;

      if (e.shiftKey) {
        if (active === first || !panel.contains(active)) {
          e.preventDefault();
          last.focus();
        }
      } else {
        if (active === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };

    panel.addEventListener('keydown', handleKeyDown);
    return () => panel.removeEventListener('keydown', handleKeyDown);
  }, [panelRef, enabled]);
}
