import { useCallback, useEffect, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { appUpdater, isUnsupportedUpdatePlatformError } from '../lib/updater';
import { usePreferences } from '../lib/preferences-context';
import { version as packageVersion } from '../../package.json';

type Status = 'idle' | 'checking' | 'current' | 'available' | 'installing' | 'installed' | 'error';
export function useAppUpdate() {
  const { preferences } = usePreferences();
  const [version, setVersion] = useState(packageVersion);
  const [status, setStatus] = useState<Status>(appUpdater.installed ? 'installed' : 'idle');
  const [update, setUpdate] = useState<{ version: string; notes?: string } | null>(null);
  const [progress, setProgress] = useState<number | null>(null);
  const [error, setError] = useState('');
  const enabled = isTauri() && !import.meta.env.DEV;
  const check = useCallback(async () => {
    if (!enabled || appUpdater.isBusy || appUpdater.installed) return;
    setStatus('checking');
    setError('');
    try {
      const result = await appUpdater.check();
      if (result === undefined) return;
      setUpdate(result);
      setStatus(result ? 'available' : 'current');
    } catch (error) {
      setError(isUnsupportedUpdatePlatformError(error) ? 'This release does not include an update for your platform yet.' : 'Could not check for updates. Check your connection and try again.');
      setStatus('error');
    }
  }, [enabled]);

  useEffect(() => { if (isTauri()) void getVersion().then(setVersion).catch(() => {}); }, []);
  useEffect(() => {
    if (!enabled || !preferences.autoCheckUpdates) return;
    const timer = setTimeout(() => { void check(); }, 3000);
    const interval = setInterval(() => { void check(); }, 4 * 60 * 60 * 1000);
    return () => { clearTimeout(timer); clearInterval(interval); };
  }, [enabled, preferences.autoCheckUpdates, check]);

  async function install() {
    if (!enabled || appUpdater.isBusy) return;
    setStatus('installing'); setError(''); setProgress(null);
    try { await invoke('stop_proxy'); await appUpdater.install(setProgress); setStatus('installed'); }
    catch { setError('Update installation failed. You can retry the download.'); setStatus('available'); }
  }
  async function restart() {
    try { await invoke('stop_proxy'); await appUpdater.relaunch(); }
    catch { setError('Could not restart. Quit and reopen AutoJev to finish updating.'); }
  }
  return { version, status, update, progress, error, enabled, check, install, restart };
}
export type AppUpdateState = ReturnType<typeof useAppUpdate>;

export function AppUpdate({ state }: { state: AppUpdateState }) {
  const { t } = usePreferences();
  const { enabled, status, update, progress, error } = state;
  if (!enabled) return <div className="update-card"><strong>{t('Software update')}</strong><p>{t(import.meta.env.DEV ? 'Development build — updates are available in the installed release.' : 'Browser preview — updates are available in the desktop app.')}</p></div>;
  const message = status === 'checking' ? t('Checking for updates…') : status === 'current' ? t('You’re using the latest version.') : status === 'installed' ? t('Restart AutoJev when your agents have finished their requests.') : status === 'installing' ? progress === null ? t('Downloading and installing…') : t('Downloading and installing… {percent}%', { percent: progress }) : update ? t('Finish active agent requests before installing. AutoJev may close during installation.') : t('Get the latest improvements to AutoJev.');
  return <div className="update-card">
    <strong>{status === 'installed' ? t('Update installed') : update ? t('Version {version} is available', { version: update.version }) : t('Software update')}</strong>
    <p aria-live="polite">{message}</p>
    {status === 'installing' && <progress aria-label={t('Installation progress')} max={100} value={progress ?? undefined} />}
    {update?.notes && <pre className="update-notes">{update.notes}</pre>}
    {error && <p role="alert" className="update-error">{t(error)}</p>}
    <button className="button ghost" disabled={status === 'checking' || status === 'installing'} onClick={() => void (status === 'installed' ? state.restart() : update ? state.install() : state.check())}>
      {status === 'installed' ? t('Restart AutoJev') : status === 'installing' ? t('Installing…') : update ? t('Install update') : status === 'checking' ? t('Checking…') : t('Check for updates')}
    </button>
  </div>;
}
