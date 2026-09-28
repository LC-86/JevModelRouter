import { expect, it } from 'vitest';
import { sortProviders } from './provider-sort';
import type { Provider } from '../types';
it('sorts enabled first, then connection status without changing the source list', () => {
  const providers = [
    { id: 'disabled-ok', enabled: false }, { id: 'failed', enabled: true },
    { id: 'untested', enabled: true }, { id: 'ok', enabled: true },
    { id: 'testing', enabled: true },
  ] as Provider[];
  expect(sortProviders(providers, { 'disabled-ok': 'success', failed: 'error', ok: 'success', testing: 'testing' }).map(p => p.id))
    .toEqual(['ok', 'testing', 'untested', 'failed', 'disabled-ok']);
  expect(providers[0].id).toBe('disabled-ok');
});
