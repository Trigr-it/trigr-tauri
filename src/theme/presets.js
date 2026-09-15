// Theme presets (Free tier). Each preset is a light + dark PAIR of the eight
// knobs the engine derives every mood token from; the user's light/dark/auto
// mode picks the half, so a preset never disables the top-right toggle.
//
// Knob keys (see engine.js KNOB_KEYS):
//   accent      brand colour: buttons, active states, section titles
//   background  window background (--bg-base)
//   panel       cards, editor panels, popovers (--bg-elevated)
//   field       inputs, textareas, selects (--bg-input)
//   text        primary text (--text-primary)
//   textMuted   secondary labels and hints (--text-muted)
//   border      dividers and outlines (--border)
//   keycap      keyboard keys and neutral nav chips (--key-bg)
//
// 'keyfire' is the shipped look. The engine emits NO overrides for it (the
// CSS defaults in global.css and the overlay mirrors ARE the Keyfire theme),
// so these knobs are only used for its preview card and as the "start from"
// seed of a custom theme. They match global.css exactly.

export const KEYFIRE_ID = 'keyfire';
export const CUSTOM_ID = 'custom';

export const PRESETS = [
  {
    id: KEYFIRE_ID,
    name: 'Keyfire',
    tagline: 'Gold on ink. The original.',
    dark:  { accent: '#e8a020', background: '#0d0d11', panel: '#1a1a24', field: '#0d0d11', text: '#f0ede8', textMuted: '#9794b3', border: '#44445e', keycap: '#1e1e2c' },
    light: { accent: '#e8a020', background: '#f0f0f5', panel: '#f5f5fa', field: '#ffffff', text: '#1a1a2e', textMuted: '#6b6b88', border: '#dcdce8', keycap: '#ffffff' },
  },
  {
    id: 'graphite',
    name: 'Graphite',
    tagline: 'Neutral greys, steel-blue accent.',
    dark:  { accent: '#7fa7c9', background: '#141416', panel: '#1e1e21', field: '#141416', text: '#ececec', textMuted: '#9a9aa2', border: '#3a3a40', keycap: '#232327' },
    light: { accent: '#3f6f96', background: '#eeeff1', panel: '#f7f7f8', field: '#ffffff', text: '#1f2023', textMuted: '#6b6e75', border: '#d6d8dc', keycap: '#ffffff' },
  },
  {
    id: 'ocean',
    name: 'Ocean',
    tagline: 'Deep navy with a clear blue accent.',
    dark:  { accent: '#4a9eff', background: '#0b1220', panel: '#141d2f', field: '#0b1220', text: '#e6edf7', textMuted: '#8a9bb8', border: '#2a3a55', keycap: '#182338' },
    light: { accent: '#1f6fd6', background: '#edf2f9', panel: '#f6f9fd', field: '#ffffff', text: '#14213a', textMuted: '#5f6f8a', border: '#d3dced', keycap: '#ffffff' },
  },
  {
    id: 'forest',
    name: 'Forest',
    tagline: 'Dark moss with a leaf-green accent.',
    dark:  { accent: '#5fcf8a', background: '#0d130f', panel: '#161f19', field: '#0d130f', text: '#e8efe9', textMuted: '#8fa697', border: '#2c3d32', keycap: '#1a251e' },
    light: { accent: '#1f8a4c', background: '#eef4ef', panel: '#f6faf7', field: '#ffffff', text: '#16281c', textMuted: '#5f7566', border: '#d2e0d6', keycap: '#ffffff' },
  },
  {
    id: 'plum',
    name: 'Plum',
    tagline: 'Aubergine surfaces, violet accent.',
    dark:  { accent: '#b57bff', background: '#120d18', panel: '#1c1524', field: '#120d18', text: '#efe9f5', textMuted: '#a08fb5', border: '#3a2d4a', keycap: '#221a2c' },
    light: { accent: '#7b3fd6', background: '#f2eef7', panel: '#f9f6fc', field: '#ffffff', text: '#24143a', textMuted: '#6f5f85', border: '#dfd5ea', keycap: '#ffffff' },
  },
  {
    id: 'rose',
    name: 'Rose',
    tagline: 'Warm charcoal with a rose accent.',
    dark:  { accent: '#ff7eb6', background: '#161013', panel: '#221a1f', field: '#161013', text: '#f3ebef', textMuted: '#ad96a3', border: '#453540', keycap: '#281f25' },
    light: { accent: '#c9337a', background: '#f6eef2', panel: '#fcf6f9', field: '#ffffff', text: '#2e1a26', textMuted: '#805f72', border: '#e8d6e0', keycap: '#ffffff' },
  },
  {
    id: 'sand',
    name: 'Sand',
    tagline: 'Warm paper and copper.',
    dark:  { accent: '#d9915a', background: '#17130f', panel: '#221d17', field: '#17130f', text: '#f2eae0', textMuted: '#ab9c8c', border: '#443a30', keycap: '#28221b' },
    light: { accent: '#b3622e', background: '#f3ede4', panel: '#faf6f0', field: '#fffdf9', text: '#2c2218', textMuted: '#7a6a58', border: '#e4d9ca', keycap: '#fffdf9' },
  },
  {
    id: 'arctic',
    name: 'Arctic',
    tagline: 'Cool slate blues with a frost accent.',
    dark:  { accent: '#88c0d0', background: '#232831', panel: '#2e3440', field: '#232831', text: '#eceff4', textMuted: '#9aa5bb', border: '#434c5e', keycap: '#333b4c' },
    light: { accent: '#5e81ac', background: '#e5e9f0', panel: '#eceff4', field: '#ffffff', text: '#2e3440', textMuted: '#5e6a80', border: '#cbd3e1', keycap: '#ffffff' },
  },
  {
    id: 'solar',
    name: 'Solar',
    tagline: 'Teal-black and cream, blue accent.',
    dark:  { accent: '#45a0e6', background: '#002b36', panel: '#073642', field: '#002b36', text: '#eee8d5', textMuted: '#93a1a1', border: '#1c4a56', keycap: '#0b3c48' },
    light: { accent: '#268bd2', background: '#eee8d5', panel: '#fdf6e3', field: '#fffdf5', text: '#073642', textMuted: '#5c717a', border: '#d9d2bd', keycap: '#fffdf5' },
  },
  {
    id: 'contrast',
    name: 'High Contrast',
    tagline: 'Near-black and white, strong borders.',
    dark:  { accent: '#ffcc33', background: '#000000', panel: '#111111', field: '#000000', text: '#ffffff', textMuted: '#c8c8c8', border: '#6a6a6a', keycap: '#161616' },
    light: { accent: '#0044cc', background: '#ffffff', panel: '#f4f4f4', field: '#ffffff', text: '#000000', textMuted: '#3c3c3c', border: '#8a8a8a', keycap: '#ffffff' },
  },
];

export function getPreset(id) {
  return PRESETS.find(p => p.id === id) || null;
}

export function isPresetId(id) {
  return typeof id === 'string' && PRESETS.some(p => p.id === id);
}
