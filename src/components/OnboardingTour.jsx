import React, { useState, useEffect, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './OnboardingTour.css';

const TOTAL_STEPS = 5;

export default function OnboardingTour({ assignments, onComplete, onSkip }) {
  const [step, setStep] = useState(1);
  const [subStep, setSubStep] = useState('a'); // Step 2 sub-stages: 'a' | 'b'
  const [targetRect, setTargetRect] = useState(null);
  const [actionFired, setActionFired] = useState(false);
  const [tooltipPosition, setTooltipPosition] = useState('below');
  const tooltipRef = useRef(null);
  const observerRef = useRef(null);
  const assignmentCountAtStep2 = useRef(null);

  // ── Lock window resize on mount, unlock on unmount ───────────
  useEffect(() => {
    invoke('set_window_resizable', { resizable: false });
    return () => { invoke('set_window_resizable', { resizable: true }); };
  }, []);

  // ── Finish / skip handler ───────────────────────────────────
  const finish = useCallback(() => {
    invoke('set_window_resizable', { resizable: true });
    onComplete();
  }, [onComplete]);

  const skip = useCallback(() => {
    invoke('set_window_resizable', { resizable: true });
    onSkip();
  }, [onSkip]);

  // ── Measure target element and track resizes ────────────────
  const measureTarget = useCallback((selector) => {
    if (!selector) {
      setTargetRect(null);
      return;
    }
    const el = document.querySelector(selector);
    if (!el) {
      setTargetRect(null);
      return;
    }
    const update = () => {
      const r = el.getBoundingClientRect();
      setTargetRect({ top: r.top, left: r.left, width: r.width, height: r.height });
    };
    update();

    // Clean up previous observer
    if (observerRef.current) observerRef.current.disconnect();

    const ro = new ResizeObserver(update);
    ro.observe(el);
    ro.observe(document.documentElement);
    observerRef.current = ro;
  }, []);

  // Clean up observer on unmount
  useEffect(() => {
    return () => {
      if (observerRef.current) observerRef.current.disconnect();
    };
  }, []);

  // ── Step-specific target selectors ──────────────────────────
  // Note: Step 2 subStep 'b' is handled by its own deferred effect below.
  useEffect(() => {
    if (step === 2) {
      if (subStep === 'a') measureTarget('.keyboard-numpad-wrap');
      // subStep 'b' — skip here, handled by the deferred effect
      return;
    }
    const selectors = {
      1: null,
      3: null,
      4: '.area-tab:nth-child(2)',
      5: '.profile-tabs',
    };
    measureTarget(selectors[step] || null);
  }, [step, subStep, measureTarget]);

  // ── Step 2a → 2b: detect when assignment panel becomes visible ──
  useEffect(() => {
    if (step !== 2 || subStep !== 'a') return;
    const check = () => {
      const panel = document.querySelector('.macro-panel');
      const empty = document.querySelector('.macro-panel-empty');
      if (panel && !empty) { setSubStep('b'); return true; }
      return false;
    };
    // If panel is already open (e.g. tour restarted with key selected), advance immediately
    if (check()) return;
    const mo = new MutationObserver(check);
    mo.observe(document.body, { childList: true, subtree: true });
    return () => mo.disconnect();
  }, [step, subStep]);

  // ── Step 2b: deferred re-measurement after panel renders ──
  useEffect(() => {
    if (step !== 2 || subStep !== 'b') return;
    setTargetRect(null);
    // Retry until .macro-panel is in the DOM and has a non-zero rect
    let attempts = 0;
    const tryMeasure = () => {
      const el = document.querySelector('.macro-panel:not(.macro-panel-empty)');
      if (el) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && r.height > 0) {
          measureTarget('.macro-panel:not(.macro-panel-empty)');
          return;
        }
      }
      attempts++;
      if (attempts < 20) setTimeout(tryMeasure, 50);
    };
    const tid = setTimeout(tryMeasure, 30);
    return () => clearTimeout(tid);
  }, [step, subStep, measureTarget]);

  // ── Step 2: snapshot assignment count when entering step 2 ──
  useEffect(() => {
    if (step === 2 && assignmentCountAtStep2.current === null) {
      assignmentCountAtStep2.current = Object.keys(assignments).length;
    }
    if (step !== 2) {
      assignmentCountAtStep2.current = null;
    }
  }, [step, assignments]);

  // ── Step 2b → Step 3: detect when a new assignment is saved ──
  useEffect(() => {
    if (step !== 2 || subStep !== 'b') return;
    const keys = Object.keys(assignments);
    const baseline = assignmentCountAtStep2.current ?? 0;
    // Advance when a new assignment appears (count increased)
    if (keys.length > baseline) {
      setStep(3);
      setSubStep('a');
    }
  }, [step, subStep, assignments]);

  // ── Step 3: listen for macro-fired event ────────────────────
  useEffect(() => {
    if (step !== 3) return;
    let unlisten = null;
    let cancelled = false;

    listen('macro-fired', () => {
      if (!cancelled) setActionFired(true);
    }).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [step]);

  // ── Tooltip positioning — deferred to measure after paint ───
  useEffect(() => {
    if (!targetRect) return;
    const tid = setTimeout(() => {
      const pad = 16;
      const tooltipHeight = tooltipRef.current?.offsetHeight || 200;
      const tooltipWidth = tooltipRef.current?.offsetWidth || 380;

      // For elements on the right side of the window, position tooltip to the left
      const leftFits = targetRect.left - pad - tooltipWidth >= 0;
      const rightHalf = targetRect.left > window.innerWidth / 2;

      if (rightHalf && leftFits) {
        setTooltipPosition('left');
      } else {
        const belowTop = targetRect.top + targetRect.height + pad;
        const belowFits = belowTop + tooltipHeight <= window.innerHeight;
        const aboveFits = targetRect.top - pad >= tooltipHeight;
        setTooltipPosition(belowFits ? 'below' : aboveFits ? 'above' : 'below');
      }
    }, 0);
    return () => clearTimeout(tid);
  }, [targetRect, step, subStep]);

  const getTooltipStyle = () => {
    if (!targetRect) return {};
    const pad = 16;
    if (tooltipPosition === 'left') {
      return {
        position: 'fixed',
        top: targetRect.top,
        left: targetRect.left - pad,
        transform: 'translateX(-100%)',
      };
    }
    const left = targetRect.left + targetRect.width / 2;
    if (tooltipPosition === 'above') {
      return { position: 'fixed', bottom: window.innerHeight - targetRect.top + pad, left, transform: 'translateX(-50%)' };
    }
    return { position: 'fixed', top: targetRect.top + targetRect.height + pad, left, transform: 'translateX(-50%)' };
  };

  // ── Render overlay with cutout ──────────────────────────────
  const renderOverlay = () => {
    if (!targetRect) {
      // Full backdrop, no cutout
      return <div className="onboarding-backdrop" />;
    }
    const pad = 8;
    const r = 8;
    const t = targetRect.top - pad;
    const l = targetRect.left - pad;
    const w = targetRect.width + pad * 2;
    const h = targetRect.height + pad * 2;

    return (
      <svg className="onboarding-backdrop-svg" width="100%" height="100%">
        <defs>
          <mask id="onboarding-mask">
            <rect x="0" y="0" width="100%" height="100%" fill="white" />
            <rect x={l} y={t} width={w} height={h} rx={r} fill="black" />
          </mask>
        </defs>
        <rect
          x="0" y="0" width="100%" height="100%"
          fill="var(--onboarding-overlay)"
          mask="url(#onboarding-mask)"
        />
        <rect
          x={l} y={t} width={w} height={h} rx={r}
          fill="none"
          stroke="var(--accent)"
          strokeWidth="2"
        />
      </svg>
    );
  };

  // ── Drag region — matches titlebar height so window stays draggable ──
  const dragRegion = (
    <div className="onboarding-drag-region" data-tauri-drag-region="true" />
  );

  const skipLink = step > 1 && (
    <span className="onboarding-skip" onClick={skip}>Skip tour</span>
  );

  const stepDots = (
    <div className="onboarding-dots">
      {Array.from({ length: TOTAL_STEPS }, (_, i) => (
        <span key={i} className={`onboarding-dot${i + 1 === step ? ' active' : ''}${i + 1 < step ? ' done' : ''}`} />
      ))}
    </div>
  );

  // ── Step 1: Welcome ─────────────────────────────────────────
  if (step === 1) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-brand">Trigr</div>
          <p className="onboarding-welcome-text">Welcome to Trigr — let's get you set up.</p>
          {stepDots}
          <button className="onboarding-btn-primary" onClick={() => setStep(2)}>
            Let's go
          </button>
        </div>
      </div>
    );
  }

  // ── Step 2: Create a hotkey (progressive sub-stages) ────────
  if (step === 2) {
    // Sub-stage 2a: tooltip inside the keyboard highlight (bottom-aligned)
    if (subStep === 'a') {
      const step2aStyle = targetRect ? {
        position: 'fixed',
        bottom: window.innerHeight - (targetRect.top + targetRect.height) + 16,
        left: targetRect.left + targetRect.width / 2,
        transform: 'translateX(-50%)',
      } : {};
      return (
        <div className="onboarding-overlay">
          {renderOverlay()}
          {dragRegion}
          <div className="onboarding-tooltip" style={step2aStyle} ref={tooltipRef}>
            <div className="onboarding-step-label">Step 2 of {TOTAL_STEPS}</div>
            <p className="onboarding-tooltip-text">
              Select a modifier key (Ctrl, Alt, Shift or Win), then click any key.
            </p>
            {stepDots}
            {skipLink}
          </div>
        </div>
      );
    }
    // Sub-stage 2b: tooltip on the assignment panel
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-tooltip" style={getTooltipStyle()} ref={tooltipRef}>
          <div className="onboarding-step-label">Step 2 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            Choose <strong>Type Text</strong>, enter <strong>Hello, World!</strong> in the text field, then click <strong>Assign to Key</strong>.
          </p>
          {stepDots}
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 3: Fire the hotkey ─────────────────────────────────
  if (step === 3) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          {!actionFired ? (
            <>
              <div className="onboarding-step-label">Step 3 of {TOTAL_STEPS}</div>
              <p className="onboarding-tooltip-text">
                Now press your new hotkey anywhere to try it.
              </p>
              <p className="onboarding-hint">Minimise Trigr first, then press the hotkey in any app.</p>
            </>
          ) : (
            <>
              <p className="onboarding-success-text">You just used Trigr!</p>
              <button className="onboarding-btn-primary" onClick={() => setStep(4)}>
                Continue
              </button>
            </>
          )}
          {stepDots}
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 4: Expansions ──────────────────────────────────────
  if (step === 4) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-tooltip" style={getTooltipStyle()} ref={tooltipRef}>
          <div className="onboarding-step-label">Step 4 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            Text expansions let you type a short trigger — like <strong>;hello</strong> — and Trigr replaces it with anything you want. No hotkey needed.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(5)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 5: Profiles ────────────────────────────────────────
  if (step === 5) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-tooltip" style={getTooltipStyle()} ref={tooltipRef}>
          <div className="onboarding-step-label">Step 5 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            Profiles let you set different hotkeys for different apps. Trigr switches automatically when you change focus.
          </p>
          <p className="onboarding-hint">
            Click the app icon next to a profile name to link it to a specific application — Trigr will switch to that profile automatically when the app is in focus.
          </p>
          {stepDots}
          <button className="onboarding-btn-primary" onClick={finish}>Finish</button>
          {skipLink}
        </div>
      </div>
    );
  }

  return null;
}
