import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Activity, ChevronDown, ChevronLeft, ChevronRight, RefreshCw, Search } from 'lucide-react';
import { getRequestLogs } from '../lib/bridge';
import { requestRoute, cacheHitRate, dailyUsage, rangeStart, summarize, totalTokens, usageGroups } from '../lib/traffic';
import { usePreferences } from '../lib/preferences-context';
import type { RequestLog } from '../types';
import { Select } from './select';

const money = (value: number | null) => value === null ? '—' : `$${value.toFixed(6)}`;
const elapsed = (ms: number | null) => ms === null ? '—' : ms < 1000 ? `${ms} ms` : `${(ms / 1000).toFixed(2)} s`;
const count = (n: number) => n.toLocaleString();
const compactTokens = (n: number) => n < 1000 ? count(n) : new Intl.NumberFormat('en', { notation: 'compact', minimumFractionDigits: 2, maximumFractionDigits: 2 }).format(n);
const TokenCount = ({ value }: { value: number }) => <span title={count(value)}>{compactTokens(value)}</span>;

export function TrafficPage({ mode }: { mode: 'logs' | 'usage' }) {
  const { t, language } = usePreferences();
  const [days, setDays] = useState(7);
  const [agent, setAgent] = useState('');
  const [provider, setProvider] = useState('');
  const [status, setStatus] = useState('');
  const [search, setSearch] = useState('');
  const [rows, setRows] = useState<RequestLog[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState('');
  const [updated, setUpdated] = useState<Date | null>(null);
  const [page, setPage] = useState(0);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [group, setGroup] = useState<'model' | 'agent' | 'provider'>('model');
  const sequence = useRef(0);
  const load = useCallback(async (quiet = false) => {
    const request = ++sequence.current;
    if (!quiet) setLoading(true);
    try {
      const result = await getRequestLogs(rangeStart(days).toISOString());
      if (request !== sequence.current) return;
      setRows(result); setUpdated(new Date()); setError('');
    } catch (e) {
      if (request === sequence.current) setError(String(e));
    } finally { if (request === sequence.current) setLoading(false); }
  }, [days]);
  useEffect(() => {
    setRows([]); setUpdated(null); void load();
    const timer = setInterval(() => { if (!document.hidden) void load(true); }, 5000);
    return () => { clearInterval(timer); sequence.current++; };
  }, [load]);
  useEffect(() => { setPage(0); setExpanded(null); }, [days, agent, provider, status, search]);
  const filtered = useMemo(() => rows.filter(r => (!agent || r.agent === agent) && (!provider || r.provider_name === provider)
    && (!status || r.status === status) && (!search || [r.id, r.agent, r.route_id, r.route_strategy, r.route_preference, r.requested_model, r.model_name, r.model_id, r.provider_name, r.endpoint, r.error].join(' ').toLowerCase().includes(search.toLowerCase()))), [rows, agent, provider, status, search]);
  const totals = useMemo(() => summarize(filtered), [filtered]);
  const series = useMemo(() => dailyUsage(filtered, days), [filtered, days]);
  const groups = useMemo(() => usageGroups(filtered, group), [filtered, group]);
  const max = Math.max(1, ...series.map(d => d.input + d.output));
  const pages = Math.max(1, Math.ceil(filtered.length / 25));
  const current = Math.min(page, pages - 1);
  const agents = [...new Set([...rows.map(r => r.agent), ...(agent ? [agent] : [])])].sort();
  const providers = [...new Set([...rows.map(r => r.provider_name), ...(provider ? [provider] : [])])].filter(Boolean).sort();
  const statusLabel = (s: string) => t(s === 'success' ? 'Success' : s === 'cancelled' ? 'Cancelled' : 'Failed');
  return <div className="stack lg traffic-page">
    <div className="page-intro"><div><h2>{t(mode === 'logs' ? 'Request logs' : 'Token usage')}</h2><p>{t(mode === 'logs' ? 'Requests from agents through AutoJev. Metadata stays on this device.' : 'Actual upstream usage, across your agents and models.')}</p></div>
      <button className="button ghost" disabled={loading} onClick={() => void load()}><RefreshCw size={15} className={loading ? 'import-spinner' : ''}/>{t('Refresh')}</button></div>
    <div className="traffic-filters">
      <Select aria-label={t('Date range')} value={String(days)} onChange={e => setDays(Number(e.target.value))} searchable={false}>{[1, 7, 30, 90].map(n => <option key={n} value={String(n)}>{n === 1 ? t('Today') : t('Last {count} days', { count: n })}</option>)}</Select>
      <Select aria-label={t('Agent')} value={agent} onChange={e => setAgent(e.target.value)}><option value="">{t('All agents')}</option>{agents.map(a => <option key={a} value={a}>{t(a)}</option>)}</Select>
      <Select aria-label={t('Provider')} value={provider} onChange={e => setProvider(e.target.value)}><option value="">{t('All providers')}</option>{providers.map(p => <option key={p} value={p}>{p}</option>)}</Select>
      {mode === 'logs' && <><Select aria-label={t('Status')} value={status} onChange={e => setStatus(e.target.value)} searchable={false}><option value="">{t('All statuses')}</option>{['success', 'error', 'cancelled'].map(s => <option key={s} value={s}>{statusLabel(s)}</option>)}</Select>
        <label className="traffic-search"><Search size={15}/><input autoComplete="off" autoCapitalize="none" aria-label={t('Search requests')} placeholder={t('Search requests')} value={search} onChange={e => setSearch(e.target.value)}/></label></>}
    </div>
    {error && <div className="traffic-error" role="alert">{t('Could not load request logs. Try refreshing.')} <span>{error}</span></div>}
    <div className="traffic-live"><span className={`status-dot ${updated && !error ? 'online' : ''}`}/>{updated ? t('Updated at {time}', { time: updated.toLocaleTimeString(language) }) : t('Loading…')}<span>{t('Refreshes every 5 seconds')}</span></div>
    {mode === 'logs' ? <div className="panel traffic-log-panel">
      <div className="traffic-panel-heading"><h3>{t('Requests')}</h3><span>{t('{count} requests', { count: filtered.length })}</span></div>
      <div className="traffic-table-scroll"><table className="traffic-table"><thead><tr><th>{t('Time / Agent')}</th><th>{t('Model / Provider')}</th><th>{t('Input / Output / Cache')}</th><th>{t('Est. cost')}</th><th>{t('First byte / Total')}</th><th>{t('Status')}</th><th><span className="sr-only">{t('Details')}</span></th></tr></thead><tbody>
        {filtered.slice(current * 25, (current + 1) * 25).map(r => <Fragment key={r.id}><tr className={expanded === r.id ? 'traffic-selected' : ''}>
          <td><time dateTime={r.created_at}>{new Date(r.created_at).toLocaleString(language, { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit' })}</time><small>{t(r.agent)}</small></td>
          <td><strong title={r.model_id}>{r.model_name || r.requested_model || '—'}</strong><small>{r.provider_name || '—'} · {r.streaming ? t('Stream') : t('JSON')}</small><RequestRoute row={r}/></td>
          <td className="traffic-number">{r.input_tokens === null ? '—' : count(r.input_tokens)} <span>/</span> {r.output_tokens === null ? '—' : count(r.output_tokens)} <span>/</span> {r.input_tokens === null ? '—' : count(r.cache_read_tokens)}<small>{t('Cache hit rate: {rate}', { rate: cacheHitRate(r) })}</small></td>
          <td className="traffic-number">{money(r.estimated_cost)}</td><td className="traffic-number">{elapsed(r.first_byte_ms)} <span>/</span> {elapsed(r.duration_ms)}</td>
          <td><span className={`traffic-status ${r.status}`}>{statusLabel(r.status)}</span><small>{r.status_code || '—'}</small></td>
          <td><button className="icon-button" aria-label={t('Details')} aria-expanded={expanded === r.id} onClick={() => setExpanded(expanded === r.id ? null : r.id)}><ChevronDown size={16}/></button></td>
        </tr>{expanded === r.id && <tr><td colSpan={7}><div className="traffic-details"><dl>{[
          ['Request ID', r.id], ['Requested model', r.requested_model || '—'], ['Upstream model', r.model_id || '—'], ['Endpoint', r.endpoint],
          ['Route source', r.source || '—'], ['Routing reason', r.reason || '—'], ['Cache read', count(r.cache_read_tokens)], ['Cache write', count(r.cache_write_tokens)],
          ['Output speed', r.output_tokens !== null && r.duration_ms > (r.first_byte_ms ?? 0) ? `${(r.output_tokens / ((r.duration_ms - (r.first_byte_ms ?? 0)) / 1000)).toFixed(1)} tok/s` : '—'],
        ].map(([label, value]) => <div key={label}><dt>{t(label)}</dt><dd>{value}</dd></div>)}</dl>{Boolean(r.attempts?.length) && <div><strong>{t('Upstream attempts')}</strong><ol>{r.attempts!.map((a, i) => <li key={i}>{a.provider} / {a.model} · HTTP {a.status} · {a.duration_ms} ms</li>)}</ol></div>}{r.error && <p className="traffic-error">{t(r.error)}</p>}<p>{t('Cost uses the prices configured when the request was made. Missing usage or cache-write pricing is shown as unknown.')}</p></div></td></tr>}</Fragment>)}
      </tbody></table></div>
      {!filtered.length && <Empty loading={loading} filtered={Boolean(agent || provider || status || search)} />}
      <div className="traffic-pagination"><span>{t('Page {page} of {pages}', { page: current + 1, pages })}</span><div><button className="button ghost" disabled={current === 0} onClick={() => setPage(current - 1)}><ChevronLeft size={15}/>{t('Previous')}</button><button className="button ghost" disabled={current + 1 === pages} onClick={() => setPage(current + 1)}>{t('Next')}<ChevronRight size={15}/></button></div></div>
    </div> : <>
      <div className="traffic-metrics">{[
        ['Total tokens', compactTokens(totals.input + totals.output), t('{count} requests', { count: totals.requests }), count(totals.input + totals.output)],
        ['Input tokens', compactTokens(totals.input), t('Includes cache reads and writes'), count(totals.input)],
        ['Output tokens', compactTokens(totals.output), t('Reported by upstream providers'), count(totals.output)],
        ['Estimated cost', money(totals.requests > 0 && totals.unpriced === totals.requests ? null : totals.cost), t('{count} requests with unknown cost', { count: totals.unpriced })],
      ].map(([label, value, note, exact]) => <div className="panel traffic-metric" key={label}><span>{t(label)}</span><strong title={exact}>{value}</strong><small>{note}</small></div>)}</div>
      <div className="traffic-summary"><span>{t('Cache read')} <b><TokenCount value={totals.cacheRead}/></b></span><span>{t('Cache write')} <b><TokenCount value={totals.cacheWrite}/></b></span><span>{t('Success rate')} <b>{totals.requests ? `${(totals.success / totals.requests * 100).toFixed(1)}%` : '—'}</b></span><span>{t('Average duration')} <b>{totals.requests ? elapsed(Math.round(totals.duration / totals.requests)) : '—'}</b></span></div>
      {totals.missing > 0 && <p className="traffic-note">{t('{count} requests did not report complete usage and are excluded from token totals.', { count: totals.missing })}</p>}
      <div className="panel traffic-chart-panel"><div className="traffic-panel-heading"><h3>{t('Daily usage')}</h3><div className="traffic-legend"><span className="input">{t('Input')}</span><span className="output">{t('Output')}</span></div></div>
        <div className="traffic-chart" role="img" aria-label={t('Daily input and output tokens')}>
          {series.map(d => <div key={d.date} className="traffic-day" tabIndex={0} aria-label={`${d.date}: ${t('Input')} ${count(d.input)}, ${t('Output')} ${count(d.output)}, ${t('Requests')} ${d.requests}`}><div className="traffic-bars"><div className="traffic-bar-output" style={{ height: `${d.output / max * 100}%` }}/><div className="traffic-bar-input" style={{ height: `${d.input / max * 100}%` }}/></div><div className="traffic-chart-tip"><b>{d.date}</b><span>{t('Input')}: {count(d.input)}</span><span>{t('Output')}: {count(d.output)}</span><span>{t('Requests')}: {d.requests}</span></div></div>)}
        </div><div className="traffic-axis"><span>{series[0]?.date}</span><span>{series.at(-1)?.date}</span></div>
        {!filtered.length && <p className="traffic-note">{t('No usage in this period. Send a request from a connected agent to get started.')}</p>}
      </div>
      <div className="panel"><div className="traffic-panel-heading"><h3>{t('Usage breakdown')}</h3><Select aria-label={t('Group by')} value={group} onChange={e => setGroup(e.target.value as typeof group)} searchable={false}><option value="model">{t('By model')}</option><option value="agent">{t('By agent')}</option><option value="provider">{t('By provider')}</option></Select></div>
        <div className="traffic-table-scroll"><table className="traffic-table"><thead><tr><th>{t(group === 'model' ? 'Model' : group === 'agent' ? 'Agent' : 'Provider')}</th><th>{t('Requests')}</th><th>{t('Input tokens')}</th><th>{t('Output tokens')}</th><th>{t('Cache read / write')}</th><th>{t('Estimated cost')}</th></tr></thead><tbody>{groups.map(g => <tr key={g.key}><td><strong>{t(g.name || 'Unresolved')}</strong><small>{g.detail}</small><div className="traffic-rank"><i style={{ width: `${(g.input + g.output) / Math.max(1, totals.input + totals.output) * 100}%` }}/></div></td><td>{count(g.requests)}</td><td><TokenCount value={g.input}/></td><td><TokenCount value={g.output}/></td><td><TokenCount value={g.cacheRead}/> / <TokenCount value={g.cacheWrite}/></td><td>{g.unpriced === g.requests ? '—' : money(g.cost)}{g.unpriced > 0 && <small>{t('{count} unknown', { count: g.unpriced })}</small>}</td></tr>)}</tbody></table></div>
        {!groups.length && <Empty loading={loading} filtered={Boolean(agent || provider)} />}
      </div>
    </>}
    <p className="traffic-note">{t('Only requests routed through AutoJev are counted. Prompt and response bodies are not stored.')}</p>
  </div>;
}
function Empty({ loading, filtered }: { loading: boolean; filtered: boolean }) {
  const { t } = usePreferences();
  return <div className="traffic-empty"><Activity size={25}/><strong>{t(loading ? 'Loading…' : filtered ? 'No matching requests' : 'No requests yet')}</strong><p>{t(filtered ? 'Try a different filter or date range.' : 'Connect an agent and send a request through AutoJev. Completed requests appear here automatically.')}</p></div>;
}

function RequestRoute({ row }: { row: RequestLog }) {
  const { t } = usePreferences();
  const route = requestRoute(row);
  if (!route) return null;
  return <small className="traffic-route-info">{t('Route ID')}: <code>{route.id}</code> · {t(route.strategy)}{route.preference ? ` · ${t(route.preference)}` : ''}</small>;
}
