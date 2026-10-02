// Offline UI acceptance: only a synthetic Tauri bridge and loopback static server.
import assert from 'node:assert/strict';
import { chromium } from 'playwright';
import { spawn, execFileSync } from 'node:child_process';
import { readFile, mkdir, writeFile } from 'node:fs/promises';
import { resolve, join } from 'node:path';
import { createHash } from 'node:crypto';
const artifacts = resolve(process.env.AUTOJEV_ARTIFACT_DIR || 'artifacts/grok-readonly-ui');
await mkdir(artifacts, { recursive: true });
const bridge = await readFile('src/lib/bridge.ts', 'utf8');
const literal = bridge.split('const MOCK: DashboardSnapshot = ')[1].split('\n};')[0] + '\n}';
const snapshot = Function('return (' + literal + ')')();
const server = spawn(process.execPath, ['node_modules/vite/bin/vite.js', 'preview', '--host', '127.0.0.1', '--port', '1439', '--strictPort'], { stdio: 'pipe' });
let browser;
try {
  for (let i = 0; i < 100; i++) {
    if (server.exitCode !== null) throw new Error('Owned loopback preview server exited');
    try { if ((await fetch('http://127.0.0.1:1439')).ok) break; } catch {}
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  browser = await chromium.launch({ channel: process.env.AUTOJEV_BROWSER_CHANNEL || 'chrome', headless: true });
  const results = [];
  for (const mode of ['success', 'sparse', '401', 'cancel']) {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1050 }, locale: 'en-US' });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.route('**/*', route => {
      assert.equal(new URL(route.request().url()).origin, 'http://127.0.0.1:1439', 'No external request is allowed');
      return route.continue();
    });
    await page.clock.install({ time: new Date('2030-01-01T00:00:00Z') });
    await page.addInitScript(({ snapshot, mode }) => {
      localStorage.setItem('autojev.preferences.v1', JSON.stringify({ language: 'en', theme: 'light', accent: 'orange', autoCheckUpdates: false }));
      const field = value => ({ state: value === null ? 'null' : 'available', value });
      const result = {
        observedAt: '2030-01-01T00:00:00Z', models: field(['grok-example']), currentModel: field('grok-example'),
        usagePercent: field(mode === 'sparse' ? null : 25), remainingPercent: field(mode === 'sparse' ? null : 75),
        subscriptionTier: field('ExamplePlan'), periodType: field('USAGE_PERIOD_TYPE_WEEKLY'),
        periodStart: field('2030-01-01T00:00:00Z'), periodEnd: field('2030-01-08T00:00:00Z'),
        billingPeriodEnd: field('2030-01-08T00:00:00Z'), periodConflict: false, realGenerationEnabled: false,
      };
      const state = window.__READONLY_FIXTURE__ = { calls: [], finish: null };
      window.__TAURI_INTERNALS__ = {
        metadata: { currentWindow: { label: 'main' }, currentWebview: { label: 'main' } },
        transformCallback: () => 1, unregisterCallback: () => {},
        invoke: async (command, args = {}) => {
          state.calls.push({ command, args });
          if (command === 'get_snapshot') return structuredClone(snapshot);
          if (command === 'refresh_grok_readonly') {
            if (mode === '401') throw 'rpc_error 401: authentication required in the official Grok CLI';
            if (mode === 'cancel') return await new Promise(resolve => { state.finish = () => resolve(result); });
            return structuredClone(result);
          }
          if (command === 'cancel_grok_readonly') { state.finish?.(); return null; }
          if (command === 'plugin:app|version') return '0.1.2';
          if (command.startsWith('plugin:event|')) return 1;
          return null;
        },
      };
    }, { snapshot, mode });
    await page.goto('http://127.0.0.1:1439');
    await page.getByRole('button', { name: 'Providers', exact: true }).click();
    const panel = page.getByTestId('grok-readonly-status');
    await panel.waitFor();
    assert.equal(await page.evaluate(() => window.__READONLY_FIXTURE__.calls.filter(v => v.command === 'refresh_grok_readonly').length), 0, 'No automatic account read');
    assert.equal(await page.getByTestId('grok-readonly-generation').innerText(), 'Off');
    await page.getByTestId('grok-readonly-refresh').click();
    if (mode === 'cancel') {
      await page.getByTestId('grok-readonly-cancel').click();
      await page.getByTestId('grok-readonly-error').waitFor();
      assert.equal(await page.getByTestId('grok-readonly-models').innerText(), 'Unknown');
    } else if (mode === '401') {
      await page.getByTestId('grok-readonly-error').waitFor();
      assert.match(await page.getByTestId('grok-readonly-error').innerText(), /401/);
      assert.equal(await page.getByTestId('grok-readonly-models').innerText(), 'Unknown');
    } else {
      await page.getByTestId('grok-readonly-models').getByText('grok-example', { exact: true }).waitFor();
      assert.match(await page.getByTestId('grok-readonly-billing').innerText(), mode === 'sparse' ? /Unknown \(null\)/ : /25% used.*75% remaining/);
      await panel.screenshot({ path: join(artifacts, `${mode}.png`) });
      if (mode === 'success') {
        await page.clock.fastForward(300000);
        assert.match(await page.getByTestId('grok-readonly-updated').innerText(), /Expired/);
        assert.equal(await page.getByTestId('grok-readonly-models').innerText(), 'Unknown');
      }
    }
    assert.equal(await page.getByTestId('grok-readonly-generation').innerText(), 'Off');
    const calls = await page.evaluate(() => window.__READONLY_FIXTURE__.calls);
    assert.equal(calls.filter(v => v.command === 'refresh_grok_readonly').length, 1);
    assert.equal(calls.filter(v => /generate|set_grok_real|test_provider|test_model/.test(v.command)).length, 0);
    assert.deepEqual(errors, []);
    results.push({ mode, result: 'passed', manualReads: 1, generationRequests: 0 });
    await page.close();
  }
  const sourceSha = execFileSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).trim();
  const files = {};
  for (const path of ['src/components/grok-readonly-status.tsx', 'src/lib/grok-readonly.ts', 'src-tauri/src/subscription/grok/readonly.rs', 'dist/index.html']) {
    files[path] = createHash('sha256').update(await readFile(path)).digest('hex');
  }
  await writeFile(join(artifacts, 'evidence.json'), JSON.stringify({ sourceSha, files, results, synthetic: true }, null, 2) + '\n');
  console.log(JSON.stringify({ artifacts, results }));
} finally {
  await browser?.close();
  server.kill('SIGTERM');
  await new Promise(resolve => { if (server.exitCode !== null) resolve(); else server.once('exit', resolve); });
}
