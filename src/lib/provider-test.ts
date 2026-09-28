import type { Translate } from './preferences-context';

export function providerTestError(error: unknown, t: Translate): string {
  const message = error instanceof Error ? error.message : String(error);
  const http = /^Provider returned HTTP (\d{3})/.exec(message);
  if (http) {
    const details = message.includes('\n') ? message.slice(message.indexOf('\n') + 1) : '';
    return t('Provider returned HTTP {status}. Check the API type, URL, key and model.', { status: http[1] }) + (details ? `\n${details}` : '');
  }
  return t(message);
}
export type ProviderTestStatus = 'testing' | 'success' | 'error';
