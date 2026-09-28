import type { Provider } from '../types';
import type { ProviderTestStatus } from './provider-test';

export function sortProviders(providers: Provider[], states: Record<string, ProviderTestStatus>): Provider[] {
  const rank = (id: string) => ({ success: 0, testing: 1, error: 3 }[states[id]] ?? 2);
  return [...providers].sort((a, b) => Number(b.enabled) - Number(a.enabled) || rank(a.id) - rank(b.id));
}
