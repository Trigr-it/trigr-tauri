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
// Lucide exports every icon three times: plain (`Scissors`), Lucide-prefixed
// (`LucideScissors`), and Icon-suffixed (`ScissorsIcon`). Iteration order in
// Object.entries is alphabetical, so a naive first-wins dedup keeps whichever
// alias sorts first — `Lock` wins because Lo < LockIcon < LucideLock, but
// `Monitor` LOSES to `LucideMonitor` because Lu < Mo. Prefer the SHORTEST
// name per component, which always resolves to the plain canonical name
// across all three variants. The runtime fallback in renderLucideIcon
// tolerates any of the three names for backwards compat with configs that
// stored a Lucide-prefixed variant before this fix landed.
export const ICON_MAP = {};
export const ALL_ICON_NAMES = [];
const _byComponent = new Map();
for (const [name, component] of Object.entries(AllLucide)) {
  if (typeof component !== 'object' || !component.$$typeof || !/^[A-Z]/.test(name)) continue;
  const existing = _byComponent.get(component);
  if (!existing || name.length < existing.length) {
    _byComponent.set(component, name);
  }
}
for (const [component, name] of _byComponent.entries()) {
  ICON_MAP[name] = component;
  ALL_ICON_NAMES.push(name);
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
  // Backwards-compat fallback: pre-fix configs may have stored the Lucide-
  // prefixed or Icon-suffixed alias (e.g. `lucide:LucideMonitor`) because the
  // old dedup kept whichever variant sorted first. New configs store the plain
  // name. Accept both so no user's picked icon disappears after the dedup fix.
  let Icon = ICON_MAP[name];
  if (!Icon && name.startsWith('Lucide')) Icon = ICON_MAP[name.slice(6)];
  if (!Icon && name.endsWith('Icon')) Icon = ICON_MAP[name.slice(0, -4)];
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
