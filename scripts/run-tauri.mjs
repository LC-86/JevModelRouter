import { spawn } from 'node:child_process';
import { createRequire } from 'node:module';

const args = process.argv.slice(2);
if (args[0] === 'dev') args.splice(1, 0, '--config', 'src-tauri/tauri.dev.conf.json');
const cli = createRequire(import.meta.url).resolve('@tauri-apps/cli/tauri.js');
const child = spawn(process.execPath, [cli, ...args], { stdio: 'inherit' });
child.on('error', error => { console.error(error.message); process.exitCode = 1; });
child.on('exit', (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exitCode = code ?? 1;
});
