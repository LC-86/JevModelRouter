import { describe, expect, it } from 'vitest';
import { renderToStaticMarkup } from 'react-dom/server';
import { debugResponseWarning, debugContent, debugHistory, parseDebugParameters, debugRequestBody, debugResponseText, type DebugMessage } from './debug-message';
import { DebugMarkdown } from '../components/debug-markdown';

describe('debug messages', () => {
  it('includes only explicit parameters and protocol-specific message fields', () => {
    const history = [{ role: 'user', content: 'before' }];
    for (const endpoint of ['chat/completions', 'messages', 'responses']) {
      const responses = endpoint === 'responses';
      const body = debugRequestBody('autojev/fast', endpoint, 'now', history, { temperature: 0.2, max_tokens: 500 });
      expect(body[responses ? 'input' : 'messages']).toEqual([...history, { role: 'user', content: 'now' }]);
      expect(body[responses ? 'max_output_tokens' : 'max_tokens']).toBe(500);
      expect(body[responses ? 'max_tokens' : 'max_output_tokens']).toBeUndefined();
      expect(body.temperature).toBe(0.2);
      expect(body.stream).toBeUndefined();
      expect(body.model).toBe('autojev/fast');
    }
    expect(history).toHaveLength(1);
    for (const endpoint of ['chat/completions', 'messages', 'responses']) {
      const body = debugRequestBody('autojev/fast', endpoint, 'now', [], {});
      expect(Object.keys(body).sort()).toEqual([endpoint === 'responses' ? 'input' : 'messages', 'model']);
    }
  });
  it('extracts text across three protocols without exposing reasoning or tool arguments', () => {
    expect(debugResponseText({ choices: [{ message: { content: 'hello' } }] })).toBe('hello');
    expect(debugResponseText({ choices: [{ message: { content: [{ type: 'text', text: 'hello' }] } }] })).toBe('hello');
    expect(debugResponseText({ content: [{ type: 'thinking', text: 'private' }, { type: 'text', text: 'answer' }] })).toBe('answer');
    expect(debugResponseText({ output: [{ type: 'function_call', arguments: 'tool' }, { type: 'message', content: [{ type: 'output_text', text: 'answer' }] }] })).toBe('answer');
    expect(debugResponseText({ choices: [{ message: { refusal: 'Cannot answer' } }] })).toBe('Cannot answer');
    expect(debugResponseText(null)).toBe('');
  });
  it('excludes pending and failed turns from subsequent request history', () => {
    const messages = ['success', 'error', 'pending'].flatMap((status, index) => ['user', 'assistant'].map(role => ({ id: `${index}-${role}`, role, status, content: `${status}-${role}` }))) as DebugMessage[];
    expect(debugHistory(messages)).toEqual([{ role: 'user', content: 'success-user' }, { role: 'assistant', content: 'success-assistant' }]);
  });
  it('renders Markdown and GFM while blocking raw HTML, unsafe URLs and remote images', () => {
    const html = renderToStaticMarkup(<DebugMarkdown>{'# Title\n\n**bold**\n\n- item\n\n```js\nconst x = 1;\n```\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n<script>alert(1)</script>\n\n[bad](javascript:alert%281%29)\n\n![image](https://example.com/tracker.png)'}</DebugMarkdown>);
    expect(html).toContain('<h1>Title</h1>');
    expect(html).toContain('<strong>bold</strong>');
    expect(html).toContain('<li>item</li>');
    expect(html).toContain('<pre><code');
    expect(html).toContain('<table>');
    expect(html).not.toContain('<script');
    expect(html).not.toContain('javascript:');
    expect(html).not.toContain('<img');
  });
});

it('preserves arbitrary parameter types and rejects duplicate or blank keys', () => {
  const rows = Object.entries({stream:'true',seed:'42',stop:'["END"]',metadata:'{"tag":"test"}',user:'alice',optional:'null',empty:'""'}).map(([key,value]) => ({key,value}));
  const params = parseDebugParameters(rows);
  expect(params).toEqual({stream:true,seed:42,stop:['END'],metadata:{tag:'test'},user:'alice',optional:null,empty:''});
  expect(debugRequestBody('autojev/fast','chat/completions','hi',[],params)).toMatchObject(params);
  expect(() => parseDebugParameters([{key:'stream',value:'true'},{key:' stream ',value:'false'}])).toThrow();
  expect(() => parseDebugParameters([{key:' ',value:'true'}])).toThrow();
});

 it('formats pasted images for each protocol and preserves them in conversation history', () => {
  const image = 'data:image/png;base64,aGVsbG8=';
  const formats = {
    'chat/completions': { type: 'image_url', image_url: { url: image } },
    responses: { type: 'input_image', image_url: image },
    messages: { type: 'image', source: { type: 'base64', media_type: 'image/png', data: 'aGVsbG8=' } },
  };
  for (const [endpoint, part] of Object.entries(formats)) {
    expect(debugContent('', [image], endpoint)).toEqual([part]);
    const content = debugContent('Describe', [image], endpoint);
    expect(content).toEqual([{ type: endpoint === 'responses' ? 'input_text' : 'text', text: 'Describe' }, part]);
    const history = debugHistory([{ id: 'image', role: 'user', status: 'success', content: 'Describe', images: [image] }], endpoint);
    expect(history).toEqual([{ role: 'user', content }]);
    expect(debugRequestBody('autojev/fast', endpoint, content, history, {})[endpoint === 'responses' ? 'input' : 'messages']).toEqual([...history, {role:'user', content}]);
  }
});

it('distinguishes truncated reasoning-only output from a missing response', () => {
  const body = {choices:[{finish_reason:'length',message:{content:'',reasoning_content:'reasoning'}}]};
  expect(debugResponseText(body)).toBe('');
  expect(debugResponseWarning(body)).toContain('before a final answer');
  expect(debugResponseWarning({choices:[{finish_reason:'length',message:{content:'Partial'}}]})).toContain('incomplete');
  expect(debugResponseWarning({stop_reason:'max_tokens',content:[]})).toContain('before a final answer');
  expect(debugResponseWarning({incomplete_details:{reason:'max_output_tokens'},output:[]})).toContain('before a final answer');
  expect(debugResponseWarning({choices:[{finish_reason:'stop',message:{content:'Done'}}]})).toBeUndefined();
});

it('keeps text files and PDFs in successful conversation history for each API', () => {
  const files = [{ name: 'notes.txt', size: 5, text: 'hello' }, { name: 'report.pdf', size: 3, data: 'data:application/pdf;base64,YWJj' }];
  for (const endpoint of ['chat/completions', 'responses', 'messages']) {
    const content = debugContent('', [], endpoint, files) as Record<string, unknown>[];
    expect(content[0].text).toBe('File: notes.txt\nhello');
    expect(JSON.stringify(content)).toContain('YWJj');
    expect(debugHistory([{ id: 'u', role: 'user', status: 'success', content: '', files }], endpoint)[0].content).toEqual(content);
  }
});
