// Full keyboard layout definition
// Each row is an array of key objects
// width is in units (1 = standard key width ~42px)
//
// Two physical form factors share one set of key IDs (the engine identifies
// symbol keys by scancode, so an assignment made on either board fires on
// both): ANSI (US) and ISO (UK / mainland Europe). Pick with getKeyboardRows.

// Half-height row of F13-F24 above the function row, each key sitting over
// its F1-F12 twin (F13 over F1, F17 over F5, F21 over F9). No physical
// keyboard has these keys, which makes them conflict-free dedicated triggers:
// a Stream Deck button, macropad key or remapped key is set to send one, then
// mapped here. Windows reports them as ordinary VKs 0x7C-0x87 so the engine
// treats them like any other key (bare-mappable, double press, hold).
// The leading spacer equals Esc (1u) + gap + the 0.5u spacer below it, minus
// the one gap the spacer itself gets, so F13 lands exactly over F1.
const EXTRA_F_ROW = [
  { id: 'SPACER_F13', spacer: true, width: (1 * 42 + 4 + 0.5 * 42) / 42 },
  ...['F13', 'F14', 'F15', 'F16'].map(id => ({ id, label: id, width: 1, half: true })),
  { id: 'SPACER_F17', spacer: true, width: 0.35 },
  ...['F17', 'F18', 'F19', 'F20'].map(id => ({ id, label: id, width: 1, half: true })),
  { id: 'SPACER_F21', spacer: true, width: 0.35 },
  ...['F21', 'F22', 'F23', 'F24'].map(id => ({ id, label: id, width: 1, half: true })),
];

const ANSI_ROWS = [
  // Row -1: extra function keys (half height)
  EXTRA_F_ROW,
  // Row 0: Function keys
  [
    { id: 'Escape', label: 'Esc', width: 1 },
    { id: 'SPACER_F1', spacer: true, width: 0.5 },
    { id: 'F1', label: 'F1', width: 1 },
    { id: 'F2', label: 'F2', width: 1 },
    { id: 'F3', label: 'F3', width: 1 },
    { id: 'F4', label: 'F4', width: 1 },
    { id: 'SPACER_F5', spacer: true, width: 0.35 },
    { id: 'F5', label: 'F5', width: 1 },
    { id: 'F6', label: 'F6', width: 1 },
    { id: 'F7', label: 'F7', width: 1 },
    { id: 'F8', label: 'F8', width: 1 },
    { id: 'SPACER_F9', spacer: true, width: 0.35 },
    { id: 'F9', label: 'F9', width: 1 },
    { id: 'F10', label: 'F10', width: 1 },
    { id: 'F11', label: 'F11', width: 1 },
    { id: 'F12', label: 'F12', width: 1 },
  ],

  // Row 1: Numbers
  [
    { id: 'Backquote', label: '`', sublabel: '~', width: 1 },
    { id: 'Digit1', label: '1', sublabel: '!', width: 1 },
    { id: 'Digit2', label: '2', sublabel: '@', width: 1 },
    { id: 'Digit3', label: '3', sublabel: '#', width: 1 },
    { id: 'Digit4', label: '4', sublabel: '$', width: 1 },
    { id: 'Digit5', label: '5', sublabel: '%', width: 1 },
    { id: 'Digit6', label: '6', sublabel: '^', width: 1 },
    { id: 'Digit7', label: '7', sublabel: '&', width: 1 },
    { id: 'Digit8', label: '8', sublabel: '*', width: 1 },
    { id: 'Digit9', label: '9', sublabel: '(', width: 1 },
    { id: 'Digit0', label: '0', sublabel: ')', width: 1 },
    { id: 'Minus', label: '-', sublabel: '_', width: 1 },
    { id: 'Equal', label: '=', sublabel: '+', width: 1 },
    { id: 'Backspace', label: '⌫', width: 2 },
  ],

  // Row 2: QWERTY
  [
    { id: 'Tab', label: 'Tab', width: 1.5 },
    { id: 'KeyQ', label: 'Q', width: 1 },
    { id: 'KeyW', label: 'W', width: 1 },
    { id: 'KeyE', label: 'E', width: 1 },
    { id: 'KeyR', label: 'R', width: 1 },
    { id: 'KeyT', label: 'T', width: 1 },
    { id: 'KeyY', label: 'Y', width: 1 },
    { id: 'KeyU', label: 'U', width: 1 },
    { id: 'KeyI', label: 'I', width: 1 },
    { id: 'KeyO', label: 'O', width: 1 },
    { id: 'KeyP', label: 'P', width: 1 },
    { id: 'BracketLeft', label: '[', sublabel: '{', width: 1 },
    { id: 'BracketRight', label: ']', sublabel: '}', width: 1 },
    { id: 'Backslash', label: '\\', sublabel: '|', width: 1.5 },
  ],

  // Row 3: ASDF
  [
    { id: 'CapsLock', label: 'Caps Lock', width: 1.75 },
    { id: 'KeyA', label: 'A', width: 1 },
    { id: 'KeyS', label: 'S', width: 1 },
    { id: 'KeyD', label: 'D', width: 1 },
    { id: 'KeyF', label: 'F', width: 1 },
    { id: 'KeyG', label: 'G', width: 1 },
    { id: 'KeyH', label: 'H', width: 1 },
    { id: 'KeyJ', label: 'J', width: 1 },
    { id: 'KeyK', label: 'K', width: 1 },
    { id: 'KeyL', label: 'L', width: 1 },
    { id: 'Semicolon', label: ';', sublabel: ':', width: 1 },
    { id: 'Quote', label: "'", sublabel: '"', width: 1 },
    { id: 'Enter', label: 'Enter', width: 2.25 },
  ],

  // Row 4: ZXCV
  [
    { id: 'ShiftLeft', label: 'Shift', width: 2.25 },
    { id: 'KeyZ', label: 'Z', width: 1 },
    { id: 'KeyX', label: 'X', width: 1 },
    { id: 'KeyC', label: 'C', width: 1 },
    { id: 'KeyV', label: 'V', width: 1 },
    { id: 'KeyB', label: 'B', width: 1 },
    { id: 'KeyN', label: 'N', width: 1 },
    { id: 'KeyM', label: 'M', width: 1 },
    { id: 'Comma', label: ',', sublabel: '<', width: 1 },
    { id: 'Period', label: '.', sublabel: '>', width: 1 },
    { id: 'Slash', label: '/', sublabel: '?', width: 1 },
    { id: 'ShiftRight', label: 'Shift', width: 2.75 },
  ],

  // Row 5: Bottom row
  [
    { id: 'ControlLeft', label: 'Ctrl', width: 1.25 },
    { id: 'MetaLeft', label: '⊞', width: 1.25 },
    { id: 'AltLeft', label: 'Alt', width: 1.25 },
    { id: 'Space', label: '', width: 6.25 },
    { id: 'AltRight', label: 'Alt', width: 1.25 },
    { id: 'MetaRight', label: '⊞', width: 1.25 },
    { id: 'ContextMenu', label: '☰', width: 1.25 },
    { id: 'ControlRight', label: 'Ctrl', width: 1.25 },
  ],
];

// ISO form factor. Same 15u width and the same key IDs as ANSI; only the
// shape differs: a tall reverse-L Enter that starts in the QWERTY row (the
// ASDF row reserves its lower 1.25u with a spacer), the scancode-0x2B key
// ("Backslash", `# ~` on UK boards) tucked beside it in the ASDF row, and the
// ISO-only IntlBackslash key (scancode 0x56) between a 1.25u left Shift and Z.
// Legends on the two ISO-specific keys are UK for now; layout-derived legends
// for every key are the follow-up phase.
const ISO_ROWS = ANSI_ROWS.map((row, i) => {
  // Row indices are offset by one for the half-height F13-F24 row at [0].
  if (i === 3) {
    return [...row.slice(0, -1), { id: 'Enter', label: 'Enter', width: 1.5, tall: true, lowerWidth: 1.25 }];
  }
  if (i === 4) {
    return [
      ...row.slice(0, -1),
      { id: 'Backslash', label: '#', sublabel: '~', width: 1 },
      { id: 'SPACER_ENTER_LOWER', spacer: true, width: 1.25 },
    ];
  }
  if (i === 5) {
    return [{ ...row[0], width: 1.25 }, { id: 'IntlBackslash', label: '\\', sublabel: '|', width: 1 }, ...row.slice(1)];
  }
  return row;
});

// Legacy export: the ANSI board. Prefer getKeyboardRows(layout).
export const KEYBOARD_ROWS = ANSI_ROWS;
export function getKeyboardRows(layout) {
  return layout === 'iso' ? ISO_ROWS : ANSI_ROWS;
}

// Live legends from the Windows layout, keyed by key id → base character.
// App fills this from get_keyboard_legends; friendlyKeyName consults it for
// the OEM symbol ids only (letters and digits already read correctly, and a
// letter id always types that letter whatever the layout). So a UK user sees
// "#" for Backslash and "@" for Quote in the sidebar, editor and toasts.
const OEM_LEGEND_IDS = new Set([
  'Backquote', 'Minus', 'Equal', 'BracketLeft', 'BracketRight', 'Semicolon',
  'Quote', 'Backslash', 'Comma', 'Period', 'Slash', 'IntlBackslash',
]);
let liveOemLegends = {};
export function setLiveKeyLegends(byKeyId) {
  liveOemLegends = {};
  for (const [id, ch] of Object.entries(byKeyId || {})) {
    if (OEM_LEGEND_IDS.has(id) && ch) liveOemLegends[id] = ch;
  }
}

// Turn a { base, shift } legend into the label / sublabel the canvas draws:
// letters show once, upper-case; everything else shows base with the shifted
// character above. Returns null when the layout gave us nothing usable so the
// caller keeps the hard-coded US legend.
export function legendToLabels(legend) {
  if (!legend) return null;
  const base = legend.base || '';
  const shift = legend.shift || '';
  if (!base) return null;
  if (base.length === 1 && shift.length === 1 && base !== shift && base.toUpperCase() === shift) {
    return { label: shift, sublabel: undefined };
  }
  return { label: base, sublabel: shift && shift !== base ? shift : undefined };
}

// Friendly display names for key IDs that aren't self-explanatory
export function friendlyKeyName(keyId) {
  if (!keyId) return '';
  if (liveOemLegends[keyId]) return liveOemLegends[keyId];
  switch (keyId) {
    case 'Backquote':    return '`';
    case 'Quote':        return "'";
    case 'Semicolon':    return ';';
    case 'BracketLeft':  return '[';
    case 'BracketRight': return ']';
    case 'Backslash':    return '\\';
    case 'IntlBackslash': return '\\'; // ISO key beside left Shift (UK legend)
    case 'Comma':        return ',';
    case 'Period':       return '.';
    case 'Slash':        return '/';
    case 'Minus':        return '-';
    case 'Equal':        return '=';
    case 'Backspace':    return '⌫';
    case 'CapsLock':     return 'Caps';
    case 'ContextMenu':  return 'Menu';
    case 'MOUSE_LEFT':   return 'Left Click';
    case 'MOUSE_RIGHT':  return 'Right Click';
    case 'MOUSE_MIDDLE': return 'Middle Click';
    case 'MOUSE_SIDE1':  return 'Side Button 1';
    case 'MOUSE_SIDE2':  return 'Side Button 2';
    case 'MOUSE_SCROLL_UP':   return 'Scroll Up';
    case 'MOUSE_SCROLL_DOWN': return 'Scroll Down';
    default: break;
  }
  if (keyId.startsWith('Key') && keyId.length === 4) return keyId.slice(3);
  if (keyId.startsWith('Digit') && keyId.length === 6) return keyId.slice(5);
  if (keyId.startsWith('Arrow')) return keyId.slice(5);
  if (keyId.startsWith('Numpad')) return keyId.replace('Numpad', 'Num');
  return keyId;
}

// Keys that can't be reassigned (modifiers used in combos)
export const SYSTEM_KEYS = new Set([
  'ShiftLeft', 'ShiftRight', 'ControlLeft', 'ControlRight',
  'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight', 'CapsLock'
]);

// Keys allowed for bare mapping in static (non-app-linked) profiles.
// Excludes character/number/punctuation keys to prevent users from
// accidentally making letters untypable system-wide.
export const STATIC_BARE_ALLOWED = new Set([
  // Function keys
  'F1', 'F2', 'F3', 'F4', 'F5', 'F6', 'F7', 'F8', 'F9', 'F10', 'F11', 'F12',
  // Extra function keys (Stream Deck / macropad / remapped-key triggers)
  'F13', 'F14', 'F15', 'F16', 'F17', 'F18', 'F19', 'F20', 'F21', 'F22', 'F23', 'F24',
  // Nav keys
  'Insert', 'Home', 'End', 'Delete', 'PageUp', 'PageDown',
  'PrintScreen', 'ScrollLock', 'Pause',
  // Numpad
  'NumLock', 'NumpadDivide', 'NumpadMultiply', 'NumpadSubtract', 'NumpadAdd',
  'Numpad0', 'Numpad1', 'Numpad2', 'Numpad3', 'Numpad4',
  'Numpad5', 'Numpad6', 'Numpad7', 'Numpad8', 'Numpad9',
  'NumpadEnter', 'NumpadDecimal',
  // Arrow keys
  'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight',
  // Misc non-character
  'Escape', 'ContextMenu',
]);

export const KEY_UNIT   = 42; // px per keyboard unit
export const KEY_GAP    = 4;  // px gap between keys (also used as row gap)
export const KEY_HEIGHT = 42; // px height of each key
export const KEY_HALF_HEIGHT = 26; // px height of the F13-F24 row keys
// Height of one row: half-height rows are marked on their keys.
export function rowHeight(row) {
  return row.some(k => !k.spacer && k.half) ? KEY_HALF_HEIGHT : KEY_HEIGHT;
}

// ── Natural (unscaled) outer dimensions of .keyboard-outer ───────────────────
// Used by KeyboardCanvas to derive the CSS scale factor.
function _rowPixelWidth(row) {
  let w = 0;
  for (let i = 0; i < row.length; i++) {
    if (i > 0) w += KEY_GAP; // flex row-gap between items
    const k = row[i];
    // Spacers use no gap correction; real keys span (width-1) extra gaps
    w += k.spacer
      ? k.width * KEY_UNIT
      : k.width * KEY_UNIT + (k.width - 1) * KEY_GAP;
  }
  return w;
}
// .keyboard-outer padding: 16px top, 18px right, 18px bottom, 18px left + 1px border each side
export function keyboardNaturalSize(layout) {
  const rows = getKeyboardRows(layout);
  const maxBodyWidth = Math.max(...rows.map(_rowPixelWidth));
  return {
    width: maxBodyWidth + 18 * 2 + 2,
    height: rows.reduce((h, r) => h + rowHeight(r), 0) + (rows.length - 1) * KEY_GAP + 16 + 18 + 2, // padding + borders
  };
}
const _ansiSize = keyboardNaturalSize('ansi');
export const KEYBOARD_NATURAL_WIDTH  = _ansiSize.width;
export const KEYBOARD_NATURAL_HEIGHT = _ansiSize.height;
