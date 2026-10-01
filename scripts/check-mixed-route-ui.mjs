import { chromium } from 'playwright';
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

const artifacts = process.env.AUTOJEV_ARTIFACT_DIR || join(tmpdir(), 'autojev-mixed-route-ui');
await mkdir(artifacts, { recursive: true });
const browser = await chromium.launch({ channel: process.env.AUTOJEV_BROWSER_CHANNEL || undefined, headless: true });
try {
  const page = await browser.newPage({ viewport: { width: 1260, height: 800 }, locale: 'zh-CN' });
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
  await page.addInitScript(() => {
    localStorage.setItem('autojev.preferences.v1', JSON.stringify({ language: 'zh-CN', theme: 'light', accent: 'orange', autoCheckUpdates: false }));
    const snapshot = {
      install_id: 'mixed-route-ui-fixture',
      routes: [],
      providers: [
        { id: 'codex-subscription', name: 'Codex Subscription', kind: 'codex_subscription', base_url: '', enabled: true, has_api_key: false, api_type: 'responses' },
        { id: 'openrouter', name: 'OpenRouter', kind: 'openrouter', base_url: 'https://openrouter.ai/api', enabled: true, has_api_key: true, api_type: 'responses' },
      ],
      models: [
        { id: 'codex-alpha', provider_id: 'codex-subscription', model_id: 'codex-alpha', name: 'Codex Alpha', tier: 'balanced', enabled: true, selected: true, supports_tools: true, supports_vision: false, supports_reasoning: false, context_window: 128000, input_cost_per_million: 0, output_cost_per_million: 0, api_type: 'responses' },
        { id: 'openrouter-qwen', provider_id: 'openrouter', model_id: 'qwen/qwen3-coder-flash', name: 'Qwen 3 Coder Flash', tier: 'fast', enabled: true, selected: true, supports_tools: true, supports_vision: false, supports_reasoning: false, context_window: 262144, input_cost_per_million: 0.3, output_cost_per_million: 1.2, api_type: 'responses' },
      ],
      subscriptions: [],
      subscription_auth: [],
      policy: { mode: 'auto', prefer_local: false, use_jev_when_ambiguous: true, jev_endpoint: 'https://openrouter.ai/api/alpha/decisions', decision_provider: 'openrouter', jev_model: '~typesafe/jev-latest', has_autojev_key: false, savings_baseline_model_id: null },
      proxy: { running: true, port: 9526, base_url: 'http://127.0.0.1:9526' },
      agents: [],
      events: [],
    };
    let callbackId = 0;
    const callbacks = new Map();
    const harness = { snapshot, calls: [], failNextRouteSave: false };
    window.__ROUTE_UI_TEST__ = harness;
    window.__TAURI_INTERNALS__ = {
      transformCallback(callback) { const id = ++callbackId; callbacks.set(id, callback); return id; },
      unregisterCallback(id) { callbacks.delete(id); },
      async invoke(command, args = {}) {
        harness.calls.push({ command, args: structuredClone(args) });
        if (command === 'get_snapshot') return structuredClone(harness.snapshot);
        if (command === 'get_model_performance') return { settings: { enabled: true, interval_minutes: 30 }, models: {}, job: { running: false, completed: 0, total: 0, completed_models: 0, total_models: 0, current_models: [], error: null } };
        if (command === 'plugin:app|version') return '0.1.2';
        if (command === 'plugin:event|listen') return 1;
        if (command === 'plugin:event|unlisten') return null;
        if (command === 'save_route') {
          if (harness.failNextRouteSave) { harness.failNextRouteSave = false; throw new Error('Controlled route save failure'); }
          harness.snapshot.routes = [...harness.snapshot.routes.filter(route => route.id !== (args.originalId ?? args.route.id)), args.route];
          return structuredClone(harness.snapshot);
        }
        throw new Error(`Unexpected IPC command: ${command}`);
      },
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener() {} };
    window.__TAURI_INTERNALS__.metadata = { currentWindow: { label: 'main' } };
  });

  await page.goto(process.env.AUTOJEV_PREVIEW_URL || 'http://localhost:1420');
  await page.locator('nav .nav-item').filter({ hasText: '路由' }).click();
  await page.getByRole('button', { name: '添加路由', exact: true }).click();
  await page.locator('.route-dialog-form input').first().fill('mixed-safe');
  await page.getByLabel('调度方式').click();
  await page.getByRole('option', { name: '负载均衡', exact: true }).click();

  const selectCandidate = async (index, option) => {
    await page.getByRole('button', { name: '添加模型', exact: true }).click();
    await page.locator('.route-model-row .search-select-trigger').nth(index).click();
    await page.getByRole('option', { name: option, exact: true }).click();
  };
  await selectCandidate(0, 'codex-subscription/codex-alpha');
  await selectCandidate(1, 'openrouter/qwen/qwen3-coder-flash');

  await page.evaluate(() => { window.__ROUTE_UI_TEST__.failNextRouteSave = true; });
  await page.getByRole('button', { name: '保存路由', exact: true }).click();
  await page.getByRole('alert').filter({ hasText: 'Controlled route save failure' }).waitFor();
  assert.equal(await page.getByRole('dialog').count(), 1, 'save error keeps the editable route dialog open');

  await page.getByRole('button', { name: '保存路由', exact: true }).click();
  await page.getByRole('dialog').waitFor({ state: 'detached' });
  const saved = await page.evaluate(() => window.__ROUTE_UI_TEST__.snapshot.routes[0]);
  assert.equal(saved.id, 'mixed-safe');
  assert.equal(saved.strategy, 'round_robin');
  assert.equal(saved.all_models, false);
  assert.deepEqual(saved.model_ids, ['codex-alpha', 'openrouter-qwen']);

  await page.getByRole('button', { name: '编辑路由', exact: true }).click();
  const reopened = await page.locator('.route-model-row .search-select-trigger').evaluateAll(buttons => buttons.map(button => button.innerText));
  assert.ok(reopened.some(value => value.includes('codex-subscription/codex-alpha')));
  assert.ok(reopened.some(value => value.includes('openrouter/qwen/qwen3-coder-flash')));
  const captured = await page.evaluate(() => window.__ROUTE_UI_TEST__.calls.filter(call => call.command === 'save_route').map(call => call.args.route));
  assert.equal(captured.length, 2, 'the failed save was surfaced and the retry was sent through the UI');
  assert.deepEqual(errors, []);
  await page.locator('.modal-backdrop').evaluate(el => Promise.all(el.getAnimations().map(animation => animation.finished)));
  await page.screenshot({ path: join(artifacts, 'mixed-route-reopened.png') });
  console.log(JSON.stringify({ result: 'mixed subscription/API candidates saved, reopened, and save failure surfaced', route: saved, reopened, screenshot: join(artifacts, 'mixed-route-reopened.png'), pageErrors: errors }));
} finally {
  await browser.close();
}
