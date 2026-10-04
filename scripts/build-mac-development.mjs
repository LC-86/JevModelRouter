// Build a local, unsigned arm64 development folder. No install/download/auth.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cp, mkdir, readFile, writeFile, readdir, access } from 'node:fs/promises';
import { resolve, join, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import { collectDevelopmentNotices } from './development-notices.mjs';

const checkout = fileURLToPath(new URL('../', import.meta.url));
const [cpa, output, target, cpaModuleCache, goLicense, mode] = process.argv.slice(2);
assert.ok(!mode || mode === '--product', 'Optional mode is --product');
const product = mode === '--product';
assert.ok([cpa, output, target, cpaModuleCache, goLicense, process.env.CARGO_HOME].every(p => p && isAbsolute(p)), 'Provide absolute CPA, new output, owned Cargo target/cache, CPA module cache and Go license paths');
assert.equal(process.platform, 'darwin'); assert.equal(process.arch, 'arm64', 'Only the pinned Mac arm64 artifact has been verified');
assert.ok(!resolve(output).startsWith(resolve(checkout)), 'Keep the artifact outside the source checkout');
let exists = true; try { await access(output); } catch (e) { if (e.code === 'ENOENT') exists = false; else throw e; }
assert.equal(exists, false, 'Output must be new; never overwrite another artifact');
const git = args => execFileSync('git', args, { cwd: checkout, encoding: 'utf8' }).trim();
assert.equal(git(['status', '--porcelain']), '', 'Commit the complete source before building a traceable artifact');
const source = git(['rev-parse', 'HEAD']);
const pin = JSON.parse(await readFile(new URL('./cpa-artifact.json', import.meta.url), 'utf8'));
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
assert.equal(hash(await readFile(cpa)), pin.binary_sha256, 'CPA checksum mismatch');
assert.equal(hash(await readFile(`${cpa}.LICENSE`)), pin.license_sha256, 'CPA license mismatch');
const env = { ...process.env, TAURI_DEV_HOST: '127.0.0.1', CARGO_TARGET_DIR: target, AUTOJEV_DEVELOPMENT_ISOLATION: '1' };
if (product) delete env.AUTOJEV_DEVELOPMENT_ISOLATION;
console.log(`Build Jev ${source}: frontend, then ${product ? 'ordinary product' : 'isolation-check'} native binary`);
execFileSync('pnpm', ['build'], { cwd: checkout, env, stdio: 'inherit' });
const nativeArgs = ['build', '--locked', '--offline', '--manifest-path', 'src-tauri/Cargo.toml'];
if (!product) nativeArgs.push('--features', 'isolation-check');
execFileSync('cargo', nativeArgs, { cwd: checkout, env, stdio: 'inherit' });
assert.equal(git(['status', '--porcelain']), '', 'Source changed during build');
assert.equal(git(['rev-parse', 'HEAD']), source, 'Source commit changed during build');
const binary = join(target, 'debug/autojev');
if (!product) {
  const bare = spawnSync(binary, [], { env: { PATH: '/usr/bin:/bin' }, encoding: 'utf8', timeout: 10000 });
  assert.equal(bare.status, 2, 'The isolated artifact must refuse ordinary startup');
  assert.match(bare.stderr, /requires the isolated runner/);
}
for (const path of [binary, cpa]) assert.match(execFileSync('/usr/bin/file', [path], { encoding: 'utf8' }), /Mach-O 64-bit executable arm64/);
await mkdir(output); await mkdir(join(output, 'bin')); await mkdir(join(output, 'scripts'));
await cp(binary, join(output, 'bin/jev')); await cp(cpa, join(output, 'bin/cpa'));
await cp(`${cpa}.LICENSE`, join(output, 'CPA.LICENSE'));
await cp(join(checkout, 'LICENSE'), join(output, 'JEV.LICENSE')); await cp(join(checkout, 'NOTICE'), join(output, 'NOTICE'));
const notices = await collectDevelopmentNotices({ checkout, output, cargoHome: process.env.CARGO_HOME, cpaModuleCache, goLicense });
await cp(join(checkout, 'dist'), join(output, 'web'), { recursive: true });
await mkdir(join(output, 'docs'));
await cp(join(checkout, 'docs/testing'), join(output, 'docs/testing'), { recursive: true });
await cp(join(checkout, 'docs/screenshots'), join(output, 'docs/screenshots'), { recursive: true });
for (const name of ['check-dsh-desktop.mjs', 'check-product-desktop.mjs', 'product-desktop-driver.js', 'launch-product.mjs', 'dsh-upstream-fixture.mjs', 'cpa-artifact.json', 'cpa-owned-window.swift']) await cp(join(checkout, 'scripts', name), join(output, 'scripts', name));
await writeFile(join(output, 'jev-source.tar'), execFileSync('git', ['archive', '--format=tar', source], { cwd: checkout, maxBuffer: 64 * 1024 * 1024 }));
await writeFile(join(output, 'RUN.md'), `# Isolated Mac development folder\n\nSource: ${source}\n\nRequires existing Node.js 22+ and macOS arm64. No dependency install is needed to run this folder. From any directory, run:\n\n\`node /absolute/folder/scripts/check-dsh-desktop.mjs /absolute/folder/bin/jev /absolute/folder/bin/cpa --web-root /absolute/folder/web --service-check --capture\`\n\nUse \`--manual\` instead of \`--service-check --capture\` for the isolated desktop without automatic requests. The Providers service panel starts only the pinned fictional CPA. Quit via the tray, or Ctrl-C to reclaim this process group. Bare bin/jev refuses startup before any ordinary profile is opened.\n\nEach launch creates and prints a new temporary home with fictional upstream; it never reads daily DSH/CLI configuration. Real credentials, login, quota and generation are disabled. Reports and owned screenshots remain in the printed temporary directory.\n\nSee docs/testing/mac-service-r6.md and docs/testing/relay-hand-run.md. Actual DSH and all real-source results remain assigned to #58.\n`);
if (product) await writeFile(join(output,'RUN.md'),`# Ordinary Mac development folder\n\nSource: ${source}\n\nRequires macOS arm64 and existing Node.js 22+. Start an ordinary desktop using a new empty or previously owned persistent profile:\n\n\`node /absolute/folder/scripts/launch-product.mjs /absolute/dedicated-profile\`\n\nAdd \`--offline\` for loopback-only validation. The binary is built without isolation-check and without forced isolation. Its static frontend is included; the launcher injects no driver or fixtures. Providers → CPA: choose Codex or xAI, create a connection, select \`/absolute/folder/bin/cpa\`, start the pinned owned service. Startup, save and recovery never log in, query account quota or grant generation. All CPA capabilities start disabled. Only independently reviewed, fresh evidence plus an explicit finite plan can enable the verified Chat path. Grok plan/cost facts remain unknown. Coding Plan has its own reviewed-evidence entry and finite permission bound to its exact plan endpoint; see docs/testing/relay-hand-run.md. No real actions were performed when building or validating this artifact.\n\nQuit in the desktop or Ctrl-C to reclaim this launcher's owned process group. Reopening preserves the dedicated profile and port, but never restores volatile generation permission. No system service, signature or notarization is installed.\n\nOffline ordinary UI verification (wrong artifact, port occupancy, own service exit/recovery, app reopen) creates a separate temporary profile:\n\n\`node /absolute/folder/scripts/check-product-desktop.mjs /absolute/folder/bin/jev /absolute/folder/web /absolute/folder/bin/cpa\`\n\nReal credentials, OAuth, quota and model results belong only in #58 after separate approval.\n`);
const files = {};
async function inventory(dir, prefix = '') {
  for (const item of await readdir(dir, { withFileTypes: true })) {
    const relative = `${prefix}${item.name}`;
    if (item.isDirectory()) await inventory(join(dir, item.name), `${relative}/`);
    else files[relative] = hash(await readFile(join(dir, item.name)));
  }
}
await inventory(output);
await writeFile(join(output, 'manifest.json'), JSON.stringify({ schema_version: 1, kind: product?'unsigned-ordinary-development-folder':'unsigned-isolated-development-folder', platform: 'darwin/arm64', built_at: new Date().toISOString(), jev: { repository: 'https://github.com/LC-86/JevModelRouter', source, version: '0.1.2', license: 'AGPL-3.0-only' }, cpa: pin, real_capabilities_enabled: false, dependency_notice_count: notices, isolation_check:!product,build_commands:['pnpm build',`cargo ${nativeArgs.join(' ')}`], files }, null, 2) + '\n');
console.log(`Development artifact: ${output}; manifest SHA-256 ${hash(await readFile(join(output, 'manifest.json')))}`);
