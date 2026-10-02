import { invoke } from '@tauri-apps/api/core';
import { useEffect, useRef, useState } from 'react';
import type { Translate } from '../lib/preferences-context';
import { acceptGrokObservation, fieldText, observationExpired, type GrokObservation } from '../lib/grok-readonly';

/** Manual, volatile observation of the official CLI's current account. Never arms generation. */
export function GrokReadOnlyStatus({ t }: { t: Translate }) {
  const [observation, setObservation] = useState<GrokObservation | null>(null);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState('');
  const [now, setNow] = useState(Date.now());
  const request = useRef<string | null>(null);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      clearInterval(timer);
      const id = request.current;
      request.current = null;
      if (id) void invoke('cancel_grok_readonly', { requestId: id });
    };
  }, []);
  const refresh = async () => {
    if (request.current) return;
    const id = crypto.randomUUID();
    request.current = id;
    setPending(true); setError('');
    // A failed new observation must not leave previous account values looking current.
    setObservation(null);
    try {
      const value = await invoke<GrokObservation>('refresh_grok_readonly', { requestId: id });
      const accepted = acceptGrokObservation(id, request.current, value);
      if (accepted) { setObservation(accepted); setNow(Date.now()); }
    } catch (failure) {
      if (request.current === id) setError(String(failure));
    } finally {
      if (request.current === id) { request.current = null; setPending(false); }
    }
  };
  const cancel = async () => {
    const id = request.current;
    if (!id) return;
    // Keep the backend busy until its child is reaped; discard the eventual result immediately.
    request.current = null;
    setObservation(null); setError(t('Read cancelled; values are Unknown.'));
    try { await invoke('cancel_grok_readonly', { requestId: id }); }
    finally { setPending(false); }
  };
  const unknown = t('Unknown');
  const expired = observation ? observationExpired(observation, now) : false;
  const current = expired ? null : observation;
  const text = (value: string) => value;
  const time = (value: string) => new Date(value).toLocaleString();
  return <section className="grok-readonly-status" data-testid="grok-readonly-status" aria-label={t('Grok read-only status')}>
    <header>
      <strong>{t('Grok read-only status')}</strong>
      <span data-testid="grok-readonly-gate">{t('Manual observation · generation off')}</span>
      <button type="button" className="button ghost small" data-testid="grok-readonly-refresh" disabled={pending} onClick={() => void refresh()}>{t(pending ? 'Reading…' : 'Refresh models and usage')}</button>
      {pending && <button type="button" className="button ghost small" data-testid="grok-readonly-cancel" onClick={() => void cancel()}>{t('Cancel')}</button>}
    </header>
    <p>{t('Reads the current account already signed in to the official Grok CLI. Model discovery and usage do not prove subscription eligibility or permission for extra fees. Real generation stays off.')}</p>
    {error && <p role="alert" data-testid="grok-readonly-error">{error}</p>}
    <p role="status" data-testid="grok-readonly-updated">{t('Last updated')} · {observation ? time(observation.observedAt) : unknown}{expired ? ` · ${t('Expired; refresh to read current values.')}` : ''}</p>
    <dl>
      <div><dt>{t('Account identity')}</dt><dd data-testid="grok-readonly-auth">{unknown}</dd></div>
      <div><dt>{t('Model catalog')}</dt><dd data-testid="grok-readonly-models">{fieldText(current?.models, (models) => models.length ? models.join(' · ') : t('Empty catalog'), unknown)}</dd></div>
      <div><dt>{t('Current CLI model')}</dt><dd>{fieldText(current?.currentModel, text, unknown)}</dd></div>
      <div><dt>{t('Subscription billing')}</dt><dd data-testid="grok-readonly-billing">{fieldText(current?.usagePercent, (value) => `${value}% ${t('used')}`, unknown)} · {fieldText(current?.remainingPercent, (value) => `${value}% ${t('remaining (calculated)')}`, unknown)}</dd></div>
      <div><dt>{t('Subscription plan')}</dt><dd>{fieldText(current?.subscriptionTier, text, unknown)}</dd></div>
      <div><dt>{t('Usage period')}</dt><dd>{fieldText(current?.periodType, text, unknown)} · {fieldText(current?.periodStart, time, unknown)}</dd></div>
      <div><dt>{t('Period end / estimated reset')}</dt><dd data-testid="grok-readonly-period-end">{fieldText(current?.periodEnd, time, unknown)}{current?.periodConflict ? ` · ${t('Usage and billing period ends conflict.')}` : ''}</dd></div>
      <div><dt>{t('Billing period end')}</dt><dd>{fieldText(current?.billingPeriodEnd, time, unknown)}</dd></div>
      <div><dt>{t('Auto top-up rule')}</dt><dd data-testid="grok-readonly-topup">{unknown}</dd></div>
      <div><dt>{t('Extra Usage permission')}</dt><dd data-testid="grok-readonly-extra-usage">{unknown}</dd></div>
      <div><dt>{t('Real generation')}</dt><dd data-testid="grok-readonly-generation">{t('Off')}</dd></div>
    </dl>
  </section>;
}
