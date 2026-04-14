import React, { useState, useEffect, useRef } from 'react';
import './TitleBar.css';
import TemplatesPanel from './TemplatesPanel';

export default function TitleBar({
  macrosEnabled,
  onToggleMacros,
  theme = 'dark',
  onToggleTheme,
  onOpenSettings,
  settingsOpen = false,
  activeArea = 'mapping',
  onAreaChange,
  listViewActive = false,
  onToggleListView,
  activeProfile = 'Default',
  onImportTemplate,
  onImportCadTemplate,
  onShowNotification,
}) {
  const handleMinimize = () => window.electronAPI?.minimize();
  const handleMaximize = () => window.electronAPI?.maximize();
  const handleClose    = () => window.electronAPI?.close();

  // Templates dropdown
  const [templatesDismissed, setTemplatesDismissed] = useState(() => {
    try { return localStorage.getItem('trigr_templates_dismissed') === 'true'; } catch { return false; }
  });
  const [templatesOpen, setTemplatesOpen] = useState(false);
  const [tplCtxMenu, setTplCtxMenu] = useState(null); // { x, y } or null
  const templatesRef = useRef(null);
  const tplCtxRef = useRef(null);

  useEffect(() => {
    if (!templatesOpen && !tplCtxMenu) return;
    function onDown(e) {
      if (templatesOpen && templatesRef.current && !templatesRef.current.contains(e.target)) setTemplatesOpen(false);
      if (tplCtxMenu && tplCtxRef.current && !tplCtxRef.current.contains(e.target)) setTplCtxMenu(null);
    }
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [templatesOpen, tplCtxMenu]);

  const handleDismissTemplates = () => {
    setTemplatesOpen(false);
    setTplCtxMenu(null);
    setTemplatesDismissed(true);
    try { localStorage.setItem('trigr_templates_dismissed', 'true'); } catch {}
    onShowNotification?.('Templates can always be found in Settings', 'info');
  };

  return (
    <div className="titlebar" data-drag="true">
      <div className="titlebar-left">
        <div className="app-logo">
          <span className="app-name">Trigr</span>
        </div>

        <div className="titlebar-divider" />

        {/* Area tabs — top-level navigation between Mapping, Text Expansion, and Analytics */}
        <div className="area-tabs" data-drag="false">
          <button
            className={`area-tab${activeArea === 'mapping' ? ' active' : ''}`}
            onClick={() => onAreaChange?.('mapping')}
            type="button"
          >
            Key Mapping
          </button>
          <button
            className={`area-tab${activeArea === 'expansions' ? ' active' : ''}`}
            onClick={() => onAreaChange?.('expansions')}
            type="button"
          >
            Text Expansion
          </button>
          <button
            className={`area-tab${activeArea === 'clipboard' ? ' active' : ''}`}
            onClick={() => onAreaChange?.('clipboard')}
            type="button"
          >
            Clipboard
          </button>
          <button
            className={`area-tab${activeArea === 'analytics' ? ' active' : ''}`}
            onClick={() => onAreaChange?.('analytics')}
            type="button"
          >
            Analytics
          </button>
        </div>
      </div>

      <div className="titlebar-right" data-drag="false">
        {activeArea === 'mapping' && !templatesDismissed && (
          <div className="tb-templates-wrap" ref={templatesRef} data-drag="false">
            <button
              className={`tb-templates-btn${templatesOpen ? ' active' : ''}`}
              onClick={() => setTemplatesOpen(v => !v)}
              onContextMenu={e => { e.preventDefault(); setTplCtxMenu({ x: e.clientX, y: e.clientY }); }}
              title="Starter templates — right-click to dismiss"
              type="button"
            >
              ◈ Templates
            </button>
            {templatesOpen && (
              <div className="tb-templates-dropdown">
                <TemplatesPanel
                  activeProfile={activeProfile}
                  onImportTemplate={onImportTemplate}
                  onImportCadTemplate={onImportCadTemplate}
                />
              </div>
            )}
            {tplCtxMenu && (
              <div
                ref={tplCtxRef}
                className="tb-tpl-ctx-menu"
                style={{ top: tplCtxMenu.y, left: tplCtxMenu.x }}
              >
                <button className="tb-tpl-ctx-item" type="button" onClick={handleDismissTemplates}>
                  Don't show this again
                </button>
              </div>
            )}
          </div>
        )}
        {activeArea === 'mapping' && (
          <button
            className={`tb-list-toggle${listViewActive ? ' active' : ''}`}
            onClick={onToggleListView}
            title={listViewActive ? 'Switch to keyboard view' : 'Switch to list view'}
            data-drag="false"
            type="button"
          >
            {listViewActive ? '⌨' : '☰'}
          </button>
        )}
        <button
          className={`macro-toggle ${macrosEnabled ? 'enabled' : 'disabled'}`}
          onClick={onToggleMacros}
          title={macrosEnabled ? 'Macros Active — Click to Disable' : 'Macros Paused — Click to Enable'}
        >
          <span className="toggle-dot" />
          {macrosEnabled ? 'ACTIVE' : 'PAUSED'}
        </button>

        <button
          className="theme-toggle-btn"
          onClick={onToggleTheme}
          title={theme === 'dark' ? 'Switch to Light Mode' : 'Switch to Dark Mode'}
          data-drag="false"
        >
          {theme === 'dark' ? (
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none">
              <circle cx="12" cy="12" r="5" stroke="currentColor" strokeWidth="2"/>
              <line x1="12" y1="2"  x2="12" y2="5"  stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="12" y1="19" x2="12" y2="22" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="2"  y1="12" x2="5"  y2="12" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="19" y1="12" x2="22" y2="12" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="4.22"  y1="4.22"  x2="6.34"  y2="6.34"  stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="17.66" y1="17.66" x2="19.78" y2="19.78" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="19.78" y1="4.22"  x2="17.66" y2="6.34"  stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
              <line x1="6.34"  y1="17.66" x2="4.22"  y2="19.78" stroke="currentColor" strokeWidth="2" strokeLinecap="round"/>
            </svg>
          ) : (
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none">
              <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/>
            </svg>
          )}
        </button>

        <button
          className={`tb-settings-btn${settingsOpen ? ' active' : ''}`}
          onClick={onOpenSettings}
          title="Settings"
          data-drag="false"
        >
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="3"/>
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z"/>
          </svg>
        </button>

        <div className="window-controls">
          <button className="wc-btn minimize" onClick={handleMinimize}>
            <svg width="10" height="2" viewBox="0 0 10 2"><rect width="10" height="2" rx="1" fill="currentColor"/></svg>
          </button>
          <button className="wc-btn maximize" onClick={handleMaximize}>
            <svg width="10" height="10" viewBox="0 0 10 10"><rect x="1" y="1" width="8" height="8" rx="1.5" stroke="currentColor" strokeWidth="1.5" fill="none"/></svg>
          </button>
          <button className="wc-btn close" onClick={handleClose}>
            <svg width="10" height="10" viewBox="0 0 10 10">
              <line x1="1" y1="1" x2="9" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
              <line x1="9" y1="1" x2="1" y2="9" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round"/>
            </svg>
          </button>
        </div>
      </div>
    </div>
  );
}
