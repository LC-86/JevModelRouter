import { describe, expect, it } from 'vitest';
import { defaults, parsePreferences, resolveLanguage, resolveTheme } from './preferences';
import { translate } from './preferences-context';

describe('preferences', () => {
  it('recovers from missing, malformed and invalid saved values', () => {
    for (const value of [null, '', '{', 'null', '42']) expect(parsePreferences(value)).toEqual(defaults);
    expect(parsePreferences(JSON.stringify({ language: 'ja', theme: 'nope', accent: 'red', autoCheckUpdates: 'false' }))).toEqual(defaults);
  });
  it('retains valid preferences and fills defaults for new fields', () => {
    expect(parsePreferences('{"language":"zh-CN","theme":"dark","autoCheckUpdates":false}')).toEqual({ ...defaults, language: 'zh-CN', theme: 'dark', autoCheckUpdates: false });
  });
  it('resolves system language and theme while preserving explicit choices', () => {
    expect(resolveLanguage('system', 'zh-TW')).toBe('zh-CN');
    expect(resolveLanguage('system', 'fr-FR')).toBe('en');
    expect(resolveLanguage('en', 'zh-CN')).toBe('en');
    expect(resolveTheme('system', true)).toBe('dark');
    expect(resolveTheme('system', false)).toBe('light');
    expect(resolveTheme('dark', false)).toBe('dark');
  });
  it('interpolates localized messages without modifying unknown backend text', () => {
    expect(translate('zh-CN', '{count} providers', { count: 3 })).toBe('3 个服务商');
    expect(translate('en', '{count} providers', { count: 3 })).toBe('3 providers');
    expect(translate('zh-CN', 'Provider error: 503')).toBe('Provider error: 503');
  });
});
