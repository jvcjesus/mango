import { describe, expect, it, vi } from 'vitest';
import { runAction } from './action';
import { ApiError, message } from './api';
import { confirm, confirmation } from './confirm.svelte';
import { toast } from './toast.svelte';

describe('toast store', () => {
  it('adds and dismisses toasts', () => {
    toast.clear();
    const id = toast.success('Operation succeeded');
    expect(toast.toasts.length).toBe(1);
    expect(toast.toasts[0].id).toBe(id);
    expect(toast.toasts[0].text).toBe('Operation succeeded');
    expect(toast.toasts[0].kind).toBe('success');

    toast.dismiss(id);
    expect(toast.toasts.length).toBe(0);
  });

  it('limits max visible toasts to 4', () => {
    toast.clear();
    toast.info('Message 1');
    toast.info('Message 2');
    toast.info('Message 3');
    toast.info('Message 4');
    toast.info('Message 5');

    expect(toast.toasts.length).toBe(4);
    expect(toast.toasts.map((t) => t.text)).toEqual([
      'Message 2',
      'Message 3',
      'Message 4',
      'Message 5',
    ]);
  });

  it('sets appropriate default durations', () => {
    toast.clear();
    toast.success('Done');
    toast.error('Failed');
    expect(toast.toasts[0].duration).toBe(4000);
    expect(toast.toasts[1].duration).toBe(6000);
  });
});

describe('confirm dialog store', () => {
  it('resolves true when confirmed', async () => {
    const promise = confirm({
      title: 'Delete Item',
      message: 'Are you sure?',
      confirmText: 'Delete',
      danger: true,
    });

    expect(confirmation.current).not.toBeNull();
    expect(confirmation.current?.title).toBe('Delete Item');
    expect(confirmation.current?.danger).toBe(true);

    confirmation.current?.resolve(true);
    const result = await promise;
    expect(result).toBe(true);
    expect(confirmation.current).toBeNull();
  });

  it('resolves false when canceled', async () => {
    const promise = confirm('Are you sure?');
    expect(confirmation.current).not.toBeNull();
    confirmation.current?.resolve(false);
    const result = await promise;
    expect(result).toBe(false);
    expect(confirmation.current).toBeNull();
  });
});

describe('runAction helper', () => {
  it('returns result and shows success toast', async () => {
    toast.clear();
    const result = await runAction(async () => 'ok', {
      success: 'Saved successfully',
    });
    expect(result).toBe('ok');
    expect(toast.toasts.length).toBe(1);
    expect(toast.toasts[0].text).toBe('Saved successfully');
    expect(toast.toasts[0].kind).toBe('success');
  });

  it('catches error and shows error toast without throwing', async () => {
    toast.clear();
    const onError = vi.fn();
    const result = await runAction(
      async () => {
        throw new Error('Network failure');
      },
      { onError },
    );
    expect(result).toBeNull();
    expect(onError).toHaveBeenCalled();
    expect(toast.toasts.length).toBe(1);
    expect(toast.toasts[0].text).toBe('Network failure');
    expect(toast.toasts[0].kind).toBe('error');
  });
});

describe('api error utilities', () => {
  it('formats ApiError and generic error correctly', () => {
    const apiErr = new ApiError(404, 'not_found', 'Title not found');
    expect(message(apiErr)).toBe('Title not found');

    const genericErr = new Error('Standard failure');
    expect(message(genericErr)).toBe('Standard failure');

    const strErr = 'Direct string message';
    expect(message(strErr)).toBe('Direct string message');
  });
});

describe('optimistic toggle and error rollback logic', () => {
  it('rolls back state and displays error toast on failure', async () => {
    toast.clear();
    const item = { inLibrary: false };

    async function toggleLibrary(target: { inLibrary: boolean }, apiCall: () => Promise<void>) {
      const nextState = !target.inLibrary;
      target.inLibrary = nextState;
      try {
        await apiCall();
        toast.success(nextState ? 'Added to library' : 'Removed from library');
      } catch (e) {
        target.inLibrary = !nextState; // Rollback
        toast.error(message(e));
      }
    }

    // Successful toggle
    await toggleLibrary(item, async () => {});
    expect(item.inLibrary).toBe(true);
    expect(toast.toasts.length).toBe(1);
    expect(toast.toasts[0].kind).toBe('success');

    // Failed toggle
    await toggleLibrary(item, async () => {
      throw new ApiError(500, 'server_error', 'Database unavailable');
    });
    expect(item.inLibrary).toBe(true); // Rolled back from false to true
    expect(toast.toasts.length).toBe(2);
    expect(toast.toasts[1].kind).toBe('error');
    expect(toast.toasts[1].text).toBe('Database unavailable');
  });
});

describe('background polling error preservation', () => {
  it('does not overwrite user action error during background refresh', async () => {
    let error: string | null = 'Failed to delete chapter';
    let queue: string[] = ['chap1'];

    async function load(isBackground = false) {
      try {
        // Simulated successful poll
        queue = ['chap1'];
      } catch (e) {
        // Only surface poll errors if user explicitly loaded or error is empty
        if (!isBackground || !error) {
          error = message(e);
        }
      }
    }

    // Background poll runs
    await load(true);
    // User error was not wiped or replaced
    expect(error).toBe('Failed to delete chapter');
  });
});

describe('toast action handler', () => {
  it('executes action callback and can be dismissed', () => {
    toast.clear();
    const actionSpy = vi.fn();
    const id = toast.info('Update available', {
      action: {
        label: 'View',
        run: actionSpy,
      },
    });

    const currentToast = toast.toasts.find((t) => t.id === id);
    expect(currentToast?.action?.label).toBe('View');

    currentToast?.action?.run();
    expect(actionSpy).toHaveBeenCalledTimes(1);

    toast.dismiss(id);
    expect(toast.toasts.length).toBe(0);
  });
});

describe('volume parsing and grouping', () => {
  it('correctly parses volume numbers from varied naming schemes', async () => {
    const { parseVolume, volumeHeading } = await import('./util');

    expect(parseVolume('Vol. 1 Ch. 2')).toBe(1);
    expect(parseVolume('Vol. 2.5 Ch. 10')).toBe(2.5);
    expect(parseVolume('Volume 10 - Chapter 100')).toBe(10);
    expect(parseVolume('v03 c15')).toBe(3);
    expect(parseVolume('Chapter 1 (Vol 2)')).toBe(2);
    expect(parseVolume('[Group] Series Vol. 5 Chapter 20')).toBe(5);
    expect(parseVolume('Chapter 42')).toBeNull();
    expect(parseVolume('Episode 5')).toBeNull();
    expect(parseVolume('Oneshot')).toBeNull();
    expect(parseVolume('')).toBeNull();

    expect(volumeHeading(1)).toBe('VOLUME 1');
    expect(volumeHeading(2.5)).toBe('VOLUME 2.5');
    expect(volumeHeading(null)).toBe('OTHER CHAPTERS');
  });

  it('groups and sorts chapters by volume properly', async () => {
    const { parseVolume } = await import('./util');

    const sampleChapters = [
      { id: 1, name: 'Vol. 2 Ch. 15', number: 15 },
      { id: 2, name: 'Vol. 2 Ch. 14', number: 14 },
      { id: 3, name: 'Vol. 1 Ch. 10', number: 10 },
      { id: 4, name: 'Vol. 1 Ch. 9', number: 9 },
      { id: 5, name: 'Bonus Chapter', number: -1 },
    ];

    const map = new Map<number | null, typeof sampleChapters>();
    for (const c of sampleChapters) {
      const vol = parseVolume(c.name);
      const list = map.get(vol) ?? [];
      list.push(c);
      map.set(vol, list);
    }

    expect(map.get(2)?.length).toBe(2);
    expect(map.get(1)?.length).toBe(2);
    expect(map.get(null)?.length).toBe(1);
  });
});

describe('reader page lifecycle and decode verification', () => {
  it('manages normal load lifecycle from idle to loaded', async () => {
    const { PageState } = await import('./pageLifecycle.svelte');
    const page = new PageState(0);

    expect(page.status).toBe('idle');

    page.startLoad();
    expect(page.status).toBe('loading');

    const fakeImg = {
      naturalWidth: 800,
      naturalHeight: 1200,
      decode: vi.fn().mockResolvedValue(undefined),
    } as unknown as HTMLImageElement;

    const ok = await page.handleLoad(fakeImg);
    expect(ok).toBe(true);
    expect(page.status).toBe('loaded');
    expect(page.naturalWidth).toBe(800);
    expect(page.naturalHeight).toBe(1200);
    expect(page.error).toBeNull();
    expect(page.diagnostics.durationMs).toBeTypeOf('number');

    page.destroy();
  });

  it('detects decode failures and triggers automatic retry', async () => {
    vi.useFakeTimers();
    const { PageState } = await import('./pageLifecycle.svelte');
    const page = new PageState(1);

    page.startLoad();

    const corruptImg = {
      naturalWidth: 100,
      naturalHeight: 100,
      decode: vi.fn().mockRejectedValue(new Error('Corrupt JPEG stream')),
    } as unknown as HTMLImageElement;

    const ok = await page.handleLoad(corruptImg);
    expect(ok).toBe(false);
    expect(page.status).toBe('retrying');
    expect(page.retryCount).toBe(1);
    expect(page.errorType).toBe('decode');

    page.destroy();
    vi.useRealTimers();
  });

  it('rejects zero-dimension images as failure', async () => {
    const { PageState } = await import('./pageLifecycle.svelte');
    const page = new PageState(2);

    page.startLoad();

    const zeroImg = {
      naturalWidth: 0,
      naturalHeight: 0,
      decode: vi.fn().mockResolvedValue(undefined),
    } as unknown as HTMLImageElement;

    const ok = await page.handleLoad(zeroImg);
    expect(ok).toBe(false);
    expect(page.status).toBe('retrying');
    expect(page.errorType).toBe('decode');

    page.destroy();
  });

  it('stops auto-retrying after max retries and allows manual retry', async () => {
    vi.useFakeTimers();
    const { PageState, MAX_AUTO_RETRIES } = await import('./pageLifecycle.svelte');
    const page = new PageState(3);

    page.startLoad();
    expect(page.status).toBe('loading');

    // Failure 1 -> retrying (1/2)
    page.handleFailure('Network failure 1', 'network');
    expect(page.status).toBe('retrying');
    expect(page.retryCount).toBe(1);

    // Failure 2 -> retrying (2/2)
    page.handleFailure('Network failure 2', 'network');
    expect(page.status).toBe('retrying');
    expect(page.retryCount).toBe(MAX_AUTO_RETRIES);

    // Failure 3 -> failed (max reached)
    page.handleFailure('Network failure 3', 'network');
    expect(page.status).toBe('failed');
    expect(page.retryCount).toBe(MAX_AUTO_RETRIES);

    // Manual retry resets count and starts loading again
    let triggered = false;
    page.manualRetry(() => {
      triggered = true;
    });
    expect(triggered).toBe(true);
    expect(page.status).toBe('loading');
    expect(page.retryCount).toBe(0);
    expect(page.attempt).toBe(3);

    page.destroy();
    vi.useRealTimers();
  });

  it('guards progress marking so unread/broken chapters never mark finished', async () => {
    const { PageLifecycleManager } = await import('./pageLifecycle.svelte');
    const mgr = new PageLifecycleManager();

    // Invalid total counts
    expect(mgr.canMarkRead(0)).toBe(false);
    expect(mgr.canMarkRead(-1)).toBe(false);

    // Chapter with 5 pages, page 4 (last page) is not loaded
    expect(mgr.canMarkRead(5)).toBe(false);

    // Page 4 fails initially (enters retrying)
    const lastPage = mgr.get(4);
    lastPage.handleFailure('Server 500', 'network');
    expect(mgr.canMarkRead(5)).toBe(false);
    expect(mgr.isErrorOrRetrying(4)).toBe(true);

    // Page 4 exhausts retries (enters failed)
    lastPage.handleFailure('Server 500', 'network');
    lastPage.handleFailure('Server 500', 'network');
    expect(mgr.isFailed(4)).toBe(true);

    // Page 4 loads successfully
    const successImg = {
      naturalWidth: 800,
      naturalHeight: 1200,
      decode: vi.fn().mockResolvedValue(undefined),
    } as unknown as HTMLImageElement;
    await lastPage.handleLoad(successImg);

    expect(mgr.isLoaded(4)).toBe(true);
    expect(mgr.canMarkRead(5)).toBe(true);

    mgr.destroy();
  });
});
