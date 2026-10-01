import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { join, relative, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const repo = fileURLToPath(new URL('..', import.meta.url));
const browser = await chromium.launch({ channel: process.env.AUTOJEV_BROWSER_CHANNEL || 'chrome', headless: true });
try {
  const bridge = await readFile(join(repo, 'src/lib/bridge.ts'), 'utf8');
  const literal = bridge.split('const MOCK: DashboardSnapshot = ')[1].split('\n};')[0] + '\n}';
  const fixture = Function(`return (${literal})`)();
  fixture.providers = [{ id: 'codex-fixture', name: 'Isolated Codex fixture', kind: 'codex_subscription', enabled: true, has_api_key: false, base_url: '', api_type: '', test_model: 'fixture-model' }];
  fixture.models = [];
  fixture.agents = [];
  fixture.subscriptions = [{
    provider_id: 'codex-fixture', label: 'Codex', generation: 7, state: 'connected', identity: 'fictional@example.invalid',
    helper_version: 'fixture-1', models: [], capabilities: [], catalog_entries: [],
    quota: { state: 'unknown' }, catalog: { state: 'unknown' }, adapter_available: true,
    real_generation_enabled: true, generation_call_limit: 3, generation_calls_remaining: 3,
    denial: null, admission_denial: null,
  }];

  const page = await browser.newPage({ viewport: { width: 1440, height: 960 }, locale: 'en-US' });
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/*', async route => {
    const url = new URL(route.request().url());
    if (url.origin !== 'http://127.0.0.1:1420') { await route.abort(); return; }
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
  await page.addInitScript(initialFixture => {
    localStorage.setItem('autojev.preferences.v1', JSON.stringify({ language: 'en', theme: 'light', accent: 'orange', autoCheckUpdates: false }));
    window.__CODEX_BUDGET_UI__ = { fixture: initialFixture, calls: [], tests: 0 };
    window.__TAURI_INTERNALS__ = {
      metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
      transformCallback: () => 1,
      invoke: async (command, args = {}) => {
        const state = window.__CODEX_BUDGET_UI__;
        state.calls.push({ command, args });
        if (command === 'get_snapshot') return structuredClone(state.fixture);
        if (command === 'test_provider') {
          state.tests += 1;
          state.fixture.subscriptions[0].generation_calls_remaining -= 1;
          return 'Isolated IPC success; no OAuth or model request was sent.';
        }
        if (command === 'plugin:app|version') return '0.1.2';
        if (command.startsWith('plugin:event|')) return 1;
        if (command === 'get_dashboard_activity') return { requests: [], totals: {}, hourly: [] };
        if (command === 'get_gateway_health') return [];
        if (command === 'get_model_performance') return { models: {}, job: { running: false, total: 0 }, settings: { enabled: false, interval_minutes: 30 } };
        if (command === 'get_window_state') return null;
        return null;
      },
    };
  }, fixture);

  await page.goto('http://127.0.0.1:1420');
  await page.getByRole('button', { name: 'Providers', exact: true }).click();
  const budget = page.locator('[data-testid="codex-generation-budget-codex-fixture"]');
  await budget.waitFor();
  const before = await budget.innerText();
  await page.locator('[data-testid="provider-test-codex-fixture"]').click();
  await page.waitForFunction(() => window.__CODEX_BUDGET_UI__.tests === 1);
  await page.waitForFunction(() => document.querySelector('[data-testid="codex-generation-budget-codex-fixture"]')?.textContent?.includes('2 of 3'));
  const after = await budget.innerText();
  const backendRemaining = await page.evaluate(() => window.__CODEX_BUDGET_UI__.fixture.subscriptions[0].generation_calls_remaining);
  assert.match(before, /3 of 3/);
  assert.match(after, /2 of 3/);
  assert.equal(backendRemaining, 2);
  await page.getByRole('button', { name: 'Debug', exact: true }).click();
  await page.getByRole('button', { name: 'Providers', exact: true }).click();
  const afterNavigation = await budget.innerText();
  assert.match(afterNavigation, /2 of 3/, 'navigation must preserve the refreshed budget snapshot');
  assert.equal(await page.evaluate(() => window.__CODEX_BUDGET_UI__.calls.filter(call => call.command === 'test_provider').length), 1);
  assert.ok(await page.evaluate(() => window.__CODEX_BUDGET_UI__.calls.filter(call => call.command === 'get_snapshot').length) >= 2);
  assert.deepEqual(errors, []);
  console.log(JSON.stringify({ test: 'mocked successful subscription Test refreshes and retains the visible Codex request budget', before, after, afterNavigation, backendRemaining, errors }, null, 2));
} finally {
  await browser.close();
}
