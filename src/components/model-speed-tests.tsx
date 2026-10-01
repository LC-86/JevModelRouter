import { useCallback, useEffect, useRef, useState } from 'react';
import { CircleGauge, LoaderCircle } from 'lucide-react';
import type { Model, ModelPerformance, PerformanceSettings, PerformanceView } from '../types';
import { cancelModelSpeedTests, getModelPerformance, savePerformanceSettings, startModelSpeedTests } from '../lib/bridge';
import { usePreferences } from '../lib/preferences-context';
import { Select } from './select';

type BudgetSnapshotRefresh = () => Promise<unknown>;

export function useModelSpeedTests(onBudgetSnapshotRefresh?: BudgetSnapshotRefresh, pollingEnabled = true) {
  const [view, setView] = useState<PerformanceView | null>(null);
  const [selected, setSelected] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const lastPollError = useRef('');
  const budgetSnapshotRefresh = useRef(onBudgetSnapshotRefresh);
  budgetSnapshotRefresh.current = onBudgetSnapshotRefresh;
  const budgetMonitor = useRef<'starting' | 'running' | null>(null);
  const lastBudgetProgress = useRef('');
  const pollingEnabledRef = useRef(pollingEnabled);
  pollingEnabledRef.current = pollingEnabled;
  const pollTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pollInFlight = useRef(false);
  const mounted = useRef(true);
  const pollRef = useRef<() => Promise<void>>(async () => {});
  const refreshBudgetSnapshot = useCallback(async () => {
    try { await budgetSnapshotRefresh.current?.(); } catch { /* Keep speed-test status visible if a dashboard refresh fails. */ }
  }, []);
  const progress = (job: PerformanceView['job']) => JSON.stringify([job.running, job.completed, job.cancelled, job.error]);
  const refresh = useCallback(async () => setView(await getModelPerformance()), []);
  const poll = useCallback(async () => {
    if (pollTimer.current) { clearTimeout(pollTimer.current); pollTimer.current = null; }
    if (pollInFlight.current || (!pollingEnabledRef.current && !budgetMonitor.current)) return;
    pollInFlight.current = true;
    try {
      const next = await getModelPerformance();
      if (mounted.current) {
        setView(next); lastPollError.current = '';
        if (next.job.running && budgetMonitor.current !== 'running') {
          const starting = budgetMonitor.current === 'starting';
          budgetMonitor.current = 'running';
          lastBudgetProgress.current = progress(next.job);
          if (!starting) await refreshBudgetSnapshot();
        } else if (budgetMonitor.current === 'running') {
          const nextProgress = progress(next.job);
          if (nextProgress !== lastBudgetProgress.current) {
            lastBudgetProgress.current = nextProgress;
            await refreshBudgetSnapshot();
          }
          if (!next.job.running) budgetMonitor.current = null;
        }
      }
    } catch (e) {
      if (mounted.current) { const message = e instanceof Error ? e.message : String(e); if (lastPollError.current !== message) setError(message); lastPollError.current = message; }
    } finally {
      pollInFlight.current = false;
      if (mounted.current && (pollingEnabledRef.current || budgetMonitor.current)) {
        pollTimer.current = setTimeout(() => void pollRef.current(), budgetMonitor.current ? 350 : 2000);
      }
    }
  }, [refreshBudgetSnapshot]);
  pollRef.current = poll;
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; if (pollTimer.current) clearTimeout(pollTimer.current); };
  }, []);
  useEffect(() => {
    if (pollingEnabled || budgetMonitor.current) void poll();
    else if (pollTimer.current) { clearTimeout(pollTimer.current); pollTimer.current = null; }
  }, [pollingEnabled, poll]);
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
      setBusy(true); setError(''); budgetMonitor.current = 'starting'; lastBudgetProgress.current = '';
      return (async () => {
        try {
          await startModelSpeedTests(ids);
          const next = await getModelPerformance(); setView(next);
          await refreshBudgetSnapshot();
          if (next.job.running) {
            budgetMonitor.current = 'running';
            lastBudgetProgress.current = progress(next.job);
          } else budgetMonitor.current = null;
        } catch (e) {
          setError(e instanceof Error ? e.message : String(e));
          await refreshBudgetSnapshot();
          budgetMonitor.current = null;
        } finally { setBusy(false); }
      })();
    },
    cancel: () => act(async () => {
      try { await cancelModelSpeedTests(); }
      finally { await refreshBudgetSnapshot(); }
    }),
    save: (settings: PerformanceSettings) => act(() => savePerformanceSettings(settings)),
  };
}
export type ModelSpeedTests = ReturnType<typeof useModelSpeedTests>;

export function SpeedTestToolbar({ tests, models }: { tests: ModelSpeedTests; models: Model[] }) {
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

export function SpeedTestSettings({ tests }: { tests: ModelSpeedTests }) {
  const { t } = usePreferences();
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
