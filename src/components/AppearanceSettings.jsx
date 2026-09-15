// Settings > Appearance: light/dark mode, theme presets (Free) and the Pro
// custom-colour editor. Renders inside SettingsPanel (main window AND the
// standalone Settings window), so every change goes through the bridged
// handler props; nothing here touches config directly.
import React, { useState, useEffect, useLayoutEffect, useRef, useCallback } from 'react';
import ReactDOM from 'react-dom';
import { Sun, Moon, Monitor, Lock, Copy, Check, AlertTriangle } from 'lucide-react';
import './AppearanceSettings.css';
import ColourPicker, { CATEGORY_COLOURS } from './ColourPicker';
import { PRESETS, KEYFIRE_ID, CUSTOM_ID, getPreset } from '../theme/presets';
import {
  KNOB_KEYS, KNOB_META, customThemeFromPreset, contrastIssues,
  encodeThemeCode, decodeThemeCode,
} from '../theme/engine';

const MODES = [
  { value: 'auto',  label: 'Follow System', Icon: Monitor },
  { value: 'light', label: 'Light',         Icon: Sun },
  { value: 'dark',  label: 'Dark',          Icon: Moon },
];

// The picker's swatch row without the "None" entry: a theme knob always has
// a colour.
const KNOB_SWATCHES = CATEGORY_COLOURS.filter(c => c.hex);

function currentHalf(theme) {
  if (theme === 'light' || theme === 'dark') return theme;
  try {
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  } catch {
    return 'dark';
  }
}

/**
 * Two-tone miniature of a theme: the dark half on top, the light half below,
 * each showing background, a panel with the accent dot, a text line and a
 * key cap. Colours travel as inline custom properties (never as text, so the
 * Settings search never matches a hex string).
 */
function ThemePreview({ light, dark, rainbow = false }) {
  const half = (k, key) => (
    <div
      key={key}
      className="appear-preview-half"
      style={{
        '--pv-bg': k.background, '--pv-panel': k.panel, '--pv-accent': k.accent,
        '--pv-text': k.text, '--pv-border': k.border, '--pv-keycap': k.keycap,
      }}
    >
      <span className="appear-preview-key" />
      <span className="appear-preview-panel">
        <span className={`appear-preview-dot${rainbow ? ' appear-preview-dot--rainbow' : ''}`} />
        <span className="appear-preview-line" />
      </span>
    </div>
  );
  return (
    <div className="appear-preview" aria-hidden="true">
      {half(dark, 'dark')}
      {half(light, 'light')}
    </div>
  );
}

export default function AppearanceSettings({
  theme = 'auto',
  onSetTheme,
  themePreset = KEYFIRE_ID,
  onSetThemePreset,
  customTheme = null,
  onSetCustomTheme,
  onSetPreviewHalf,
  isPro = false,
  onShowUpgrade,
  // SettingsPanel's Esc chain calls escInterceptRef.current() first; return
  // true to swallow the key (an open colour picker closes instead of the
  // whole Settings window).
  escInterceptRef,
  // Windows accent, high-contrast follow, popup transparency, interface scale.
  themeAccentFollowsWindows = false,
  onSetAccentFollowsWindows,
  windowsAccent = null,
  themeFollowHighContrast = true,
  onSetFollowHighContrast,
  systemHighContrast = false,
  overlayOpacity = 1,
  onSetOverlayOpacity,
  uiScale = 1,
  onSetUiScale,
}) {
  const highContrastActive = themeFollowHighContrast && systemHighContrast;
  const opacityPct = Math.round((Number.isFinite(overlayOpacity) ? overlayOpacity : 1) * 100);
  const scalePct = Math.round((Number.isFinite(uiScale) ? uiScale : 1) * 100);
  const keyfire = getPreset(KEYFIRE_ID);
  const customPreview = customTheme || { light: keyfire.light, dark: keyfire.dark };
  const customActive = themePreset === CUSTOM_ID;
  // A saved custom theme whose Pro has lapsed: still shown (greyed, PRO
  // badge) so the user sees it is kept and comes back on upgrade.
  const customLocked = !isPro;
  const editorVisible = isPro && customActive && !!customTheme;

  const pickCustom = () => {
    if (customLocked) { onShowUpgrade?.('Custom themes'); return; }
    // First time: start from whatever the user is looking at right now.
    if (!customTheme) onSetCustomTheme?.(customThemeFromPreset(themePreset));
    onSetThemePreset?.(CUSTOM_ID);
  };

  // ── Editor: which half is being edited ──────────────────────────────────
  // Follows the live half (and any Mode change) but is published as a
  // transient preview override, never written to `theme`, so an Auto user
  // can edit the dark half without being flipped to explicit Dark.
  const [editHalf, setEditHalf] = useState(() => currentHalf(theme));
  useEffect(() => { setEditHalf(currentHalf(theme)); }, [theme]);
  // One publish per change (no clear-then-set pair, which would flash the
  // real half between two bridged events); cleared on unmount and while the
  // Settings window is hidden.
  useEffect(() => {
    onSetPreviewHalf?.(editorVisible ? editHalf : null);
  }, [editorVisible, editHalf]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => () => onSetPreviewHalf?.(null), []); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!editorVisible) return undefined;
    const onVis = () => onSetPreviewHalf?.(document.visibilityState === 'visible' ? editHalf : null);
    document.addEventListener('visibilitychange', onVis);
    return () => document.removeEventListener('visibilitychange', onVis);
  }, [editorVisible, editHalf]); // eslint-disable-line react-hooks/exhaustive-deps

  const knobs = editorVisible ? customTheme[editHalf] : null;
  const baseKnobs = editorVisible ? getPreset(customTheme.base || KEYFIRE_ID)[editHalf] : null;
  const issues = editorVisible ? contrastIssues(knobs, editHalf) : [];

  const setKnob = useCallback((knob, hex) => {
    if (!customTheme) return;
    onSetCustomTheme?.({
      ...customTheme,
      [editHalf]: { ...customTheme[editHalf], [knob]: hex },
    });
  }, [customTheme, editHalf, onSetCustomTheme]);

  // ── Colour picker popover (portal, canonical <ColourPicker>) ────────────
  const [picker, setPicker] = useState(null); // { knob, x, y }
  const pickerRef = useRef(null);
  const openPicker = (e, knob) => {
    e.stopPropagation();
    const rect = e.currentTarget.getBoundingClientRect();
    // anchorH lets the clamp flip the popover above the swatch when the
    // space below runs out.
    setPicker(prev => (prev?.knob === knob ? null : { knob, x: rect.left, y: rect.bottom + 4, anchorH: rect.height + 4 }));
  };
  useEffect(() => {
    if (!picker) return undefined;
    const onDown = (e) => {
      if (!pickerRef.current?.contains(e.target)) setPicker(null);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [picker]);
  // Keep the popover inside the viewport. It opens below the swatch, which
  // can sit near the bottom of a short Settings window, and it GROWS when the
  // "+" custom panel expands, so the clamp re-runs on every size change
  // (ResizeObserver) rather than once at open. Prefers flipping above the
  // swatch when the space below is too short; otherwise pins to the edge.
  useLayoutEffect(() => {
    if (!picker || !pickerRef.current) return undefined;
    const el = pickerRef.current;
    const margin = 8;
    const place = () => {
      const { height, width } = el.getBoundingClientRect();
      const spaceBelow = window.innerHeight - margin - picker.y;
      let top = picker.y;
      if (height > spaceBelow) {
        const above = picker.y - picker.anchorH - 8 - height;
        top = above >= margin ? above : Math.max(margin, window.innerHeight - height - margin);
      }
      let left = picker.x;
      if (left + width > window.innerWidth - margin) {
        left = Math.max(margin, window.innerWidth - width - margin);
      }
      el.style.top = `${top}px`;
      el.style.left = `${left}px`;
    };
    place();
    const ro = new ResizeObserver(place);
    ro.observe(el);
    window.addEventListener('resize', place);
    return () => { ro.disconnect(); window.removeEventListener('resize', place); };
  }, [picker]);
  useEffect(() => {
    if (!escInterceptRef) return undefined;
    escInterceptRef.current = () => {
      if (!picker) return false;
      setPicker(null);
      return true;
    };
    return () => { escInterceptRef.current = null; };
  }, [escInterceptRef, picker]);
  useEffect(() => { if (!editorVisible) setPicker(null); }, [editorVisible]);

  // ── Share: theme code copy / paste ──────────────────────────────────────
  const [codeInput, setCodeInput] = useState('');
  const [codeError, setCodeError] = useState(null);
  const [chip, setChip] = useState(null); // transient feedback text
  const chipTimer = useRef(null);
  const flash = (text) => {
    setChip(text);
    clearTimeout(chipTimer.current);
    chipTimer.current = setTimeout(() => setChip(null), 1400);
  };
  useEffect(() => () => clearTimeout(chipTimer.current), []);

  const copyCode = () => {
    const code = encodeThemeCode(customTheme);
    if (!code) return;
    window.electronAPI?.copyText?.(code);
    flash('Theme code copied');
  };
  const applyCode = () => {
    const decoded = decodeThemeCode(codeInput);
    if (!decoded) {
      setCodeError('That is not a Keyfire theme code. It starts with kf1.');
      return;
    }
    setCodeError(null);
    setCodeInput('');
    onSetCustomTheme?.(decoded);
    flash('Theme applied');
  };

  return (
    <>
      <label className="settings-field-label">Mode</label>
      <div className="appear-mode-seg" role="radiogroup" aria-label="Light or dark mode">
        {MODES.map(({ value, label, Icon }) => (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={theme === value}
            className={`appear-seg-btn${theme === value ? ' active' : ''}`}
            onClick={() => onSetTheme?.(value)}
          >
            <Icon size={13} strokeWidth={2} />
            <span>{label}</span>
          </button>
        ))}
      </div>
      <p className="settings-toggle-sub appear-sub">
        Every theme below comes as a light and dark pair. This mode picks which half you see, and Follow System switches with Windows.
      </p>

      <div className="settings-toggle-row">
        <div className="settings-toggle-info">
          <span className="settings-toggle-label">
            Use Windows accent colour
            {themeAccentFollowsWindows && windowsAccent && (
              <span className="appear-accent-chip" style={{ '--knob-hex': windowsAccent }} title={windowsAccent} aria-hidden="true" />
            )}
          </span>
          <span className="settings-toggle-sub">
            Keyfire's accent follows the colour set in Windows Settings &gt; Personalisation &gt; Colours, on top of any theme. Everything else in the theme stays as it is.
          </span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={themeAccentFollowsWindows}
          className={`settings-toggle${themeAccentFollowsWindows ? ' on' : ''}`}
          onClick={() => onSetAccentFollowsWindows?.(!themeAccentFollowsWindows)}
          aria-label="Use Windows accent colour"
        />
      </div>
      <div className="settings-toggle-row">
        <div className="settings-toggle-info">
          <span className="settings-toggle-label">Follow Windows high contrast</span>
          <span className="settings-toggle-sub">
            Switch to the High Contrast theme automatically while a Windows high-contrast theme is on. Your own choice below is kept for when it is off.
            {highContrastActive && ' Active now.'}
          </span>
        </div>
        <button
          type="button"
          role="switch"
          aria-checked={themeFollowHighContrast}
          className={`settings-toggle${themeFollowHighContrast ? ' on' : ''}`}
          onClick={() => onSetFollowHighContrast?.(!themeFollowHighContrast)}
          aria-label="Follow Windows high contrast"
        />
      </div>

      <div className="settings-subheader settings-subsection">Display</div>
      <div className="settings-slider-row">
        <div className="settings-slider-info">
          <span className="settings-toggle-label">Popup transparency</span>
          <span className="settings-toggle-sub">Quick Search and the clipboard popup show a little of what is behind them. Text stays solid.</span>
        </div>
        <div className="settings-slider-ctrl">
          <input
            type="range"
            className="settings-slider"
            min="60" max="100" step="5"
            value={opacityPct}
            onChange={e => onSetOverlayOpacity?.(Number(e.target.value) / 100)}
            aria-label="Popup opacity"
          />
          <span className="settings-slider-val">{opacityPct}%</span>
          {opacityPct !== 100 && (
            <button
              type="button"
              className="settings-slider-reset"
              onClick={() => onSetOverlayOpacity?.(1)}
              title="Reset to opaque"
              aria-label="Reset popup transparency"
            >↺</button>
          )}
        </div>
      </div>

      <div className="settings-toggle-row appear-scale-row">
        <div className="settings-toggle-info">
          <span className="settings-toggle-label">Interface size</span>
          <span className="settings-toggle-sub">Scales the main window and Settings on this PC. Popups keep their own size so nothing gets clipped.</span>
        </div>
        <div className="appear-mode-seg appear-scale-seg" role="radiogroup" aria-label="Interface size">
          {[90, 100, 110, 125].map(pct => (
            <button
              key={pct}
              type="button"
              role="radio"
              aria-checked={scalePct === pct}
              className={`appear-seg-btn${scalePct === pct ? ' active' : ''}`}
              onClick={() => onSetUiScale?.(pct / 100)}
            >
              {pct}%
            </button>
          ))}
        </div>
      </div>

      <div className="settings-subheader settings-subsection">Theme</div>
      {highContrastActive && (
        <p className="settings-toggle-sub appear-sub">
          Windows high contrast is on, so the High Contrast theme is showing. The theme selected below returns when it is turned off.
        </p>
      )}
      <div className="appear-theme-grid" role="radiogroup" aria-label="Theme preset">
        {PRESETS.map(p => {
          const active = themePreset === p.id;
          return (
            <button
              key={p.id}
              type="button"
              role="radio"
              aria-checked={active}
              className={`appear-theme-card${active ? ' active' : ''}`}
              onClick={() => onSetThemePreset?.(p.id)}
            >
              <ThemePreview light={p.light} dark={p.dark} />
              <span className="appear-theme-name">{p.name}</span>
              <span className="appear-theme-tagline">{p.tagline}</span>
            </button>
          );
        })}

        <button
          type="button"
          role="radio"
          aria-checked={customActive && !customLocked}
          className={`appear-theme-card appear-theme-card--custom${customActive && !customLocked ? ' active' : ''}${customLocked ? ' locked' : ''}`}
          onClick={pickCustom}
          title={customLocked ? 'Pro: build your own theme from eight colours' : 'Your own colours'}
        >
          <ThemePreview light={customPreview.light} dark={customPreview.dark} rainbow={!customTheme} />
          <span className="appear-theme-name">
            Custom
            {customLocked && <span className="pro-badge">PRO</span>}
            {customLocked && <Lock size={11} strokeWidth={2} className="appear-lock" />}
          </span>
          <span className="appear-theme-tagline">
            {customTheme ? 'Your saved colours.' : 'Pick your own eight colours and share them as a code.'}
          </span>
        </button>
      </div>

      {editorVisible && (
        <div className="appear-editor">
          <div className="appear-editor-head">
            <div className="settings-subheader">Custom colours</div>
            <div className="appear-mode-seg" role="radiogroup" aria-label="Half being edited">
              {[['dark', 'Dark', Moon], ['light', 'Light', Sun]].map(([h, label, Icon]) => (
                <button
                  key={h}
                  type="button"
                  role="radio"
                  aria-checked={editHalf === h}
                  className={`appear-seg-btn${editHalf === h ? ' active' : ''}`}
                  onClick={() => setEditHalf(h)}
                  title={`Edit the ${label.toLowerCase()} half (previews it while you are here)`}
                >
                  <Icon size={13} strokeWidth={2} />
                  <span>Editing {label}</span>
                </button>
              ))}
            </div>
          </div>
          <p className="settings-toggle-sub appear-sub">
            Each half has its own eight colours. Changes apply everywhere as you confirm them, and the app previews the half you are editing while this section is open.
          </p>

          <div className="appear-start-row">
            <span className="settings-field-label appear-inline-label">Start from</span>
            <select
              className="settings-select appear-start-select"
              value=""
              onChange={e => {
                if (e.target.value) onSetCustomTheme?.(customThemeFromPreset(e.target.value));
              }}
              aria-label="Copy a preset into the custom theme"
            >
              <option value="">Copy a preset into both halves</option>
              {PRESETS.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}
            </select>
          </div>

          <div className="appear-knobs">
            {KNOB_KEYS.map(knob => {
              const hex = knobs[knob];
              const baseHex = baseKnobs[knob];
              const issue = issues.find(i => i.knob === knob);
              const meta = KNOB_META[knob];
              // The accent knob is overridden while the Windows accent option
              // is on: show the live value, disable the picker, say why.
              const followed = knob === 'accent' && themeAccentFollowsWindows && !!windowsAccent;
              return (
                <div key={knob} className={`appear-knob-row${followed ? ' followed' : ''}`}>
                  <button
                    type="button"
                    className={`appear-knob-swatch${picker?.knob === knob ? ' open' : ''}`}
                    style={{ '--knob-hex': followed ? windowsAccent : hex }}
                    onClick={e => { if (!followed) openPicker(e, knob); }}
                    disabled={followed}
                    aria-label={`Choose ${meta.label} colour`}
                    aria-haspopup="dialog"
                    aria-expanded={picker?.knob === knob}
                  />
                  <div className="settings-toggle-info appear-knob-info">
                    <span className="settings-toggle-label">{meta.label}</span>
                    <span className="settings-toggle-sub">
                      {followed ? 'Following the Windows accent colour while that option is on.' : meta.hint}
                    </span>
                    {issue && (
                      <span className="appear-knob-warn">
                        <AlertTriangle size={11} strokeWidth={2} />
                        Low contrast against the {KNOB_META[issue.against].label.toLowerCase()} ({issue.ratio}:1, aim for 4.5)
                      </span>
                    )}
                  </div>
                  <span className="appear-knob-hex" title={hex}>{hex}</span>
                  {hex !== baseHex && (
                    <button
                      type="button"
                      className="settings-slider-reset"
                      onClick={() => setKnob(knob, baseHex)}
                      title={`Reset to the ${getPreset(customTheme.base || KEYFIRE_ID).name} value`}
                      aria-label={`Reset ${meta.label}`}
                    >↺</button>
                  )}
                </div>
              );
            })}
          </div>

          <div className="settings-subheader settings-subsection">Share</div>
          <p className="settings-toggle-sub appear-sub">
            A theme code carries both halves as text. Paste one from a colleague to match their setup.
          </p>
          <div className="appear-share-row">
            <button type="button" className="settings-action-btn" onClick={copyCode}>
              <Copy size={13} strokeWidth={2} />
              Copy theme code
            </button>
            <input
              type="text"
              className="appear-code-input"
              value={codeInput}
              onChange={e => { setCodeInput(e.target.value); if (codeError) setCodeError(null); }}
              onKeyDown={e => { e.stopPropagation(); if (e.key === 'Enter') { e.preventDefault(); applyCode(); } }}
              placeholder="Paste a theme code (kf1.…)"
              spellCheck={false}
              aria-label="Theme code"
            />
            <button
              type="button"
              className="settings-action-btn"
              onClick={applyCode}
              disabled={!codeInput.trim()}
            >
              Apply
            </button>
            {chip && (
              <span className="appear-chip" role="status">
                <Check size={12} strokeWidth={2.5} />
                {chip}
              </span>
            )}
          </div>
          {codeError && <div className="settings-conflict-warn">{codeError}</div>}
        </div>
      )}

      {picker && editorVisible && ReactDOM.createPortal(
        <div
          ref={pickerRef}
          className="appear-colour-popover"
          style={{ left: picker.x, top: picker.y }}
          role="dialog"
          aria-label={`${KNOB_META[picker.knob].label} colour`}
        >
          <ColourPicker
            value={knobs[picker.knob]}
            presets={KNOB_SWATCHES}
            onChange={(hex) => { if (hex) setKnob(picker.knob, hex); }}
          />
        </div>,
        document.body
      )}
    </>
  );
}
