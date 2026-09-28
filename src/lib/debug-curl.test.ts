import { describe, expect, it } from 'vitest';
import { makeCurl, parseCurl } from './debug-curl';
describe('debug cURL', () => {
  it('round trips quoted multiline JSON without injecting parameters', () => {
    const body = { model: 'autojev/fast', messages: [{role:'user',content:"What's this?\n你好 $(secret)"}] };
    expect(parseCurl(makeCurl(9526, 'chat/completions', body), 9526).body).toEqual(body);
  });
  it('preserves explicit stream, API body, and headers', () => {
    const parsed = parseCurl(`curl http://localhost:9526/v1/responses -H 'X-Test: yes' -d '{"model":"autojev/fast","input":"hello","stream":true}'`, 9526);
    expect(parsed.endpoint).toBe('responses');
    expect(parsed.body.stream).toBe(true);
    expect(parsed.headers['X-Test']).toBe('yes');
  });
  it('rejects remote targets, shell operators, file bodies and unsupported options', () => {
    for (const command of [
      `curl https://example.com/v1/responses -d '{}'`,
      `curl http://127.0.0.1:9527/v1/responses -d '{}'`,
      `curl http://127.0.0.1:9526/v1/responses -d '{}' ; echo bad`,
      `curl http://127.0.0.1:9526/v1/responses -d @secret`,
      `curl http://127.0.0.1:9526/v1/responses --output file -d '{}'`,
    ]) expect(() => parseCurl(command, 9526)).toThrow();
  });
});
