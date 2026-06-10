// ── Heavy icon renderers ─────────────────────────────────────────────────────
// This module owns the full lucide-react + simple-icons imports (~5.9MB of
// bundled JS). It must NEVER be statically imported from an eagerly-loaded
// module — only via iconUtils.loadIconRenderers() or from the lazily-loaded
// IconPicker. See iconUtils.jsx for the rationale (idle RAM in the main and
// radial menu windows).

import React, { createElement } from 'react';
import * as AllLucide from 'lucide-react';
import * as SimpleIcons from 'simple-icons';

// ── Build deduplicated Lucide icon map ────────────────────────────────────
// Lucide exports aliases (e.g. Cog ↔ Settings) — same component, different
// name.  We keep only one name per unique component reference.
export const ICON_MAP = {};
export const ALL_ICON_NAMES = [];
const _seen = new Set();
for (const [name, component] of Object.entries(AllLucide)) {
  if (typeof component === 'object' && component.$$typeof && /^[A-Z]/.test(name)) {
    if (_seen.has(component)) continue; // skip alias
    _seen.add(component);
    ICON_MAP[name] = component;
    ALL_ICON_NAMES.push(name);
  }
}
ALL_ICON_NAMES.sort();

// ── Build Simple Icons (brands) list ──────────────────────────────────────
export const BRAND_ICONS = [];
export const BRAND_MAP = {};
for (const [key, icon] of Object.entries(SimpleIcons)) {
  if (icon && icon.slug && icon.path) {
    BRAND_ICONS.push({ name: icon.title, slug: icon.slug, hex: icon.hex, path: icon.path });
    BRAND_MAP[icon.slug] = icon;
  }
}
BRAND_ICONS.sort((a, b) => a.name.localeCompare(b.name));

// ── Rendering helpers ─────────────────────────────────────────────────────

export function renderLucideIcon(name, size = 16, color = 'currentColor', duotone = false) {
  const Icon = ICON_MAP[name];
  if (!Icon) return null;
  const strokeWidth = size >= 20 ? 2.2 : 2;
  if (duotone) {
    return createElement('div', { style: { position: 'relative', width: size, height: size } },
      createElement(Icon, { size, color, strokeWidth: 0, fill: color, opacity: 0.18, style: { position: 'absolute', top: 0, left: 0 } }),
      createElement(Icon, { size, color, strokeWidth, fill: 'none', style: { position: 'relative' } }),
    );
  }
  return createElement(Icon, { size, color, strokeWidth });
}

export function renderSimpleIcon(slug, size = 16, color) {
  const icon = BRAND_MAP[slug];
  if (!icon) return null;
  const fill = color || `#${icon.hex}`;
  return (
    <svg viewBox="0 0 24 24" width={size} height={size} fill={fill} xmlns="http://www.w3.org/2000/svg">
      <path d={icon.path} />
    </svg>
  );
}
