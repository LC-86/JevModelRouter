import type { RequestLog } from '../types';

export function localDay(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`;
}
export function rangeStart(days: number, now = new Date()): Date {
  const start = new Date(now);
  start.setHours(0, 0, 0, 0);
  start.setDate(start.getDate() - days + 1);
  return start;
}
export function totalTokens(row: RequestLog): number { return (row.input_tokens ?? 0) + (row.output_tokens ?? 0); }
export function summarize(rows: RequestLog[]) {
  return rows.reduce((s, r) => ({
    requests: s.requests + 1, success: s.success + Number(r.status === 'success'),
    input: s.input + (r.input_tokens ?? 0), output: s.output + (r.output_tokens ?? 0),
    cacheRead: s.cacheRead + r.cache_read_tokens, cacheWrite: s.cacheWrite + r.cache_write_tokens,
    cost: s.cost + (r.estimated_cost ?? 0), unpriced: s.unpriced + Number(r.estimated_cost === null),
    missing: s.missing + Number(r.input_tokens === null || r.output_tokens === null), duration: s.duration + r.duration_ms,
  }), { requests: 0, success: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, missing: 0, unpriced: 0, duration: 0 });
}
export function dailyUsage(rows: RequestLog[], days: number, now = new Date()) {
  const series = Array.from({ length: days }, (_, i) => {
    const date = rangeStart(days - i, now);
    return { date: localDay(date), input: 0, output: 0, requests: 0 };
  });
  const byDay = new Map(series.map(day => [day.date, day]));
  for (const row of rows) {
    const day = byDay.get(localDay(new Date(row.created_at)));
    if (day) { day.input += row.input_tokens ?? 0; day.output += row.output_tokens ?? 0; day.requests++; }
  }
  return series;
}
export function usageGroups(rows: RequestLog[], group: 'model' | 'agent' | 'provider') {
  const groups = new Map<string, { name: string; detail: string; rows: RequestLog[] }>();
  for (const r of rows) {
    const key = group === 'model' ? JSON.stringify([r.provider_name, r.model_id || r.model_name]) : group === 'agent' ? r.agent : r.provider_name;
    const name = group === 'model' ? r.model_name || r.model_id : group === 'agent' ? r.agent : r.provider_name;
    const item = groups.get(key) ?? { name, detail: group === 'model' ? r.provider_name : '', rows: [] };
    item.rows.push(r); groups.set(key, item);
  }
  return [...groups.entries()].map(([key, item]) => ({ key, name: item.name, detail: item.detail, ...summarize(item.rows) }))
    .sort((a, b) => b.input + b.output - a.input - a.output || b.requests - a.requests);
}

// Normalized input usage already includes cached tokens.
export function cacheHitRate(row: RequestLog): string {
  if (row.input_tokens === null || row.input_tokens <= 0) return '—';
  return `${(row.cache_read_tokens / row.input_tokens * 100).toFixed(1)}%`;
}

export function requestRoute(row: RequestLog) {
  const id = row.route_id || (/^autojev\/(?!model\/)(.+)$/.exec(row.requested_model)?.[1] ?? '');
  if (!id) return null;
  const strategies: Record<string, string> = { jev: 'Intelligent selection', round_robin: 'Load balancing', fixed: 'Fixed model', auto: 'Built-in automatic routing' };
  const preferences: Record<string, string> = { balanced: 'Balance quality, cost and speed', cost: 'Lower cost', quality: 'Higher quality', speed: 'Lower latency' };
  return { id, strategy: strategies[row.route_strategy ?? ''] ?? 'Strategy not recorded', preference: row.route_strategy === 'jev' ? preferences[row.route_preference ?? ''] : undefined };
}
