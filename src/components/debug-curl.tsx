import { DebugDetailButton } from './debug-detail-dialog';
import { useEffect, useRef, useState } from 'react';
import { Copy, Play, Square, LoaderCircle } from 'lucide-react';
import { cancelDebugCurl, executeDebugCurl, type CurlResult } from '../lib/bridge';
import { parseCurl } from '../lib/debug-curl';
import { usePreferences } from '../lib/preferences-context';

export function DebugCurl({ initial, port, onRefreshBudgetSnapshot }: { initial: string; port: number; onRefreshBudgetSnapshot: () => Promise<unknown> }) {
  const { t } = usePreferences();
  const [command, setCommand] = useState(initial);
  const [output, setOutput] = useState('');
  const [result, setResult] = useState<CurlResult | null>(null);
  const [status, setStatus] = useState<number>();
  const [headers, setHeaders] = useState<Record<string, string>>({});
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [started, setStarted] = useState(false);
  const [copied, setCopied] = useState(false);
  const [formatted, setFormatted] = useState(true);
  const active = useRef<string | null>(null);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; if (active.current) void cancelDebugCurl(active.current); }; }, []);
  const run = async () => {
    if (active.current) return;
    let request;
    try { request = parseCurl(command, port); } catch (e) { setError(String(e)); return; }
    const id = crypto.randomUUID(); active.current = id;
    setBusy(true); setStarted(false); setOutput(''); setResult(null); setStatus(undefined); setHeaders({}); setError('');
    const decoder = new TextDecoder();
    try {
      const reply = await executeDebugCurl(id, request, event => {
        if (!mounted.current) { if (event.started) void cancelDebugCurl(id); return; }
        if (event.started) setStarted(true);
        if (event.status) setStatus(event.status);
        if (event.headers) setHeaders(event.headers);
        if (event.bytes) { const text = decoder.decode(new Uint8Array(event.bytes), { stream: true }); setOutput(previous => previous + text); }
      });
      if (mounted.current) { setResult(reply); setOutput(reply.body); }
    } catch (e) { if (mounted.current) { setOutput(previous => previous + decoder.decode()); setError(t(String(e))); } }
    finally {
      active.current = null;
      if (mounted.current) { setBusy(false); setStarted(false); }
      try { await onRefreshBudgetSnapshot(); } catch { /* Keep the cURL result visible. */ }
    }
  };
  let display = output;
  if (formatted) { try { display = JSON.stringify(JSON.parse(output), null, 2); } catch { /* SSE and non-JSON remain raw. */ } }
  return <div className="curl-layout">
    <section className="panel curl-panel">
      <div className="debug-panel-heading"><h3>{t('cURL request')}</h3><button className="button ghost small" onClick={async () => { try { await navigator.clipboard.writeText(command); setCopied(true); } catch { setError(t('Could not copy request')); } }}><Copy size={14}/>{t(copied ? 'Copied' : 'Copy request')}</button></div>
      <textarea wrap="off" className="curl-editor" aria-label={t('cURL request')} spellCheck={false} autoCapitalize="none" value={command} disabled={busy} onChange={e => { setCommand(e.target.value); setCopied(false); }}/>
      <div className="curl-toolbar">{busy ? <button className="button ghost" disabled={!started} onClick={() => { if (active.current) void cancelDebugCurl(active.current).finally(() => onRefreshBudgetSnapshot().catch(() => {})).catch(e => setError(String(e))); }}><Square size={14}/>{t('Stop')}</button> : <button className="button primary" onClick={() => void run()} disabled={!command.trim()}><Play size={14}/>{t('Execute')}</button>}</div>
    </section>
    <section className="panel curl-panel">
      <div className="debug-panel-heading"><h3>{t('cURL output')}</h3><label className="curl-format"><input type="checkbox" checked={formatted} onChange={e => setFormatted(e.target.checked)}/>{t('Format JSON')}</label></div>

      {error && <div className="curl-error route-error" role="alert">{error}</div>}
      <pre className="curl-output" aria-label={t('cURL output')}>{display || t(busy ? 'Waiting for response…' : 'Execute a request to inspect the response.')}</pre>
      <div className="curl-toolbar curl-result-toolbar">
        <div className="curl-metadata" aria-live="polite">{busy && <LoaderCircle size={14} className="import-spinner"/>}{status !== undefined && <span>HTTP {status}</span>}{result && <><span>{(result.elapsed_ms / 1000).toFixed(2)} s</span><span>Token: {result.telemetry?.input_tokens?.toLocaleString() ?? '—'} / {result.telemetry?.output_tokens?.toLocaleString() ?? '—'}</span><span>{result.telemetry?.estimated_cost == null ? '—' : '$' + result.telemetry.estimated_cost.toFixed(6)}</span></>}</div>
        {!!Object.keys(headers).length && <DebugDetailButton title={t('Response headers')}><pre className="debug-response">{Object.entries(headers).map(([key, value]) => `${key}: ${value}`).join('\n')}</pre></DebugDetailButton>}
      </div>
    </section>
  </div>;
}
