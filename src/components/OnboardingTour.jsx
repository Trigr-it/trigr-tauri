import React, { useState, useEffect, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './OnboardingTour.css';

const TOTAL_STEPS = 11;

export default function OnboardingTour({ assignments, onComplete, onSkip, onAreaChange }) {
  const [step, setStep] = useState(1);
  const [subStep, setSubStep] = useState('a'); // Step 2 sub-stages: 'a' | 'b' | 'c'
  const [targetRect, setTargetRect] = useState(null);
  const [secondaryRect, setSecondaryRect] = useState(null); // second highlight area
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
    onAreaChange?.('mapping');
    onComplete();
  }, [onComplete, onAreaChange]);

  const skip = useCallback(() => {
    invoke('set_window_resizable', { resizable: true });
    onAreaChange?.('mapping');
    onSkip();
  }, [onSkip, onAreaChange]);

  // ── Navigate to area + advance step ─────────────────────────
  const goToStep = useCallback((nextStep, area) => {
    if (area) onAreaChange?.(area);
    // Small delay for tab switch to render before measuring target
    setTimeout(() => setStep(nextStep), area ? 80 : 0);
  }, [onAreaChange]);

  // ── Measure target element and track resizes ────────────────
  const measureTarget = useCallback((selector) => {
    if (!selector) {
      setTargetRect(null);
      return;
    }
    // Retry for elements that haven't rendered yet after tab switch
    let attempts = 0;
    const tryMeasure = () => {
      const el = document.querySelector(selector);
      if (!el) {
        attempts++;
        if (attempts < 30) { setTimeout(tryMeasure, 60); return; }
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
    };
    tryMeasure();
  }, []);

  // Clean up observer on unmount
  useEffect(() => {
    return () => {
      if (observerRef.current) observerRef.current.disconnect();
    };
  }, []);

  // ── Step-specific target selectors ──────────────────────────
  useEffect(() => {
    if (step === 2) {
      if (subStep === 'a') measureTarget('.modifier-bar');
      else if (subStep === 'b') measureTarget('.keyboard-outer');
      return;
    }
    const selectors = {
      1: null,            // Welcome modal
      3: null,            // Fire hotkey modal
      4: null,            // Action types modal
      5: '.sidebar',      // Profiles sidebar (still on mapping)
      6: '.area-tab:nth-child(2)',  // Text Expansion tab
      7: null,            // Quick Search intro modal
      8: '.area-tab:nth-child(3)',  // Quick Search tab (Quick Actions)
      9: '.area-tab:nth-child(3)',  // Quick Search tab (Search Templates)
      10: '.area-tab:nth-child(4)', // Clipboard tab
      11: null,           // Finish modal
    };
    measureTarget(selectors[step] || null);

    // Secondary highlight — the main panel area alongside the tab
    const secondarySelectors = {
      6: '.te-content',   // Text Expansions panel
      8: '.stp-panel',    // Quick Search panel (Quick Actions)
      9: '.stp-panel',    // Quick Search panel (Search Templates)
      10: '.cbg-panel',   // Clipboard panel
    };
    const secSel = secondarySelectors[step];
    if (secSel) {
      // Delay to allow panel to render after tab switch
      const tid = setTimeout(() => {
        const el = document.querySelector(secSel);
        if (el) {
          const r = el.getBoundingClientRect();
          setSecondaryRect({ top: r.top, left: r.left, width: r.width, height: r.height });
        } else {
          setSecondaryRect(null);
        }
      }, 150);
      return () => { clearTimeout(tid); setSecondaryRect(null); };
    } else {
      setSecondaryRect(null);
    }
  }, [step, subStep, measureTarget]);

  // ── Step 2a → 2b: detect when a modifier is selected ──
  useEffect(() => {
    if (step !== 2 || subStep !== 'a') return;
    // Snapshot which modifiers are already active on entry so we only
    // transition when the user clicks a NEW modifier during the tour.
    const alreadyActive = document.querySelectorAll('.modifier-bar-keys .mod-layer-btn.active').length;
    const mo = new MutationObserver(() => {
      const nowActive = document.querySelectorAll('.modifier-bar-keys .mod-layer-btn.active').length;
      if (nowActive > 0 && nowActive !== alreadyActive) {
        setSubStep('b');
      }
    });
    mo.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['class'] });
    return () => mo.disconnect();
  }, [step, subStep]);

  // ── Step 2b → 2c: detect when assignment panel becomes visible ──
  useEffect(() => {
    if (step !== 2 || subStep !== 'b') return;
    const check = () => {
      const panel = document.querySelector('.macro-panel');
      const empty = document.querySelector('.macro-panel-empty');
      if (panel && !empty) { setSubStep('c'); return true; }
      return false;
    };
    if (check()) return;
    const mo = new MutationObserver(check);
    mo.observe(document.body, { childList: true, subtree: true });
    return () => mo.disconnect();
  }, [step, subStep]);

  // ── Step 2c: deferred re-measurement after panel renders ──
  useEffect(() => {
    if (step !== 2 || subStep !== 'c') return;
    setTargetRect(null);
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

  // ── Step 2c → Step 3: detect when a new assignment is saved ──
  useEffect(() => {
    if (step !== 2 || subStep !== 'c') return;
    const keys = Object.keys(assignments);
    const baseline = assignmentCountAtStep2.current ?? 0;
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

  // ── Step 5: expand profile accordion when entering ──────────
  useEffect(() => {
    if (step !== 5) return;
    const tid = setTimeout(() => {
      const header = document.querySelector('.profile-accordion-header');
      const chevron = document.querySelector('.profile-accordion-chevron');
      // Only click to expand if currently collapsed (chevron shows ▾)
      if (header && chevron && chevron.textContent.trim() === '▾') {
        header.click();
      }
    }, 100);
    return () => clearTimeout(tid);
  }, [step]);

  // ── Step 9: click Search Templates pill when entering ──────
  useEffect(() => {
    if (step !== 9) return;
    const tid = setTimeout(() => {
      document.querySelector('.stp-mode-tab:nth-child(2)')?.click();
    }, 120);
    return () => clearTimeout(tid);
  }, [step]);

  // ── Tooltip positioning — deferred to measure after paint ───
  useEffect(() => {
    if (!targetRect) return;
    const tid = setTimeout(() => {
      const pad = 16;
      const tooltipHeight = tooltipRef.current?.offsetHeight || 200;
      const tooltipWidth = tooltipRef.current?.offsetWidth || 380;

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
    const tooltipW = tooltipRef.current?.offsetWidth || 380;
    const tooltipH = tooltipRef.current?.offsetHeight || 200;
    const winW = window.innerWidth;
    const winH = window.innerHeight;

    let top, left;

    if (tooltipPosition === 'left') {
      top = targetRect.top;
      left = targetRect.left - pad - tooltipW;
    } else if (tooltipPosition === 'above') {
      top = targetRect.top - pad - tooltipH;
      left = targetRect.left + targetRect.width / 2 - tooltipW / 2;
    } else {
      // below
      top = targetRect.top + targetRect.height + pad;
      left = targetRect.left + targetRect.width / 2 - tooltipW / 2;
    }

    // Clamp within window bounds
    if (left < 8) left = 8;
    if (left + tooltipW > winW - 8) left = winW - tooltipW - 8;
    if (top < 8) top = 8;
    if (top + tooltipH > winH - 8) top = winH - tooltipH - 8;

    return { position: 'fixed', top, left };
  };

  // ── Render overlay with cutout ──────────────────────────────
  const renderOverlay = () => {
    if (!targetRect) {
      return <div className="onboarding-backdrop" />;
    }
    const pad = 8;
    const r = 8;
    const t1 = targetRect.top - pad;
    const l1 = targetRect.left - pad;
    const w1 = targetRect.width + pad * 2;
    const h1 = targetRect.height + pad * 2;

    // Optional second cutout
    const has2 = secondaryRect && secondaryRect.width > 0;
    const t2 = has2 ? secondaryRect.top - pad : 0;
    const l2 = has2 ? secondaryRect.left - pad : 0;
    const w2 = has2 ? secondaryRect.width + pad * 2 : 0;
    const h2 = has2 ? secondaryRect.height + pad * 2 : 0;

    return (
      <svg className="onboarding-backdrop-svg" width="100%" height="100%">
        <defs>
          <mask id="onboarding-mask">
            <rect x="0" y="0" width="100%" height="100%" fill="white" />
            <rect x={l1} y={t1} width={w1} height={h1} rx={r} fill="black" />
            {has2 && <rect x={l2} y={t2} width={w2} height={h2} rx={r} fill="black" />}
          </mask>
        </defs>
        <rect
          x="0" y="0" width="100%" height="100%"
          fill="var(--onboarding-overlay)"
          mask="url(#onboarding-mask)"
        />
        <rect
          x={l1} y={t1} width={w1} height={h1} rx={r}
          fill="none"
          stroke="var(--accent)"
          strokeWidth="2"
        />
        {has2 && (
          <rect
            x={l2} y={t2} width={w2} height={h2} rx={r}
            fill="none"
            stroke="var(--accent)"
            strokeWidth="2"
          />
        )}
      </svg>
    );
  };

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
          <p className="onboarding-welcome-text">Welcome to Trigr — let's take a quick tour of what you can do.</p>
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
    // 2a: Highlight modifier bar — pick a modifier
    if (subStep === 'a') {
      return (
        <div className="onboarding-overlay">
          {renderOverlay()}
          {dragRegion}
          <div className="onboarding-tooltip" style={getTooltipStyle()} ref={tooltipRef}>
            <div className="onboarding-step-label">Step 2 of {TOTAL_STEPS}</div>
            <p className="onboarding-tooltip-text">
              First, select a <strong>modifier key layer</strong> — click one of the buttons highlighted above (Ctrl, Alt, Shift or Win).
            </p>
            <p className="onboarding-hint">
              The modifier + key you choose becomes the hotkey combination you'll press to fire the action. For example, selecting Ctrl then pressing E creates the hotkey Ctrl+E.
            </p>
            {stepDots}
            {skipLink}
          </div>
        </div>
      );
    }
    // 2b: Highlight keyboard — pick a key
    if (subStep === 'b') {
      return (
        <div className="onboarding-overlay">
          {renderOverlay()}
          {dragRegion}
          <div className="onboarding-tooltip" style={getTooltipStyle()} ref={tooltipRef}>
            <div className="onboarding-step-label">Step 2 of {TOTAL_STEPS}</div>
            <p className="onboarding-tooltip-text">
              Now click any key on the keyboard to assign an action to it.
            </p>
            <p className="onboarding-hint">
              This key combined with your modifier becomes your hotkey. Press that combination anywhere on your PC to fire the action you assign next.
            </p>
            {stepDots}
            {skipLink}
          </div>
        </div>
      );
    }
    // 2c: Highlight macro panel — fill in the action
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

  // ── Step 4: Action types overview ───────────────────────────
  if (step === 4) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 4 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            Hotkeys can do a lot more than type text. Each key can trigger:
          </p>
          <div className="onboarding-feature-list">
            <div className="onboarding-feature-item"><strong>Send Hotkey</strong> — simulate key combos (with hold and repeat modes)</div>
            <div className="onboarding-feature-item"><strong>Open App / URL / Folder</strong> — launch anything instantly</div>
            <div className="onboarding-feature-item"><strong>Macro Sequence</strong> — chain multiple steps (Press Key, Click Mouse, Wait, and more)</div>
            <div className="onboarding-feature-item"><strong>Run AHK Script</strong> — execute AutoHotkey scripts</div>
          </div>
          <p className="onboarding-hint">
            Double-tap a key for a second action. Use the mouse canvas for mouse button mapping.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(5)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 5: Profiles (still on mapping) ─────────────────────
  if (step === 5) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 5 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Profiles</strong> give each app its own hotkeys. Link a profile to an application and Trigr switches automatically when you change focus.
          </p>
          <p className="onboarding-hint">
            Right-click a profile to link it to an app, export it, or duplicate it.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => goToStep(6, 'expansions')}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 6: Text Expansions ─────────────────────────────────
  if (step === 6) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 6 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Text Expansions</strong> replace short triggers with full text — no hotkey needed. Type <strong>;sig</strong> + Space and your email signature appears.
          </p>
          <p className="onboarding-hint">
            Organise with colour-coded categories. Use dynamic fields like dates, clipboard contents, cursor position, and fill-in prompts. Paste images too.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(7)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 7: Quick Search intro ─────────────────────────────
  if (step === 7) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 7 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Quick Search</strong> is your command centre. Press the shortcut below from anywhere on your PC to instantly search and launch any hotkey, expansion, or action.
          </p>
          <div className="onboarding-shortcut-row onboarding-shortcut-row--centred">
            <kbd className="onboarding-kbd">Ctrl</kbd>
            <span className="onboarding-kbd-plus">+</span>
            <kbd className="onboarding-kbd">Space</kbd>
            <span className="onboarding-shortcut-label">Quick Search</span>
          </div>
          <p className="onboarding-hint">
            You can also set up Quick Actions and Search Templates in the Quick Search tab — let's take a look.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => goToStep(8, 'templates')}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 8: Quick Actions ───────────────────────────────────
  if (step === 8) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 8 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Quick Actions</strong> let you launch apps, open folders, URLs, or run macros — accessible instantly from Quick Search without assigning a hotkey.
          </p>
          <p className="onboarding-hint">
            Organise with categories. Search by name from the Ctrl+Space overlay.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(9)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 9: Search Templates ────────────────────────────────
  if (step === 9) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 9 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Search Templates</strong> let you search any website from Quick Search. Type a trigger + Space, then your query.
          </p>
          <p className="onboarding-hint">
            Presets include Google, ChatGPT, Perplexity, GitHub, and more. Add your own for any website with a search URL.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => goToStep(10, 'clipboard')}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 10: Clipboard Manager ──────────────────────────────
  if (step === 10) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 10 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Clipboard Manager</strong> saves everything you copy — text and images. Browse, search, pin favourites, and re-paste from any app.
          </p>
          <div className="onboarding-shortcut-row onboarding-shortcut-row--centred">
            <kbd className="onboarding-kbd">Ctrl</kbd>
            <span className="onboarding-kbd-plus">+</span>
            <kbd className="onboarding-kbd">Shift</kbd>
            <span className="onboarding-kbd-plus">+</span>
            <kbd className="onboarding-kbd">V</kbd>
            <span className="onboarding-shortcut-label">Clipboard popup — paste from history anywhere</span>
          </div>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(11)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 11: You're All Set ─────────────────────────────────
  if (step === 11) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 11 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            You're all set. Here are the shortcuts you'll use most:
          </p>
          <div className="onboarding-shortcut-row onboarding-shortcut-row--centred">
            <kbd className="onboarding-kbd">Ctrl</kbd>
            <span className="onboarding-kbd-plus">+</span>
            <kbd className="onboarding-kbd">Space</kbd>
            <span className="onboarding-shortcut-label">Quick Search — find and fire anything</span>
          </div>
          <div className="onboarding-shortcut-row onboarding-shortcut-row--centred">
            <kbd className="onboarding-kbd">Ctrl</kbd>
            <span className="onboarding-kbd-plus">+</span>
            <kbd className="onboarding-kbd">Shift</kbd>
            <span className="onboarding-kbd-plus">+</span>
            <kbd className="onboarding-kbd">V</kbd>
            <span className="onboarding-shortcut-label">Clipboard popup — paste from history</span>
          </div>
          <p className="onboarding-hint">
            Check <strong>Analytics</strong> to see your time saved. Explore <strong>Settings</strong> for macro speed, global pause, and more.
          </p>
          {stepDots}
          <button className="onboarding-btn-primary" onClick={finish}>Finish</button>
        </div>
      </div>
    );
  }

  return null;
}
