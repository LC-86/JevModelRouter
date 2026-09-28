import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';

// Tauri may return this when a release does not contain the current OS/architecture.
export function isUnsupportedUpdatePlatformError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return message.includes('None of the fallback platforms') && message.includes('were found in the response');
}

// One controller for the whole app prevents overlapping startup/manual checks.
export class AppUpdater {
  private pending: Update | null = null;
  private busy = false;
  installed = false;

  get isBusy() { return this.busy; }

  constructor(private readonly checkUpdate = check, private readonly restart = relaunch) {}

  async check() {
    if (this.busy || this.installed) return undefined;
    this.busy = true;
    try {
      const next = await this.checkUpdate({ timeout: 15_000 });
      const previous = this.pending;
      this.pending = next;
      if (previous) await previous.close();
      return next ? { version: next.version, notes: next.body } : null;
    } finally {
      this.busy = false;
    }
  }

  async install(progress: (percent: number | null) => void) {
    if (this.installed) return;
    if (this.busy) throw new Error('An update operation is already running.');
    if (!this.pending) throw new Error('Check for updates before installing.');
    this.busy = true;
    let total = 0;
    let received = 0;
    try {
      await this.pending.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          total = event.data.contentLength ?? 0;
          received = 0;
          progress(total ? 0 : null);
        } else if (event.event === 'Progress') {
          received += event.data.chunkLength;
          progress(total ? Math.min(99, Math.round(received / total * 100)) : null);
        }
        // Finished means download complete; installation can still fail.
      });
      this.installed = true;
      progress(100);
    } finally {
      this.busy = false;
    }
  }

  async relaunch() {
    if (!this.installed) throw new Error('Install the update before restarting.');
    await this.restart();
  }
}

export const appUpdater = new AppUpdater();
