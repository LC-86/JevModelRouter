import type { RoutePreviewInput } from '../types';

export const DEBUG_PREFERENCES_KEY = 'autojev.debug.preferences.v1';
export interface DebugPreferences {
  target: string;
  endpoint: RoutePreviewInput['endpoint'];
  parameters: { key: string; value: string }[];
}

export function parseDebugPreferences(raw: string | null, targets: string[]): DebugPreferences {
  const defaults: DebugPreferences = {
    target: targets[0] ?? '', endpoint: 'chat/completions',
    parameters: [{ key: 'temperature', value: '0.2' }],
  };
  try {
    const value = JSON.parse(raw ?? '{}');
    if (!value || typeof value !== 'object') return defaults;
    return {
      target: targets.includes(value.target) ? value.target : defaults.target,
      endpoint: ['chat/completions', 'responses', 'messages'].includes(value.endpoint) ? value.endpoint : defaults.endpoint,
      parameters: Array.isArray(value.parameters) && value.parameters.every((p: unknown) =>
        !!p && typeof p === 'object' && 'key' in p && typeof p.key === 'string' && 'value' in p && typeof p.value === 'string')
        ? value.parameters.map(({ key, value }: { key: string; value: string }) => ({ key, value })) : defaults.parameters,
    };
  } catch { return defaults; }
}
