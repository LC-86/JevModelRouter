import { spawnSync } from 'node:child_process';
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

process.chdir(resolve(dirname(fileURLToPath(import.meta.url)), '..'));
if (process.platform !== 'darwin') throw new Error('Run release:mac on macOS.');
const args = process.argv.slice(2);
const checkOnly = args.includes('--check');
const env = { ...process.env };
for (const name of ['APPLE_SIGNING_IDENTITY', 'APPLE_ID', 'APPLE_PASSWORD', 'APPLE_TEAM_ID']) {
  if (!env[name]) throw new Error(`Set ${name} before building a release (use the same credentials as Termany).`);
}
// Only read local credentials; never copy signing material into the repository.
if (!env.TAURI_SIGNING_PRIVATE_KEY) {
  const key = ['autojev-updater.key', 'termany-updater.key']
    .map(name => join(homedir(), '.tauri', name)).find(existsSync);
  if (!key) throw new Error('Set TAURI_SIGNING_PRIVATE_KEY or provide ~/.tauri/termany-updater.key.');
  const publicKey = `${key}.pub`;
  if (existsSync(publicKey)) {
    const config = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
    if (readFileSync(publicKey, 'utf8').trim() !== config.plugins.updater.pubkey.trim()) {
      throw new Error('Local updater key does not match the public key in tauri.conf.json.');
    }
  }
  env.TAURI_SIGNING_PRIVATE_KEY = readFileSync(key, 'utf8').trim();
}
env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ??= '';
function run(command, parameters) {
  const result = spawnSync(command, parameters, { env, stdio: 'inherit' });
  if (result.error) throw new Error(`Could not run ${command}: ${result.error.message}`);
  if (result.status !== 0) process.exit(result.status ?? 1);
}
run(process.execPath, ['scripts/check-release.mjs']);
if (checkOnly) {
  console.log('Local signing inputs are present. Certificate validity and Apple notarization will be checked during the build.');
} else {
  const cli = createRequire(import.meta.url).resolve('@tauri-apps/cli/tauri.js');
  run(process.execPath, [cli, 'build', '--bundles', 'app,dmg', '--locked', ...args]);
  const targetIndex = args.indexOf('--target');
  const target = targetIndex >= 0 ? args[targetIndex + 1] : args.find(arg => arg.startsWith('--target='))?.split('=')[1];
  const profile = args.includes('--debug') ? 'debug' : 'release';
  const bundle = join(resolve(env.CARGO_TARGET_DIR || 'src-tauri/target'), target || '', profile, 'bundle');
  const dmgDir = join(bundle, 'dmg');
  const installers = readdirSync(dmgDir).filter(name => name.endsWith('.dmg'));
  if (!installers.length) throw new Error('No DMG was produced.');
  for (const name of installers) {
    const dmg = join(dmgDir, name);
    run('xcrun', ['notarytool', 'submit', dmg, '--apple-id', env.APPLE_ID, '--password', env.APPLE_PASSWORD, '--team-id', env.APPLE_TEAM_ID, '--wait']);
    run('xcrun', ['stapler', 'staple', dmg]);
    run('xcrun', ['stapler', 'validate', dmg]);
  }
  console.log(`Signed bundles ready: ${bundle}. Publish the updater archive and its .sig together.`);
}
