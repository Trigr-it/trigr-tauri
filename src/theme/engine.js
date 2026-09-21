// Theme engine: turns eight knobs into the full set of MOOD tokens the CSS
// references, and resolves config (mode + preset + custom theme + tier) into
// the snapshot every window applies. Pure functions, no DOM.
//
// Meaning colours (--danger*, --status-*, --type-*, --mod-*, the #22c55e
// green dot) are deliberately NOT in THEME_TOKENS: they carry semantics, not
// mood, and never follow a theme.
import {
  mix, rgba, toRgbTriple, luminance, contrast, shiftLightness,
  adjustForContrast, normaliseHex,
} from './colour.js';
import { PRESETS, KEYFIRE_ID, CUSTOM_ID, getPreset, isPresetId } from './presets.js';

export const KNOB_KEYS = ['accent', 'background', 'panel', 'field', 'text', 'textMuted', 'border', 'keycap'];

export const KNOB_META = {
  accent:     { label: 'Accent',        hint: 'Buttons, active states, section titles and highlights.' },
  background: { label: 'Background',    hint: 'The window itself, behind every panel.' },
  panel:      { label: 'Panels',        hint: 'Cards, editors, popovers and the sidebar.' },
  field:      { label: 'Fields',        hint: 'Inputs, text areas and selects. One step below the panel.' },
  text:       { label: 'Text',          hint: 'Primary text and icons.' },
  textMuted:  { label: 'Muted text',    hint: 'Labels, hints and secondary text.' },
  border:     { label: 'Borders',       hint: 'Dividers and outlines.' },
  keycap:     { label: 'Key caps',      hint: 'Keyboard keys and the neutral view-switcher chips.' },
};

// Every CSS custom property deriveTokens() emits. applySnapshot() walks this
// list and REMOVES any entry the snapshot does not carry, so switching back
// to Keyfire (tokens === null) restores the stylesheet defaults everywhere.
// scripts/check-theme-tokens.mjs asserts each is declared in global.css.
export const THEME_TOKENS = [
  '--bg-base', '--bg-surface', '--bg-elevated', '--bg-hover', '--bg-active', '--bg-input',
  '--accent', '--accent-hover', '--accent-dim', '--accent-rgb', '--accent-glow', '--accent-glow-strong',
  '--accent-surface', '--accent-surface-hover', '--on-accent',
  '--accent-text', '--accent-text-strong', '--accent-bright', '--accent-highlight',
  '--text-primary', '--text-secondary', '--text-muted',
  '--border', '--border-bright', '--border-accent',
  '--key-bg', '--key-bg-hover', '--key-bg-assigned', '--key-bg-selected',
  '--key-border', '--key-border-assigned', '--key-border-selected',
  '--btn-primary-bg', '--btn-primary-text', '--btn-primary-hover',
  '--btn-secondary-border', '--btn-secondary-text', '--btn-ghost-text',
  '--radial-wedge', '--radial-plate', '--radial-hub', '--radial-hairline', '--radial-rim',
  // Not derived from the knobs: popup transparency (Settings > Appearance),
  // emitted as a percentage only when below 100 %.
  '--overlay-alpha',
];

const INK = '#0d0d11';
const WHITE = '#ffffff';

/** True when `knobs` has all eight keys as valid hex strings. */
export function isValidKnobs(knobs) {
  return !!knobs && typeof knobs === 'object'
    && KNOB_KEYS.every(k => normaliseHex(knobs[k]) !== null);
}

/** Lower-case, '#rrggbb' copy of the eight knobs, or null when incomplete. */
export function normaliseKnobs(knobs) {
  if (!isValidKnobs(knobs)) return null;
  const out = {};
  for (const k of KNOB_KEYS) out[k] = normaliseHex(knobs[k]);
  return out;
}

/**
 * Derive every mood token from the eight knobs.
 * `half` is 'dark' | 'light' and only steers the few derivations whose
 * direction depends on the side (accent hover, glow alpha); every surface
 * step mixes toward `text`, which is what today's ramps do in both halves
 * (ink-700 -> 600 -> 500 on dark, paper-100 -> 300 -> 400 on light).
 */
export function deriveTokens(rawKnobs, half) {
  const k = normaliseKnobs(rawKnobs);
  if (!k) return null;
  const light = half === 'light';
  const { accent, background, panel, field, text, textMuted, border, keycap } = k;

  const accentHover = shiftLightness(accent, light ? -0.06 : 0.06);
  const accentDim = shiftLightness(accent, -0.14);
  const accentBright = shiftLightness(accent, 0.10);
  const accentHighlight = shiftLightness(accent, 0.20);
  // Text on the accent: white unless the accent is light enough that white
  // would fall under ~3:1 (relative luminance above 0.30).
  const onAccent = luminance(accent) > 0.30 ? INK : WHITE;
  // Accent used AS text on the light half must hold 4.5:1 against the panel.
  const accentText = light ? adjustForContrast(accent, panel, 4.5, -1) : accent;
  const accentTextStrong = light ? shiftLightness(accentText, -0.08) : accent;
  const textSecondary = mix(text, textMuted, 0.5);
  // Surface steps toward the text colour. Calibrated against the Keyfire
  // ramps: ink-700 -> 600 -> 500 is ~+6/+11 per channel on dark, paper-100 ->
  // 300 -> 400 is ~-10/-21 on light, so the dark half steps half as far.
  const hoverT = light ? 0.05 : 0.03;
  const activeT = light ? 0.10 : 0.06;

  return {
    '--bg-base': background,
    '--bg-surface': mix(background, panel, 0.5),
    '--bg-elevated': panel,
    '--bg-hover': mix(panel, text, hoverT),
    '--bg-active': mix(panel, text, activeT),
    '--bg-input': field,

    '--accent': accent,
    '--accent-hover': accentHover,
    '--accent-dim': accentDim,
    '--accent-rgb': toRgbTriple(accent),
    '--accent-glow': rgba(accent, light ? 0.10 : 0.15),
    '--accent-glow-strong': rgba(accent, 0.3),
    '--accent-surface': mix(panel, accent, 0.18),
    '--accent-surface-hover': mix(panel, accent, 0.28),
    '--on-accent': onAccent,
    '--accent-text': accentText,
    '--accent-text-strong': accentTextStrong,
    '--accent-bright': accentBright,
    '--accent-highlight': accentHighlight,

    '--text-primary': text,
    '--text-secondary': textSecondary,
    '--text-muted': textMuted,

    '--border': border,
    '--border-bright': mix(border, text, 0.09),
    '--border-accent': rgba(accent, light ? 0.5 : 0.4),

    '--key-bg': keycap,
    '--key-bg-hover': mix(keycap, text, hoverT),
    '--key-bg-assigned': mix(keycap, accent, 0.08),
    '--key-bg-selected': mix(keycap, accent, 0.14),
    '--key-border': border,
    '--key-border-assigned': rgba(accent, light ? 0.5 : 0.65),
    '--key-border-selected': accent,

    '--btn-primary-bg': accent,
    '--btn-primary-text': onAccent,
    '--btn-primary-hover': accentHover,
    '--btn-secondary-border': mix(border, text, 0.09),
    '--btn-secondary-text': textSecondary,
    '--btn-ghost-text': textMuted,

    '--radial-wedge': panel,
    // Layered rings: plate halfway from the panel to the window background,
    // hub = the background itself (both one tone below the wedge in either
    // half), hairline/rim = faint text-coloured lines like the Keyfire values.
    '--radial-plate': mix(panel, background, 0.5),
    '--radial-hub': background,
    '--radial-hairline': rgba(text, light ? 0.10 : 0.09),
    '--radial-rim': rgba(text, light ? 0.08 : 0.06),
  };
}

/** Contrast warnings for the Pro editor. Returns [{ knob, against, ratio }]. */
export function contrastIssues(rawKnobs, half, minRatio = 4.5) {
  const k = normaliseKnobs(rawKnobs);
  if (!k) return [];
  const issues = [];
  const check = (knob, colour, againstKnob, bg) => {
    const ratio = contrast(colour, bg);
    if (ratio < minRatio) issues.push({ knob, against: againstKnob, ratio: Math.round(ratio * 10) / 10 });
  };
  check('text', k.text, 'panel', k.panel);
  check('text', k.text, 'background', k.background);
  check('textMuted', k.textMuted, 'panel', k.panel);
  const accentText = half === 'light' ? adjustForContrast(k.accent, k.panel, minRatio, -1) : k.accent;
  check('accent', accentText, 'panel', k.panel);
  return issues;
}

// ── Custom theme shape ──────────────────────────────────────────────────────
// { v: 1, base: presetId, light: Knobs, dark: Knobs }. `base` is the preset
// the colours were seeded from (drives the per-knob reset). Stored in the
// shared config; older builds ignore it. normaliseCustomTheme() is the only
// way a value reaches state, so a hand-edited or pasted object can never
// carry extra keys.

export function normaliseCustomTheme(obj) {
  if (!obj || typeof obj !== 'object') return null;
  const light = normaliseKnobs(obj.light);
  const dark = normaliseKnobs(obj.dark);
  if (!light || !dark) return null;
  const base = isPresetId(obj.base) ? obj.base : KEYFIRE_ID;
  return { v: 1, base, light, dark };
}

/** A custom theme seeded from a preset (both halves copied). */
export function customThemeFromPreset(presetId) {
  const p = getPreset(presetId) || getPreset(KEYFIRE_ID);
  return { v: 1, base: p.id, light: { ...p.light }, dark: { ...p.dark } };
}

// ── Theme codes (share a custom theme as text) ─────────────────────────────
// 'kf1.' + base64url(JSON { v:1, light, dark }). Decoding is strict: version,
// exactly the eight knobs per half, '#rrggbb' values only.

const CODE_PREFIX = 'kf1.';

function b64urlEncode(str) {
  const bytes = new TextEncoder().encode(str);
  let bin = '';
  bytes.forEach(b => { bin += String.fromCharCode(b); });
  return btoa(bin).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function b64urlDecode(str) {
  const b64 = str.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - (str.length % 4)) % 4);
  const bin = atob(b64);
  const bytes = Uint8Array.from(bin, c => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

export function encodeThemeCode(customTheme) {
  const t = normaliseCustomTheme(customTheme);
  if (!t) return null;
  return CODE_PREFIX + b64urlEncode(JSON.stringify(t));
}

/** Returns a normalised custom theme, or null for anything malformed. */
export function decodeThemeCode(code) {
  if (typeof code !== 'string') return null;
  const s = code.trim();
  if (!s.startsWith(CODE_PREFIX)) return null;
  try {
    const obj = JSON.parse(b64urlDecode(s.slice(CODE_PREFIX.length)));
    if (!obj || obj.v !== 1) return null;
    if (Object.keys(obj).some(x => !['v', 'base', 'light', 'dark'].includes(x))) return null;
    for (const half of ['light', 'dark']) {
      const keys = Object.keys(obj[half] || {});
      if (keys.length !== KNOB_KEYS.length || keys.some(x => !KNOB_KEYS.includes(x))) return null;
    }
    return normaliseCustomTheme(obj);
  } catch {
    return null;
  }
}

// ── Resolution ──────────────────────────────────────────────────────────────

export const HIGH_CONTRAST_ID = 'contrast';
export const OVERLAY_OPACITY_MIN = 0.6;

/**
 * Resolve what every window should paint.
 *   half           'dark' | 'light' (App.jsx has already resolved 'auto')
 *   themePreset    preset id, or 'custom'
 *   customTheme    normalised custom theme or null
 *   isPro          custom themes are Pro; a lapsed Pro falls back to Keyfire
 *                  WITHOUT touching config, so re-upgrading restores the theme
 *   previewHalf    transient editor override for the half (never persisted)
 *   highContrast   Windows high-contrast is on and the user follows it: the
 *                  High Contrast preset wins over everything (not persisted)
 *   accentOverride '#rrggbb' from the Windows accent colour, or null
 *   overlayOpacity 0.6..1 popup transparency; below 1 emits --overlay-alpha
 * Returns { v: 1, half, presetId, tokens | null }. tokens is null only for
 * an untouched Keyfire, which is what keeps the shipped look byte-identical.
 */
export function resolveTheme({
  half, themePreset, customTheme, isPro, previewHalf,
  highContrast = false, accentOverride = null, overlayOpacity = 1,
} = {}) {
  const h = (previewHalf === 'light' || previewHalf === 'dark') ? previewHalf
    : (half === 'light' ? 'light' : 'dark');
  let presetId = typeof themePreset === 'string' ? themePreset : KEYFIRE_ID;
  let knobs = null;

  if (highContrast) {
    presetId = HIGH_CONTRAST_ID;
    knobs = getPreset(HIGH_CONTRAST_ID)[h];
  } else if (presetId === CUSTOM_ID) {
    const custom = normaliseCustomTheme(customTheme);
    if (isPro && custom) {
      knobs = custom[h];
    } else {
      presetId = KEYFIRE_ID;
    }
  } else {
    const p = getPreset(presetId);
    if (!p) presetId = KEYFIRE_ID;
    else if (presetId !== KEYFIRE_ID) knobs = p[h];
  }

  // Windows accent: swap the accent knob (Keyfire's own knobs when nothing
  // else is active). High contrast keeps its own accent for legibility.
  const accent = normaliseHex(accentOverride);
  if (accent && !highContrast) {
    knobs = { ...(knobs || getPreset(KEYFIRE_ID)[h]), accent };
  }

  let tokens = knobs ? deriveTokens(knobs, h) : null;

  const alpha = Number(overlayOpacity);
  if (Number.isFinite(alpha) && alpha < 1) {
    const pct = Math.round(Math.max(OVERLAY_OPACITY_MIN, alpha) * 100);
    tokens = { ...(tokens || {}), '--overlay-alpha': `${pct}%` };
  }

  return { v: 1, half: h, presetId, tokens };
}

export { PRESETS, KEYFIRE_ID, CUSTOM_ID, getPreset, isPresetId };
