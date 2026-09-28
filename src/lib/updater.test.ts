import { describe, expect, it, vi } from 'vitest';
import type { DownloadEvent, Update } from '@tauri-apps/plugin-updater';
import { AppUpdater, isUnsupportedUpdatePlatformError } from './updater';

function update(overrides: Partial<Update> = {}) {
  return { version: '0.2.0', body: 'Release notes', close: vi.fn().mockResolvedValue(undefined), downloadAndInstall: vi.fn().mockResolvedValue(undefined), ...overrides } as unknown as Update;
}

describe('AppUpdater', () => {
  it('clears stale updates when the next check finds no release', async () => {
    const first = update();
    const controller = new AppUpdater(vi.fn().mockResolvedValueOnce(first).mockResolvedValueOnce(null));
    expect(await controller.check()).toEqual({ version: '0.2.0', notes: 'Release notes' });
    expect(await controller.check()).toBeNull();
    expect(first.close).toHaveBeenCalledOnce();
    await expect(controller.install(() => {})).rejects.toThrow('Check for updates');
  });

  it('prevents concurrent checks', async () => {
    let resolve!: (value: null) => void;
    const check = vi.fn(() => new Promise<null>(done => { resolve = done; }));
    const controller = new AppUpdater(check);
    const pending = controller.check();
    expect(await controller.check()).toBeUndefined();
    expect(check).toHaveBeenCalledOnce();
    resolve(null);
    await pending;
  });

  it('does not mark download completion as installation success and permits retry', async () => {
    const download = vi.fn().mockImplementationOnce(async (callback: (event: DownloadEvent) => void) => {
      callback({ event: 'Started', data: { contentLength: 10 } });
      callback({ event: 'Progress', data: { chunkLength: 10 } });
      callback({ event: 'Finished' });
      throw new Error('signature invalid');
    }).mockResolvedValueOnce(undefined);
    const restart = vi.fn().mockResolvedValue(undefined);
    const controller = new AppUpdater(vi.fn().mockResolvedValue(update({ downloadAndInstall: download })), restart);
    const progress = vi.fn();
    await controller.check();
    await expect(controller.install(progress)).rejects.toThrow('signature invalid');
    expect(controller.installed).toBe(false);
    expect(progress).not.toHaveBeenCalledWith(100);
    await expect(controller.relaunch()).rejects.toThrow('Install the update');
    await controller.install(progress);
    expect(controller.installed).toBe(true);
    expect(progress).toHaveBeenLastCalledWith(100);
    await controller.relaunch();
    expect(restart).toHaveBeenCalledOnce();
  });

  it('handles downloads without a content length', async () => {
    const download = vi.fn(async (callback: (event: DownloadEvent) => void) => {
      callback({ event: 'Started', data: {} });
      callback({ event: 'Progress', data: { chunkLength: 50 } });
    });
    const controller = new AppUpdater(vi.fn().mockResolvedValue(update({ downloadAndInstall: download })));
    const progress = vi.fn();
    await controller.check();
    await controller.install(progress);
    expect(progress.mock.calls).toEqual([[null], [null], [100]]);
  });
});

 it('distinguishes unsupported platforms from download errors', () => {
  expect(isUnsupportedUpdatePlatformError(new Error('None of the fallback platforms darwin-aarch64 were found in the response platforms'))).toBe(true);
  expect(isUnsupportedUpdatePlatformError(new Error('Network timeout'))).toBe(false);
});
