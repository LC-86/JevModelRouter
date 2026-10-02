import { invoke } from '@tauri-apps/api/core';
import { useEffect, useState } from 'react';
import type { GrokReadOnlyStatus } from '../types';
import type { Translate } from '../lib/preferences-context';

const CLOSED_STATUS: GrokReadOnlyStatus = {
  state: 'blocked_unverified_source',
  sourceVerified: false,
  sourceVersion: null,
  sourceCommit: null,
  authInfo: 'unknown',
  modelCatalog: 'unknown',
  billing: 'unknown',
  autoTopupRule: 'unknown',
  extraUsagePermission: 'unknown',
  realGenerationEnabled: false,
};

/** Shows the production source gate; it never initiates an ACP read or helper process. */
export function GrokReadOnlyStatus({ t }: { t: Translate }) {
  const [status, setStatus] = useState(CLOSED_STATUS);

  useEffect(() => {
    let active = true;
    if (!('__TAURI_INTERNALS__' in window)) return () => { active = false; };
    void invoke<GrokReadOnlyStatus>('grok_readonly_status')
      .then((value) => { if (active) setStatus(value); })
      .catch(() => { /* Keep the fail-closed Unknown display if the status command is unavailable. */ });
    return () => { active = false; };
  }, []);

  return <section className="grok-readonly-status" data-testid="grok-readonly-status" aria-label={t('Grok read-only status')}>
    <header>
      <strong>{t('Grok read-only status')}</strong>
      <span data-testid="grok-readonly-gate">{t('Source/version verification required')}</span>
    </header>
    <p>{t('No Grok ACP request was sent. This app build has no verified public source pin for the Grok helper, so account identity, model catalog, billing, auto top-up, and Extra Usage remain Unknown; real generation stays off.')}</p>
    <dl>
      <div><dt>{t('Account identity')}</dt><dd data-testid="grok-readonly-auth">{t(status.authInfo === 'unknown' ? 'Unknown' : 'Available')}</dd></div>
      <div><dt>{t('Model catalog')}</dt><dd data-testid="grok-readonly-models">{t(status.modelCatalog === 'unknown' ? 'Unknown' : 'Available')}</dd></div>
      <div><dt>{t('Subscription billing')}</dt><dd data-testid="grok-readonly-billing">{t(status.billing === 'unknown' ? 'Unknown' : 'Available')}</dd></div>
      <div><dt>{t('Auto top-up rule')}</dt><dd data-testid="grok-readonly-topup">{t(status.autoTopupRule === 'unknown' ? 'Unknown' : 'Available')}</dd></div>
      <div><dt>{t('Extra Usage permission')}</dt><dd data-testid="grok-readonly-extra-usage">{t(status.extraUsagePermission === 'unknown' ? 'Unknown' : 'Available')}</dd></div>
      <div><dt>{t('Real generation')}</dt><dd data-testid="grok-readonly-generation">{t(status.realGenerationEnabled ? 'Enabled' : 'Off')}</dd></div>
    </dl>
  </section>;
}
