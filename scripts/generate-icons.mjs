import { readFileSync, writeFileSync, mkdirSync, rmSync, readdirSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { createRequire } from 'node:module';

const logo = readFileSync('public/logo.svg', 'utf8');
const symbol = logo.slice(logo.indexOf('<defs>'), logo.lastIndexOf('</svg>'));
// Keep the original AutoJev paths, colour and arrow cutout. Desktop icons have
// the same inset rounded-square silhouette and DEV badge convention as Termany.
for (const dev of [false, true]) {
  const directory = dev ? 'src-tauri/icons-dev' : 'src-tauri/icons';
  mkdirSync(directory, { recursive: true });
  const badge = dev ? `<rect x="648" y="738" width="290" height="160" rx="44" fill="#17232B"/>
    <rect x="660" y="750" width="266" height="136" rx="34" fill="#D45A2A"/>
    <g transform="translate(690 785)" fill="none" stroke="#FFF8EF" stroke-width="13" stroke-linejoin="round">
      <path d="M0 0v66h20q26 0 26-33T20 0Z"/><path d="M110 0H69v66h41M69 32h34"/><path d="m131 0 24 66 24-66"/>
    </g>` : '';
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
    <defs><linearGradient id="tile" x2="0.3" y2="1"><stop stop-color="#FFF8EE"/><stop offset="1" stop-color="#EEDAC6"/></linearGradient></defs>
    <rect x="98" y="98" width="828" height="828" rx="194" fill="url(#tile)"/>
    <svg x="216" y="206" width="592" height="592" viewBox="4 7 56 56">${symbol}</svg>${badge}
  </svg>\n`;
  writeFileSync(`${directory}/icon.svg`, svg);
  const result = spawnSync(process.execPath, [createRequire(import.meta.url).resolve('@tauri-apps/cli/tauri.js'), 'icon', `${directory}/icon.svg`, '--output', directory], { stdio: 'inherit' });
  if (result.status !== 0) process.exit(result.status ?? 1);
  const keep = new Set(['icon.svg', 'icon.png', 'icon.icns', 'icon.ico', '32x32.png', '64x64.png', '128x128.png', '128x128@2x.png']);
  for (const file of readdirSync(directory)) {
    if (!keep.has(file)) rmSync(`${directory}/${file}`, { recursive: true, force: true });
  }
}
