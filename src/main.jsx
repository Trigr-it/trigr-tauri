import './tauriAPI'; // Initialize window.electronAPI bridge before anything else
import React from 'react';
import ReactDOM from 'react-dom/client';

// Keyboard-modality flag: focus rings stay hidden until the user presses Tab.
// First Tab adds .using-keyboard on <html>; first mousedown removes it. CSS
// rules gate visible :focus styles behind that class so programmatic .focus()
// calls (e.g. auto-focusing the first fill-in input on open) don't paint a
// ring before the user has any reason to see one.
(() => {
  let usingKeyboard = false;
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Tab' && !usingKeyboard) {
      usingKeyboard = true;
      document.documentElement.classList.add('using-keyboard');
    }
  }, true);
  document.addEventListener('mousedown', () => {
    if (usingKeyboard) {
      usingKeyboard = false;
      document.documentElement.classList.remove('using-keyboard');
    }
  }, true);
})();

const root = ReactDOM.createRoot(document.getElementById('root'));

const params = new URLSearchParams(window.location.search);
if (params.get('overlay') === '1') {
  // Lazy import — avoids loading App.jsx and its global.css/app.css
  // which set html/body background to --bg-base (dark), breaking transparency
  const SearchOverlay = React.lazy(() => import('./components/SearchOverlay'));
  root.render(
    <React.Suspense fallback={null}>
      <SearchOverlay />
    </React.Suspense>
  );
} else if (params.get('fillin') === '1') {
  const FillInWindow = React.lazy(() => import('./components/FillInWindow'));
  root.render(
    <React.Suspense fallback={null}>
      <FillInWindow />
    </React.Suspense>
  );
} else if (params.get('radialmenu') === '1') {
  const RadialMenu = React.lazy(() => import('./components/RadialMenu'));
  root.render(
    <React.Suspense fallback={null}>
      <RadialMenu />
    </React.Suspense>
  );
} else if (params.get('clipboardoverlay') === '1') {
  const ClipboardOverlay = React.lazy(() => import('./components/ClipboardOverlay'));
  root.render(
    <React.Suspense fallback={null}>
      <ClipboardOverlay />
    </React.Suspense>
  );
} else {
  // Only import App (and its global.css/app.css) for the main window
  const App = React.lazy(() => import('./App'));
  root.render(
    <React.Suspense fallback={null}>
      <App />
    </React.Suspense>
  );
}
