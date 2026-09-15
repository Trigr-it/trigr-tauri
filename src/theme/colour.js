// Colour maths for the theme engine. Plain functions, no dependencies.
// Everything works on '#rrggbb' strings; invalid input falls back to black so
// a bad knob can never throw while a window is painting.

const HEX_RE = /^#[0-9a-f]{6}$/i;

export function isHex(v) {
  return typeof v === 'string' && HEX_RE.test(v);
}

export function normaliseHex(v) {
  if (typeof v !== 'string') return null;
  let s = v.trim().toLowerCase();
  if (/^[0-9a-f]{6}$/.test(s)) s = `#${s}`;
  if (/^#[0-9a-f]{3}$/.test(s)) s = `#${s[1]}${s[1]}${s[2]}${s[2]}${s[3]}${s[3]}`;
  return HEX_RE.test(s) ? s : null;
}

export function hexToRgb(hex) {
  const h = normaliseHex(hex) || '#000000';
  return {
    r: parseInt(h.slice(1, 3), 16),
    g: parseInt(h.slice(3, 5), 16),
    b: parseInt(h.slice(5, 7), 16),
  };
}

const clamp255 = (n) => Math.max(0, Math.min(255, Math.round(n)));

export function rgbToHex({ r, g, b }) {
  const to2 = (n) => clamp255(n).toString(16).padStart(2, '0');
  return `#${to2(r)}${to2(g)}${to2(b)}`;
}

/** "r, g, b" — the shape the rgba(var(--accent-rgb), a) pattern expects. */
export function toRgbTriple(hex) {
  const { r, g, b } = hexToRgb(hex);
  return `${r}, ${g}, ${b}`;
}

export function rgba(hex, alpha) {
  return `rgba(${toRgbTriple(hex)}, ${alpha})`;
}

/** Linear blend of a toward b by t (0..1) in sRGB space. */
export function mix(a, b, t) {
  const A = hexToRgb(a);
  const B = hexToRgb(b);
  const k = Math.max(0, Math.min(1, t));
  return rgbToHex({
    r: A.r + (B.r - A.r) * k,
    g: A.g + (B.g - A.g) * k,
    b: A.b + (B.b - A.b) * k,
  });
}

export function hexToHsl(hex) {
  const { r, g, b } = hexToRgb(hex);
  const R = r / 255, G = g / 255, B = b / 255;
  const max = Math.max(R, G, B), min = Math.min(R, G, B);
  const l = (max + min) / 2;
  if (max === min) return { h: 0, s: 0, l };
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h;
  if (max === R) h = ((G - B) / d + (G < B ? 6 : 0));
  else if (max === G) h = (B - R) / d + 2;
  else h = (R - G) / d + 4;
  return { h: h * 60, s, l };
}

export function hslToHex({ h, s, l }) {
  const S = Math.max(0, Math.min(1, s));
  const L = Math.max(0, Math.min(1, l));
  const H = ((h % 360) + 360) % 360;
  if (S === 0) return rgbToHex({ r: L * 255, g: L * 255, b: L * 255 });
  const q = L < 0.5 ? L * (1 + S) : L + S - L * S;
  const p = 2 * L - q;
  const f = (t) => {
    let T = t;
    if (T < 0) T += 1;
    if (T > 1) T -= 1;
    if (T < 1 / 6) return p + (q - p) * 6 * T;
    if (T < 1 / 2) return q;
    if (T < 2 / 3) return p + (q - p) * (2 / 3 - T) * 6;
    return p;
  };
  const hk = H / 360;
  return rgbToHex({ r: f(hk + 1 / 3) * 255, g: f(hk) * 255, b: f(hk - 1 / 3) * 255 });
}

/** Shift HSL lightness by delta (-1..1), preserving hue and saturation. */
export function shiftLightness(hex, delta) {
  const hsl = hexToHsl(hex);
  return hslToHex({ ...hsl, l: hsl.l + delta });
}

/** WCAG relative luminance (0 black .. 1 white). */
export function luminance(hex) {
  const { r, g, b } = hexToRgb(hex);
  const lin = (c) => {
    const v = c / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** WCAG contrast ratio between two colours (1 .. 21). */
export function contrast(a, b) {
  const la = luminance(a), lb = luminance(b);
  const hi = Math.max(la, lb), lo = Math.min(la, lb);
  return (hi + 0.05) / (lo + 0.05);
}

/**
 * Darken (or lighten, when `direction` is +1) `hex` in small lightness steps
 * until it reaches `ratio` contrast against `bg`. Gives up after `maxSteps`
 * and returns the best it found, so a saturated accent on a mid-grey panel
 * never loops forever.
 */
export function adjustForContrast(hex, bg, ratio = 4.5, direction = -1, maxSteps = 24) {
  let cur = normaliseHex(hex) || '#000000';
  for (let i = 0; i < maxSteps; i++) {
    if (contrast(cur, bg) >= ratio) return cur;
    const next = shiftLightness(cur, direction * 0.03);
    if (next === cur) break;
    cur = next;
  }
  return cur;
}
