// Single entry point for opening the Featurebase Feedback Widget. Used by
// both SettingsPanel.jsx and TitleBar.jsx so the board slug and the offline
// mailto fallback live in one place.
//
// The widget is initialised in App.jsx (`Featurebase('initialize_feedback_widget', …)`).
// If the SDK script failed to load (offline at boot, CDN blocked, CSP misconfig)
// `window.Featurebase` will be undefined and the postMessage call would be a
// no-op — we fall back to opening a mailto in the user's default mail client
// via Tauri's openExternal so the trigger is never a dead click.

const FEEDBACK_BOARD = 'feature-requests';
const FALLBACK_EMAIL = 'admin@usetrigr.com';

export function openFeedback() {
  if (typeof window.Featurebase === 'function') {
    window.postMessage({
      target: 'FeaturebaseWidget',
      data: { action: 'openFeedbackWidget', setBoard: FEEDBACK_BOARD },
    });
    return;
  }
  const subject = encodeURIComponent('Trigr Feedback');
  const body = encodeURIComponent(
    "What happened (or what would you like to see)?\n\n\n\nSent from Trigr"
  );
  window.electronAPI?.openExternal(`mailto:${FALLBACK_EMAIL}?subject=${subject}&body=${body}`);
}
