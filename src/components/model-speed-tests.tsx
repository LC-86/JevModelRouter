import { useCallback, useEffect, useRef, useState } from 'react';
import { CircleGauge, LoaderCircle } from 'lucide-react';
import type { Model, ModelPerformance, PerformanceSettings, PerformanceView } from '../types';
import { cancelModelSpeedTests, getModelPerformance, savePerformanceSettings, startModelSpeedTests } from '../lib/bridge';
import { usePreferences } from '../lib/preferences-context';
import { Select } from './select';

export function useModelSpeedTests() {
  const [view, setView] = useState<PerformanceView | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const lastPollError = useRef('');
  const refresh = useCallback(async () => setView(await getModelPerformance()), []);
  useEffect(() => {
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try { const next = await getModelPerformance(); if (!stopped) { setView(next); lastPollError.current = ''; } }
      catch (e) { if (!stopped) { const message = e instanceof Error ? e.message : String(e); if (lastPollError.current !== message) setError(message); lastPollError.current = message; } }
      if (!stopped) timer = setTimeout(() => void poll(), 2000);
    };
    void poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, []);
  const act = async (work: () => Promise<void>) => {
    setBusy(true); setError('');
    try { await work(); await refresh(); }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  return { view, selected, busy, error,
    dismissError: () => setError(''),
    toggle: (id: string) => setSelected(ids => ids.includes(id) ? ids.filter(i => i !== id) : [...ids, id]),
    select: setSelected,
    start: (ids: string[]) => {
      if (!ids.length) { setError('Select models before running a speed test.'); return Promise.resolve(); }
      return act(() => startModelSpeedTests(ids));
    },
    cancel: () => act(cancelModelSpeedTests),
    save: (settings: PerformanceSettings) => act(() => savePerformanceSettings(settings)),
  };
}
export function SpeedTestToolbar({ tests, models }: { tests: ReturnType<typeof useModelSpeedTests>; models: Model[] }) {
  const { t } = usePreferences();
  const selected = models.filter(m => tests.selected.includes(m.id));
  const running = tests.view?.job.running ?? false;
  const job = tests.view?.job;
  return <div className="model-speed-actions">
    {job && job.total > 0 && <span role="status" className="model-speed-count">
      {running && <LoaderCircle size={14} className="import-spinner"/>}
      {t(job.cancelled ? 'Stopped after {completed} of {total} requests.' : 'Requests {completed} of {total}; models {completedModels} of {totalModels}.', {
        completed: job.completed, total: job.total, completedModels: job.completed_models, totalModels: job.total_models,
      })}
    </span>}
    {running
      ? <button className="button ghost" disabled={tests.busy || job?.cancelled} onClick={() => void tests.cancel()}>{t(job?.cancelled ? 'Stopping…' : 'Stop speed tests')}</button>
      : <button className="button ghost" disabled={tests.busy} onClick={() => void tests.start(selected.map(m => m.id))}><CircleGauge size={16}/>{t(selected.length ? 'Test selected' : 'Speed test')}{selected.length ? ` (${selected.length})` : ''}</button>}
    <small className="model-speed-help">{t('Manual speed tests send three streaming requests per model, one at a time. API requests ask for at most 24 output tokens; subscription responses are capped at 64 KiB. Stopping cancels unfinished requests. Calls may use API or subscription allowance. Automatic speed tests skip subscription models.')}</small>
    {job?.error && <small role="status" className="model-speed-help">{job.error}</small>}
  </div>;
}

export function SpeedTestSettings() {
  const { t } = usePreferences();
  const tests = useModelSpeedTests();
  const settings = tests.view?.settings;
  return <section className="settings-group speed-test-settings">
    <div className="setting-row"><div><strong id="auto-speed-label">{t('Automatic speed tests')}</strong><p>{t('Retest API models without recent measurements while the app is running. Subscription models are skipped.')}</p></div><button type="button" role="switch" aria-labelledby="auto-speed-label" aria-checked={settings?.enabled ?? true} disabled={tests.busy || !settings} className={`switch ${settings?.enabled ? 'on' : ''}`} onClick={() => settings && void tests.save({ ...settings, enabled: !settings.enabled })}><span/></button></div>
    <div className="setting-row"><label htmlFor="speed-test-interval">{t('Test interval')}</label><Select id="speed-test-interval" aria-label={t('Test interval')} searchable={false} disabled={tests.busy || !settings || !settings.enabled} value={String(settings?.interval_minutes ?? 30)} onChange={e => settings && void tests.save({ ...settings, interval_minutes: Number(e.target.value) })}>{[5,15,30,60,120].map(n => <option key={n} value={String(n)}>{`${n} ${t('minutes')}`}</option>)}</Select></div>
    {tests.error && <p role="alert" className="route-error">{t(tests.error)}</p>}
  </section>;
}

export function SpeedCell({ result, running }: { result?: ModelPerformance; running: boolean }) {
  const { t } = usePreferences();
  if (running) return <span className="provider-connection testing"><LoaderCircle size={14} className="import-spinner"/>{t('Testing…')}</span>;
  if (!result?.last_test_at) return <span className="muted" title={t('Not measured')}>—</span>;
  const detail = `${t('First content')} · ${t('Samples')}: ${result.samples} · ${t('Success rate')}: ${Math.round(result.success_rate * 100)}% · ${new Date(result.last_test_at).toLocaleString()}`;
  return <span className="model-speed-cell" title={detail} tabIndex={0}>
    {result.stale ? t('Measurements expired') : result.first_content_ms === null ? '—' : <><span title={t('Time until the first content arrives. Lower is faster.')}>{Math.round(result.first_content_ms)} ms</span><small> / </small><span title={t(result.tokens_per_second === null ? 'Complete token usage is unavailable, so output speed cannot be calculated.' : 'Output tokens per second after the first content. Higher is faster.')}>{result.tokens_per_second === null ? '—' : `${result.tokens_per_second.toFixed(1)} tok/s`}</span></>}
  </span>;
}
