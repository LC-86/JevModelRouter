import type { Provider } from '../types';

export function providerIdentifier(provider: Pick<Provider, 'id' | 'name'>): string {
  const source = /^import-(termany|ccswitch)-/.exec(provider.id)?.[1];
  if (!source) return provider.id;
  const name = provider.name.trim().toLowerCase().replace(/[^\p{L}\p{N}_-]+/gu, '-').replace(/^-+|-+$/g, '');
  return `${source}-${name || 'provider'}`;
}
