import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const pkg = JSON.parse(readFileSync('package.json', 'utf8'));
const config = JSON.parse(readFileSync('src-tauri/tauri.conf.json', 'utf8'));
const cargo = readFileSync('src-tauri/Cargo.toml', 'utf8').match(/^version = "([^"]+)"/m)?.[1];
assert.match(pkg.version, /^\d+\.\d+\.\d+$/, 'Release version must be stable semver');
assert.equal(pkg.version, config.version, 'package.json and Tauri versions differ');
assert.equal(pkg.version, cargo, 'package.json and Cargo versions differ');
if (process.env.GITHUB_REF_TYPE === 'tag') {
  assert.equal(process.env.GITHUB_REF_NAME, `v${pkg.version}`, 'Tag must match the app version');
}
assert.equal(config.bundle.createUpdaterArtifacts, true);
assert.equal(pkg.license, 'AGPL-3.0-only');
assert.equal(config.bundle.license, 'AGPL-3.0-only');
assert.equal(config.bundle.licenseFile, '../LICENSE');
const licenseText = readFileSync('LICENSE', 'utf8');
assert.match(licenseText, /^AutoJev\n/);
assert.match(licenseText, /GNU AFFERO GENERAL PUBLIC LICENSE/);
assert.match(licenseText, /Version 3, 19 November 2007/);
assert.match(readFileSync('src-tauri/Cargo.toml', 'utf8'), /^license = "AGPL-3.0-only"$/m);
assert.equal(JSON.parse(readFileSync('src-tauri/capabilities/default.json', 'utf8')).permissions.includes('updater:default'), true);
const publicKey = Buffer.from(config.plugins.updater.pubkey, 'base64').toString('utf8');
assert.match(publicKey, /^untrusted comment: minisign public key:/, 'Invalid updater public key');
assert.ok(config.plugins.updater.endpoints.every(url => url.startsWith('https://')));
// The feed is written by scripts/publish-release.sh (publish-cdn.yml).
assert.deepEqual(config.plugins.updater.endpoints, ['https://cdn.autojev.ai/latest.json'], 'Updater endpoint must be the CDN feed');
console.log(`Release configuration OK: v${pkg.version}`);
