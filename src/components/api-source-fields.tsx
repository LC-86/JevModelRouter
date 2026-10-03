import type { ApiSourceDraft, ApiSourceKind } from '../types';
import { usePreferences } from '../lib/preferences-context';

export const API_SOURCE_LABELS: Record<ApiSourceKind, string> = {
  official_api: 'Official API', third_party_api: 'Third-party API', coding_plan: 'Coding Plan (Key)',
};

export function ApiSourceFields({ source, locked, onChange }: {
  source?: ApiSourceDraft; locked: boolean; onChange: (source: ApiSourceDraft | undefined) => void;
}) {
  const { t } = usePreferences();
  return <div data-testid="api-source-fields">
    <label className="form-field"><span>{t('Source category')}</span>
      <select data-testid="api-source-kind" disabled={locked} value={source?.kind || ''} onChange={event => onChange(event.target.value ? { ...source, kind: event.target.value as ApiSourceKind } : undefined)}>
        <option value="">{t('Legacy API (unclassified)')}</option>
        {Object.entries(API_SOURCE_LABELS).map(([value, label]) => <option key={value} value={value}>{t(label)}</option>)}
      </select>
    </label>
    {source && <div className="field-pair">
      <label className="form-field"><span>{t('Account label (user declared)')}</span><input data-testid="api-source-account" autoComplete="off" disabled={locked} maxLength={160} value={source.account_label || ''} placeholder={t('Unknown')} onChange={event => onChange({ ...source, account_label: event.target.value || null })}/></label>
      <label className="form-field"><span>{t('Plan label (user declared)')}</span><input data-testid="api-source-plan" autoComplete="off" disabled={locked} maxLength={160} value={source.plan_label || ''} placeholder={t('Unknown')} onChange={event => onChange({ ...source, plan_label: event.target.value || null })}/></label>
    </div>}
    <p className="provider-test-help">{t('A key or model entry does not verify a plan. Fees and remaining allowance are unknown.')}</p>
    {source?.kind === 'coding_plan' && <p className="provider-test-help">{t('Enter the Coding Plan protocol base including its plan path. It is separate from the ordinary API endpoint.')}</p>}
    {locked && <p className="provider-test-help">{t('Create a new connection to change the endpoint, account, plan, protocol or credential. Existing targets keep their identity.')}</p>}
  </div>;
}
