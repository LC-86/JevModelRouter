// Read published code only. Never boots DSH or opens profiles/credential storage.
import assert from 'node:assert/strict';
import { readFile, realpath, writeFile, readdir } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { createHash } from 'node:crypto';

const cli = await realpath(resolve(process.argv[2])); // explicit installed lib/bin.js
const publishedRoot = resolve(process.argv[3]); // explicit PNPM published-package inventory
async function installedPackage(name) {
  const versionRoot = join(publishedRoot, name, '0.1.7-rc.1');
  const candidates = await readdir(versionRoot);
  assert.equal(candidates.length, 1, `Select the exact published artifact for ${name}`);
  return join(versionRoot, candidates[0], 'node_modules', name, 'package.json');
}
const packages = ['@deepseek-ai/dsh', '@deepseek-ai/dsh-llm-pi-ai', '@deepseek-ai/dsh-agent', '@deepseek-ai/dsh-agent-default-model', '@deepseek-ai/dsh-api-session-controller'];
const files = [];
for (const name of packages) {
  const packagePath = name === '@deepseek-ai/dsh' ? join(dirname(dirname(cli)), 'package.json') : await installedPackage(name);
  const meta = JSON.parse(await readFile(packagePath, 'utf8'));
  assert.equal(meta.version, '0.1.7-rc.1', `Re-inspect changed ${name}`);
  for (const file of ['package.json', name === '@deepseek-ai/dsh' ? 'lib/bin.js' : 'lib/index.js']) {
    const bytes = await readFile(join(dirname(packagePath), file));
    files.push({ package: name, version: meta.version, file, sha256: createHash('sha256').update(bytes).digest('hex') });
  }
}
const adapterPath = join(dirname(await installedPackage('@deepseek-ai/dsh-llm-pi-ai')), 'lib/index.js');
const piPath = await realpath(join(dirname(dirname(dirname(dirname(adapterPath)))), '@earendil-works/pi-ai/package.json'));
const pi = JSON.parse(await readFile(piPath, 'utf8'));
assert.equal(pi.version, '0.85.1');
const piFile = 'dist/api/openai-completions.js';
files.push({ package: pi.name, version: pi.version, file: piFile, sha256: createHash('sha256').update(await readFile(join(dirname(piPath), piFile))).digest('hex') });
const evidence = {
  layer: 'read-only-installed-published-code',
  runtime_started: false,
  user_configuration_read_or_written: false,
  real_model_calls: 0, logins: 0, account_quota_queries: 0,
  files,
  contract: {
    adapter: '@deepseek-ai/dsh-llm-pi-ai',
    profile: 'config.providers.<route>: api, baseURL, apiKeyEnv, models[].id, retryPolicy',
    protocol: 'openai-completions',
    model_endpoint: 'POST {baseURL}/chat/completions',
    catalog_endpoint: 'GET {baseURL}/models (discovery only)',
    selection: 'AgentDefaultModelConfig {provider, model}; session controller selectModel({sessionId, provider, model}) calls selectForNextRequest',
    request_snapshot: 'installModelSelection snapshots provider/model at prompt assembly; PiAdapter captures current() before streamWithSnapshot',
    cancellation: 'GenerateOptions.signal is combined with consumer AbortController',
    tools: 'client function schemas; tool_calls[].id; history tool_call_id; streamed arguments and finish_reason=tool_calls',
    default_retries: 'normal mode defaults to 5; fictional profile explicitly sets maxRetries=0'
  }
};
await writeFile(resolve(process.argv[4] || 'docs/testing/dsh-interface.json'), `${JSON.stringify(evidence, null, 2)}\n`);
console.log(`Pinned ${files.length} published files; DSH ${files[0].version}, pi-ai ${pi.version}; no runtime started`);
