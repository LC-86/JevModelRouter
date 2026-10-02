import { describe, expect, it } from 'vitest';
import { acceptGrokObservation, fieldText, observationExpired, type GrokObservation } from './grok-readonly';
const observation = { observedAt: '2030-01-01T00:00:00Z', realGenerationEnabled: false } as GrokObservation;
describe('manual Grok observation', () => {
  it('discards late responses after cancellation, unmount, or a new request', () => {
    expect(acceptGrokObservation('old', null, observation)).toBeNull();
    expect(acceptGrokObservation('old', 'new', observation)).toBeNull();
    expect(acceptGrokObservation('current', 'current', observation)).toBe(observation);
  });
  it('expires old, invalid, and future timestamps', () => {
    const now = Date.parse(observation.observedAt);
    expect(observationExpired(observation, now + 299999)).toBe(false);
    expect(observationExpired(observation, now + 300000)).toBe(true);
    expect(observationExpired(observation, now - 1)).toBe(true);
    expect(observationExpired({ ...observation, observedAt: 'invalid' }, now)).toBe(true);
  });
  it('keeps missing/null/malformed usage Unknown while displaying explicit zero', () => {
    for (const state of ['missing', 'null', 'invalid', 'out_of_range']) {
      expect(fieldText({state, value: null}, String, 'Unknown')).toBe(`Unknown (${state})`);
    }
    expect(fieldText({state: 'available', value: 0}, String, 'Unknown')).toBe('0');
  });
});
