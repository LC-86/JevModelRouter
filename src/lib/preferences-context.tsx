import { createContext, useContext, useEffect, useLayoutEffect, useMemo, useState, type ReactNode } from 'react';
import { defaults, parsePreferences, PREFERENCES_KEY, resolveLanguage, resolveTheme, type Preferences } from './preferences';
import zh from '../i18n/zh-CN.json';

export type Translate = (message: string, values?: Record<string, string | number>) => string;
export function translate(language: string, message: string, values?: Record<string, string | number>) {
  const template = language === 'zh-CN' ? (zh as Record<string, string>)[message] ?? message : message;
  return template.replace(/\{(\w+)\}/g, (match, key: string) => String(values?.[key] ?? match));
}
const PreferencesContext = createContext<{
  preferences: Preferences; setPreferences: (patch: Partial<Preferences>) => void;
  language: 'en' | 'zh-CN'; theme: 'light' | 'dark'; t: Translate; storageError: boolean;
} | null>(null);

export function PreferencesProvider({ children }: { children: ReactNode }) {
  const [preferences, setState] = useState(() => {
    try { return parsePreferences(localStorage.getItem(PREFERENCES_KEY)); } catch { return { ...defaults }; }
  });
  const [storageError, setStorageError] = useState(false);
  const [systemDark, setSystemDark] = useState(() => matchMedia('(prefers-color-scheme: dark)').matches);
  const [systemLanguage, setSystemLanguage] = useState(navigator.language);
  const language = resolveLanguage(preferences.language, systemLanguage);
  const theme = resolveTheme(preferences.theme, systemDark);

  useEffect(() => {
    const media = matchMedia('(prefers-color-scheme: dark)');
    const updateTheme = () => setSystemDark(media.matches);
    const updateLanguage = () => setSystemLanguage(navigator.language);
    const sync = (event: StorageEvent) => {
      if (event.storageArea === localStorage && (event.key === PREFERENCES_KEY || event.key === null)) setState(parsePreferences(event.newValue));
    };
    media.addEventListener('change', updateTheme);
    window.addEventListener('languagechange', updateLanguage);
    window.addEventListener('storage', sync);
    return () => {
      media.removeEventListener('change', updateTheme);
      window.removeEventListener('languagechange', updateLanguage);
      window.removeEventListener('storage', sync);
    };
  }, []);

  useLayoutEffect(() => {
    document.documentElement.lang = language;
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.accent = preferences.accent;
    document.documentElement.style.colorScheme = theme;
  }, [language, theme, preferences.accent]);

  useEffect(() => {
    try { localStorage.setItem(PREFERENCES_KEY, JSON.stringify(preferences)); setStorageError(false); }
    catch { setStorageError(true); }
  }, [preferences]);

  const value = useMemo(() => ({
    preferences, language, theme, storageError,
    setPreferences: (patch: Partial<Preferences>) => setState(current => ({ ...current, ...patch })),
    t: (message: string, values?: Record<string, string | number>) => translate(language, message, values),
  }), [preferences, language, theme, storageError]);
  return <PreferencesContext.Provider value={value}>{children}</PreferencesContext.Provider>;
}
export function usePreferences() {
  const context = useContext(PreferencesContext);
  if (!context) throw new Error('PreferencesProvider is required');
  return context;
}
