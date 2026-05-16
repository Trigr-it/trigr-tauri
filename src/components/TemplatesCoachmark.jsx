import React, { useEffect, useRef } from 'react';
import { Sparkles, X } from 'lucide-react';
import './TemplatesCoachmark.css';

/**
 * Drops down from the Templates pill in the TitleBar after onboarding finishes.
 * Points users at the starter packs without forcing a setup step inside the tour.
 *
 *   <TemplatesCoachmark
 *     anchorRect={pillRect}           // DOMRect of the Templates pill
 *     onOpenTemplates={...}           // open the templates dropdown
 *     onDismiss={...}                 // mark seen, close
 *   />
 *
 * Dismisses on ESC or click outside. Caret aligns to the pill's centre.
 */
export default function TemplatesCoachmark({ anchorRect, onOpenTemplates, onDismiss }) {
  const panelRef = useRef(null);

  useEffect(() => {
    function onKey(e) {
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        onDismiss?.();
      }
    }
    function onDown(e) {
      if (panelRef.current && !panelRef.current.contains(e.target)) {
        onDismiss?.();
      }
    }
    document.addEventListener('keydown', onKey);
    // Defer mousedown listener by a tick so the click that opened the coachmark
    // (e.g. closing the trial modal) doesn't immediately close it.
    const t = setTimeout(() => document.addEventListener('mousedown', onDown), 0);
    return () => {
      document.removeEventListener('keydown', onKey);
      document.removeEventListener('mousedown', onDown);
      clearTimeout(t);
    };
  }, [onDismiss]);

  if (!anchorRect) return null;

  const CARD_WIDTH = 296;
  const VIEWPORT_PAD = 12;

  // Centre under the pill, then clamp to viewport so the card never spills off-screen.
  const pillCentre = anchorRect.left + anchorRect.width / 2;
  let cardLeft = pillCentre - CARD_WIDTH / 2;
  cardLeft = Math.max(VIEWPORT_PAD, Math.min(cardLeft, window.innerWidth - CARD_WIDTH - VIEWPORT_PAD));
  const top = anchorRect.bottom + 10;
  const caretLeft = pillCentre - cardLeft - 7; // 7 = half caret width

  return (
    <div
      ref={panelRef}
      className="tpl-coachmark"
      style={{ top: `${top}px`, left: `${cardLeft}px`, width: `${CARD_WIDTH}px` }}
      role="dialog"
      aria-label="Starter templates"
    >
      <div className="tpl-coachmark-caret" style={{ left: `${caretLeft}px` }} />
      <button
        className="tpl-coachmark-close"
        type="button"
        onClick={onDismiss}
        aria-label="Dismiss"
      >
        <X size={12} strokeWidth={2} />
      </button>
      <div className="tpl-coachmark-header">
        <Sparkles size={14} strokeWidth={1.75} className="tpl-coachmark-icon" />
        <span className="tpl-coachmark-title">Pick a starter pack</span>
      </div>
      <div className="tpl-coachmark-body">
        Skip the blank slate. Import a ready-made set of expansions and shortcuts, edit anything you don't like.
      </div>
      <div className="tpl-coachmark-actions">
        <button
          className="tpl-coachmark-btn tpl-coachmark-btn--secondary"
          type="button"
          onClick={onDismiss}
        >
          Maybe later
        </button>
        <button
          className="tpl-coachmark-btn tpl-coachmark-btn--primary"
          type="button"
          onClick={onOpenTemplates}
        >
          Browse templates
        </button>
      </div>
    </div>
  );
}
