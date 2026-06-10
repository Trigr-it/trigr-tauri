// ── Light icon helpers ───────────────────────────────────────────────────────
// Pure string/canvas utilities with NO icon-library imports. Everything that
// pulls in lucide-react or simple-icons (~5.9MB of JS) lives in
// iconRenderers.jsx, which is only ever loaded via the dynamic import below.
// Import from THIS module in any eagerly-loaded code (App.jsx, RadialWheel.jsx)
// — a static import of iconRenderers.jsx or IconPicker.jsx from an eager path
// drags the full icon libraries back into the startup bundle of both the main
// window and the radial menu window.

// ── Custom icon downscaling ────────────────────────────────────────────────
// The radial wheel renders icons at ~32px; 64px PNG gives 2x retina headroom.
// Data URLs below the threshold (and all SVGs) are stored as-is.
export const ICON_MAX_DIM = 64;
export const ICON_DOWNSCALE_THRESHOLD = 20 * 1024;

// Downscale an image data URL to fit ICON_MAX_DIM, preserving aspect ratio.
// Calls cb with the scaled PNG data URL, or the original on any failure
// (never blocks an icon pick on a decode error).
export function downscaleIconDataUrl(dataUrl, cb) {
  const img = new Image();
  img.onload = () => {
    try {
      const scale = Math.min(ICON_MAX_DIM / img.width, ICON_MAX_DIM / img.height, 1);
      if (scale >= 1) { cb(dataUrl); return; }
      const canvas = document.createElement('canvas');
      canvas.width = Math.max(1, Math.round(img.width * scale));
      canvas.height = Math.max(1, Math.round(img.height * scale));
      canvas.getContext('2d').drawImage(img, 0, 0, canvas.width, canvas.height);
      cb(canvas.toDataURL('image/png'));
    } catch {
      cb(dataUrl);
    }
  };
  img.onerror = () => cb(dataUrl);
  img.src = dataUrl;
}

// ── Icon type detection ───────────────────────────────────────────────────

export function isLucideIcon(iconStr) {
  return iconStr && iconStr.startsWith('lucide:');
}

export function isSimpleIcon(iconStr) {
  return iconStr && iconStr.startsWith('simple:');
}

export function isCustomIcon(iconStr) {
  return iconStr && iconStr.startsWith('custom:');
}

export function getLucideIconName(iconStr) {
  return iconStr?.replace('lucide:', '') || '';
}

export function getSimpleIconSlug(iconStr) {
  return iconStr?.replace('simple:', '') || '';
}

export function getCustomIconData(iconStr) {
  return iconStr?.replace('custom:', '') || '';
}

// ── Lazy access to the heavy renderers ────────────────────────────────────
// The icon libraries load on demand the first time a wheel actually contains
// a lucide:/simple: icon (or the IconPicker opens). One promise per session;
// the chunk is bundled locally so this stays offline-first.

let _renderers = null;
let _renderersPromise = null;

export function loadIconRenderers() {
  if (!_renderersPromise) {
    _renderersPromise = import('./iconRenderers.jsx').then((mod) => {
      _renderers = mod;
      return mod;
    });
  }
  return _renderersPromise;
}

// Synchronous accessor — null until loadIconRenderers() has resolved.
export function getIconRenderers() {
  return _renderers;
}
