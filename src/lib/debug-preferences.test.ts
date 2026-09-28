import { expect, it } from 'vitest';
import { parseDebugPreferences } from './debug-preferences';

it('restores selection and parameters, including an intentionally empty list', () => {
  const saved = { target: 'autojev/fast', endpoint: 'responses', parameters: [{ key: 'max_tokens', value: '4096' }] };
  expect(parseDebugPreferences(JSON.stringify(saved), ['autojev/cheap', 'autojev/fast'])).toEqual(saved);
  expect(parseDebugPreferences(JSON.stringify({ ...saved, parameters: [] }), ['autojev/fast']).parameters).toEqual([]);
});
it('falls back to the first available option when the saved target is unavailable', () => {
  const raw = JSON.stringify({ target: 'autojev/removed', endpoint: 'messages', parameters: [] });
  expect(parseDebugPreferences(raw, ['autojev/model/first']).target).toBe('autojev/model/first');
  expect(parseDebugPreferences(raw, []).target).toBe('');
  expect(parseDebugPreferences(raw, []).endpoint).toBe('messages');
});
it('handles missing, malformed and invalid stored preferences', () => {
  for (const raw of [null, '{bad', 'null', '{"endpoint":"invalid","parameters":[null]}']) {
    expect(parseDebugPreferences(raw, ['autojev/first'])).toEqual({ target: 'autojev/first', endpoint: 'chat/completions', parameters: [{ key: 'temperature', value: '0.2' }] });
  }
});
