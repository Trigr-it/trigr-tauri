import React, { useState, useEffect, useRef, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { Globe } from 'lucide-react';
import './OnboardingTour.css';

const TOTAL_STEPS = 13;

// Parse an assignment storage key ("Default::Ctrl::E", "Default::BARE::F1",
// "AppName::Modifier::KEYCODE", optional "::double" suffix) into a friendly
// hotkey string for display. Returns "" if the format doesn't match.
function parseAssignmentHotkey(key) {
  if (!key || typeof key !== 'string') return '';
  const parts = key.split('::');
  if (parts.length < 3) return '';
  const modifier = parts[1];
  const keyCode = parts[2];
  if (modifier === 'BARE') return keyCode;
  // Modifier already a "+"-joined string (e.g. "Ctrl+Shift") in some cases;
  // normalise the visual separator for kbd-badge rendering at the callsite.
  return `${modifier.replace(/\+/g, ' + ')} + ${keyCode}`;
}

export default function OnboardingTour({ assignments, onComplete, onSkip, onAreaChange, onShowUpgrade }) {
  const [step, setStep] = useState(1);
  const [subStep, setSubStep] = useState('a'); // Step 2 sub-stages: 'a' | 'b' | 'c'
  const [targetRect, setTargetRect] = useState(null);
  const [secondaryRect, setSecondaryRect] = useState(null); // second highlight area
  const [actionFired, setActionFired] = useState(false);
  const [searchFired, setSearchFired] = useState(false); // Step 8 gate
  const [tooltipPosition, setTooltipPosition] = useState('below');
  // Captured hotkey from the assignment the user just created at Step 2c,
  // displayed back to them at Step 3 as a memory aid before they minimise.
  // Format: "Ctrl + E" or "BARE + F1" — split by " + " for kbd-badge render.
  const [userHotkey, setUserHotkey] = useState('');
  const tooltipRef = useRef(null);
  const observerRef = useRef(null);
  // Set of assignment keys present when Step 2 was entered. Used both to gate
  // the Step 2 → 3 transition (a new key must appear) and to identify which
  // key the user assigned so we can show it back at Step 3.
  const assignmentKeysAtStep2 = useRef(null);

  // Pre-tour gate: if the user already has hotkeys on the keyboard/mouse canvas
  // (any non-GLOBAL:: assignment), show a brief "Welcome back" modal before
  // Step 1. Warns them to pick an empty key during Step 2 so the tour can
  // complete cleanly. Lazy-initialised so it captures the state at tour
  // start and doesn't re-trigger when assignments change mid-tour.
  const [showWelcomeBack, setShowWelcomeBack] = useState(() =>
    Object.keys(assignments).some(k => !k.startsWith('GLOBAL::'))
  );

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
  // Optional `view` is forwarded to the area-change handler so a step can
  // jump to a specific sub-view (e.g. mapping + radial) in one motion.
  const goToStep = useCallback((nextStep, area, view) => {
    if (area) onAreaChange?.(area, view);
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
      6: '.rev-editor',   // Radial Menu — highlight the wheel canvas (still on mapping, radial view)
      7: '.area-tab:nth-child(2)',  // Text Expansion tab
      8: null,            // Quick Search intro modal
      9: '.area-tab:nth-child(3)',  // Quick Search tab (Quick Actions)
      10: '.area-tab:nth-child(3)', // Quick Search tab (Search Templates)
      11: '.area-tab:nth-child(4)', // Clipboard tab
      12: null,           // Finish modal
    };
    measureTarget(selectors[step] || null);

    // Secondary highlight — the main panel area alongside the tab
    const secondarySelectors = {
      7: '.te-content',   // Text Expansions panel
      9: '.stp-panel',    // Quick Search panel (Quick Actions)
      10: '.stp-panel',   // Quick Search panel (Search Templates)
      11: '.cbg-panel',   // Clipboard panel
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

  // ── Step 2: snapshot assignment keys when entering step 2 ──
  useEffect(() => {
    if (step === 2 && assignmentKeysAtStep2.current === null) {
      assignmentKeysAtStep2.current = new Set(Object.keys(assignments));
    }
    if (step !== 2) {
      assignmentKeysAtStep2.current = null;
    }
  }, [step, assignments]);

  // ── Step 2c → Step 3: detect when a new assignment is saved ──
  // Diffs against the entry-time key set to identify WHICH key the user just
  // assigned, so Step 3 can show their hotkey back to them as a reminder.
  useEffect(() => {
    if (step !== 2 || subStep !== 'c') return;
    const baseline = assignmentKeysAtStep2.current ?? new Set();
    const newKeys = Object.keys(assignments).filter(k => !baseline.has(k));
    if (newKeys.length > 0) {
      setUserHotkey(parseAssignmentHotkey(newKeys[0]));
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

  // ── Step 10: click Search Templates pill when entering ─────
  useEffect(() => {
    if (step !== 10) return;
    const tid = setTimeout(() => {
      document.querySelector('.stp-mode-tab:nth-child(2)')?.click();
    }, 120);
    return () => clearTimeout(tid);
  }, [step]);

  // ── Step 8: listen for search-overlay-shown to unlock Next ──
  useEffect(() => {
    if (step !== 8) return;
    let unlisten = null;
    let cancelled = false;
    listen('search-overlay-shown', () => {
      if (!cancelled) setSearchFired(true);
    }).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [step]);

  // ── Reset gates when leaving a gated step so re-entry works cleanly ──
  useEffect(() => {
    if (step !== 8 && searchFired) setSearchFired(false);
    if (step !== 3 && actionFired) setActionFired(false);
  }, [step, searchFired, actionFired]);

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

  // ── Pre-tour: Welcome back (returning users only) ───────────
  if (showWelcomeBack) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-modal-title">Welcome back</div>
          <p className="onboarding-welcome-text">
            You already have hotkeys set up. This tour will walk you through creating a new one — when prompted, please pick an empty key on the keyboard so the tour can continue cleanly.
          </p>
          <button className="onboarding-btn-primary" onClick={() => setShowWelcomeBack(false)}>
            Start tour
          </button>
        </div>
      </div>
    );
  }

  // ── Step 1: Welcome ─────────────────────────────────────────
  if (step === 1) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          {/* Keyfire logo — same SVG as the titlebar wordmark, with unique
              gradient IDs so the two SVGs don't collide on `url(#id)` lookup
              when both are mounted in the document at once. */}
          <span className="onboarding-logo" aria-hidden="true">
            <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" role="img" aria-label="Keyfire">
              <defs>
                <linearGradient id="onboarding-trigr-base" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#f0b942"/>
                  <stop offset="100%" stopColor="#c8860a"/>
                </linearGradient>
                <linearGradient id="onboarding-trigr-keytop" x1="0" y1="0" x2="0" y2="1">
                  <stop offset="0%" stopColor="#ffffff"/>
                  <stop offset="100%" stopColor="#e8e5dc"/>
                </linearGradient>
              </defs>
              <rect x="0" y="0" width="64" height="64" rx="9" fill="url(#onboarding-trigr-base)"/>
              <rect x="7.68" y="6.4" width="48.64" height="43.52" rx="6.5" fill="url(#onboarding-trigr-keytop)"/>
              <rect x="7.68" y="46.5" width="48.64" height="3.42" rx="1.5" fill="#000000" opacity="0.06"/>
              <path d="M 33 14 C 36 18, 41 23, 41 30 C 41 37, 36 41, 32 41 C 26 41, 22 37, 22 32 C 22 28, 25 26, 27 23 C 28 26, 30 27, 30 24 C 30 20, 32 17, 33 14 Z" fill="#c8860a"/>
            </svg>
          </span>
          <div className="onboarding-brand">Keyfire</div>
          <p className="onboarding-welcome-text">Welcome to Keyfire — let's take a quick tour of what you can do.</p>
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
            Choose the{' '}
            <span className="onboarding-action-badge">
              <Globe size={14} strokeWidth={2} style={{ color: '#ffc832' }} />
              <strong>Open URL</strong>
            </span>
            {' '}action, enter <strong>www.google.com</strong>, name it <strong>Open Google</strong>, then click <strong>Assign to Key</strong>.
          </p>
          {stepDots}
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 3: Fire the hotkey (minimise + press → Google opens) ─
  if (step === 3) {
    const hotkeyParts = userHotkey ? userHotkey.split(' + ') : [];
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal onboarding-modal--wide">
          {!actionFired ? (
            <>
              <div className="onboarding-step-label">Step 3 of {TOTAL_STEPS}</div>
              <p className="onboarding-tooltip-text">
                Try your new hotkey now.
              </p>
              <div className="onboarding-instruction-list">
                <div className="onboarding-instruction-item">
                  <span className="onboarding-instruction-num">1</span>
                  <span className="onboarding-instruction-text">
                    <strong>Minimise Keyfire</strong> — click the Keyfire icon on your taskbar.
                  </span>
                </div>
                <div className="onboarding-instruction-item">
                  <span className="onboarding-instruction-num">2</span>
                  <span className="onboarding-instruction-text">
                    Press your hotkey
                    {hotkeyParts.length > 0 && (
                      <>
                        {' — '}
                        {hotkeyParts.map((part, i) => (
                          <React.Fragment key={i}>
                            {i > 0 && <span className="onboarding-kbd-plus">+</span>}
                            <kbd className="onboarding-kbd">{part}</kbd>
                          </React.Fragment>
                        ))}
                      </>
                    )}
                    . Google will open in your browser.
                  </span>
                </div>
                <div className="onboarding-instruction-item">
                  <span className="onboarding-instruction-num">3</span>
                  <span className="onboarding-instruction-text">
                    Come back to Keyfire — this step continues automatically.
                  </span>
                </div>
              </div>
              <p className="onboarding-hint">
                Keyfire stays running in the background — your hotkey works anywhere on your PC, in any app.
              </p>
            </>
          ) : (
            <>
              <p className="onboarding-success-text">You just used Keyfire!</p>
              <p className="onboarding-hint">
                Your hotkey works the same way in any app on your PC, <strong>except when Fire itself is the focused window</strong>. Hotkeys are paused while you're in Keyfire so you can configure them without firing them by accident.
              </p>
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
            {/* AHK is Windows-only — hidden from the mac tour */}
            {!navigator.platform.toUpperCase().includes('MAC') && (
              <div className="onboarding-feature-item"><strong>Run AHK Script</strong> — execute AutoHotkey scripts</div>
            )}
            <div className="onboarding-feature-item">
              <strong>Double-tap a key</strong> <span className="onboarding-pro-badge onboarding-pro-badge--inline">Pro</span> — tap once for one action, twice quickly for a second
            </div>
          </div>
          <p className="onboarding-hint">
            Use the mouse canvas to map mouse buttons the same way.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(5)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 5: Profiles (with app-specific as inline Pro mention) ──
  if (step === 5) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 5 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Profiles</strong> group hotkeys, expansions and quick actions together. Switch profiles to load a different set instantly.
          </p>
          <p className="onboarding-hint">
            Build one for everyday use, another for coding, another for design — keep your most-used shortcuts at your fingertips.
          </p>
          <div className="onboarding-pro-inline">
            <span className="onboarding-pro-badge onboarding-pro-badge--inline">Pro</span>
            <span className="onboarding-pro-inline-text">
              <strong>App-specific profiles</strong> link a profile to an app — Keyfire auto-switches in the background as you change focus. Excel hotkeys when Excel is open, Photoshop hotkeys when Photoshop is open.
            </span>
          </div>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => goToStep(6, 'mapping', 'radial')}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 6: Radial Menu (opens radial editor live) ──────────
  if (step === 6) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal onboarding-modal--right">
          <div className="onboarding-step-label">Step 6 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            The <strong>Radial Menu</strong> is a wheel of actions that pops up wherever your mouse is when you trigger it. Hover a wedge, release, and the action fires.
          </p>
          <p className="onboarding-hint">
            8 inner segments per wheel. <strong>Right-click an empty segment</strong> to make it a folder — folders open an outer ring of 8 more actions. Fill every segment with folders and you get up to <strong>64 actions</strong> in one wheel.
          </p>
          <p className="onboarding-hint">
            <strong>Drag and drop</strong> existing actions from your profile onto segments, or click a segment to <strong>create a new action just for the wheel</strong> using the standard editor. Set the hotkey that triggers the wheel in this panel's top-right.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => goToStep(7, 'expansions')}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 7: Text Expansions (descriptive) ───────────────────
  if (step === 7) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 7 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Text Expansions</strong> replace short triggers with full text — no hotkey needed. Type <strong>;sig</strong> + Space anywhere and your email signature appears.
          </p>
          <p className="onboarding-hint">
            Organise with colour-coded categories. Use dynamic fields like dates, clipboard contents, cursor position, and fill-in prompts. Paste images too.
          </p>
          <p className="onboarding-hint">
            <strong>Try it after the tour:</strong> create an expansion in this panel, then open any text field on your PC and type your trigger + Space.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(8)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 8: Quick Search (interactive — wait for fire) ──────
  if (step === 8) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 8 of {TOTAL_STEPS}</div>
          {!searchFired ? (
            <>
              <p className="onboarding-tooltip-text">
                <strong>Quick Search</strong> is your command centre. Search and launch any hotkey, expansion or action from anywhere on your PC.
              </p>
              <div className="onboarding-try-it">
                <span className="onboarding-try-it-pill">Try it now</span>
                <div className="onboarding-try-it-shortcut">
                  <kbd className="onboarding-kbd onboarding-kbd--lg">Ctrl</kbd>
                  <span className="onboarding-kbd-plus">+</span>
                  <kbd className="onboarding-kbd onboarding-kbd--lg">Space</kbd>
                </div>
              </div>
              <p className="onboarding-hint">
                Press the combo above — Keyfire's Quick Search overlay will appear. This step continues automatically when it opens.
              </p>
            </>
          ) : (
            <>
              <p className="onboarding-success-text">Quick Search fired!</p>
              <p className="onboarding-hint">
                You'll set up <strong>Quick Actions</strong> and <strong>Search Templates</strong> next — let's take a look at the Quick Search tab.
              </p>
              <button className="onboarding-btn-primary" onClick={() => goToStep(9, 'templates')}>
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

  // ── Step 9: Quick Actions ───────────────────────────────────
  if (step === 9) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 9 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Quick Actions</strong> let you launch apps, open folders, URLs, or run macros — accessible instantly from Quick Search without assigning a hotkey.
          </p>
          <p className="onboarding-hint">
            Organise with categories. Search by name from the Ctrl+Space overlay.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(10)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 10: Search Templates ───────────────────────────────
  if (step === 10) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 10 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Search Templates</strong> let you search any website from Quick Search. Type a trigger + Space, then your query.
          </p>
          <p className="onboarding-hint">
            Presets include Google, ChatGPT, Perplexity, GitHub, and more. Add your own for any website with a search URL.
          </p>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => goToStep(11, 'clipboard')}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 11: Clipboard Manager ──────────────────────────────
  if (step === 11) {
    return (
      <div className="onboarding-overlay">
        {renderOverlay()}
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 11 of {TOTAL_STEPS}</div>
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
          <button className="onboarding-btn-secondary" onClick={() => setStep(12)}>Next</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 12: Closing ≠ Quitting — Keyfire lives in the tray ───
  if (step === 12) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal">
          <div className="onboarding-step-label">Step 12 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            <strong>Closing Keyfire hides it — it doesn't quit it.</strong>
          </p>
          <p className="onboarding-hint">
            When you click the × on the window, Keyfire keeps running in the system tray so your hotkeys, expansions and clipboard history all stay active. To fully quit Keyfire, right-click the tray icon and choose <strong>Quit</strong>.
          </p>
          <div className="onboarding-tray-illustration">
            <span className="onboarding-tray-arrow">↘</span>
            <span className="onboarding-tray-label">System tray (bottom-right of your taskbar)</span>
          </div>
          {stepDots}
          <button className="onboarding-btn-secondary" onClick={() => setStep(13)}>Got it</button>
          {skipLink}
        </div>
      </div>
    );
  }

  // ── Step 13: You're All Set — shortcuts cheatsheet ──────────
  if (step === 13) {
    return (
      <div className="onboarding-overlay">
        <div className="onboarding-backdrop" />
        {dragRegion}
        <div className="onboarding-modal onboarding-modal--wide">
          <div className="onboarding-step-label">Step 13 of {TOTAL_STEPS}</div>
          <p className="onboarding-tooltip-text">
            You're all set. Here are the global shortcuts you'll use most:
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
          <div className="onboarding-shortcut-row onboarding-shortcut-row--centred">
            <kbd className="onboarding-kbd">Ctrl</kbd>
            <span className="onboarding-kbd-plus">+</span>
            <kbd className="onboarding-kbd">Alt</kbd>
            <span className="onboarding-kbd-plus">+</span>
            <kbd className="onboarding-kbd">Q</kbd>
            <span className="onboarding-shortcut-label">Global Pause — toggle Keyfire on/off</span>
          </div>
          <p className="onboarding-hint">
            All three shortcuts are <strong>customisable in Settings</strong>. Check <strong>Analytics</strong> for time saved.
          </p>
          <p className="onboarding-hint">
            During the beta, Keyfire sends one anonymous daily count (just totals: no content, no identifiers) to help us improve. Toggle off any time in <strong>Settings → Privacy &amp; Security</strong>.
          </p>
          {stepDots}
          <button className="onboarding-btn-primary" onClick={finish}>Finish</button>
        </div>
      </div>
    );
  }

  return null;
}
