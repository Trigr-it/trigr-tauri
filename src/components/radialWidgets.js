// Radial widgets (Pro, 2026-09-21): small information pills that sit around
// the outside of the radial wheel. Shared by the live overlay
// (RadialMenu.jsx) and the editor preview (RadialEditorView) so both resolve
// the same config into the same pills.
//
// Config shape (top-level `radialWidgets`):
//   [{ id: 'clock', type: 'clock', angle: -90, hour12: false },
//    { id: 'clock-2', type: 'clock', angle: 90, label: 'NYC', timeZone: 'America/New_York', size: 'tall' },
//    { id: 'date', type: 'date', angle: -150, format: 'short' },
//    { id: 'battery', type: 'battery', angle: -30, size: 'compact' }, ...]
// `angle` is where the pill's centre sits on the rim, in SVG degrees
// (0 = right, -90 = top, 90 = bottom), anywhere on the circumference; the
// editor keeps neighbours at least PILL.MIN_GAP_PX apart. One entry per type
// except clocks, which may repeat (other cities). `size` is 'full' (glyph +
// word + value), 'compact' (glyph + value) or 'tall' (two rows: caption
// above value). Rust passes the list through untouched (empty on Free) plus
// a `facts` object (battery, app, volume, lock keys, CPU, RAM); time and
// date are formatted here with the user's locale.

export const WIDGET_GROUPS = [
  { id: 'time',        label: 'Date & time' },
  { id: 'performance', label: 'Performance' },
  { id: 'device',      label: 'Device' },
  { id: 'context',     label: 'Context' },
];

export const WIDGET_TYPES = [
  { type: 'clock',   group: 'time',        label: 'Clock',       multi: true, hint: 'Local time, or another city with its own label.' },
  { type: 'date',    group: 'time',        label: 'Date' },
  { type: 'week',    group: 'time',        label: 'Week number' },
  { type: 'cpu',     group: 'performance', label: 'CPU',         live: true,  hint: 'Processor load, updated every two seconds while the wheel is open.' },
  { type: 'ram',     group: 'performance', label: 'Memory',      live: true },
  { type: 'battery', group: 'device',      label: 'Battery' },
  { type: 'volume',  group: 'device',      label: 'Volume',      live: true },
  { type: 'locks',   group: 'device',      label: 'Lock keys',   live: true,  hint: 'Caps Lock and Num Lock state.' },
  { type: 'app',     group: 'context',     label: 'Active app',  hint: 'The app the wheel opened over.' },
  { type: 'profile', group: 'context',     label: 'Profile',     hint: 'The profile whose wheel is showing.' },
];

// Two independent size axes: width ('full' = word + value, 'compact' =
// value only) and rows (1, or 2 = caption above value). The editor's end-cap
// drag changes width only, the outside-edge drag rows only.
export const WIDGET_WIDTHS = ['full', 'compact'];

// Named positions offered in the card's Position select and used to seat a
// new pill; drag places a pill at any angle in between.
export const ANGLE_PRESETS = [
  { id: 'top',         label: 'Top',          angle: -90 },
  { id: 'topLeft',     label: 'Top left',     angle: -135 },
  { id: 'topRight',    label: 'Top right',    angle: -45 },
  { id: 'left',        label: 'Left',         angle: 180 },
  { id: 'right',       label: 'Right',        angle: 0 },
  { id: 'bottomLeft',  label: 'Bottom left',  angle: 135 },
  { id: 'bottom',      label: 'Bottom',       angle: 90 },
  { id: 'bottomRight', label: 'Bottom right', angle: 45 },
];

export const MAX_WIDGETS = 12;

// Rings: tier 0 hugs the plate, tier 1 sits above it (Rory 2026-09-21,
// "stack widgets over each other"). Two is the limit: a third ring would
// leave the 420-unit viewBox.
export const MAX_TIER = 1;

export const DEFAULT_ANGLE = {
  clock: -90, date: -150, battery: -30,
  cpu: 150, ram: 30, volume: 90, locks: 90, app: 90, profile: 90, week: 150,
};

// First-shape config stored a slot id instead of an angle.
const LEGACY_SLOT_ANGLE = { top: -90, topLeft: -150, topRight: -30, bottomLeft: 150, bottomRight: 30, bottom: 90 };

// Pill geometry, shared by the renderer (RadialWheel) and the editor's
// placement rules. Units are the wheel's 420-unit viewBox.
export const PILL = {
  BASE_R: 118,          // plate rim (OUTER_R 105 + PLATE_PAD 3) + 10 gap = pill inner edge
  H: 22,                // one-row pill height
  H_TALL: 34,           // two-row pill height (caption above value)
  ROW_OFFSET: 7,        // rows sit this far either side of the pill's centre line
  ICON: 12,
  ICON_GAP: 5,
  PAD: 22,              // total padding along the arc (both ends)
  CHAR_W: 6.2,          // average glyph advance at --text-xs (10px, bold)
  CAPTION_CHAR_W: 5.2,  // --text-2xs (9px) caption row
  MAX_HALF_DEG: 40,     // one pill never spans more than 80 degrees
  MIN_GAP_PX: 25,       // clearance between neighbouring pills (Rory 2026-09-21)
  TIER_GAP: 6,          // radial gap between the inner ring's tallest pill and the outer ring
};

/**
 * Inner radius of each ring for this pill set: ring 0 at BASE_R, ring 1 just
 * past the tallest pill in ring 0 (so two one-row rings pack tight).
 */
export function tierBases(pills) {
  const inner = (pills || []).filter(p => !p.tier);
  const maxH = inner.reduce((m, p) => Math.max(m, p.caption ? PILL.H_TALL : PILL.H), inner.length ? 0 : PILL.H);
  return [PILL.BASE_R, PILL.BASE_R + maxH + PILL.TIER_GAP];
}

// Real text measurement (canvas) in the pill fonts, cached per string. The
// char-count estimate under-read wide glyphs and let pills touch; measured
// widths plus a 6 % margin are what the clearance rules trust. Falls back to
// the estimate when no canvas is available (tests) or the font is not ready.
const VALUE_FONT = '700 10px Rajdhani, sans-serif';
const CAPTION_FONT = '600 9px Rajdhani, sans-serif';
const VALUE_TRACKING_PX = 0;   // letter-spacing: normal (see RadialWheel.css)
const CAPTION_TRACKING_PX = 0;
const measureCache = new Map();
let measureCtx = null;
function fontReady(font) {
  try { return typeof document !== 'undefined' && document.fonts?.check?.(font) !== false; } catch { return true; }
}
export function measureTextPx(text, font, trackingPx, fallbackCharW) {
  const str = text || '';
  if (!str) return 0;
  const key = `${font}|${str}`;
  const hit = measureCache.get(key);
  if (hit != null) return hit;
  let width = null;
  try {
    if (typeof document !== 'undefined' && fontReady(font)) {
      if (!measureCtx) measureCtx = document.createElement('canvas').getContext('2d');
      if (measureCtx) {
        measureCtx.font = font;
        width = measureCtx.measureText(str).width;
      }
    }
  } catch { width = null; }
  if (width == null) return str.length * fallbackCharW; // not cached: font may load later
  const out = width * 1.06 + str.length * trackingPx;
  measureCache.set(key, out);
  return out;
}

/** Measured px width of one run of pill text (value row or caption row). */
export function measurePillText(text, caption = false) {
  return caption
    ? measureTextPx(text, CAPTION_FONT, CAPTION_TRACKING_PX, PILL.CAPTION_CHAR_W)
    : measureTextPx(text, VALUE_FONT, VALUE_TRACKING_PX, PILL.CHAR_W);
}

/**
 * Lay a row out along an arc WORD BY WORD: each word is one straight block
 * rotated to the tangent at its own centre, and the words follow the curve.
 * Softer than glyph-by-glyph text on a path (which never read smooth at
 * this radius) and truer to the arc than one straight line. Returns
 * [{ text, angle }] centred on `centreDeg`; `dir` is +1 when reading runs
 * with increasing angle (top half) and -1 when against it (bottom half).
 */
export function layoutWordsOnArc(text, radius, centreDeg, dir, caption = false) {
  const words = String(text || '').split(' ').filter(Boolean);
  if (!words.length) return [];
  // measurePillText carries a 6 % safety margin for the pill BODY; the words
  // themselves pack at true width so the gaps between them stay natural.
  const tight = (px) => px / 1.06;
  const space = tight(measurePillText(' ', caption)) || 3;
  const widths = words.map(w => tight(measurePillText(w, caption)));
  const total = widths.reduce((s, w) => s + w, 0) + space * (words.length - 1);
  const pxToDeg = (px) => (px / radius) * 180 / Math.PI;
  let cursor = -total / 2;
  return words.map((w, i) => {
    const centre = cursor + widths[i] / 2;
    cursor += widths[i] + space;
    return { text: w, angle: centreDeg + dir * pxToDeg(centre) };
  });
}

/** Measured px widths of a pill's rows (value + caption) in the pill fonts. */
export function pillTextWidths(pill) {
  const textW = measureTextPx(pill.label || '', VALUE_FONT, VALUE_TRACKING_PX, PILL.CHAR_W);
  const captionW = pill.caption ? measureTextPx(String(pill.caption).toLocaleUpperCase(), CAPTION_FONT, CAPTION_TRACKING_PX, PILL.CAPTION_CHAR_W) : 0;
  return { textW, captionW };
}

/**
 * Geometry of a resolved pill. `pathHalfDeg` is the stroked path's half
 * span (text area + padding); the round caps add `capDeg` (half the pill
 * height) beyond each end, so `halfDeg` = pathHalfDeg + capDeg is the FULL
 * visual half-extent and is what every clearance rule uses. Leaving the
 * caps out was why two-row pills (17px caps) overlapped their neighbours.
 */
export function pillGeometry(pill, baseR = PILL.BASE_R) {
  const tall = !!pill.caption;
  const h = tall ? PILL.H_TALL : PILL.H;
  const r = baseR + h / 2;
  const measure = measureTextPx(pill.widthLabel || pill.label || '', VALUE_FONT, VALUE_TRACKING_PX, PILL.CHAR_W);
  // Caption is drawn uppercased (RadialWheel.jsx), so measure it that way.
  const captionW = tall ? measureTextPx(String(pill.caption || '').toLocaleUpperCase(), CAPTION_FONT, CAPTION_TRACKING_PX, PILL.CAPTION_CHAR_W) : 0;
  const iconLen = pill.icon ? PILL.ICON + PILL.ICON_GAP : 0;
  // Full / two-row pills: text centred on the pill's centre, icon hanging
  // to its left, body widened by the icon on BOTH sides so it stays
  // symmetric and the text never shifts for the glyph. Compact pills: icon +
  // text centred as one group (no balance space; the text sits half an
  // icon right of centre), because on a short value the balance was most of
  // the pill (Rory 2026-09-21, option 3).
  const rowW = Math.max(measure, captionW);
  const contentLen = pill.compact ? iconLen + rowW : rowW + 2 * iconLen;
  const capDeg = ((h / 2) / r) * 180 / Math.PI;
  const pathHalfDeg = Math.min(((contentLen + PILL.PAD) / 2) / r * 180 / Math.PI, PILL.MAX_HALF_DEG - capDeg);
  return { r, h, halfDeg: pathHalfDeg + capDeg, pathHalfDeg, capDeg, tall, iconLen };
}

/**
 * Push overlapping pills apart until every neighbouring pair clears
 * MIN_GAP_PX, whatever angles they were stored with (a size change or a
 * wider value can grow a pill into its neighbour). Symmetric pushes, up to
 * 60 relaxation rounds, wrap-around included. If the ring is genuinely too
 * full the last state is returned as-is. Order of `pills` is preserved.
 */
export function resolveOverlaps(pills) {
  if (!Array.isArray(pills) || pills.length < 2) return pills;
  const bases = tierBases(pills);
  let out = pills.slice();
  for (let tier = 0; tier <= MAX_TIER; tier++) {
    const idx = [];
    pills.forEach((p, i) => { if ((p.tier || 0) === tier) idx.push(i); });
    if (idx.length < 2) continue;
    const relaxed = relaxRing(idx.map(i => pills[i]), bases[tier]);
    idx.forEach((i, k) => { out[i] = relaxed[k]; });
  }
  return out;
}

function relaxRing(pills, baseR) {
  const items = pills.map((p, i) => ({ i, angle: normAngle(p.angle), geom: pillGeometry(p, baseR) }));
  // Relax to a hair MORE than the minimum: neighbouring pushes cancel each
  // other's slack and the fixed point lands exactly on the requirement,
  // which a strict check (pillFitsAt) then fails by rounding noise.
  const RESOLVE_SLACK_PX = 0.5;
  const need = (a, b) => a.geom.halfDeg + b.geom.halfDeg
    + ((PILL.MIN_GAP_PX + RESOLVE_SLACK_PX) / Math.min(a.geom.r, b.geom.r)) * 180 / Math.PI;
  const total = items.reduce((s, it) => s + it.geom.halfDeg * 2, 0)
    + items.length * (PILL.MIN_GAP_PX / baseR) * 180 / Math.PI;
  if (total >= 360) return pills; // cannot be satisfied; leave as stored
  for (let round = 0; round < 150; round++) {
    items.sort((a, b) => a.angle - b.angle);
    let moved = false;
    for (let k = 0; k < items.length; k++) {
      const a = items[k];
      const b = items[(k + 1) % items.length];
      if (a === b) break;
      let d = b.angle - a.angle;
      if (k === items.length - 1) d += 360;
      const req = need(a, b);
      if (d < req - 0.001) {
        // Push a hair past the requirement so rounding below never lands a
        // pair a fraction short of the gap.
        const push = (req - d) / 2 + 0.02;
        a.angle = normAngle(a.angle - push);
        b.angle = normAngle(b.angle + push);
        moved = true;
      }
    }
    if (!moved) break;
  }
  const out = pills.slice();
  for (const it of items) out[it.i] = { ...pills[it.i], angle: Math.round(it.angle * 100) / 100 };
  return out;
}

/** Wrap to -180..180. */
export function normAngle(a) {
  return ((((a + 180) % 360) + 360) % 360) - 180;
}

export function angleDiff(a, b) {
  return Math.abs(normAngle(a - b));
}

/**
 * True when a pill with `geom` centred at `angle` keeps at least MIN_GAP_PX
 * from every entry of `others` ([{ angle, geom }]).
 */
export function pillFitsAt(angle, geom, others) {
  for (const o of others) {
    const gapDeg = (PILL.MIN_GAP_PX / Math.min(geom.r, o.geom.r)) * 180 / Math.PI;
    if (angleDiff(angle, o.angle) < geom.halfDeg + o.geom.halfDeg + gapDeg) return false;
  }
  return true;
}

/** The preset angle nearest `preferred` where `geom` fits, or null. */
export function freeAngle(geom, others, preferred) {
  const ordered = [...ANGLE_PRESETS].sort((a, b) => angleDiff(a.angle, preferred) - angleDiff(b.angle, preferred));
  if (typeof preferred === 'number' && pillFitsAt(preferred, geom, others)) return normAngle(preferred);
  const hit = ordered.find(p => pillFitsAt(p.angle, geom, others));
  return hit ? hit.angle : null;
}

// IANA zones for the extra-clock field; Chromium exposes the full list.
export const TIME_ZONES = (() => {
  try {
    const list = Intl.supportedValuesOf?.('timeZone');
    if (Array.isArray(list) && list.length) return list;
  } catch { /* fall through */ }
  return ['UTC', 'Europe/London', 'Europe/Paris', 'Europe/Berlin', 'America/New_York', 'America/Chicago',
    'America/Denver', 'America/Los_Angeles', 'Asia/Tokyo', 'Asia/Shanghai', 'Asia/Kolkata', 'Australia/Sydney'];
})();

// Offset regions offered alongside the cities: UTC-12 .. UTC+14.
export const UTC_OFFSETS = Array.from({ length: 27 }, (_, i) => i - 12)
  .filter(h => h !== 0)
  .map(h => `UTC${h > 0 ? '+' : ''}${h}`);

const TYPE_BY_ID = Object.fromEntries(WIDGET_TYPES.map(w => [w.type, w]));

export function widgetMeta(type) {
  return TYPE_BY_ID[type] || null;
}

function isValidTimeZone(tz) {
  try {
    new Intl.DateTimeFormat(undefined, { timeZone: tz });
    return true;
  } catch {
    return false;
  }
}

/**
 * Turn what the user typed into a canonical time zone id, or null.
 * Accepts an IANA name with spaces or any case ("america/new york"), a bare
 * "UTC", and offsets ("UTC+2", "utc -5", "+05:30"). Whole-hour offsets map
 * to the POSIX-signed Etc/GMT zones (Etc/GMT-2 IS UTC+2); half-hour offsets
 * are handed to Intl as "+05:30", which Chromium accepts.
 */
export function normaliseTimeZone(input) {
  const raw = (input || '').trim();
  if (!raw) return null;
  if (/^utc$/i.test(raw) || /^gmt$/i.test(raw)) return 'UTC';
  const m = raw.match(/^(?:utc|gmt)?\s*([+-])\s*(\d{1,2})(?::?(\d{2}))?$/i);
  if (m) {
    const sign = m[1];
    const h = parseInt(m[2], 10);
    const mins = m[3] ? parseInt(m[3], 10) : 0;
    if (h > 14 || mins >= 60) return null;
    if (mins === 0) {
      if (h === 0) return 'UTC';
      const tz = `Etc/GMT${sign === '+' ? '-' : '+'}${h}`;
      return isValidTimeZone(tz) ? tz : null;
    }
    const tz = `${sign}${String(h).padStart(2, '0')}:${String(mins).padStart(2, '0')}`;
    return isValidTimeZone(tz) ? tz : null;
  }
  const wanted = raw.replace(/\s+/g, '_').toLowerCase();
  const hit = TIME_ZONES.find(z => z.toLowerCase() === wanted);
  if (hit) return hit;
  return isValidTimeZone(raw) ? raw : null;
}

/** Display form of a stored zone id: Etc/GMT-2 -> UTC+2, underscores -> spaces. */
export function displayTimeZone(tz) {
  if (!tz) return '';
  const m = tz.match(/^Etc\/GMT([+-])(\d{1,2})$/);
  if (m) return `UTC${m[1] === '-' ? '+' : '-'}${m[2]}`;
  return tz.replace(/_/g, ' ');
}

/** Config list -> valid entries with an id, an angle and a size each. */
export function normaliseWidgets(list) {
  if (!Array.isArray(list)) return [];
  const ids = new Set();
  const typesSeen = new Set();
  const out = [];
  for (const w of list) {
    if (!w || typeof w !== 'object') continue;
    const meta = TYPE_BY_ID[w.type];
    if (!meta) continue;
    if (!meta.multi && typesSeen.has(w.type)) continue;
    typesSeen.add(w.type);
    let id = typeof w.id === 'string' && w.id ? w.id : w.type;
    let n = 2;
    while (ids.has(id)) id = `${w.type}-${n++}`;
    ids.add(id);
    // Earlier shapes: `compact: true`, then `size: full|compact|tall`, and
    // `slot` for the position. Migrate to width + rows + angle.
    let width = WIDGET_WIDTHS.includes(w.width) ? w.width : null;
    let rows = w.rows === 2 ? 2 : (w.rows === 1 ? 1 : null);
    if (width == null) width = (w.size === 'compact' || w.compact) ? 'compact' : 'full';
    if (rows == null) rows = w.size === 'tall' ? 2 : 1;
    const angle = typeof w.angle === 'number' && Number.isFinite(w.angle)
      ? normAngle(w.angle)
      : (LEGACY_SLOT_ANGLE[w.slot] ?? DEFAULT_ANGLE[w.type] ?? -90);
    const showIcon = w.showIcon !== false;
    const tier = w.tier === 1 ? 1 : 0;
    const { compact, slot, size, ...rest } = w;
    out.push({ ...rest, id, angle, width, rows, showIcon, tier });
    if (out.length >= MAX_WIDGETS) break;
  }
  return out;
}

/** New unique id for a widget of `type`. */
export function newWidgetId(type, widgets) {
  const ids = new Set((widgets || []).map(w => w.id));
  if (!ids.has(type)) return type;
  let n = 2;
  while (ids.has(`${type}-${n}`)) n++;
  return `${type}-${n}`;
}

/** True when any widget needs the facts re-polled while the wheel is open. */
export function hasLiveWidget(widgets) {
  return (widgets || []).some(w => TYPE_BY_ID[w.type]?.live);
}

export function formatClock(date, hour12, timeZone) {
  const opts = { hour: '2-digit', minute: '2-digit', hour12: !!hour12 };
  try {
    return date.toLocaleTimeString(undefined, timeZone ? { ...opts, timeZone } : opts);
  } catch {
    try {
      return date.toLocaleTimeString(undefined, opts);
    } catch {
      const h = String(date.getHours()).padStart(2, '0');
      const m = String(date.getMinutes()).padStart(2, '0');
      return `${h}:${m}`;
    }
  }
}

export function formatDate(date, format, compact = false) {
  const opts = compact
    ? { day: 'numeric', month: 'short' }
    : format === 'long'
      ? { weekday: 'long', day: 'numeric', month: 'long' }
      : { weekday: 'short', day: 'numeric', month: 'short' };
  try {
    return date.toLocaleDateString(undefined, opts);
  } catch {
    return date.toDateString();
  }
}

/** ISO 8601 week number (weeks start Monday, week 1 holds the year's first Thursday). */
export function isoWeek(date) {
  const d = new Date(Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()));
  const day = d.getUTCDay() || 7;
  d.setUTCDate(d.getUTCDate() + 4 - day);
  const yearStart = new Date(Date.UTC(d.getUTCFullYear(), 0, 1));
  return Math.ceil(((d - yearStart) / 86400000 + 1) / 7);
}

function titleCase(s) {
  return s ? s.charAt(0).toUpperCase() + s.slice(1) : s;
}

// The widest a clock reads in either format ("23:59" / "11:59 PM"), and a
// wide date ("Sat 30 Sep" / "Saturday 30 September") to size date pills.
const WIDEST_TIME = new Date(2000, 0, 1, 23, 59);
const WIDEST_DATE = new Date(2000, 8, 30);

export function formatWeekday(date) {
  try {
    return date.toLocaleDateString(undefined, { weekday: 'long' });
  } catch {
    return ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'][date.getDay()];
  }
}

/**
 * Resolve config + live facts into the pills RadialWheel draws:
 * [{ id, angle, icon, label, widthLabel, caption? }]. `icon` is a key
 * RadialWheel maps to a lucide glyph. `widthLabel` is the widest value the
 * pill can show, which is what it is sized from, so it never breathes as the
 * figure changes. Sizes: 'full' = word + value in one row, 'compact' = value
 * only (the glyph says what it is), 'tall' = `caption` above `label`.
 * Facts that are not available (no battery, no volume endpoint, no CPU
 * sample yet) drop the pill in the overlay; the editor passes `showMissing`
 * so the user still sees where it would sit. `profile` is the name of the
 * profile whose wheel is showing.
 */
export function resolvePills(widgets, facts, now, { showMissing = false, profile = '' } = {}) {
  const date = now instanceof Date ? now : new Date(now || Date.now());
  const f = facts || {};
  const pills = [];

  for (const w of normaliseWidgets(widgets)) {
    const tall = w.rows === 2;
    const compact = w.width === 'compact';
    const size = tall ? 'tall' : compact ? 'compact' : 'full';
    // word = the caption / prefix for this widget; value = the live figure;
    // widthValue = the WIDEST figure this pill can show. Two rows always
    // carry the word as the caption; compact only shortens the value row.
    const compose = (word, value) => (tall || compact || !word ? value : `${word} ${value}`);
    const emit = (icon, word, value, widthValue = value) => {
      pills.push({
        id: w.id, angle: w.angle, icon: w.showIcon ? icon : null,
        compact: compact && !tall,
        tier: w.tier || 0,
        label: compose(word, value),
        widthLabel: compose(word, widthValue),
        ...(tall ? { caption: word } : {}),
      });
    };
    const missing = (icon, word, widthValue = '–') => { if (showMissing) emit(icon, word, '–', widthValue); };
    const PCT_MAX = '100%';

    switch (w.type) {
      case 'clock': {
        const time = formatClock(date, w.hour12, w.timeZone);
        const label = (w.label || '').trim();
        // A clock with no label has no word in the one-row sizes; tall
        // falls back to the type name so the caption row is never empty.
        emit('clock', tall ? (label || 'Clock') : label, time, formatClock(WIDEST_TIME, w.hour12));
        break;
      }
      case 'date':
        // Never the word "Date": full = "Mon 21 Sep", compact = "21 Sep",
        // two rows = weekday above "21 Sep" (Rory 2026-09-21).
        if (tall) emit('calendar', formatWeekday(date), formatDate(date, w.format, true), formatDate(WIDEST_DATE, w.format, true));
        else emit('calendar', '', formatDate(date, w.format, compact), formatDate(WIDEST_DATE, w.format, compact));
        break;
      case 'week':
        emit('week', 'Week', String(isoWeek(date)), '52');
        break;
      case 'battery': {
        const b = f.battery;
        if (!b?.present || typeof b.percent !== 'number') { missing('battery', 'Battery', PCT_MAX); break; }
        const pct = Math.max(0, Math.min(100, Math.round(b.percent)));
        const icon = b.charging ? 'battery-charging' : pct <= 20 ? 'battery-low' : 'battery';
        // Full size shows the bare figure: "84%" already reads as battery.
        emit(icon, size === 'full' ? '' : 'Battery', `${pct}%`, PCT_MAX);
        break;
      }
      case 'cpu':
        if (typeof f.cpu !== 'number') { missing('cpu', 'CPU', PCT_MAX); break; }
        emit('cpu', 'CPU', `${Math.round(f.cpu)}%`, PCT_MAX);
        break;
      case 'ram': {
        if (typeof f.ramPct !== 'number') { missing('ram', 'Memory', PCT_MAX); break; }
        const gb = w.mode === 'gb' && typeof f.ramUsedGb === 'number' && typeof f.ramTotalGb === 'number';
        const value = gb
          ? (compact ? `${f.ramUsedGb} GB` : `${f.ramUsedGb} / ${f.ramTotalGb} GB`)
          : `${Math.round(f.ramPct)}%`;
        const widest = gb
          ? (compact ? `${f.ramTotalGb} GB` : `${f.ramTotalGb} / ${f.ramTotalGb} GB`)
          : PCT_MAX;
        emit('ram', gb ? 'Memory' : 'RAM', value, widest);
        break;
      }
      case 'volume':
        if (typeof f.volume !== 'number') { missing('volume', 'Volume', PCT_MAX); break; }
        emit(f.volume === 0 ? 'volume-x' : 'volume', size === 'full' ? '' : 'Volume', `${Math.round(f.volume)}%`, PCT_MAX);
        break;
      case 'locks': {
        const caps = !!f.capsLock;
        const num = !!f.numLock;
        const short = caps && num ? 'Caps+Num' : caps ? 'Caps' : num ? 'Num' : '–';
        const long = caps && num ? 'Caps + Num' : caps ? 'Caps on' : num ? 'Num on' : 'Locks off';
        // Sized to the CURRENT state, not the widest ("Caps+Num"): a state
        // label changes only on a deliberate key press, and the widest form
        // left a "Num" pill mostly empty (Rory 2026-09-21). Numeric widgets
        // keep max-width sizing so they never breathe as figures tick.
        if (tall) emit('keyboard', 'Locks', short);
        else emit('keyboard', '', compact ? short : long);
        break;
      }
      case 'app':
        if (!f.app) { missing('app', 'App'); break; }
        emit('app', 'App', titleCase(f.app));
        break;
      case 'profile':
        if (!profile) { missing('profile', 'Profile'); break; }
        emit('profile', 'Profile', profile);
        break;
      default:
        break;
    }
  }
  return resolveOverlaps(pills);
}
