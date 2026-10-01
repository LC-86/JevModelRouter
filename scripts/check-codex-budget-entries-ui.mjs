import { chromium } from 'playwright';
import { readFile, writeFile, mkdir, mkdtemp } from 'node:fs/promises';
import { join, relative, sep } from 'node:path';
import { createHash } from 'node:crypto';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import { tmpdir } from 'node:os';

const repo = process.env.PR44_UI_REPO || fileURLToPath(new URL('..', import.meta.url));
const artifacts = await mkdtemp(join(tmpdir(), 'jev-codex-budget-ui-'));
await mkdir(artifacts, { recursive: true });
const sha = process.env.PR44_UI_SHA || 'working-tree';
const bridge = await readFile(join(repo, 'src/lib/bridge.ts'), 'utf8');
const app = await readFile(join(repo, 'src/App.tsx'), 'utf8');
const literal = bridge.split('const MOCK: DashboardSnapshot = ')[1].split('\n};')[0] + '\n}';
const fixture = Function('return (' + literal + ')')();
const model = { ...fixture.models[0], id: 'codex-model', provider_id: 'codex-fixture', model_id: 'fixture-model', name: 'Fixture Model', api_type: 'chat_completions', enabled: true, selected: true, input_price_known: false, output_price_known: false };
fixture.providers = [{ id: 'codex-fixture', name: 'Isolated Codex fixture', kind: 'codex_subscription', enabled: true, has_api_key: false, base_url: '', api_type: '', test_model: 'fixture-model' }];
fixture.models = [model];
fixture.agents = [];
fixture.subscriptions = [{ provider_id: 'codex-fixture', label: 'Codex', generation: 7, connection_instance_id: 'fixture-instance-7', state: 'connected', identity: 'fictional@example.invalid', helper_version: 'fixture-1', models: [], capabilities: [], catalog_entries: [], quota: { state: 'unknown' }, catalog: { state: 'unknown' }, adapter_available: true, real_generation_enabled: true, generation_call_limit: 3, generation_calls_remaining: 3, denial: null, admission_denial: null }];

const scenarios = [
  { name: 'provider-row-success', entry: 'provider-row' },
  { name: 'provider-row-failed-attempt', entry: 'provider-row', failure: true },
  { name: 'provider-dialog-success', entry: 'provider-dialog' },
  { name: 'model-row-success', entry: 'model-row' },
  { name: 'model-row-failed-attempt', entry: 'model-row', failure: true },
  { name: 'model-dialog-success', entry: 'model-dialog' },
  { name: 'model-dialog-failed-attempt', entry: 'model-dialog', failure: true },
  ...['chat/completions', 'responses', 'messages'].flatMap(protocol => [
    { name: `debug-${protocol.replace('/', '-')}-text-success`, entry: 'debug', protocol },
    { name: `debug-${protocol.replace('/', '-')}-stream-success`, entry: 'debug', protocol, stream: true },
  ]),
  { name: 'debug-failed-attempt', entry: 'debug', protocol: 'chat/completions', failure: true },
  { name: 'debug-curl-success', entry: 'debug-curl' },
  { name: 'debug-curl-cancelled-attempt', entry: 'debug-curl', cancel: true },
  { name: 'speed-three-successes', entry: 'speed', spent: 3 },
  { name: 'speed-delayed-completion-after-navigation', entry: 'speed', spent: 3, asyncSpeed: true, leaveModels: true },
  { name: 'speed-cancelled-attempt', entry: 'speed', spent: 1, cancel: true },
  { name: 'speed-failed-attempt', entry: 'speed', spent: 1, failure: true },
];
const browser = await chromium.launch({ channel: 'chrome', headless: true });
const results = [];
try {
  for (const scenario of scenarios) {
    const page = await browser.newPage({ viewport: { width: 1680, height: 1050 }, locale: 'en-US' });
    page.setDefaultTimeout(7000);
    const errors = [];
    const blockedExternal = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/*', async route => {
      const url = new URL(route.request().url());
      if (url.origin !== 'http://127.0.0.1:1420') { blockedExternal.push(url.href); await route.abort(); return; }
      const path = url.pathname === '/' ? 'index.html' : url.pathname.slice(1);
      const resolved = join(repo, 'dist', path);
      const relativePath = relative(join(repo, 'dist'), resolved);
      if (relativePath.startsWith(`..${sep}`) || relativePath === '..') { await route.abort(); return; }
      try {
        const body = await readFile(resolved);
        const contentType = path.endsWith('.html') ? 'text/html' : path.endsWith('.js') ? 'text/javascript' : path.endsWith('.css') ? 'text/css' : path.endsWith('.svg') ? 'image/svg+xml' : 'application/octet-stream';
        await route.fulfill({ status: 200, contentType, body });
      } catch { await route.fulfill({ status: 404, body: '' }); }
    });
    await page.addInitScript(({ fixture, scenario }) => {
      localStorage.setItem('autojev.preferences.v1', JSON.stringify({ language: 'en', theme: 'light', accent: 'orange', autoCheckUpdates: false }));
      const job = { running: false, cancelled: false, completed: 0, total: 0, completed_models: 0, total_models: 0, current_models: [], error: null };
      const state = window.__PR44_ALL_PROOF__ = { fixture, scenario, calls: [], spent: 0, completed: 0, performance: { models: {}, job, settings: { enabled: false, interval_minutes: 30 } } };
      const spend = count => { state.spent += count; state.fixture.subscriptions[0].generation_calls_remaining -= count; };
      const chatBody = { id: 'local-fixture-response', model: 'fixture-model', choices: [{ message: { role: 'assistant', content: 'OK' }, finish_reason: 'stop' }] };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => 1,
        invoke: async (command, args = {}) => {
          state.calls.push({ command, spentBeforeCall: state.spent, args: { ...args, onProgress: args.onProgress ? '[isolated Channel]' : undefined } });
          if (command === 'get_snapshot') return structuredClone(state.fixture);
          if (command === 'test_provider' || command === 'test_provider_draft') {
            spend(1); state.completed += 1;
            if (scenario.failure) throw new Error('Isolated admitted attempt failed after consuming one request.');
            return 'Isolated IPC success; no OAuth or model request was sent.';
          }
          if (command === 'debug_request' || command === 'debug_curl') {
            spend(1); state.completed += 1;
            if (command === 'debug_curl' && args.onProgress?.onmessage) args.onProgress.onmessage({ started: true });
            if (scenario.failure) throw new Error('Isolated admitted Debug attempt failed after consuming one request.');
            if (command === 'debug_curl' && scenario.cancel) {
              return await new Promise(resolve => {
                state.finishCurl = () => resolve({ status: 0, elapsed_ms: 1, body: '', telemetry: null });
              });
            }
            const body = args.endpoint === 'responses' ? { id: 'local-fixture-response', model: 'fixture-model', status: 'completed', output: [{ type: 'message', role: 'assistant', content: [{ type: 'output_text', text: 'OK' }] }] } : args.endpoint === 'messages' ? { id: 'local-fixture-response', type: 'message', role: 'assistant', model: 'fixture-model', content: [{ type: 'text', text: 'OK' }], stop_reason: 'end_turn' } : chatBody;
            return { status: 200, model: 'fixture-model', source: 'local', elapsed_ms: 1, body: JSON.stringify(body), parsed_body: body, request_body: { model: 'autojev/model/codex-model', messages: [{ role: 'user', content: 'fictional input' }] }, telemetry: null };
          }
          if (command === 'cancel_debug_curl') { state.finishCurl?.(); return null; }
          if (command === 'start_model_speed_tests') {
            spend(scenario.asyncSpeed ? 1 : scenario.spent);
            Object.assign(job, { running: Boolean(scenario.cancel || scenario.asyncSpeed), cancelled: false, completed: scenario.cancel ? 0 : scenario.asyncSpeed ? 1 : scenario.spent, total: 3, total_models: 1, completed_models: scenario.cancel || scenario.asyncSpeed ? 0 : 1, current_models: scenario.cancel || scenario.asyncSpeed ? ['codex-model'] : [], error: scenario.failure ? 'Isolated admitted speed attempt failed.' : null });
            if (scenario.asyncSpeed) {
              setTimeout(() => {
                spend(2);
                Object.assign(job, { running: false, completed: 3, completed_models: 1, current_models: [] });
                state.backgroundComplete = true;
                state.calls.push({ command: '[isolated background HTTP attempts 2 and 3 completed]', spentBeforeCall: state.spent });
              }, 1400);
            }
            state.completed += 1;
            return null;
          }
          if (command === 'cancel_model_speed_tests') { Object.assign(job, { running: false, cancelled: true, completed: 1, current_models: [] }); return null; }
          if (command === 'get_model_performance') return structuredClone(state.performance);
          if (command === 'get_gateway_health' || command === 'get_request_logs') return [];
          if (command === 'plugin:app|version') return '0.1.2';
          if (command.startsWith('plugin:event|')) return 1;
          if (command === 'get_window_state') return null;
          return null;
        },
      };
    }, { fixture, scenario });
    try {
      await page.goto('http://127.0.0.1:1420');
      await page.getByRole('button', { name: 'Providers', exact: true }).click();
      const budget = page.locator('[data-testid="codex-generation-budget-codex-fixture"]');
      await budget.waitFor();
      const before = await budget.innerText();
      assert.match(before, /3 of 3/);
      if (scenario.entry === 'provider-row') {
        await page.locator('[data-testid="provider-test-codex-fixture"]').click();
      } else if (scenario.entry === 'provider-dialog') {
        await page.locator('[data-testid="provider-configure-codex-fixture"]').click();
        await page.getByRole('dialog').getByRole('button', { name: 'Test', exact: true }).click();
      } else if (scenario.entry.startsWith('model-') || scenario.entry === 'speed') {
        await page.getByRole('button', { name: 'Models', exact: true }).click();
        if (scenario.entry === 'model-row') await page.locator('.models-table tbody').getByRole('button', { name: 'Test', exact: true }).click();
        else if (scenario.entry === 'model-dialog') {
          await page.locator('[data-testid="model-configure-codex-model"]').click();
          await page.getByRole('dialog').getByRole('button', { name: 'Test', exact: true }).click();
        } else {
          await page.getByRole('checkbox', { name: 'Select model for speed test: codex-fixture/fixture-model', exact: true }).check();
          await page.getByRole('button', { name: 'Test selected (1)', exact: true }).click();
      if (scenario.cancel) await page.getByRole('button', { name: 'Stop speed tests', exact: true }).click();
        }
      } else {
        await page.getByRole('button', { name: 'Debug', exact: true }).click();
        await page.getByRole('button', { name: 'Select model or route', exact: true }).click();
        await page.getByRole('option', { name: 'codex-fixture/fixture-model', exact: true }).click();
        if (scenario.entry === 'debug-curl') {
          await page.getByRole('tab', { name: 'cURL mode', exact: true }).click();
          await page.getByRole('button', { name: 'Execute', exact: true }).click();
          if (scenario.cancel) {
            await page.waitForFunction(() => {
              const button = [...document.querySelectorAll('button')].find(item => item.textContent?.trim() === 'Stop');
              return button && !button.disabled;
            });
            await page.getByRole('button', { name: 'Stop', exact: true }).click();
          }
        } else {
          if (scenario.protocol !== 'chat/completions') {
            await page.getByRole('button', { name: 'API type', exact: true }).click();
            await page.getByRole('option', { name: scenario.protocol === 'responses' ? 'OpenAI Responses' : 'Anthropic Messages', exact: true }).click();
          }
          await page.getByRole('textbox', { name: 'Test prompt', exact: true }).fill('fictional input');
          if (scenario.stream) {
            await page.getByRole('button', { name: 'Add parameter', exact: true }).click();
            await page.getByRole('textbox', { name: 'Parameter name', exact: true }).last().fill('stream');
            await page.getByRole('textbox', { name: 'Parameter value', exact: true }).last().fill('true');
          }
          await page.getByRole('button', { name: 'Send test request', exact: true }).click();
        }
      }
      await page.waitForFunction(() => window.__PR44_ALL_PROOF__.completed === 1);
      const asyncStages = {};
      if (scenario.asyncSpeed) {
        await page.waitForFunction(() => window.__PR44_ALL_PROOF__.calls.some(call => call.command === 'get_snapshot' && call.spentBeforeCall === 1));
        if (scenario.leaveModels) {
          await page.getByRole('button', { name: 'Providers', exact: true }).click();
          await budget.waitFor();
          asyncStages.afterFirstRequest = await budget.innerText();
          await page.waitForFunction(() => window.__PR44_ALL_PROOF__.backgroundComplete === true);
          await page.waitForTimeout(2300);
          asyncStages.whileOnProviders = await budget.innerText();
          await page.getByRole('button', { name: 'Models', exact: true }).click();
          await page.getByRole('button', { name: 'Providers', exact: true }).click();
          await page.waitForTimeout(2300);
          asyncStages.afterNavigationBack = await budget.innerText();
        } else {
          await page.waitForFunction(() => window.__PR44_ALL_PROOF__.backgroundComplete === true);
          await page.waitForTimeout(2300);
        }
      }
      if (await page.getByRole('dialog').count()) {
        const close = page.getByRole('dialog').getByRole('button', { name: 'Close', exact: true });
        if (await close.count()) await close.click();
      }
      await page.getByRole('button', { name: 'Providers', exact: true }).click();
      await budget.waitFor();
      await page.waitForTimeout(300);
      const after = await budget.innerText();
      const state = await page.evaluate(() => window.__PR44_ALL_PROOF__);
      const spent = scenario.spent ?? 1;
      assert.equal(state.spent, spent, 'The intended UI entry must have reached exactly one isolated consuming IPC action');
      assert.equal(state.fixture.subscriptions[0].generation_calls_remaining, 3 - spent);
      if (scenario.entry === 'debug') {
        const call = state.calls.find(call => call.command === 'debug_request');
        assert.equal(call.args.endpoint, scenario.protocol);
        assert.equal(call.args.parameters.stream === true, Boolean(scenario.stream));
      }
      if (scenario.cancel && scenario.entry === 'speed') assert.equal(state.calls.filter(call => call.command === 'cancel_model_speed_tests').length, 1);
      if (scenario.cancel && scenario.entry === 'debug-curl') assert.equal(state.calls.filter(call => call.command === 'cancel_debug_curl').length, 1);
      assert.deepEqual(errors, []);
      assert.deepEqual(blockedExternal, [], 'The browser must not contact any external auth or model endpoint.');
      const stale = !after.includes(`${3 - spent} of 3`) || (scenario.asyncSpeed && (!asyncStages.whileOnProviders?.includes('0 of 3') || !asyncStages.afterNavigationBack?.includes('0 of 3')));
      const result = { name: scenario.name, spent, before, after, backendRemaining: 3 - spent, stale, asyncStages, snapshotReads: state.calls.filter(call => call.command === 'get_snapshot').length, errors, blockedExternal, calls: state.calls, fakeBoundary: 'Entire browser IPC is mocked; all assets come from the local build; external requests are blocked; no OAuth, quota, model or backend admission call.' };
      await writeFile(join(artifacts, `${scenario.name}.json`), JSON.stringify(result, null, 2));
      await page.screenshot({ path: join(artifacts, `${scenario.name}.png`), fullPage: true });
      results.push(result);
      console.log(JSON.stringify({ name: result.name, displayed: after, backendRemaining: result.backendRemaining, stale, snapshotReads: result.snapshotReads }));
    } catch (error) {
      await page.screenshot({ path: join(artifacts, `${scenario.name}-setup-error.png`), fullPage: true }).catch(() => {});
      await writeFile(join(artifacts, `${scenario.name}-setup-error.html`), await page.content());
      throw error;
    } finally { await page.close(); }
  }
  const report = { sha, base: '697e947aa021aef572dca267612858ad71e5bdc6', repo, appSourceSHA256: createHash('sha256').update(app).digest('hex'), buildIndex: await readFile(join(repo, 'dist/index.html'), 'utf8'), count: results.length, staleCount: results.filter(result => result.stale).length, results };
  await writeFile(join(artifacts, 'report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ report: join(artifacts, 'report.json'), sha, count: report.count, staleCount: report.staleCount }));
  assert.equal(report.staleCount, 0, 'Every consuming UI entry must show the current shared generation-call budget after success, failure, or cancellation.');
} finally { await browser.close(); }
