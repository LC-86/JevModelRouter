// Fictional model boundary. It returns tool calls and never executes them.
import assert from 'node:assert/strict';
export const toolSchema = { type: 'function', function: { name: 'client_echo', description: 'Executed only by the fictional client', parameters: { type: 'object', properties: { note: { type: 'string' } }, required: ['note'], additionalProperties: false } } };
export function dshFixture(records, cancelled, held, modes) {
  return async (req, res, body) => {
    const account = req.url.split('/')[1];
    assert.ok(['a1', 'a2', 'b1', 'paid'].includes(account));
    assert.equal(req.url, `/${account}/v1/chat/completions`);
    const tag = body.messages.find(m => m.role === 'user')?.content;
    records.push({ account, path: req.url, authorization: req.headers.authorization, body });
    res.on('close', () => { if (!res.writableEnded) cancelled.push(tag); });
    const mode = modes.get(account);
    if (Number.isInteger(mode)) { res.writeHead(mode, { 'content-type': 'application/json' }); res.end(JSON.stringify({ error: { message: `fictional upstream ${mode}`, type: 'fixture_error' } })); return; }
    const callId = `call_${tag}`;
    const tool = body.messages.find(m => m.role === 'tool');
    if (tool) {
      assert.equal(tool.tool_call_id, callId, 'The client result belongs to this session');
      assert.equal(tool.content, `client-result:${tag}`);
      assert.equal(body.messages.find(m => m.role === 'assistant').tool_calls[0].id, callId);
    }
    const content = tool ? `resumed:${account}:${tag}` : `${account}:${tag}`;
    const wantsTool = tag?.startsWith('r5-tool:') && !tool;
    if (wantsTool) assert.deepEqual(body.tools, [toolSchema]);
    const call = { id: callId, type: 'function', function: { name: 'client_echo', arguments: '{"note":"fictional result"}' } };
    if (!body.stream) {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ id: `fictional_${tag}`, object: 'chat.completion', model: 'same-model', choices: [{ index: 0, message: wantsTool ? { role: 'assistant', content: null, tool_calls: [call] } : { role: 'assistant', content }, finish_reason: wantsTool ? 'tool_calls' : 'stop' }], usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 } }));
      return;
    }
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    const frame = (delta, finish_reason = null) => res.write(`data: ${JSON.stringify({ id: `fictional_${tag}`, object: 'chat.completion.chunk', created: 0, model: 'same-model', choices: [{ index: 0, delta, finish_reason }] })}\n\n`);
    const finish = () => { frame({}, wantsTool ? 'tool_calls' : 'stop'); res.end('data: [DONE]\n\n'); };
    if (wantsTool) {
      frame({ role: 'assistant', tool_calls: [{ index: 0, id: callId, type: 'function', function: { name: 'client_echo', arguments: '{"note":' } }] });
      frame({ tool_calls: [{ index: 0, function: { arguments: '"fictional result"}' } }] });
      finish(); return;
    }
    frame({ role: 'assistant', content: `${tool ? 'resumed' : 'first'}:${account}:` });
    frame({ content: tag }); // two frames flush the existing redactor's lookahead
    if (tag?.startsWith('r5-hold:') || tag?.startsWith('r5-cancel:')) {
      held.set(tag, () => { frame({ content: `:last:${account}` }); finish(); held.delete(tag); });
      return;
    }
    if (tag?.startsWith('r5-partial:')) {
      res.end(`data: ${JSON.stringify({ error: { code: 'fixture_partial_failure', type: 'fixture_error', message: 'fictional partial stream failure' } })}\n\n`);
      return;
    }
    frame({ content: `:last:${account}` }); finish();
  };
}
