import { describe, expect, it } from 'vitest';
import { requestRoute, cacheHitRate, dailyUsage, rangeStart, summarize, totalTokens, usageGroups } from './traffic';
import type { RequestLog } from '../types';
const row = (patch: Partial<RequestLog> = {}): RequestLog => ({
  id: '1', created_at: new Date(2026, 8, 20, 12).toISOString(), agent: 'Codex', endpoint: 'responses',
  requested_model: 'auto', provider_name: 'Provider A', model_name: 'Model', model_id: 'model', source: 'local',
  reason: '', streaming: true, status: 'success', status_code: 200, duration_ms: 2000, first_byte_ms: 100,
  input_tokens: 100, output_tokens: 20, cache_read_tokens: 40, cache_write_tokens: 10, estimated_cost: 0.003, error: '', ...patch,
});
describe('request usage', () => {
  it('computes cache hits from input only, excluding output and cache writes', () => {
    expect(cacheHitRate(row())).toBe('40.0%');
    expect(cacheHitRate(row({ cache_read_tokens: 0 }))).toBe('0.0%');
    expect(cacheHitRate(row({ cache_read_tokens: 100 }))).toBe('100.0%');
    expect(cacheHitRate(row({ input_tokens: 0 }))).toBe('—');
    expect(cacheHitRate(row({ input_tokens: null }))).toBe('—');
  });
  it('counts cache as a subset of input and keeps unknown usage separate', () => {
    const rows = [row(), row({ input_tokens: null, output_tokens: null, cache_read_tokens: 0, cache_write_tokens: 0, estimated_cost: null, status: 'error' })];
    expect(totalTokens(rows[0])).toBe(120);
    expect(summarize(rows)).toMatchObject({ requests: 2, success: 1, input: 100, output: 20, cacheRead: 40, cacheWrite: 10, missing: 1, unpriced: 1, cost: 0.003 });
  });
  it('zero reported usage is distinct from unavailable usage', () => {
    expect(summarize([row({ input_tokens: 0, output_tokens: 0, estimated_cost: 0 })])).toMatchObject({ missing: 0, unpriced: 0, cost: 0 });
  });
  it('fills calendar days without compressing gaps and includes today', () => {
    const now = new Date(2026, 8, 20, 23, 59);
    expect(rangeStart(7, now)).toEqual(new Date(2026, 8, 14));
    const days = dailyUsage([row()], 7, now);
    expect(days).toHaveLength(7); expect(days[0]).toMatchObject({ date: '2026-09-14', input: 0 });
    expect(days[6]).toMatchObject({ date: '2026-09-20', input: 100, output: 20, requests: 1 });
  });
  it('keeps the same model at different providers separate and supports agent grouping', () => {
    const rows = [row(), row({ provider_name: 'Provider B', agent: 'Claude Code' })];
    expect(usageGroups(rows, 'model')).toHaveLength(2);
    expect(usageGroups(rows, 'agent').map(g => g.name)).toEqual(['Codex', 'Claude Code']);
  });
});

it('shows recorded route strategy and preserves unknown historical strategies', () => {
  expect(requestRoute(row({ requested_model: 'autojev/model/direct' }))).toBeNull();
  expect(requestRoute(row({ requested_model: 'deepseek-flash' }))).toBeNull();
  expect(requestRoute(row({ requested_model: 'autojev/fast' }))).toMatchObject({ id: 'fast', strategy: 'Strategy not recorded' });
  expect(requestRoute(row({ requested_model: 'public-model-id', route_id: 'fast', route_strategy: 'jev', route_preference: 'speed' }))).toEqual({ id: 'fast', strategy: 'Intelligent selection', preference: 'Lower latency' });
  expect(requestRoute(row({ route_id: 'lb', route_strategy: 'round_robin' }))).toMatchObject({ id: 'lb', strategy: 'Load balancing' });
});
