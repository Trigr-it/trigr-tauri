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
  browseForAudio:  () => invoke('browse_for_audio'),
  browseForVideo:  () => invoke('browse_for_video'),
  browseForFolder: () => invoke('browse_for_folder'),
  readImageBase64: (path) => invoke('read_image_base64', { path }),
  listInstalledApps: () => invoke('list_installed_apps'),

  // ── Profile settings ────────────────────────────────────────────────────────
  updateProfileSettings: (settings) =>
    invoke('update_profile_settings', { settings }),

  // ── Event listeners (main → renderer) ───────────────────────────────────────
  onMacroFired: (callback) => {
    listen('macro-fired', (event) => callback(event.payload)).then(u => { listeners['macro-fired'] = u; });
  },

  // Fired when a macro begins looping. Payload: { label, trigger, mode, count }.
  // Used by App.jsx to show the "loop running" toast so the user knows how to stop.
  onLoopFireStarted: (callback) => {
    listen('loop-fire-started', (event) => callback(event.payload)).then(u => { listeners['loop-fire-started'] = u; });
  },

  onLoopFireEnded: (callback) => {
    listen('loop-fire-ended', (event) => callback(event.payload)).then(u => { listeners['loop-fire-ended'] = u; });
  },

  onEngineStatus: (callback) => {
    listen('engine-status', (event) => callback(event.payload)).then(u => { listeners['engine-status'] = u; });
  },

  onProfileSwitched: (callback) => {
    listen('profile-switched', (event) => callback(event.payload)).then(u => { listeners['profile-switched'] = u; });
  },

  onSharedConfigMigrated: (callback) => {
    listen('shared-config-migrated', (event) => callback(event.payload)).then(u => { listeners['shared-config-migrated'] = u; });
  },

  // Fired by clipboard.rs when 5+ rows fail decryption in one session
  // (key/data mismatch). One-time per session; points the user at
  // Settings > Privacy & Security > Reset clipboard storage.
  onClipboardEncryptionError: (callback) => {
    listen('clipboard-encryption-error', (event) => callback(event.payload)).then(u => { listeners['clipboard-encryption-error'] = u; });
  },

  // Fired when the main window is hidden to the tray (X button / tray toggle).
  // Renderer clears its selection so reopening shows a blank slate.
  onResetEditingOnHide: (callback) => {
    listen('reset-editing-on-hide', () => callback()).then(u => { listeners['reset-editing-on-hide'] = u; });
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

  // ── Editing-active gate (suppresses foreground auto-switch during edits) ────
  setEditingActive: (active) => invoke('set_editing_active', { active }),

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
  openConfigFolder:    () => invoke('open_config_folder'),
  openLogsFolder:      () => invoke('open_logs_folder'),
  openClipboardFolder: () => invoke('open_clipboard_folder'),

  // ── Shared config ──────────────────────────────────────────────────────────
  getSharedConfigPath:   ()     => invoke('get_shared_config_path'),
  setSharedConfigPath:   (path, mode) => invoke('set_shared_config_path', { path, mode: mode || null }),
  clearSharedConfigPath: ()     => invoke('clear_shared_config_path'),
  onConfigReloadedFromSync: (callback) => {
    listen('config-reloaded-from-sync', (event) => callback(event.payload)).then(u => { listeners['config-reloaded-from-sync'] = u; });
  },
  onSyncConflictResolved: (callback) => {
    listen('sync-conflict-resolved', (event) => callback(event.payload)).then(u => { listeners['sync-conflict-resolved'] = u; });
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
  // Accepts both shapes the registration sites use: a resolved unlisten function
  // (the .then(u => listeners[ch] = u) pattern) or an in-flight Promise<unlisten>
  // (the listeners[ch] = listen(...) pattern). Awaiting handles the race where
  // a strict-mode double-mount triggers cleanup before listen() has resolved —
  // without the await the unlisten handle was getting orphaned and the next
  // mount stacked a second listener on top, doubling every clipboard event.
  removeAllListeners: async (channel) => {
    const entry = listeners[channel];
    if (!entry) return;
    delete listeners[channel];
    try {
      const unlisten = typeof entry === 'function' ? entry : await entry;
      if (typeof unlisten === 'function') unlisten();
    } catch { /* unlisten failure is non-fatal — listener just stays */ }
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

  // ── Macro recorder (Phase 1 — literal replay) ──────────────────────────────
  // Start captures the LL keyboard + mouse stream. Stop returns the captured
  // events as a JSON array; the caller stuffs the JSON into a "Replay
  // Recording" macro step value and saves via the normal config flow.
  startMacroRecording:   () => invoke('start_macro_recording'),
  stopMacroRecording:    () => invoke('stop_macro_recording'),
  discardMacroRecording: () => invoke('discard_macro_recording'),
  getRecordingStatus:    () => invoke('get_recording_status'),
  // Countdown overlay — orchestrates the minimise → 3-2-1 → record → restore
  // flow. Show positions the window centred on the cursor's monitor; the
  // countdown component animates and emits recorder-countdown-recording when
  // the count finishes (Rust listens, morphs to pill, calls start). The
  // listener below fires when the LL hook detects Ctrl+Shift+R.
  showRecorderCountdown: () => invoke('show_recorder_countdown'),
  hideRecorderCountdown: () => invoke('hide_recorder_countdown'),
  // Hide / restore the main window for the recorder flow. We use hide()
  // (not minimize) because Windows bounces minimised windows back when a
  // sibling window in the same process is shown. hide() also skips the
  // tray-hide side effects so the macro editor selection survives.
  recorderHideMain:    () => invoke('recorder_hide_main'),
  recorderRestoreMain: () => invoke('recorder_restore_main'),
  // Countdown emits this if the user hits Esc / Cancel during the 3-2-1.
  onRecorderCountdownCancelled: (callback) => {
    listeners['recorder-countdown-cancelled'] = listen(
      'recorder-countdown-cancelled',
      (event) => callback(event.payload),
    );
  },
  // Fired by the hook when the Ctrl+Shift+R stop hotkey is detected. The
  // listener should call stopMacroRecording() to retrieve the captured buffer.
  onRecorderStopRequested: (callback) => {
    listeners['recorder-stop-requested'] = listen(
      'recorder-stop-requested',
      (event) => callback(event.payload),
    );
  },

  // ── Quick Search overlay ────────────────────────────────────────────────────
  closeOverlay:          ()          => invoke('close_overlay'),
  resizeOverlay:         (height)    => invoke('overlay_resize', { height }),
  voiceOverlayErrorExpand: ()        => invoke('voice_overlay_error_expand'),
  voiceOverlayExamplesExpand: ()     => invoke('voice_overlay_examples_expand'),
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
  getClipboardHistory:    (page, perPage, filters = {}) => invoke('get_clipboard_history', {
    page, perPage,
    dateFilter: filters.dateFilter ?? null,
    appFilter: filters.appFilter ?? null,
    tagFilter: filters.tagFilter ?? null,
    search: filters.search ?? null,
    // Main UI: promoteStarred=true puts starred items above pinned. Popup
    // omits this flag so only pinned items promote (starred stays in timeline).
    promoteStarred: filters.promoteStarred === true,
  }),
  pasteClipboardItem:     (id)            => invoke('paste_clipboard_item', { id }),
  pasteText:              (text, sourceId = null) => invoke('paste_text', { text, sourceId }),
  // Fill-in webview only: opens the clipboard popup in fill-in mode. Rust
  // sets a flag so paste_clipboard_item / paste_text emit `fillin-insert-text`
  // instead of running Ctrl+V injection (unreliable WebView2 → WebView2).
  showClipboardOverlayForFillIn: () => invoke('show_clipboard_overlay_for_fillin'),
  onFillInInsertText: (callback) => {
    listen('fillin-insert-text', (event) => callback(event.payload)).then(u => { listeners['fillin-insert-text'] = u; });
  },
  copyClipboardItem:      (id)            => invoke('copy_clipboard_item', { id }),
  copyText:               (text)          => invoke('copy_text', { text }),
  ocrClipboardImage:      (id)            => invoke('ocr_clipboard_image', { id }),
  getClipboardImageColors:(id)            => invoke('get_clipboard_image_colors', { id }),
  saveClipboardImageAs:   (id, format)    => invoke('save_clipboard_image_as', { id, format }),
  deleteClipboardItem:    (id)            => invoke('delete_clipboard_item', { id }),
  clearClipboardHistory:  ()              => invoke('clear_clipboard_history'),
  // Quick Record (temp macro) — global hotkeys, persistent slot.
  setTempMacroRecordHotkey: (combo)   => invoke('set_temp_macro_record_hotkey', { combo }),
  clearTempMacroRecordHotkey: ()      => invoke('clear_temp_macro_record_hotkey'),
  setTempMacroPlayHotkey: (combo)     => invoke('set_temp_macro_play_hotkey', { combo }),
  clearTempMacroPlayHotkey: ()        => invoke('clear_temp_macro_play_hotkey'),
  setTempMacroLoopHotkey: (combo)     => invoke('set_temp_macro_loop_hotkey', { combo }),
  clearTempMacroLoopHotkey: ()        => invoke('clear_temp_macro_loop_hotkey'),
  getTempMacroStatus:     ()          => invoke('get_temp_macro_status'),
  clearTempMacro:         ()          => invoke('clear_temp_macro'),

  pinClipboardItem:       (id, pinned)    => invoke('pin_clipboard_item', { id, pinned }),
  starClipboardItem:      (id, starred)   => invoke('star_clipboard_item', { id, starred }),
  reorderClipboardPinned: (ids)           => invoke('reorder_clipboard_pinned', { ids }),
  reorderClipboardStarred:(ids)           => invoke('reorder_clipboard_starred', { ids }),
  // Saved folders (internal naming stays folder/starred; UI says "Saved")
  createClipboardFolder:  (name)          => invoke('create_clipboard_folder', { name }),
  renameClipboardFolder:  (id, name)      => invoke('rename_clipboard_folder', { id, name }),
  deleteClipboardFolder:  (id)            => invoke('delete_clipboard_folder', { id }),
  moveClipboardItemToFolder: (id, folderId) => invoke('move_clipboard_item_to_folder', { id, folderId: folderId ?? null }),
  getClipboardFolders:    ()              => invoke('get_clipboard_folders'),
  getClipboardImage:      (id)            => invoke('get_clipboard_image', { id }),
  getDistinctSourceApps:  ()              => invoke('get_distinct_source_apps'),
  getClipboardDateBuckets: (filters = {}) => invoke('get_clipboard_date_buckets', {
    appFilter: filters.appFilter ?? null,
    tagFilter: filters.tagFilter ?? null,
  }),
  updateClipboardItem:    (id, newText)   => invoke('update_clipboard_item', { id, newText }),
  getClipboardSettings:   ()              => invoke('get_clipboard_settings'),
  setClipboardSettings:   (retentionDays) => invoke('set_clipboard_settings', { retentionDays }),
  setClipboardCaptureEnabled: (enabled)   => invoke('set_clipboard_capture_enabled', { enabled }),
  setClipboardExcludedApps: (apps)        => invoke('set_clipboard_excluded_apps', { apps }),
  getClipboardStorageSize: ()             => invoke('get_clipboard_storage_size'),
  // Clipboard encryption (v0.5): status line + plaintext-backup controls + nuke-and-restart
  getClipboardEncryptionStatus: ()        => invoke('get_clipboard_encryption_status'),
  deleteClipboardPlaintextBackup: ()      => invoke('delete_clipboard_plaintext_backup'),
  resetClipboardStorage:  ()              => invoke('reset_clipboard_storage'),
  // Telemetry opt-out (machine-local). true = sending stats, false = disabled.
  getTelemetryEnabled:    ()              => invoke('get_telemetry_enabled'),
  setTelemetryEnabled:    (enabled)       => invoke('set_telemetry_enabled', { enabled }),
  closeClipboardOverlay:     ()       => invoke('close_clipboard_overlay'),
  resizeClipboardOverlay:    (width, height) => invoke('clipboard_overlay_resize', { width, height }),
  onClipboardNewItem: (callback) => {
    // Store the Promise itself (not the resolved unlisten) so removeAllListeners
    // can await it on cleanup — closes the race where the .then() hadn't fired
    // yet when an unmount happened and the unlisten handle was lost.
    listeners['clipboard-new-item'] = listen('clipboard-new-item', (event) => callback(event.payload));
  },
  // Promote-on-use: a row's timestamp was rewritten (panel copy or popup
  // paste) — the panel floats it to the top of the timeline. Same Promise
  // storage pattern as onClipboardNewItem.
  onClipboardItemTouched: (callback) => {
    listeners['clipboard-item-touched'] = listen('clipboard-item-touched', (event) => callback(event.payload));
  },
  onClipboardOverlayData: (callback) => {
    listen('clipboard-overlay-data', (event) => callback(event.payload)).then(u => { listeners['clipboard-overlay-data'] = u; });
  },

  // ── Global pause toggle ─────────────────────────────────────────────────────
  setPauseHotkey:      (combo) => invoke('set_global_pause_key', { combo }),
  clearPauseHotkey:    ()      => invoke('clear_global_pause_key'),
  setClipboardPasteHotkey: (combo) => invoke('set_clipboard_paste_key', { combo }),
  clearClipboardPasteHotkey: ()    => invoke('clear_clipboard_paste_key'),
  setVoiceHotkey:      (combo) => invoke('set_voice_hotkey', { combo }),
  clearVoiceHotkey:    ()      => invoke('clear_voice_hotkey'),
  startVoiceRecognition:  (phrases) => invoke('start_voice_recognition', { phrases }),
  stopVoiceRecognition:   ()        => invoke('stop_voice_recognition'),
  startVoiceContinuous:   (phrases) => invoke('start_voice_continuous', { phrases }),
  stopVoiceContinuous:    ()        => invoke('stop_voice_continuous'),
  setVoiceContinuous:     (on)      => invoke('set_voice_continuous', { on }),
  onVoiceResult: (callback) => {
    listen('voice-result', (event) => callback(event.payload)).then(u => { listeners['voice-result'] = u; });
  },
  onVoiceError: (callback) => {
    listen('voice-error', (event) => callback(event.payload)).then(u => { listeners['voice-error'] = u; });
  },
  onVoiceSoundStarted: (callback) => {
    listen('voice-sound-started', () => callback()).then(u => { listeners['voice-sound-started'] = u; });
  },
  onVoiceSoundEnded: (callback) => {
    listen('voice-sound-ended', () => callback()).then(u => { listeners['voice-sound-ended'] = u; });
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
  checkHotkeyConflict: (combo, fromSlot) => invoke('check_hotkey_conflict', { combo, fromSlot: fromSlot || null }),

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
  enumMonitors:       ()         => invoke('enum_monitors'),
  checkForUpdates:    ()         => invoke('check_for_updates'),

  // ── Licence ──────────────────────────────────────────────────────────────
  getLicenceStatus:          ()    => invoke('get_licence_status'),
  activateLicence:           (key) => invoke('activate_licence', { key }),
  deactivateLicence:         ()    => invoke('deactivate_licence'),
  checkLicenceRevalidation:  ()    => invoke('check_licence_revalidation'),
  startTrial:                ()    => invoke('start_trial'),
  resetTrial:                ()    => invoke('reset_trial'),
  getGracePeriodState:       ()    => invoke('get_grace_period_state'),
  migrateSharedToLocalNow:   ()    => invoke('migrate_shared_to_local_now'),
  markTrialOfferShown:       ()    => invoke('mark_trial_offer_shown'),
};

// ── Suppress webview browser accelerators ──────────────────────────────────
// Keyfire is a desktop app, not a browser. Prevent Ctrl+F (find), Ctrl+P (print),
// Ctrl+R (reload), etc. from triggering built-in WebView2 browser UI.
// Preserve Ctrl+C/V/X/A/Z for normal text editing within the Keyfire UI.
document.addEventListener('keydown', (e) => {
  // Block browser accelerator Ctrl/Meta combos
  if ((e.ctrlKey || e.metaKey) && !['c', 'v', 'x', 'a', 'z'].includes(e.key.toLowerCase())) {
    e.preventDefault();
    // Ctrl+Space: toggle overlay (JS path for when Keyfire has focus)
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

  // CRITICAL: Two-path capture: JS listener (Keyfire focused) + LL hook (other apps).
  // __trigr_recording and __trigr_capturing MUST be kept in sync with the
  // Rust IS_RECORDING_HOTKEY / IS_CAPTURING_KEY flags.
  // Any new capture entry point must set these flags AND call the Rust command.
  // The LL hook can't see keypresses when Keyfire's WebView2 has focus,
  // so this JS listener provides an alternative capture path.
  if (window.__trigr_capturing || window.__trigr_recording) {
    // Do not intercept when focus is on a text input — let typing work normally
    const tag = document.activeElement?.tagName?.toLowerCase();
    const isEditable = document.activeElement?.isContentEditable;
    if (tag === 'input' || tag === 'textarea' || isEditable) {
      return;
    }

    const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta']);
    if (MODIFIER_KEYS.has(e.key)) {
      // Bare-modifier capture (sole-mod tracking, mirrors Rust state.capture_sole_modifier
      // for the LL-hook path). On keydown of a modifier with no other mods held, mark
      // intent. If a second modifier is pressed, clear intent so Ctrl+Shift etc. doesn't
      // get captured as a sole modifier. Emit happens on keyup once all mods are released.
      const otherModsHeld =
        (e.key !== 'Control' && e.ctrlKey) ||
        (e.key !== 'Shift'   && e.shiftKey) ||
        (e.key !== 'Alt'     && e.altKey)   ||
        (e.key !== 'Meta'    && e.metaKey);
      window.__trigr_capture_sole_mod = otherModsHeld
        ? null
        : (e.key === 'Meta' ? 'Win' : e.key);
    } else {
      e.preventDefault();
      e.stopPropagation();
      // Pressing a real key invalidates any pending sole-modifier capture
      window.__trigr_capture_sole_mod = null;
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

// Bare-modifier capture — emit on keyup when the user releases the last modifier
// and the captured intent (set in the keydown listener above) is still valid.
// Sends js_key_event with code='' + a single modifier flag set; Rust treats an
// empty code as a sole-modifier capture and emits the modifier name as the
// captured combo (e.g. "Ctrl"). Gated to __trigr_capturing only — hotkey trigger
// recording (__trigr_recording) deliberately rejects bare modifiers since they
// would conflict with normal modifier usage in everyday combos.
document.addEventListener('keyup', (e) => {
  if (!window.__trigr_capturing) return;
  const MODIFIER_KEYS = new Set(['Control', 'Shift', 'Alt', 'Meta']);
  if (!MODIFIER_KEYS.has(e.key)) return;
  // Don't fire when focus is on an editable element (consistency with keydown)
  const tag = document.activeElement?.tagName?.toLowerCase();
  const isEditable = document.activeElement?.isContentEditable;
  if (tag === 'input' || tag === 'textarea' || isEditable) return;
  // On the LAST modifier keyup, all *Key flags read false (the released one is
  // excluded; any still-held mods would show true).
  const noOtherMods = !e.ctrlKey && !e.shiftKey && !e.altKey && !e.metaKey;
  const sole = window.__trigr_capture_sole_mod;
  if (noOtherMods && sole) {
    window.__trigr_capture_sole_mod = null;
    invoke('js_key_event', {
      code: '',
      ctrl: sole === 'Control',
      shift: sole === 'Shift',
      alt: sole === 'Alt',
      meta: sole === 'Win',
    });
  }
}, true);
