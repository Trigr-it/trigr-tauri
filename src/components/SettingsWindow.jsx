// Standalone Settings window (?settings=1) — pre-created hidden at startup.
//
// The main window (App.jsx) stays the single owner of all settings state and
// config persistence. This window is a remote control:
//   - App.jsx broadcasts a "settings-state" payload (all values + theme)
//     whenever any of them change, on "settings-shown", and on
//     "settings-request-state".
//   - Every handler prop is proxied as a fire-and-forget "settings-action"
//     event { action, args } that App.jsx dispatches to its existing handler.
// No handler return values are consumed anywhere in SettingsPanel, so the
// one-way proxy is safe. Capture events (key-captured / hotkey-recorded) are
// app-wide broadcasts from Rust and reach this window directly.
//
// This is an ordinary opaque window, so unlike the transparent overlays it
// imports global.css for the full theme + shared component styles (the
// no-global-css rule exists for transparency, which doesn't apply here).
import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Settings } from 'lucide-react';
import { listen, emit } from '@tauri-apps/api/event';
import '../styles/global.css';
import SettingsPanel from './SettingsPanel';
import './SettingsWindow.css';

export default function SettingsWindow() {
  const [bridge, setBridge] = useState(null);
  const [navRequest, setNavRequest] = useState(null); // { section, ts }
  const navNonce = useRef(0);

  useEffect(() => {
    let unState, unShown;
    listen('settings-state', (e) => {
      const s = e.payload || {};
      if (s.theme) document.documentElement.setAttribute('data-theme', s.theme);
      setBridge(s);
    }).then(u => { unState = u; });
    // Rust show path broadcasts "settings-shown" (with an optional deep-link
    // section) before .show() — re-request state as belt-and-braces in case
    // an emit was lost across a WebView2 suspend/resume cycle.
    listen('settings-shown', (e) => {
      const section = e.payload?.section;
      if (section) setNavRequest({ section, ts: ++navNonce.current });
      emit('settings-request-state');
    }).then(u => { unShown = u; });
    emit('settings-request-state');
    return () => { unState?.(); unShown?.(); };
  }, []);

  // APP_INPUT_FOCUSED parity with App.jsx — typing in this window's inputs
  // (licence key, exclusion chips, search…) must never feed the expansion /
  // autocorrect keystroke buffer.
  useEffect(() => {
    function isEditable(el) {
      if (!el) return false;
      const tag = el.tagName;
      return tag === 'INPUT' || tag === 'TEXTAREA' || el.contentEditable === 'true';
    }
    function onFocusIn(e) {
      if (isEditable(e.target)) window.electronAPI?.notifyInputFocus(true);
    }
    function onFocusOut(e) {
      if (isEditable(e.target) && !isEditable(e.relatedTarget)) {
        window.electronAPI?.notifyInputFocus(false);
      }
    }
    document.addEventListener('focusin', onFocusIn);
    document.addEventListener('focusout', onFocusOut);
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
    };
  }, []);

  // Proxy factory: act('setPauseKey') returns a handler that ships its args
  // to App.jsx's dispatch table.
  //
  // Args are filtered before emit — React SyntheticEvents (from raw
  // onClick={onFoo} handlers) carry DOM refs + nativeEvent that structured
  // cloning inside Tauri's cross-window emit cannot serialise. Without this
  // guard the whole emit throws asynchronously and the action silently
  // vanishes. Symptoms in the wild: Restart Onboarding, Replay Welcome,
  // Reset Hidden Tips, Export / Import Config all no-op'd after the
  // dedicated Settings window shipped (v0.7.4). Filter identifies
  // SyntheticEvents by their .nativeEvent property.
  const act = useCallback((action) => (...args) => {
    const clean = args.filter(a => !(a && typeof a === 'object' && a.nativeEvent));
    emit('settings-action', { action, args: clean });
  }, []);

  const close = useCallback(() => {
    window.electronAPI?.hideSettingsWindow();
  }, []);

  // Nothing renders until the first state payload lands — App.jsx emits it
  // before the window is shown, so this never flashes defaults at the user.
  if (!bridge) return null;

  return (
    <div className="settings-window">
      <div className="sw-titlebar">
        <span className="sw-title-wrap">
          <span className="sw-title-icon" aria-hidden="true">
            <Settings size={15} strokeWidth={1.75} />
          </span>
          <span className="sw-title">Settings</span>
        </span>
        <div className="sw-controls" data-drag="false">
          <button
            className="sw-btn sw-minimize"
            onClick={() => window.electronAPI?.minimize()}
            title="Minimize"
            aria-label="Minimize"
            type="button"
          >
            <svg width="10" height="10" viewBox="0 0 10 10">
              <line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
          </button>
          <button
            className="sw-btn sw-close"
            onClick={close}
            title="Close"
            aria-label="Close"
            type="button"
          >
            <svg width="10" height="10" viewBox="0 0 10 10">
              <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
              <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
            </svg>
          </button>
        </div>
      </div>
      <SettingsPanel
        navRequest={navRequest}
        onClose={close}
        macrosEnabledOnStartup={bridge.macrosEnabledOnStartup}
        onToggleMacrosOnStartup={act('toggleMacrosOnStartup')}
        physicalKeyboardLayout={bridge.physicalKeyboardLayout}
        resolvedPhysicalLayout={bridge.resolvedPhysicalLayout}
        onSetPhysicalKeyboardLayout={act('setPhysicalKeyboardLayout')}
        onExportConfig={act('exportConfig')}
        onImportConfig={act('importConfig')}
        onRestoreBackup={act('restoreBackup')}
        expansionExcludedApps={bridge.expansionExcludedApps}
        onUpdateExpansionExcludedApps={act('updateExpansionExcludedApps')}
        globalInputMethod={bridge.globalInputMethod}
        macroSpeed={bridge.macroSpeed}
        keystrokeDelay={bridge.keystrokeDelay}
        macroTriggerDelay={bridge.macroTriggerDelay}
        doubleTapWindow={bridge.doubleTapWindow}
        holdThresholdMs={bridge.holdThresholdMs}
        fireOnPress={bridge.fireOnPress}
        defaultDateFormat={bridge.defaultDateFormat}
        onUpdateGlobalSettings={act('updateGlobalSettings')}
        searchOverlayHotkey={bridge.searchOverlayHotkey}
        searchOverlayEnabled={bridge.searchOverlayEnabled}
        overlayShowAll={bridge.overlayShowAll}
        overlayCloseAfterFiring={bridge.overlayCloseAfterFiring}
        overlayIncludeAutocorrect={bridge.overlayIncludeAutocorrect}
        onUpdateSearchSettings={act('updateSearchSettings')}
        globalPauseToggleKey={bridge.globalPauseToggleKey}
        onSetPauseKey={act('setPauseKey')}
        onClearPauseKey={act('clearPauseKey')}
        voiceEnabled={bridge.voiceEnabled}
        onToggleVoiceEnabled={act('toggleVoiceEnabled')}
        voiceHotkey={bridge.voiceHotkey}
        onSetVoiceKey={act('setVoiceKey')}
        onClearVoiceKey={act('clearVoiceKey')}
        onRestartOnboarding={act('restartOnboarding')}
        onReplayWelcome={act('replayWelcome')}
        onResetHiddenTips={act('resetHiddenTips')}
        hiddenTipsCount={bridge.hiddenTipsCount}
        activeProfile={bridge.activeProfile}
        onImportTemplate={act('importTemplate')}
        onImportCadTemplate={act('importCadTemplate')}
        onImportAppProfile={act('importAppProfile')}
        isPro={bridge.isPro}
        licenceStatus={bridge.licenceStatus || {}}
        onLicenceStatusChange={act('licenceStatusChange')}
        onShowUpgrade={act('showUpgrade')}
        onResetTrial={act('resetTrial')}
        clipboardCaptureEnabled={bridge.clipboardCaptureEnabled}
        onToggleClipboardCapture={act('toggleClipboardCapture')}
        clipboardExcludedApps={bridge.clipboardExcludedApps}
        onUpdateClipboardExcludedApps={act('updateClipboardExcludedApps')}
        clipboardPasteHotkey={bridge.clipboardPasteHotkey}
        onSetClipboardPasteKey={act('setClipboardPasteKey')}
        onClearClipboardPasteKey={act('clearClipboardPasteKey')}
        telemetryEnabled={bridge.telemetryEnabled}
        onToggleTelemetry={act('toggleTelemetry')}
      />
    </div>
  );
}
