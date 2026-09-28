import { describe, expect, it } from 'vitest';
import { providerIdentifier } from './provider-name';

describe('providerIdentifier', () => {
  it('shows source and supplier name for imports', () => {
    expect(providerIdentifier({ id: 'import-termany-abc123', name: 'DeepSeek' })).toBe('termany-deepseek');
    expect(providerIdentifier({ id: 'import-ccswitch-abc123', name: 'OpenRouter' })).toBe('ccswitch-openrouter');
    expect(providerIdentifier({ id: 'import-termany-abc123', name: 'aicoding.sh' })).toBe('termany-aicoding-sh');
  });
  it('preserves manual identifiers', () => {
    expect(providerIdentifier({ id: 'custom', name: 'My provider' })).toBe('custom');
  });
});
