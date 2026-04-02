import './tauriAPI'; // Initialize window.electronAPI bridge before anything else
import React from 'react';
import ReactDOM from 'react-dom/client';

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
} else {
  // Only import App (and its global.css/app.css) for the main window
  const App = React.lazy(() => import('./App'));
  root.render(
    <React.Suspense fallback={null}>
      <App />
    </React.Suspense>
  );
}
