import { DebugDetailButton } from './debug-detail-dialog';
import type { DebugMessage } from '../lib/debug-message';
import { usePreferences } from '../lib/preferences-context';

export function DebugMessageDetails({ message }: { message: DebugMessage }) {
  const { t } = usePreferences();
  const result = message.result;
  const log = result?.telemetry;
  const unknown = t('Not available');
  const source = log?.source || result?.source;
  const sourceLabel = source === 'jev' ? 'Jev' : source === 'local' ? t('Local selection') : source || unknown;
  return <div className="debug-message-details">
    {result && <div className="debug-message-metrics">
      <span className="debug-response-model" title={log?.model_id || result.model || unknown}>{t('Model')}: {log?.model_id || result.model || unknown}</span>
      <span>{t('Duration')}: {(result.elapsed_ms / 1000).toFixed(2)} s</span>
      <span title={t('Input / output tokens')}>Token: {log?.input_tokens ?? '—'} / {log?.output_tokens ?? '—'}</span>
      <span title={t('Cost uses the prices configured when the request was made. Missing usage or cache-write pricing is shown as unknown.')}>{t('Cost')}: {log?.estimated_cost != null ? '$' + log.estimated_cost.toFixed(6) : unknown}</span>
    </div>}
    <div className="debug-detail-actions">
    {result && <DebugDetailButton title={t('Request and routing details')}>
      <dl className="debug-detail-grid">
        <dt>HTTP</dt><dd>{result.status}</dd>
        <dt>{t('Model / route')}</dt><dd>{message.request?.target || unknown}</dd>
        <dt>{t('Provider')}</dt><dd>{log?.provider_name || unknown}</dd>
        <dt>{t('Model')}</dt><dd>{log?.model_id || result.model || unknown}</dd>
        <dt>{t('Routing source')}</dt><dd>{sourceLabel}</dd>
        <dt>{t('Decision reason')}</dt><dd>{log?.reason ? t(log.reason) : unknown}</dd>
        <dt>{t('First byte')}</dt><dd>{log?.first_byte_ms != null ? log.first_byte_ms + ' ms' : unknown}</dd>
        <dt>{t('Cache read tokens')}</dt><dd>{log ? log.cache_read_tokens : unknown}</dd>
        <dt>{t('Cache write tokens')}</dt><dd>{log ? log.cache_write_tokens : unknown}</dd>
        <dt>{t('API type')}</dt><dd>{message.request?.endpoint || unknown}</dd>
        <dt>{t('Parameters')}</dt><dd><code>{JSON.stringify(message.request?.parameters ?? {})}</code></dd>
      </dl>
      {!!log?.attempts?.length && <div className="debug-attempts"><strong>{t('Upstream attempts')}</strong>{log.attempts.map((attempt, index) => <p key={index}>{index + 1}. {attempt.provider} / {attempt.model} · HTTP {attempt.status} · {attempt.duration_ms} ms</p>)}</div>}
    </DebugDetailButton>}
    {message.raw && <DebugDetailButton title={t('Raw JSON response')}><pre className="debug-response">{message.raw}</pre></DebugDetailButton>}
    </div>
  </div>;
}
