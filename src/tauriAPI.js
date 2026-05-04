/**
 * tauriAPI.js — Drop-in replacement for Electron's preload.js
 *
 * Maps every window.electronAPI.* call to Tauri's invoke() / listen() / emit().
 * React components continue calling window.electronAPI.* unchanged.
 *
 * Stubs return sensible defaults so the UI renders even before Rust commands
 * are implemented.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen, emit } from '@tauri-apps/api/event';

// Store unlisten handles for cleanup
const listeners = {};

window.electronAPI = {
  // ── Window controls ─────────────────────────────────────────────────────────
  minimize: () => invoke('window_minimize'),
  maximize: () => invoke('window_maximize'),
  close:    () => invoke('window_close'),

  // ── Config persistence ──────────────────────────────────────────────────────
  loadConfig:  ()       => invoke('load_config'),
  saveConfig:  (config) => invoke('save_config', { config }),

  // ── Hotkey engine ───────────────────────────────────────────────────────────
  updateAssignments: (assignments, profile) =>
    invoke('update_assignments', { assignments, profile }),

  toggleMacros: (enabled) =>
    invoke('toggle_macros', { enabled }),

  getEngineStatus: () =>
    invoke('get_engine_status'),

  browseForFile:   () => invoke('browse_for_file'),
  browseForImage:  () => invoke('browse_for_image'),
  browseForFolder: () => invoke('browse_for_folder'),
  readImageBase64: (path) => invoke('read_image_base64', { path }),

  // ── Profile settings ────────────────────────────────────────────────────────
  updateProfileSettings: (settings) =>
    invoke('update_profile_settings', { settings }),

  // ── Event listeners (main → renderer) ───────────────────────────────────────
  onMacroFired: (callback) => {
    listen('macro-fired', (event) => callback(event.payload)).then(u => { listeners['macro-fired'] = u; });
  },

  onEngineStatus: (callback) => {
    listen('engine-status', (event) => callback(event.payload)).then(u => { listeners['engine-status'] = u; });
  },

  onProfileSwitched: (callback) => {
    listen('profile-switched', (event) => callback(event.payload)).then(u => { listeners['profile-switched'] = u; });
  },

  // ── Fill-in field dialog ────────────────────────────────────────────────────
  onFillInPrompt: (callback) => {
    listen('fill-in-prompt', (event) => callback(event.payload)).then(u => { listeners['fill-in-prompt'] = u; });
  },
  respondFillIn: (value) => emit('fill-in-response', value),

  fillInReady:    ()         => invoke('fill_in_ready'),
  resizeFillin:   (height)   => invoke('fillin_resize', { height }),
  onFillInRequestReady: (callback) => {
    listen('fill-in-request-ready', () => callback()).then(u => { listeners['fill-in-request-ready'] = u; });
  },
  onFillInShow: (callback) => {
    listen('fill-in-show', (event) => callback(event.payload)).then(u => { listeners['fill-in-show'] = u; });
  },
  submitFillIn: (values) => invoke('fill_in_submit', { values }),

  // ── Active global profile ───────────────────────────────────────────────────
  setActiveGlobalProfile: (profile) => invoke('set_active_global_profile', { profile }),

  // ── Input focus state ───────────────────────────────────────────────────────
  notifyInputFocus: (focused) => invoke('input_focus_changed', { focused }),

  // ── Autocorrect ─────────────────────────────────────────────────────────────
  updateAutocorrectEnabled: (enabled) => invoke('update_autocorrect_enabled', { enabled }),

  // ── Global compatibility settings ───────────────────────────────────────────
  updateGlobalSettings: (settings) => invoke('update_global_settings', { settings }),

  // ── Global variables (text expansion tokens) ───────────────────────────────
  updateGlobalVariables: (vars) => invoke('update_global_variables', { vars }),

  // ── Onboarding ──────────────────────────────────────────────────────────────
  resetOnboarding: () => invoke('reset_onboarding'),

  // ── Startup ─────────────────────────────────────────────────────────────────
  getStartupEnabled:  ()        => invoke('get_startup_enabled'),
  setStartupEnabled:  (enabled) => invoke('set_startup_enabled', { enabled }),
  getAppVersion:      ()        => invoke('get_app_version'),

  // ── Help ────────────────────────────────────────────────────────────────────
  openHelp:     ()    => invoke('open_help'),
  openExternal: (url) => invoke('open_external', { url }),

  // ── Config path & folder ────────────────────────────────────────────────────
  getConfigPath:    () => invoke('get_config_path'),
  openConfigFolder: () => invoke('open_config_folder'),
  openLogsFolder:   () => invoke('open_logs_folder'),

  // ── Shared config ──────────────────────────────────────────────────────────
  getSharedConfigPath:   ()     => invoke('get_shared_config_path'),
  setSharedConfigPath:   (path, mode) => invoke('set_shared_config_path', { path, mode: mode || null }),
  clearSharedConfigPath: ()     => invoke('clear_shared_config_path'),
  onConfigReloadedFromSync: (callback) => {
    listen('config-reloaded-from-sync', (event) => callback(event.payload)).then(u => { listeners['config-reloaded-from-sync'] = u; });
  },

  // ── Backup & restore ────────────────────────────────────────────────────────
  exportConfig:   ()         => invoke('export_config'),
  importConfig:   ()         => invoke('import_config'),

  // ── Profile export/import ──────────────────────────────────────────────────
  exportProfile:  (filenameHint, content) => invoke('export_profile', { filenameHint, content }),
  importProfile:  ()         => invoke('import_profile'),
  listBackups:    ()         => invoke('list_backups'),
  restoreBackup:  (filename) => invoke('restore_backup', { filename }),

  // ── Hotkey recording ────────────────────────────────────────────────────────
  startHotkeyRecording: () => { window.__trigr_recording = true; return invoke('start_hotkey_recording'); },
  stopHotkeyRecording:  () => { window.__trigr_recording = false; return invoke('stop_hotkey_recording'); },
  onHotkeyRecorded: (callback) => {
    listen('hotkey-recorded', (event) => {
      window.__trigr_recording = false; // Clear flag so JS interceptor stops eating keys
      callback(event.payload);
    }).then(u => { listeners['hotkey-recorded'] = u; });
  },

  // ── Cleanup listeners ──────────────────────────────────────────────────────
  removeAllListeners: (channel) => {
    const unlisten = listeners[channel];
    if (unlisten) {
      unlisten();
      delete listeners[channel];
    }
  },

  // ── Key capture ─────────────────────────────────────────────────────────────
  startKeyCapture: ()         => { window.__trigr_capturing = true; return invoke('start_key_capture'); },
  stopKeyCapture:  ()         => { window.__trigr_capturing = false; return invoke('stop_key_capture'); },
  onKeyCaptured:   (callback) => {
    listen('key-captured', (event) => {
      window.__trigr_capturing = false; // Clear flag so JS interceptor stops eating keys
      callback(event.payload);
    }).then(u => { listeners['key-captured'] = u; });
  },

  // ── Quick Search overlay ────────────────────────────────────────────────────
  closeOverlay:          ()          => invoke('close_overlay'),
  resizeOverlay:         (height)    => invoke('overlay_resize', { height }),
  voiceOverlayErrorExpand: ()        => invoke('voice_overlay_error_expand'),
  executeSearchResult:   (result)    => invoke('execute_search_result', { result }),
  updateSearchSettings:  (settings)  => invoke('update_search_settings', { settings }),

  onOverlaySearchData: (callback) => {
    listen('overlay-search-data', (event) => callback(event.payload)).then(u => { listeners['overlay-search-data'] = u; });
  },
  onOverlayFired: (callback) => {
    listen('overlay-fired', (event) => callback(event.payload)).then(u => { listeners['overlay-fired'] = u; });
  },

  // ── Analytics ───────────────────────────────────────────────────────────────
  getAnalytics:  () => invoke('get_analytics'),
  resetAnalytics: () => invoke('reset_analytics'),
  getDailyChart:          (days) => invoke('get_daily_chart', { days: days || 14 }),
  getAssignmentBreakdown: (days) => invoke('get_assignment_breakdown', { days: days || null }),
  getTypeBreakdown:       (days) => invoke('get_type_breakdown', { days: days || null }),
  getHourlyHeatmap:       (days) => invoke('get_hourly_heatmap', { days: days || null }),
  getTopApps:             (days) => invoke('get_top_apps', { days: days || null }),
  getExpansionEfficiency: ()     => invoke('get_expansion_efficiency'),
  getExpansionCounts:     ()     => invoke('get_expansion_counts'),
  getStreaks:              ()     => invoke('get_streaks'),
  exportAnalyticsCsv:     ()     => invoke('export_analytics_csv'),

  // ── Clipboard Manager ──────────────────────────────────────────────────────
  getClipboardHistory:    (page, perPage) => invoke('get_clipboard_history', { page, perPage }),
  pasteClipboardItem:     (id)            => invoke('paste_clipboard_item', { id }),
  deleteClipboardItem:    (id)            => invoke('delete_clipboard_item', { id }),
  clearClipboardHistory:  ()              => invoke('clear_clipboard_history'),
  pinClipboardItem:       (id, pinned)    => invoke('pin_clipboard_item', { id, pinned }),
  getClipboardImage:      (id)            => invoke('get_clipboard_image', { id }),
  getDistinctSourceApps:  ()              => invoke('get_distinct_source_apps'),
  updateClipboardItem:    (id, newText)   => invoke('update_clipboard_item', { id, newText }),
  getClipboardSettings:   ()              => invoke('get_clipboard_settings'),
  setClipboardSettings:   (retentionDays) => invoke('set_clipboard_settings', { retentionDays }),
  getClipboardStorageSize: ()             => invoke('get_clipboard_storage_size'),
  closeClipboardOverlay:     ()       => invoke('close_clipboard_overlay'),
  resizeClipboardOverlay:    (width, height) => invoke('clipboard_overlay_resize', { width, height }),
  onClipboardNewItem: (callback) => {
    listen('clipboard-new-item', (event) => callback(event.payload)).then(u => { listeners['clipboard-new-item'] = u; });
  },
  onClipboardOverlayData: (callback) => {
    listen('clipboard-overlay-data', (event) => callback(event.payload)).then(u => { listeners['clipboard-overlay-data'] = u; });
  },

  // ── Global pause toggle ─────────────────────────────────────────────────────
  setPauseHotkey:      (combo) => invoke('set_global_pause_key', { combo }),
  clearPauseHotkey:    ()      => invoke('clear_global_pause_key'),
  setVoiceHotkey:      (combo) => invoke('set_voice_hotkey', { combo }),
  clearVoiceHotkey:    ()      => invoke('clear_voice_hotkey'),
  startVoiceRecognition:  (phrases) => invoke('start_voice_recognition', { phrases }),
  stopVoiceRecognition:   ()        => invoke('stop_voice_recognition'),
  setVoiceContinuous:     (on)      => invoke('set_voice_continuous', { on }),
  onVoiceResult: (callback) => {
    listen('voice-result', (event) => callback(event.payload)).then(u => { listeners['voice-result'] = u; });
  },
  onVoiceError: (callback) => {
    listen('voice-error', (event) => callback(event.payload)).then(u => { listeners['voice-error'] = u; });
  },
  onOverlayVoiceData: (callback) => {
    listen('overlay-voice-data', (event) => callback(event.payload)).then(u => { listeners['overlay-voice-data'] = u; });
  },
  onVoiceContinuousOn: (callback) => {
    listen('voice-continuous-on', () => callback()).then(u => { listeners['voice-continuous-on'] = u; });
  },
  onVoiceContinuousRestart: (callback) => {
    listen('voice-continuous-restart', () => callback()).then(u => { listeners['voice-continuous-restart'] = u; });
  },
  checkHotkeyConflict: (combo) => invoke('check_hotkey_conflict', { combo }),

  // ── Radial Menu ────────────────────────────────────────────────────────────
  getAppIcon:             (path) => invoke('get_app_icon', { path }),
  setRadialMenuHotkey:    (combo) => invoke('set_radial_menu_hotkey', { combo }),
  clearRadialMenuHotkey:  ()      => invoke('clear_radial_menu_hotkey'),
  closeRadialMenu:        ()      => invoke('close_radial_menu'),
  resizeRadialMenu:       (width, height) => invoke('radial_menu_resize', { width, height }),
  executeRadialMenuItem:  (result) => invoke('execute_radial_menu_item', { result }),
  onRadialMenuData: (callback) => {
    listen('radial-menu-data', (event) => callback(event.payload)).then(u => { listeners['radial-menu-data'] = u; });
  },
  onRadialMenuKeyReleased: (callback) => {
    listen('radial-menu-key-released', () => callback()).then(u => { listeners['radial-menu-key-released'] = u; });
  },

  // ── Auto-updater ────────────────────────────────────────────────────────────
  onUpdateAvailable:  (callback) => {
    listen('update-available', (event) => callback(event.payload)).then(u => { listeners['update-available'] = u; });
  },
  onDownloadProgress: (callback) => {
    listen('download-progress', (event) => callback(event.payload)).then(u => { listeners['download-progress'] = u; });
  },
  onUpdateDownloaded: (callback) => {
    listen('update-downloaded', () => callback()).then(u => { listeners['update-downloaded'] = u; });
  },
  installUpdate:      ()         => invoke('install_update'),
  startDownload:      (version)  => invoke('start_download', { version }),
  getCursorPosition:  ()         => invoke('get_cursor_position'),
  checkForUpdates:    ()         => invoke('check_for_updates'),

  // ── Licence ──────────────────────────────────────────────────────────────
  getLicenceStatus:          ()    => invoke('get_licence_status'),
  activateLicence:           (key) => invoke('activate_licence', { key }),
  deactivateLicence:         ()    => invoke('deactivate_licence'),
  checkLicenceRevalidation:  ()    => invoke('check_licence_revalidation'),
};

// ── Suppress webview browser accelerators ──────────────────────────────────
// Trigr is a desktop app, not a browser. Prevent Ctrl+F (find), Ctrl+P (print),
// Ctrl+R (reload), etc. from triggering built-in WebView2 browser UI.
// Preserve Ctrl+C/V/X/A/Z for normal text editing within the Trigr UI.
document.addEventListener('keydown', (e) => {
  // Block browser accelerator Ctrl/Meta combos
  if ((e.ctrlKey || e.metaKey) && !['c', 'v', 'x', 'a', 'z'].includes(e.key.toLowerCase())) {
    e.preventDefault();
    // Ctrl+Space: toggle overlay (JS path for when Trigr has focus)
    if (e.code === 'Space' && e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey) {
      e.stopPropagation();
      invoke('js_key_event', { code: 'Space', ctrl: true, shift: false, alt: false, meta: false });
      return;
    }
  }
  // Block Alt key activating the system menu in WebView2
  if (e.altKey) {
    e.preventDefault();
  }
  // Block standalone browser keys
  if (e.key === 'F5' || e.key === 'F12') {
    e.preventDefault();
  }

  // CRITICAL: Two-path capture: JS listener (Trigr focused) + LL hook (other apps).
  // __trigr_recording and __trigr_capturing MUST be kept in sync with the
  // Rust IS_RECORDING_HOTKEY / IS_CAPTURING_KEY flags.
  // Any new capture entry point must set these flags AND call the Rust command.
  // The LL hook can't see keypresses when Trigr's WebView2 has focus,
  // so this JS listener provides an alternative capture path.
  if (window.__trigr_capturing || window.__trigr_recording) {
    // Do not intercept when focus is on a text input — let typing work normally
    const tag = document.activeElement?.tagName?.toLowerCase();
    const isEditable = document.activeElement?.isContentEditable;
    if (tag === 'input' || tag === 'textarea' || isEditable) {
      return;
    }

    const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta']);
    if (!MODIFIER_KEYS.has(e.key)) {
      e.preventDefault();
      e.stopPropagation();
      invoke('js_key_event', {
        code: e.code,
        ctrl: e.ctrlKey,
        shift: e.shiftKey,
        alt: e.altKey,
        meta: e.metaKey,
      });
    }
  }
}, true);
