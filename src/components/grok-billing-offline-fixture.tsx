import { invoke } from '@tauri-apps/api/core';
import { useEffect, useState } from 'react';
import type { BillingObserved, GrokBillingOfflineFixture } from '../types';
import type { Translate } from '../lib/preferences-context';

function observed<T>(field: BillingObserved<T> | undefined, t: Translate): string {
  if (!field) return t('Unknown');
  if (field.state !== 'available') return `${t('Unknown')} (${field.state})`;
  if (field.value === null) return t('Unknown');
  return String(field.value);
}

function cent(field: BillingObserved<{ val: BillingObserved<number> }> | undefined, t: Translate): string {
  if (!field || field.state !== 'available' || !field.value) return observed(field, t);
  const value = field.value.val;
  return value.state === 'available' && value.value !== null
    ? `${value.value} ${t('raw cents')}`
    : `${t('Unknown')} (${value.state})`;
}

/** Visible only in the explicit native isolation-check build; it never reads provider or auth state. */
export function GrokBillingOfflineFixture({ t }: { t: Translate }) {
  const [fixture, setFixture] = useState<GrokBillingOfflineFixture | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    let requested = false;
    const load = () => {
      if (requested || !window.__ISOLATION_CHECK__ || !('__TAURI_INTERNALS__' in window)) return;
      requested = true;
      void invoke<GrokBillingOfflineFixture>('grok_billing_offline_fixture').then((value) => {
        if (active) setFixture(value);
      }).catch((error: unknown) => {
        if (active) setLoadError(error instanceof Error ? error.message : String(error));
      });
    };
    window.addEventListener('autojev-isolation-ready', load);
    load();
    return () => {
      active = false;
      window.removeEventListener('autojev-isolation-ready', load);
    };
  }, []);

  if (!fixture && !loadError) return null;
  const billing = fixture?.billing.value;
  const configField = billing?.config;
  const config = configField?.state === 'available' ? configField.value : null;
  const currentPeriodField = config?.currentPeriod;
  const currentPeriod = currentPeriodField?.state === 'available' ? currentPeriodField.value : null;
  const topupState = fixture?.autoTopup.state;
  const historyText = config?.history.state === 'available' && config.history.value !== null
    ? `${config.history.value.length} ${t('synthetic history entries')}`
    : `${t('Unknown')} (${config?.history.state ?? 'missing'})`;

  return <section className="grok-billing-offline-fixture" data-testid="grok-billing-offline-fixture" aria-label={t('Offline Grok billing fixture')}>
    <header>
      <strong>{t('Offline Grok billing fixture')}</strong>
      <span>{fixture?.mode ?? t('Fixture unavailable')}</span>
    </header>
    <p>{t('Synthetic data only. No account, credential, ACP request, quota grant, or generation eligibility is read or changed.')}</p>
    {loadError && <p role="alert">{t('Fixture parser command failed')}: {loadError}</p>}
    {fixture && <>
      <p>{t('Pinned parser source')}: <code>{fixture.sourceRevision}</code></p>
      <dl>
        <div><dt>{t('Subscription tier')}</dt><dd>{observed(billing?.subscriptionTier, t)}</dd></div>
        <div><dt>{t('On-demand setting')}</dt><dd>{observed(billing?.onDemandEnabled, t)}</dd></div>
        <div><dt>{t('Usage percent')}</dt><dd>{observed(billing?.usagePercent, t)}{billing?.usagePercent.origin ? ` · ${billing.usagePercent.origin}` : ''}</dd></div>
        <div><dt>{t('Period type')}</dt><dd>{observed(currentPeriod?.periodType, t)}</dd></div>
        <div><dt>{t('Period start / end')}</dt><dd>{observed(currentPeriod?.start, t)} / {observed(currentPeriod?.end, t)}</dd></div>
        <div><dt>{t('Included / on-demand / total used')}</dt><dd>{cent(currentPeriod?.includedUsed, t)} / {cent(currentPeriod?.onDemandUsed, t)} / {cent(currentPeriod?.totalUsed, t)}</dd></div>
        <div><dt>{t('Legacy used / monthly limit')}</dt><dd>{cent(config?.used, t)} / {cent(config?.monthlyLimit, t)}</dd></div>
        <div><dt>{t('On-demand cap / used')}</dt><dd>{cent(config?.onDemandCap, t)} / {cent(config?.onDemandUsed, t)}</dd></div>
        <div><dt>{t('Prepaid balance')}</dt><dd>{cent(config?.prepaidBalance, t)}</dd></div>
        <div><dt>{t('Unified billing user')}</dt><dd>{observed(config?.isUnifiedBillingUser, t)}</dd></div>
        <div><dt>{t('Billing period start / end')}</dt><dd>{observed(config?.billingPeriodStart, t)} / {observed(config?.billingPeriodEnd, t)}</dd></div>
        <div><dt>{t('History')}</dt><dd>{historyText}</dd></div>
        <div><dt>{t('Auto top-up read')}</dt><dd>{topupState === 'sample_failed' ? `${t('Synthetic partial read failure')} (${fixture.autoTopup.failureCode})` : t('Unknown')}</dd></div>
      </dl>
      <p data-testid="grok-billing-scope-replay">{t('Scope guard')}: {fixture.scopeReplay.currentResponseAccepted ? t('matching response accepted') : t('matching response rejected')} · {fixture.scopeReplay.responseAfterAccountSwitchRejected ? t('old account-generation response rejected') : t('old account-generation response accepted')} · {fixture.scopeReplay.responseAfterConnectionSwitchRejected ? t('old connection response rejected') : t('old connection response accepted')}</p>
      <p>{t('The sample is not quota evidence. Production subscription and Extra Usage permissions remain Unknown; generation stays denied.')}</p>
    </>}
  </section>;
}
