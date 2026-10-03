import assert from 'node:assert/strict';
import { cp, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';

// Retain notices from the installed, locked build inputs. This inventory can
// include build-only dependencies; it does not relicense the delivered binaries.
export async function collectDevelopmentNotices({ checkout, output, cargoHome, cpaModuleCache, goLicense }) {
  const destination = join(output, 'THIRD_PARTY_LICENSES');
  await mkdir(destination);
  const copied = [];
  const notice = /^(licen[cs]e|copying|copyright|notice|ofl)([._-].*)?$/i;
  const copyNotices = async (root, label) => {
    for (const item of await readdir(root, { withFileTypes: true })) {
      if (!notice.test(item.name) && item.name.toLowerCase() !== 'licenses') continue;
      if (!item.isFile() && !item.isDirectory()) continue;
      const dir = join(destination, label); await mkdir(dir, { recursive: true });
      await cp(join(root, item.name), join(dir, item.name), { recursive: true });
      copied.push(`${label}/${item.name}`);
    }
  };
  const packages = async root => {
    for (const item of await readdir(root, { withFileTypes: true })) {
      if (!item.isDirectory()) continue;
      const path = join(root, item.name);
      if (item.name.startsWith('@')) { await packages(path); continue; }
      let pkg; try { pkg = JSON.parse(await readFile(join(path, 'package.json'), 'utf8')); } catch { continue; }
      await copyNotices(path, `node/${pkg.name.replaceAll('/', '_')}@${pkg.version}`);
    }
  };
  for (const item of await readdir(join(checkout, 'node_modules/.pnpm'), { withFileTypes: true })) {
    if (item.isDirectory() && item.name !== 'node_modules') await packages(join(checkout, 'node_modules/.pnpm', item.name, 'node_modules'));
  }
  for (const required of ['react', '@fontsource-variable/manrope', '@fontsource/ibm-plex-mono']) {
    assert.ok(copied.some(p => p.startsWith(`node/${required.replaceAll('/', '_')}@`)), `Missing bundled ${required} license`);
  }
  const lock = await readFile(join(checkout, 'src-tauri/Cargo.lock'), 'utf8');
  const locked = new Set([...lock.matchAll(/\[\[package\]\]\s+name = "([^"]+)"\s+version = "([^"]+)"/g)].map(m => `${m[1]}-${m[2]}`));
  const registry = join(cargoHome, 'registry/src');
  for (const index of await readdir(registry)) {
    for (const pkg of await readdir(join(registry, index))) if (locked.has(pkg)) await copyNotices(join(registry, index, pkg), `rust/${pkg}`);
  }
  const rustRoot = execFileSync('rustc', ['--print', 'sysroot'], { encoding: 'utf8' }).trim();
  await copyNotices(rustRoot, 'rust-toolchain');
  const modules = async root => {
    for (const item of await readdir(root, { withFileTypes: true })) {
      if (!item.isDirectory() || item.name === 'cache') continue;
      const path = join(root, item.name);
      if (item.name.includes('@')) await copyNotices(path, `cpa-go/${path.slice(cpaModuleCache.length + 1).replaceAll('/', '_')}`);
      else await modules(path);
    }
  };
  await modules(cpaModuleCache);
  await cp(goLicense, join(destination, 'GO.LICENSE'));
  await writeFile(join(destination, 'README.txt'), `Notices retained from installed Node packages, locked Rust crates, the supplied pinned-CPA module cache and Go toolchain. Includes build-only notices. Jev remains AGPL-3.0-only; CPA remains MIT.\n\n${copied.sort().join('\n')}\nGO.LICENSE\n`);
  return copied.length + 1;
}
