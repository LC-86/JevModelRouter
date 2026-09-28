import { expect, it } from 'vitest';
import { providerTestError } from './provider-test';
import { translate } from './preferences-context';

it('localizes HTTP failures without losing the status code', () => {
  const t = (message: string, values?: Record<string, string | number>) => translate('zh-CN', message, values);
  expect(providerTestError('Provider returned HTTP 401 Unauthorized', t)).toBe('服务商返回 HTTP 401，请检查 API 类型、地址、密钥和模型。');
  expect(providerTestError('Provider returned HTTP 429 Too Many Requests\n{"message":"Quota exceeded"}', t)).toContain('\n{"message":"Quota exceeded"}');
  expect(providerTestError('Provider returned an API error or invalid model response\n{"code":522}', t)).toContain('522');
  expect(providerTestError(new Error('Enter a test model'), t)).toBe('请输入测试模型');
  expect(providerTestError('Connection failed or timed out. Check the base URL and network.', t)).toBe('连接失败或超时，请检查接口地址和网络。');
});
