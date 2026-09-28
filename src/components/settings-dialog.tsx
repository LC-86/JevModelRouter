import { GatewaySettingsPanel } from './gateway-settings';
import type { DashboardSnapshot } from '../types';
import { Select } from './select';
import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Check, ExternalLink, Globe2, Info, Moon, Monitor, Palette, Settings2, Sun, X } from 'lucide-react';
import { isTauri } from '@tauri-apps/api/core';
import { openUrl } from '@tauri-apps/plugin-opener';
import { usePreferences } from '../lib/preferences-context';
import type { Accent, Language, Theme } from '../lib/preferences';
import { AppUpdate, type AppUpdateState } from './app-update';
import { license } from '../../package.json';

export type SettingsSection = 'general' | 'appearance' | 'gateway' | 'about';
const sections = [{ id: 'general', label: 'General', icon: Settings2 }, { id: 'appearance', label: 'Appearance', icon: Palette }, { id: 'gateway', label: 'Gateway and routing', icon: Settings2 }, { id: 'about', label: 'About', icon: Info }] as const;
const themes: { id: Theme; label: string; icon: typeof Sun }[] = [{ id: 'light', label: 'Light', icon: Sun }, { id: 'dark', label: 'Dark', icon: Moon }, { id: 'system', label: 'System', icon: Monitor }];
const accents: { id: Accent; label: string; color: string }[] = [{ id: 'orange', label: 'AutoJev orange', color: '#f26122' }, { id: 'blue', label: 'Ocean blue', color: '#3b82f6' }, { id: 'green', label: 'Forest green', color: '#239b72' }, { id: 'violet', label: 'Iris violet', color: '#9b72e8' }];
const links = [{ label: 'Website', url: 'https://autojev.ai' }, { label: 'Source code', url: 'https://github.com/thinkany-ai/autojev' }, { label: 'Feedback', url: 'https://github.com/thinkany-ai/autojev/issues' }];

export function SettingsDialog({ section, onSectionChange, onClose, update, snapshot, onChange }: { snapshot: DashboardSnapshot; onChange: (s: DashboardSnapshot) => void; section: SettingsSection; onSectionChange: (section: SettingsSection) => void; onClose: () => void; update: AppUpdateState }) {
  const { preferences, setPreferences, t, storageError } = usePreferences();
  const dialog = useRef<HTMLDialogElement>(null);
  const initialFocus = useRef<HTMLDivElement>(null);
  const [linkError, setLinkError] = useState(false);
  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement as HTMLElement | null;
    element.showModal();
    initialFocus.current?.focus({ preventScroll: true });
    return () => { element.close(); previous?.focus(); };
  }, []);

  return createPortal(<dialog ref={dialog} className="settings-dialog" aria-labelledby="settings-title" onCancel={(event) => { event.preventDefault(); onClose(); }} onClick={(event) => { if (event.target === event.currentTarget) { const box = event.currentTarget.getBoundingClientRect(); if (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom) onClose(); } }}>
    <div ref={initialFocus} tabIndex={-1} className="settings-layout">
      <aside className="settings-sidebar">
        <div className="settings-heading"><h2 id="settings-title">{t('Settings')}</h2></div>
        <nav aria-label={t('Settings sections')}>{sections.map(({ id, label, icon: Icon }) => <button key={id} className={`settings-nav-item ${section === id ? 'active' : ''}`} aria-current={section === id ? 'page' : undefined} onClick={() => onSectionChange(id)}><Icon size={17} />{t(label)}{id === 'about' && update.update && <span className="settings-update-dot" />}</button>)}</nav>
        <div className="settings-footnote">AutoJev <code>v{update.version}</code></div>
      </aside>
      <div className="settings-content">
        <button className="icon-action settings-close" aria-label={t('Close settings')} onClick={onClose}><X size={20} /></button>
        <div className="settings-body">
          {section === 'general' && <>
            <section className="settings-group"><h4><Globe2 size={16} />{t('Language')}</h4><div className="setting-row"><div><label htmlFor="settings-language">{t('Display language')}</label></div><Select id="settings-language" value={preferences.language} onChange={event => setPreferences({ language: event.target.value as Language })}><option value="system">{t('Follow system')}</option><option value="en">English</option><option value="zh-CN">简体中文</option></Select></div></section>
            <section className="settings-group"><h4>{t('Updates')}</h4><div className="setting-row"><div><strong id="auto-update-label">{t('Automatically check for updates')}</strong><p id="auto-update-description">{t('Check at startup and every four hours. You choose when to install.')}</p></div><button type="button" role="switch" aria-labelledby="auto-update-label" aria-describedby="auto-update-description" aria-checked={preferences.autoCheckUpdates} className={`switch ${preferences.autoCheckUpdates ? 'on' : ''}`} onClick={() => setPreferences({ autoCheckUpdates: !preferences.autoCheckUpdates })}><span /></button></div></section>
            <p className="settings-description">{t('Closing the window keeps the gateway running. Use the gateway stop button or Quit AutoJev in the tray menu to restore agent configurations.')}</p>

          </>}
          {section === 'gateway' && <GatewaySettingsPanel snapshot={snapshot} onChange={onChange} />}
          {section === 'appearance' && <>
            <section className="settings-group"><h4>{t('Color theme')}</h4><div className="theme-options" role="group" aria-label={t('Color theme')}>{themes.map(({ id, label, icon: Icon }) => <button key={id} className={`theme-option ${preferences.theme === id ? 'selected' : ''}`} aria-pressed={preferences.theme === id} onClick={() => setPreferences({ theme: id })}><div className={`theme-preview ${id}`}><i /><div><b /><span /><span /><span /></div></div><span><Icon size={15} />{t(label)}{preferences.theme === id && <Check size={14} />}</span></button>)}</div></section>
            <section className="settings-group"><h4>{t('Accent color')}</h4><div className="accent-options" role="group" aria-label={t('Accent color')}>{accents.map(({ id, label, color }) => <button key={id} className={`accent-option ${preferences.accent === id ? 'selected' : ''}`} aria-pressed={preferences.accent === id} onClick={() => setPreferences({ accent: id })}><span className="accent-swatch" style={{ background: color }}>{preferences.accent === id && <Check size={16} />}</span><span>{t(label)}</span></button>)}</div></section>

          </>}
          {section === 'about' && <>
            <div className="about-brand"><img src="/brand/app-icon.png" alt="AutoJev" /><div><h4>AutoJev</h4><p>{t('A local model router for AI agents.')}</p><span className="about-version">v{update.version}<span>{t(import.meta.env.DEV ? 'Development' : isTauri() ? 'Desktop' : 'Browser preview')}</span></span></div></div>
            <AppUpdate state={update} />
            <div className="about-links">{links.map(link => <a key={link.label} href={link.url} target="_blank" rel="noreferrer" onClick={event => { if (isTauri()) { event.preventDefault(); setLinkError(false); void openUrl(link.url).catch(() => setLinkError(true)); } }}>{t(link.label)}<ExternalLink size={14} /></a>)}</div>
            {linkError && <p role="alert" className="update-error">{t('Could not open the link. Please try again.')}</p>}
            <footer className="about-footer"><span>{t('{license} license', { license })}</span><span>© {new Date().getFullYear()} ThinkAny, LLC</span></footer>
          </>}
          {storageError && <p role="alert" className="update-error">{t('Preferences could not be saved. Changes will last for this session only.')}</p>}
        </div>
      </div>
    </div>
  </dialog>, document.body);
}
