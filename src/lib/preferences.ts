export type Language = 'system' | 'en' | 'zh-CN';
export type Theme = 'system' | 'light' | 'dark';
export type Accent = 'orange' | 'blue' | 'green' | 'violet';
export interface Preferences { language: Language; theme: Theme; accent: Accent; autoCheckUpdates: boolean }
export const PREFERENCES_KEY = 'autojev.preferences.v1';
export const defaults: Preferences = { language: 'system', theme: 'system', accent: 'orange', autoCheckUpdates: true };

export function parsePreferences(raw: string | null): Preferences {
  try {
    const value = JSON.parse(raw ?? '{}');
    if (!value || typeof value !== 'object') return { ...defaults };
    return {
      language: ['system', 'en', 'zh-CN'].includes(value.language) ? value.language : defaults.language,
      theme: ['system', 'light', 'dark'].includes(value.theme) ? value.theme : defaults.theme,
      accent: ['orange', 'blue', 'green', 'violet'].includes(value.accent) ? value.accent : defaults.accent,
      autoCheckUpdates: typeof value.autoCheckUpdates === 'boolean' ? value.autoCheckUpdates : defaults.autoCheckUpdates,
    };
  } catch { return { ...defaults }; }
}

export function resolveLanguage(language: Language, systemLanguage: string): 'en' | 'zh-CN' {
  return language === 'system' ? (/^zh(?:-|$)/i.test(systemLanguage) ? 'zh-CN' : 'en') : language;
}
export function resolveTheme(theme: Theme, systemDark: boolean): 'light' | 'dark' {
  return theme === 'system' ? (systemDark ? 'dark' : 'light') : theme;
}
