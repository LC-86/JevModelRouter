import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const [file, tag, repository] = process.argv.slice(2);
const manifest = JSON.parse(readFileSync(file, 'utf8'));
assert.equal(manifest.version.replace(/^v/, ''), tag.replace(/^v/, ''));
for (const platform of ['darwin-aarch64', 'darwin-x86_64', 'windows-x86_64', 'linux-x86_64']) {
  const entry = manifest.platforms[platform];
  assert.ok(entry?.signature?.trim(), `${platform}: missing signature`);
  assert.ok(entry.url.startsWith(`https://github.com/${repository}/releases/download/${tag}/`), `${platform}: wrong download URL`);
}
console.log('All four signed updater platforms are present.');
