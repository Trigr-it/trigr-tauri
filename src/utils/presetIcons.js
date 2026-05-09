// Shared lookup: given a URL, return the bundled preset-icon filename for its
// domain (e.g. github.com -> 'github.png'). Returns null if no match.
//
// The domain → icon map is defined in SearchTemplatesPanel.jsx (built from the
// PRESETS array at module load) and re-exported as PRESET_ICONS_BY_DOMAIN.
// This util re-exposes the lookup so other components (ClipboardPanel etc.)
// can map URLs to brand icons without depending on SearchTemplatesPanel
// directly.

import { PRESET_ICONS_BY_DOMAIN } from '../components/SearchTemplatesPanel';

export function findPresetIconForUrl(url) {
  if (!url) return null;
  try {
    const host = new URL(url).hostname.replace(/^www\./, '');
    if (PRESET_ICONS_BY_DOMAIN[host]) return PRESET_ICONS_BY_DOMAIN[host];
    // Suffix match — e.g. gist.github.com → github.com icon.
    for (const [domain, icon] of Object.entries(PRESET_ICONS_BY_DOMAIN)) {
      if (host.endsWith('.' + domain)) return icon;
    }
    return null;
  } catch {
    return null;
  }
}
