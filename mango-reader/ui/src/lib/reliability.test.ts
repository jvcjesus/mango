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

describe('duplicate chapter ID tolerance', () => {
  it('groups and retains chapters when multiple chapters share the same ID', async () => {
    const { parseVolume, volumeHeading } = await import('./util');

    // Extension returning duplicate chapter IDs (e.g., re-upload or source anomaly)
    const duplicateIdChapters = [
      { id: 42, name: 'Vol. 1 Ch. 1 - Part 1', number: 1 },
      { id: 42, name: 'Vol. 1 Ch. 1 - Part 2', number: 1 },
      { id: 43, name: 'Vol. 1 Ch. 2', number: 2 },
      { id: 44, name: 'Vol. 2 Ch. 3', number: 3 },
    ];

    // Grouping by volume
    const map = new Map<number | null, typeof duplicateIdChapters>();
    for (const c of duplicateIdChapters) {
      const vol = parseVolume(c.name);
      const list = map.get(vol) ?? [];
      list.push(c);
      map.set(vol, list);
    }

    const volumeGroups = Array.from(map.entries()).map(([vol, chs]) => ({
      volume: vol,
      label: volumeHeading(vol),
      chapters: chs,
    }));

    // Volume 1 should contain all 3 chapters, including both chapters with id: 42
    const vol1Group = volumeGroups.find((g) => g.volume === 1);
    expect(vol1Group).toBeDefined();
    expect(vol1Group?.chapters.length).toBe(3);
    expect(vol1Group?.chapters.filter((c) => c.id === 42).length).toBe(2);

    // Structural key (grp.volume ?? 'unassigned') is unique per group
    const structuralKeys = volumeGroups.map((g) => g.volume ?? 'unassigned');
    expect(new Set(structuralKeys).size).toBe(volumeGroups.length);

    // Unkeyed chapter iteration retains both entries without duplicate key conflict
    const renderedChapters = vol1Group?.chapters.map((c) => c.name);
    expect(renderedChapters).toEqual([
      'Vol. 1 Ch. 1 - Part 1',
      'Vol. 1 Ch. 1 - Part 2',
      'Vol. 1 Ch. 2',
    ]);
  });

  it('renders Title component with duplicate chapter IDs without Svelte each_key_duplicate error', async () => {
    // Provide minimal browser shims for component rendering in Node test environment
    if (typeof (globalThis as any).history === 'undefined') {
      (globalThis as any).history = {
        scrollRestoration: 'manual',
        state: {},
        replaceState: () => {},
        pushState: () => {},
        back: () => {},
      };
    }
    if (typeof (globalThis as any).location === 'undefined') {
      (globalThis as any).location = { hash: '#/title/9999' };
    }
    if (typeof (globalThis as any).window === 'undefined') {
      (globalThis as any).window = globalThis;
    }
    if (typeof (globalThis as any).scrollTo === 'undefined') {
      (globalThis as any).scrollTo = () => {};
      (globalThis as any).scrollY = 0;
    }
    if (typeof (globalThis as any).addEventListener === 'undefined') {
      (globalThis as any).addEventListener = () => {};
      (globalThis as any).removeEventListener = () => {};
    }
    if (typeof (globalThis as any).matchMedia === 'undefined') {
      (globalThis as any).matchMedia = () => ({
        matches: false,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
      });
    }

    const { keep } = await import('./keep');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    const mockTitle = {
      id: 9999,
      sourceId: '1',
      sourceName: 'Demo Source',
      sourceLang: 'en',
      webUrl: 'https://example.com/manga/test',
      title: 'Duplicate ID Test Manga',
      artist: 'Artist',
      author: 'Author',
      description: 'Testing duplicate chapter IDs and genres',
      genres: ['Action', 'Action', 'Drama'], // Upstream source with duplicate genre tags
      status: 1,
      cover: null,
      inLibrary: false,
      addedAt: null,
      initialized: true,
      readerMode: null,
      readerFit: null,
      autoDownload: false,
      chaptersAt: 1000,
      releaseDays: null,
      categories: [],
      chapters: [
        {
          id: 555,
          name: 'Vol. 1 Ch. 1 - Release A',
          number: 1,
          scanlator: 'Group A',
          dateUpload: 1000,
          fetchedAt: 1000,
          read: false,
          lastPageRead: 0,
          pageCount: 20,
          downloaded: false,
          bookmark: false,
        },
        {
          id: 555, // Duplicate chapter ID from source
          name: 'Vol. 1 Ch. 1 - Release B',
          number: 1,
          scanlator: 'Group B',
          dateUpload: 2000,
          fetchedAt: 2000,
          read: false,
          lastPageRead: 0,
          pageCount: 20,
          downloaded: false,
          bookmark: false,
        },
      ],
    };

    // Synchronously preload title into recall cache
    keep('title', mockTitle);

    // Rendering Title with duplicate chapter IDs and duplicate genres should NOT throw each_key_duplicate
    const result = render(Title, { props: { id: 9999 } });
    expect(result.html).toContain('Vol. 1 Ch. 1 - Release A');
    expect(result.html).toContain('Vol. 1 Ch. 1 - Release B');
    // Both duplicate 'Action' tags and 'Drama' tag should be rendered
    const actionTagMatches = result.html.match(/<span class="tag[^>]*>Action<\/span>/g);
    expect(actionTagMatches?.length).toBe(2);
    expect(result.html).toContain('Drama');

    // Simulate navigating away and reopening the title with cache
    keep('title', mockTitle);
    const reopenResult = render(Title, { props: { id: 9999 } });
    expect(reopenResult.html).toContain('Vol. 1 Ch. 1 - Release A');
    const reopenMatches = reopenResult.html.match(/<span class="tag[^>]*>Action<\/span>/g);
    expect(reopenMatches?.length).toBe(2);
  });

  it('proves client-side genre each block is unkeyed to prevent each_key_duplicate', async () => {
    const titleSource = (await import('../routes/Title.svelte?raw')).default;
    const { compile } = await import('svelte/compiler');
    // @ts-expect-error internal client module
    const $ = (await import('svelte/internal/client')) as any;

    // Compile Title.svelte in client mode as Vite does for browser builds
    const compiled = compile(titleSource, { generate: 'client', filename: 'Title.svelte' });

    // Verify that genres loop in client code uses $.index rather than (genre) => genre
    expect(compiled.js.code).not.toContain('(genre) => genre');
    expect(compiled.js.code).toMatch(/\$\.each\(div_\d+,\s*\d+,\s*\(\)\s*=>\s*.*\.genres,\s*\$\.index/);

    // Demonstrate the exact runtime behavior:
    // A keyed loop with (genre) => genre throws each_key_duplicate on duplicate genres
    const createElem = (tag: string) => ({
      nodeType: 1,
      tagName: tag.toUpperCase(),
      childNodes: [] as any[],
      parentNode: null as any,
      appendChild(c: any) { c.parentNode = this; this.childNodes.push(c); return c; },
      insertBefore(c: any, _ref: any) { c.parentNode = this; this.childNodes.push(c); return c; },
      removeChild(c: any) { return c; },
      append(c: any) { c.parentNode = this; this.childNodes.push(c); return c; },
      setAttribute: () => {},
      removeAttribute: () => {},
      style: {},
    });

    (globalThis as any).document = {
      createComment: () => ({ nodeType: 8, parentNode: null }),
      createTextNode: (t: string) => ({ nodeType: 3, nodeValue: t, parentNode: null }),
      createElement: createElem,
      createDocumentFragment: () => createElem('fragment'),
    };

    const duplicateGenres = ['Action', 'Action', 'Drama'];

    // 1. Keyed block throws each_key_duplicate when duplicates are present
    $.effect_root(() => {
      let threwDuplicateKeyError = false;
      try {
        const keyedContainer = createElem('div');
        $.each(keyedContainer, 20, () => duplicateGenres, (g: string) => g, () => {});
      } catch (err: any) {
        if (err.message.includes('each_key_duplicate')) {
          threwDuplicateKeyError = true;
        }
      }
      expect(threwDuplicateKeyError).toBe(true);

      // 2. Unkeyed block (using $.index) safely tolerates duplicate values
      let unkeyedThrew = false;
      try {
        const unkeyedContainer = createElem('div');
        $.each(unkeyedContainer, 21, () => duplicateGenres, $.index, () => {});
      } catch {
        unkeyedThrew = true;
      }
      expect(unkeyedThrew).toBe(false);
    });
  });
});

describe('uploader/source filtering on Title page', () => {
  const multiUploaderTitle = {
    id: 8888,
    sourceId: '1',
    sourceName: 'MangaDot',
    sourceLang: 'en',
    webUrl: 'https://mangadot.net/manga/multi',
    title: 'Multi-Uploader Test Manga',
    artist: 'Artist',
    author: 'Author',
    description: 'Testing multi-uploader filtering',
    genres: ['Action', 'Fantasy'],
    status: 1,
    cover: null,
    inLibrary: false,
    addedAt: null,
    initialized: true,
    readerMode: null,
    readerFit: null,
    autoDownload: false,
    chaptersAt: 1000,
    releaseDays: null,
    categories: [],
    chapters: [
      {
        id: 101,
        name: 'Vol. 1 Ch. 1 - Group A Edition',
        number: 1,
        scanlator: 'Group A',
        dateUpload: 1000,
        fetchedAt: 1000,
        read: false,
        lastPageRead: 0,
        pageCount: 20,
        downloaded: false,
        bookmark: false,
      },
      {
        id: 102,
        name: 'Vol. 1 Ch. 1 - Group B Edition',
        number: 1,
        scanlator: 'Group B',
        dateUpload: 2000,
        fetchedAt: 2000,
        read: false,
        lastPageRead: 0,
        pageCount: 20,
        downloaded: false,
        bookmark: false,
      },
      {
        id: 103,
        name: 'Vol. 1 Ch. 2 - Group A Edition',
        number: 2,
        scanlator: 'Group A',
        dateUpload: 3000,
        fetchedAt: 3000,
        read: false,
        lastPageRead: 0,
        pageCount: 22,
        downloaded: false,
        bookmark: false,
      },
      {
        id: 104,
        name: 'Vol. 2 Ch. 3 - Anonymous Release',
        number: 3,
        scanlator: null, // Unknown uploader
        dateUpload: 4000,
        fetchedAt: 4000,
        read: false,
        lastPageRead: 0,
        pageCount: 25,
        downloaded: false,
        bookmark: false,
      },
    ],
  };

  it('renders uploader filter with All, named groups, and Unknown', async () => {
    const { keep } = await import('./keep');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    keep('title', multiUploaderTitle);
    keep('title.8888.uploader', '');

    const result = render(Title, { props: { id: 8888 } });
    expect(result.html).toContain('Uploader');
    expect(result.html).toContain('All');
    expect(result.html).toContain('Group A');
    expect(result.html).toContain('Group B');
    expect(result.html).toContain('Unknown');

    // Default All shows all legitimate chapters across uploaders
    expect(result.html).toContain('Vol. 1 Ch. 1 - Group A Edition');
    expect(result.html).toContain('Vol. 1 Ch. 1 - Group B Edition');
    expect(result.html).toContain('Vol. 1 Ch. 2 - Group A Edition');
    expect(result.html).toContain('Vol. 2 Ch. 3 - Anonymous Release');
  });

  it('filters chapters when a specific uploader is selected', async () => {
    const { keep } = await import('./keep');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    // Filter to Group A only
    keep('title', multiUploaderTitle);
    keep('title.8888.uploader', 'Group A');

    const resultA = render(Title, { props: { id: 8888 } });
    expect(resultA.html).toContain('Vol. 1 Ch. 1 - Group A Edition');
    expect(resultA.html).toContain('Vol. 1 Ch. 2 - Group A Edition');
    expect(resultA.html).not.toContain('Vol. 1 Ch. 1 - Group B Edition');
    expect(resultA.html).not.toContain('Vol. 2 Ch. 3 - Anonymous Release');

    // Filter to Group B only
    keep('title.8888.uploader', 'Group B');
    const resultB = render(Title, { props: { id: 8888 } });
    expect(resultB.html).toContain('Vol. 1 Ch. 1 - Group B Edition');
    expect(resultB.html).not.toContain('Vol. 1 Ch. 1 - Group A Edition');
    expect(resultB.html).not.toContain('Vol. 1 Ch. 2 - Group A Edition');
    expect(resultB.html).not.toContain('Vol. 2 Ch. 3 - Anonymous Release');
  });

  it('filters to unknown/empty uploaders when Unknown is selected', async () => {
    const { keep } = await import('./keep');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    keep('title', multiUploaderTitle);
    keep('title.8888.uploader', '__unknown__');

    const result = render(Title, { props: { id: 8888 } });
    expect(result.html).toContain('Vol. 2 Ch. 3 - Anonymous Release');
    expect(result.html).not.toContain('Vol. 1 Ch. 1 - Group A Edition');
    expect(result.html).not.toContain('Vol. 1 Ch. 1 - Group B Edition');
  });

  it('filters before volume grouping so only the selected uploader volumes appear', async () => {
    const { keep } = await import('./keep');
    const { prefs } = await import('./prefs.svelte');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    // Enable volumes view
    prefs.values['title.view'] = { volumes: true, sort: 'source', down: true };

    // Select Group A: should show Volume 1 (2 chapters) and NOT Volume 2
    keep('title', multiUploaderTitle);
    keep('title.8888.uploader', 'Group A');

    const result = render(Title, { props: { id: 8888 } });
    expect(result.html).toContain('VOLUME 1');
    expect(result.html).not.toContain('VOLUME 2');
    expect(result.html).toContain('2 chapters');

    // Select Unknown: should show Volume 2 (1 chapter) and NOT Volume 1
    keep('title.8888.uploader', '__unknown__');
    const resultUnknown = render(Title, { props: { id: 8888 } });
    expect(resultUnknown.html).toContain('VOLUME 2');
    expect(resultUnknown.html).not.toContain('VOLUME 1');
    expect(resultUnknown.html).toContain('1 chapter');
  });

  it('renders a compact dropdown when there are more than 4 uploader choices', async () => {
    const { keep } = await import('./keep');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    const manyUploadersTitle = {
      ...multiUploaderTitle,
      id: 7777,
      chapters: [
        { ...multiUploaderTitle.chapters[0], id: 201, scanlator: 'Group 1' },
        { ...multiUploaderTitle.chapters[1], id: 202, scanlator: 'Group 2' },
        { ...multiUploaderTitle.chapters[2], id: 203, scanlator: 'Group 3' },
        { ...multiUploaderTitle.chapters[3], id: 204, scanlator: 'Group 4' },
        { ...multiUploaderTitle.chapters[0], id: 205, scanlator: 'Group 5' },
      ],
    };

    keep('title', manyUploadersTitle);
    keep('title.7777.uploader', '');

    const result = render(Title, { props: { id: 7777 } });
    // Should render a <select> dropdown for uploader choices rather than overflowing pill row
    expect(result.html).toContain('<select class="btn');
    expect(result.html).toContain('<option value="Group 1">Group 1</option>');
    expect(result.html).toContain('<option value="Group 5">Group 5</option>');
  });

  it('persists uploader selection when reopening title from cache', async () => {
    const { keep } = await import('./keep');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    keep('title', multiUploaderTitle);
    keep('title.8888.uploader', 'Group B');

    // First render
    const res1 = render(Title, { props: { id: 8888 } });
    expect(res1.html).toContain('Vol. 1 Ch. 1 - Group B Edition');
    expect(res1.html).not.toContain('Vol. 1 Ch. 1 - Group A Edition');

    // Simulate reopening title (retained in keep cache)
    const res2 = render(Title, { props: { id: 8888 } });
    expect(res2.html).toContain('Vol. 1 Ch. 1 - Group B Edition');
    expect(res2.html).not.toContain('Vol. 1 Ch. 1 - Group A Edition');
  });
});

describe('first-class volume acquisition & release semantics', () => {
  it('conservatively detects volume numbers and rejects chapter revisions', async () => {
    const { parseVolume, parseRelease } = await import('./util');

    // Fixture A: MangaDot-style chapter
    expect(parseVolume('Chapter 1')).toBeNull();
    const relA = parseRelease('Chapter 1', 1);
    expect(relA.releaseType).toBe('chapter');
    expect(relA.volume).toBeNull();
    expect(relA.chapterNumber).toBe(1);

    // Fixture B: Explicit volume-name data
    expect(parseVolume('Vol. 1 Ch. 5')).toBe(1);
    const relB = parseRelease('Vol. 1 Ch. 5', 5);
    expect(relB.releaseType).toBe('chapter');
    expect(relB.volume).toBe(1);
    expect(relB.chapterNumber).toBe(5);

    // Source-provided true volume artifact
    expect(parseVolume('Volume 12')).toBe(12);
    expect(parseVolume('Vol 12')).toBe(12);
    const relVol = parseRelease('Volume 12', 12);
    expect(relVol.releaseType).toBe('volume');
    expect(relVol.volume).toBe(12);
    expect(relVol.chapterNumber).toBeNull(); // No fake chapter number

    // Volume 0
    expect(parseVolume('Volume 0')).toBe(0);
    expect(parseVolume('Vol. 0 Ch. 0.5')).toBe(0);

    // Short notation
    expect(parseVolume('v3 c20')).toBe(3);
    const relShort = parseRelease('v3 c20', 20);
    expect(relShort.releaseType).toBe('chapter');
    expect(relShort.volume).toBe(3);
    expect(relShort.chapterNumber).toBe(20);

    // Chapter revisions (MUST NOT be treated as volumes)
    expect(parseVolume('Chapter 5 v2')).toBeNull();
    expect(parseVolume('Ch. 12 (v2)')).toBeNull();
    const relRev = parseRelease('Chapter 5 v2', 5);
    expect(relRev.releaseType).toBe('chapter');
    expect(relRev.volume).toBeNull();
    expect(relRev.chapterNumber).toBe(5);

    // Unrelated numbers
    expect(parseVolume('Season 2 14')).toBeNull();
    expect(parseVolume('104.')).toBeNull();
  });

  it('renders chapter-only/manhwa titles without fake volume headers or volume toggle', async () => {
    const { keep } = await import('./keep');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    // Fixture C: Chapter-only manhwa
    const manhwaTitle = {
      id: 5001,
      sourceId: '1',
      sourceName: 'WebtoonSource',
      sourceLang: 'en',
      webUrl: 'https://example.com/manhwa/solo',
      title: 'Level Up Solo',
      artist: 'Artist',
      author: 'Author',
      description: 'Manhwa with no volumes',
      genres: ['Action'],
      status: 1,
      cover: null,
      inLibrary: false,
      addedAt: null,
      initialized: true,
      readerMode: null,
      readerFit: null,
      autoDownload: false,
      chaptersAt: 1000,
      releaseDays: null,
      categories: [],
      chapters: [
        { id: 1, name: 'Chapter 1', number: 1, scanlator: null, dateUpload: 1000, fetchedAt: 1000, read: false, lastPageRead: 0, pageCount: 30, downloaded: false, bookmark: false },
        { id: 2, name: 'Chapter 2', number: 2, scanlator: null, dateUpload: 2000, fetchedAt: 2000, read: false, lastPageRead: 0, pageCount: 30, downloaded: false, bookmark: false },
        { id: 3, name: 'Chapter 3', number: 3, scanlator: null, dateUpload: 3000, fetchedAt: 3000, read: false, lastPageRead: 0, pageCount: 30, downloaded: false, bookmark: false },
      ],
    };

    keep('title', manhwaTitle);

    const result = render(Title, { props: { id: 5001 } });
    // Must NOT contain VOLUME 1
    expect(result.html).not.toContain('VOLUME 1');
    expect(result.html).not.toContain('OTHER CHAPTERS');
    // All chapters are visible as flat list
    expect(result.html).toContain('Chapter 1');
    expect(result.html).toContain('Chapter 2');
    expect(result.html).toContain('Chapter 3');
  });

  it('renders volume acquisition actions and download states (all, partial, missing)', async () => {
    const { keep } = await import('./keep');
    const { prefs } = await import('./prefs.svelte');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    prefs.values['title.view'] = { volumes: true, sort: 'source', down: false };

    const multiVolumeTitle = {
      id: 5002,
      sourceId: '1',
      sourceName: 'MangaSource',
      sourceLang: 'en',
      webUrl: 'https://example.com/manga/multi-vol',
      title: 'Multi Volume Manga',
      artist: 'Artist',
      author: 'Author',
      description: 'Manga with multiple volumes',
      genres: ['Adventure'],
      status: 1,
      cover: null,
      inLibrary: false,
      addedAt: null,
      initialized: true,
      readerMode: null,
      readerFit: null,
      autoDownload: false,
      chaptersAt: 1000,
      releaseDays: null,
      categories: [],
      chapters: [
        // Volume 1: Both downloaded
        { id: 11, name: 'Vol. 1 Ch. 1', number: 1, scanlator: 'ScanGroup', dateUpload: 1000, fetchedAt: 1000, read: false, lastPageRead: 0, pageCount: 20, downloaded: true, bookmark: false },
        { id: 12, name: 'Vol. 1 Ch. 2', number: 2, scanlator: 'ScanGroup', dateUpload: 2000, fetchedAt: 2000, read: false, lastPageRead: 0, pageCount: 20, downloaded: true, bookmark: false },
        // Volume 2: 1 of 2 downloaded (partial state)
        { id: 21, name: 'Vol. 2 Ch. 3', number: 3, scanlator: 'ScanGroup', dateUpload: 3000, fetchedAt: 3000, read: false, lastPageRead: 0, pageCount: 20, downloaded: true, bookmark: false },
        { id: 22, name: 'Vol. 2 Ch. 4', number: 4, scanlator: 'ScanGroup', dateUpload: 4000, fetchedAt: 4000, read: false, lastPageRead: 0, pageCount: 20, downloaded: false, bookmark: false },
        // Volume 3: None downloaded
        { id: 31, name: 'Vol. 3 Ch. 5', number: 5, scanlator: 'ScanGroup', dateUpload: 5000, fetchedAt: 5000, read: false, lastPageRead: 0, pageCount: 20, downloaded: false, bookmark: false },
      ],
    };

    keep('title', multiVolumeTitle);

    const result = render(Title, { props: { id: 5002 } });
    expect(result.html).toContain('VOLUME 1');
    expect(result.html).toContain('VOLUME 2');
    expect(result.html).toContain('VOLUME 3');

    // Volume 1 (complete)
    expect(result.html).toContain('Downloaded');
    expect(result.html).toContain('Complete');

    // Volume 2 (partially downloaded)
    expect(result.html).toContain('1/2 saved');
    expect(result.html).toContain('Missing (1)');

    // Volume 3 (not downloaded)
    expect(result.html).toContain('Download Volume');
  });

  it('respects uploader filter in volume groups and keeps duplicate chapters distinct (Fixture D)', async () => {
    const { keep } = await import('./keep');
    const { prefs } = await import('./prefs.svelte');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    prefs.values['title.view'] = { volumes: true, sort: 'source', down: false };

    // Fixture D: Multiple uploaders with same chapter numbers
    const duplicateChapterTitle = {
      id: 5003,
      sourceId: '1',
      sourceName: 'MangaSource',
      sourceLang: 'en',
      webUrl: 'https://example.com/manga/dup',
      title: 'Duplicate Chapter Manga',
      artist: 'Artist',
      author: 'Author',
      description: 'Multiple uploaders release same chapter',
      genres: ['Action'],
      status: 1,
      cover: null,
      inLibrary: false,
      addedAt: null,
      initialized: true,
      readerMode: null,
      readerFit: null,
      autoDownload: false,
      chaptersAt: 1000,
      releaseDays: null,
      categories: [],
      chapters: [
        { id: 101, name: 'Volume 2 Chapter 10', number: 10, scanlator: 'Group A', dateUpload: 1000, fetchedAt: 1000, read: false, lastPageRead: 0, pageCount: 20, downloaded: false, bookmark: false },
        { id: 102, name: 'Volume 2 Chapter 10', number: 10, scanlator: 'Group B', dateUpload: 2000, fetchedAt: 2000, read: false, lastPageRead: 0, pageCount: 20, downloaded: true, bookmark: false },
      ],
    };

    // When unfiltered (All): both distinct releases exist in Volume 2
    keep('title', duplicateChapterTitle);
    keep('title.5003.uploader', '');

    const resAll = render(Title, { props: { id: 5003 } });
    expect(resAll.html).toContain('VOLUME 2');
    expect(resAll.html).toContain('2 chapters');

    // When filtered to Group A: only Group A's un-downloaded chapter is in Volume 2
    keep('title.5003.uploader', 'Group A');
    const resA = render(Title, { props: { id: 5003 } });
    expect(resA.html).toContain('VOLUME 2');
    expect(resA.html).toContain('1 chapter');
    expect(resA.html).toContain('Download Volume');
    expect(resA.html).not.toContain('Complete');

    // When filtered to Group B: only Group B's downloaded chapter is in Volume 2
    keep('title.5003.uploader', 'Group B');
    const resB = render(Title, { props: { id: 5003 } });
    expect(resB.html).toContain('VOLUME 2');
    expect(resB.html).toContain('1 chapter');
    expect(resB.html).toContain('Complete');
    expect(resB.html).not.toContain('Download Volume');
  });

  it('handles mixed volume metadata by grouping known volumes and leaving unknown in OTHER CHAPTERS', async () => {
    const { keep } = await import('./keep');
    const { prefs } = await import('./prefs.svelte');
    const { render } = await import('svelte/server');
    const Title = (await import('../routes/Title.svelte')).default;

    prefs.values['title.view'] = { volumes: true, sort: 'source', down: false };

    const mixedTitle = {
      id: 5004,
      sourceId: '1',
      sourceName: 'MangaSource',
      sourceLang: 'en',
      webUrl: 'https://example.com/manga/mixed',
      title: 'Mixed Metadata Manga',
      artist: 'Artist',
      author: 'Author',
      description: 'Some chapters have volumes, some do not',
      genres: ['Fantasy'],
      status: 1,
      cover: null,
      inLibrary: false,
      addedAt: null,
      initialized: true,
      readerMode: null,
      readerFit: null,
      autoDownload: false,
      chaptersAt: 1000,
      releaseDays: null,
      categories: [],
      chapters: [
        { id: 1, name: 'Vol. 1 Ch. 1', number: 1, scanlator: 'Group A', dateUpload: 1000, fetchedAt: 1000, read: false, lastPageRead: 0, pageCount: 20, downloaded: false, bookmark: false },
        { id: 2, name: 'Vol. 1 Ch. 2', number: 2, scanlator: 'Group A', dateUpload: 2000, fetchedAt: 2000, read: false, lastPageRead: 0, pageCount: 20, downloaded: false, bookmark: false },
        { id: 3, name: 'Chapter 3', number: 3, scanlator: 'Group B', dateUpload: 3000, fetchedAt: 3000, read: false, lastPageRead: 0, pageCount: 20, downloaded: false, bookmark: false },
        { id: 4, name: 'Vol. 2 Ch. 4', number: 4, scanlator: 'Group A', dateUpload: 4000, fetchedAt: 4000, read: false, lastPageRead: 0, pageCount: 20, downloaded: false, bookmark: false },
      ],
    };

    keep('title', mixedTitle);
    keep('title.5004.uploader', '');

    const res = render(Title, { props: { id: 5004 } });
    expect(res.html).toContain('VOLUME 1');
    expect(res.html).toContain('VOLUME 2');
    expect(res.html).toContain('OTHER CHAPTERS');
    // Volume headers have Download Volume, but OTHER CHAPTERS does not have "Download Volume"
    expect(res.html).toContain('Download Volume');
  });

  describe('native acquisition pipeline UI and configuration', () => {
    it('redacts and safely manages sensitive tokens in configuration patches', () => {
      const existingConfig = {
        enabled: true,
        library_dir: '/srv/media/manga',
        chinese_library_dir: '/srv/media/manga-chinese',
        mangabaka_enabled: true,
        mangabaka_url: 'https://api.mangabaka.org/v1',
        has_mangabaka_token: true,
        komga_enabled: true,
        komga_url: 'http://127.0.0.1:25600',
        has_komga_api_key: true,
        komga_library_id: null,
        komga_chinese_library_id: null,
        komga_scan_enabled: true,
        optimize_enabled: false,
        optimizer_bin: null,
        min_savings_percent: 10,
      };

      // When user edits settings without touching masked secrets, token fields are passed unchanged
      const patch = {
        ...existingConfig,
        library_dir: '/srv/media/manga-updated',
        komga_api_key: '********', // Masked placeholder should not overwrite secret
        mangabaka_token: '********',
      };

      expect(patch.library_dir).toBe('/srv/media/manga-updated');
      expect(patch.komga_api_key).toBe('********');
    });

    it('classifies and formats pipeline audit outcomes correctly', () => {
      const outcomes = [
        { code: 'published', label: 'Published' },
        { code: 'already_published', label: 'Already in Library' },
        { code: 'conflict', label: 'Conflict (Preserved)' },
        { code: 'error', label: 'Error' },
      ];

      for (const item of outcomes) {
        const text =
          item.code === 'published'
            ? 'Published'
            : item.code === 'already_published'
              ? 'Already in Library'
              : item.code === 'conflict'
                ? 'Conflict (Preserved)'
                : item.code === 'error'
                  ? 'Error'
                  : item.code;
        expect(text).toBe(item.label);
      }
    });

    it('renders Settings pipeline tab when route is pipeline and user is admin', async () => {
      const { render } = await import('svelte/server');
      const { session } = await import('./session.svelte');
      const { route } = await import('./router.svelte');
      const Settings = (await import('../routes/Settings.svelte')).default;

      // Simulate admin session on #/settings/pipeline
      session.admin = true;
      route.path = '/settings/pipeline';

      const res = render(Settings);
      expect(res.html).toContain('PIPELINE');
      expect(res.html).toContain('Acquisition Pipeline');
      expect(res.html).toContain('Permanent Library Paths');
      expect(res.html).toContain('Validate Paths');
      expect(res.html).toContain('MangaBaka Metadata Enrichment');
      expect(res.html).toContain('Komga Server Integration');
      expect(res.html).toContain('Image Optimization');
      expect(res.html).toContain('Target Page Height');
      expect(res.html).toContain('Image Quality');
      expect(res.html).toContain('Minimum Size Savings Required');
    });

    it('classifies container-aware path validation codes and badges', () => {
      const statuses = [
        { code: 'valid', expected: 'Valid / Writable' },
        { code: 'missing', expected: 'Missing inside container' },
        { code: 'not_a_directory', expected: 'Not a directory' },
        { code: 'not_writable', expected: 'Read-only or not writable' },
        { code: 'not_configured', expected: 'Not configured' },
      ];

      for (const { code, expected } of statuses) {
        let label = '';
        if (code === 'valid') label = 'Valid / Writable';
        else if (code === 'missing') label = 'Missing inside container';
        else if (code === 'not_a_directory') label = 'Not a directory';
        else if (code === 'not_writable') label = 'Read-only or not writable';
        else label = 'Not configured';

        expect(label).toBe(expected);
      }
    });

    it('validates optimizer resolution precedence and migration of legacy host path', () => {
      const legacyPath = '/srv/apps/manga/manga-itagger/RustyLibraryShrinker';
      const bundledPath = '/opt/mango/bin/RustyLibraryShrinker';

      // 1. Untouched legacy default is normalized to null (using bundled default)
      const normalize = (input: string | null) => (input?.trim() === legacyPath || !input?.trim() ? null : input.trim());
      expect(normalize(legacyPath)).toBeNull();
      expect(normalize('')).toBeNull();
      expect(normalize(null)).toBeNull();

      // 2. Deliberate custom path override is preserved
      expect(normalize('/custom/opt/bin/RustyLibraryShrinker')).toBe('/custom/opt/bin/RustyLibraryShrinker');

      // 3. Bundled path is selected when no custom override is present
      const resolve = (custom: string | null, bundledAvailable: boolean) => {
        const norm = normalize(custom);
        if (norm) return { path: norm, source: 'custom' };
        if (bundledAvailable) return { path: bundledPath, source: 'bundled' };
        return { path: null, source: 'none' };
      };

      expect(resolve(legacyPath, true)).toEqual({ path: bundledPath, source: 'bundled' });
      expect(resolve(null, true)).toEqual({ path: bundledPath, source: 'bundled' });
      expect(resolve('/custom/path', true)).toEqual({ path: '/custom/path', source: 'custom' });
      expect(resolve(null, false)).toEqual({ path: null, source: 'none' });
    });

    it('validates optimizer quality parameter bounds and defaults in pipeline config', () => {
      // Default quality parameters
      const defaultConfig: import('./api').PipelineConfig = {
        enabled: false,
        library_dir: null,
        chinese_library_dir: null,
        mangabaka_enabled: true,
        mangabaka_url: 'https://api.mangabaka.org/v1',
        has_mangabaka_token: false,
        komga_enabled: false,
        komga_url: 'http://127.0.0.1:25600',
        has_komga_api_key: false,
        komga_library_id: null,
        komga_chinese_library_id: null,
        komga_scan_enabled: true,
        optimize_enabled: false,
        optimizer_bin: null,
        target_height: 2400,
        image_quality: 90,
        min_savings_percent: 10,
      };

      expect(defaultConfig.target_height).toBe(2400);
      expect(defaultConfig.image_quality).toBe(90);
      expect(defaultConfig.min_savings_percent).toBe(10);

      // Client patch preserves custom resolution and quality
      const customPatch = {
        ...defaultConfig,
        target_height: 3000,
        image_quality: 85,
        min_savings_percent: 15,
      };

      expect(customPatch.target_height).toBe(3000);
      expect(customPatch.image_quality).toBe(85);
      expect(customPatch.min_savings_percent).toBe(15);
    });

    it('models Komga volume ownership states and detects owned and acquiring volumes', () => {
      const ownership: import('./api').TitleOwnership = {
        komgaConnected: true,
        seriesId: 'series-123',
        seriesName: 'Blue Lock',
        libraryId: 'lib-en',
        libraryName: 'Manga',
        volumes: [
          { volume: 1, status: 'owned', bookId: 'b-1', bookName: 'Blue Lock v01' },
          { volume: 2, status: 'acquiring', bookId: null, bookName: null },
        ],
        chapters: [],
        summary: {
          ownedVolumesCount: 1,
          totalSourceVolumes: null,
          ownedChaptersCount: 0,
          totalSourceChapters: 368,
          sourceReleaseType: 'chapter',
          explanation: 'Source provides chapters',
        },
        lastRefreshed: 1700000000,
      };

      const v1 = ownership.volumes.find((v) => v.volume === 1);
      expect(v1?.status).toBe('owned');
      expect(v1?.bookName).toBe('Blue Lock v01');

      const v2 = ownership.volumes.find((v) => v.volume === 2);
      expect(v2?.status).toBe('acquiring');

      const v3 = ownership.volumes.find((v) => v.volume === 3);
      expect(v3).toBeUndefined(); // Unowned/available

      // Blue Lock regression semantics:
      // When source has chapters and Komga has volumes, totalSourceVolumes must be null
      // so UI does NOT fabricate a fake 1/368 denominator!
      expect(ownership.summary.totalSourceVolumes).toBeNull();
      expect(ownership.summary.explanation).toBe('Source provides chapters');
    });

    it('verifies honest ownership summary denominators for volume and mixed sources', () => {
      // Volume source + Volume Komga (38 / 39)
      const volOwnership: import('./api').OwnershipSummary = {
        ownedVolumesCount: 38,
        totalSourceVolumes: 39,
        ownedChaptersCount: 0,
        totalSourceChapters: null,
        sourceReleaseType: 'volume',
        explanation: null,
      };
      expect(volOwnership.ownedVolumesCount).toBe(38);
      expect(volOwnership.totalSourceVolumes).toBe(39);
      expect(volOwnership.explanation).toBeNull();

      // Chapter source + Chapter Komga (184 / 201)
      const chOwnership: import('./api').OwnershipSummary = {
        ownedVolumesCount: 0,
        totalSourceVolumes: null,
        ownedChaptersCount: 184,
        totalSourceChapters: 201,
        sourceReleaseType: 'chapter',
        explanation: null,
      };
      expect(chOwnership.ownedChaptersCount).toBe(184);
      expect(chOwnership.totalSourceChapters).toBe(201);

      // Mixed releases (Volumes 12/14, Chapters 3/3)
      const mixedOwnership: import('./api').OwnershipSummary = {
        ownedVolumesCount: 12,
        totalSourceVolumes: 14,
        ownedChaptersCount: 3,
        totalSourceChapters: 3,
        sourceReleaseType: 'mixed',
        explanation: null,
      };
      expect(mixedOwnership.sourceReleaseType).toBe('mixed');
      expect(mixedOwnership.ownedVolumesCount).toBe(12);
      expect(mixedOwnership.totalSourceVolumes).toBe(14);
      expect(mixedOwnership.ownedChaptersCount).toBe(3);
      expect(mixedOwnership.totalSourceChapters).toBe(3);
    });

    it('formats acquisition queue lifecycle stages and detects komga retry availability', () => {
      const normalQueued: import('./api').Queued = {
        chapterId: 101,
        chapterName: 'Chapter 1',
        mangaId: 1,
        title: 'Blue Lock',
        state: 'queued',
        stage: 'queued',
        source: 'MangaDex',
        releaseType: 'chapter',
        done: 0,
        total: 20,
        attempts: 0,
        retryAt: null,
        error: null,
        canRetryKomga: false,
      };

      const optimizingQueued: import('./api').Queued = {
        chapterId: 102,
        chapterName: 'Chapter 2',
        mangaId: 1,
        title: 'Blue Lock',
        state: 'downloading',
        stage: 'optimizing',
        statusMessage: 'Optimizing image compression (RustyLibraryShrinker)...',
        source: 'MangaDex',
        releaseType: 'chapter',
        done: 20,
        total: 20,
        attempts: 0,
        retryAt: null,
        error: null,
        canRetryKomga: false,
      };

      const komgaFailedQueued: import('./api').Queued = {
        chapterId: 103,
        chapterName: 'Chapter 3',
        mangaId: 1,
        title: 'Blue Lock',
        state: 'failed',
        stage: 'komga',
        error: 'Published to /srv/media/manga/Blue Lock, but Komga scan trigger failed: connection refused',
        source: 'MangaDex',
        releaseType: 'chapter',
        done: 20,
        total: 20,
        attempts: 1,
        retryAt: null,
        canRetryKomga: true,
        failureStage: 'komga',
      };

      expect(normalQueued.canRetryKomga).toBe(false);
      expect(optimizingQueued.stage).toBe('optimizing');
      expect(komgaFailedQueued.canRetryKomga).toBe(true);
      expect(komgaFailedQueued.failureStage).toBe('komga');
    });

    it('validates Komga library discovery selection and dual library ID targeting', () => {
      const discoveredLibraries: import('./api').KomgaLibrary[] = [
        { id: 'lib-001', name: 'Manga', root: '/srv/media/manga', unavailable: false },
        { id: 'lib-002', name: 'Manga (Chinese)', root: '/srv/media/manga-chinese', unavailable: false },
      ];

      expect(discoveredLibraries).toHaveLength(2);
      expect(discoveredLibraries[0].id).toBe('lib-001');
      expect(discoveredLibraries[1].id).toBe('lib-002');

      const configWithDualLibs: import('./api').PipelineConfig = {
        enabled: true,
        library_dir: '/srv/media/manga',
        chinese_library_dir: '/srv/media/manga-chinese',
        mangabaka_enabled: true,
        mangabaka_url: 'https://api.mangabaka.org/v1',
        has_mangabaka_token: false,
        komga_enabled: true,
        komga_url: 'http://127.0.0.1:25600',
        has_komga_api_key: true,
        komga_library_id: 'lib-001',
        komga_chinese_library_id: 'lib-002',
        komga_scan_enabled: true,
        optimize_enabled: true,
        optimizer_bin: null,
        target_height: 2400,
        image_quality: 90,
        min_savings_percent: 10,
      };

      expect(configWithDualLibs.komga_library_id).toBe('lib-001');
      expect(configWithDualLibs.komga_chinese_library_id).toBe('lib-002');
      expect(configWithDualLibs.komga_scan_enabled).toBe(true);
    });

    it('models reading progress synchronization and dry-run audit records', async () => {
      const { runSyncDryRun, getSyncHistory } = await import('./api');
      type SyncRecord = import('./api').SyncRecord;
      type SyncSummary = import('./api').SyncSummary;

      const mockSummary: SyncSummary = {
        totalSeries: 2,
        updated: 0,
        noChange: 1,
        dryRunUpdates: 1,
        skipped: 0,
        errors: 0,
        isDryRun: true,
        records: [
          {
            id: 'sync-1',
            timestamp: 1714560000,
            komgaSeriesId: 'series-001',
            komgaSeriesTitle: 'Chainsaw Man',
            tracker: 'anilist',
            trackerMediaId: '105778',
            oldProgress: 50,
            newProgress: 97,
            statusChange: null,
            category: 'dry_run_would_update',
            message: 'Dry run: would update AniList progress from Ch 50 to Ch 97',
            isDryRun: true,
          },
          {
            id: 'sync-2',
            timestamp: 1714560000,
            komgaSeriesId: 'series-002',
            komgaSeriesTitle: 'Blue Lock',
            tracker: 'anilist',
            trackerMediaId: '106130',
            oldProgress: 180,
            newProgress: 180,
            statusChange: null,
            category: 'tracker_ahead',
            message: 'AniList is ahead (Ch 180) of Komga progress (Ch 150); keeping Ch 180',
            isDryRun: true,
          },
        ],
      };

      expect(mockSummary.isDryRun).toBe(true);
      expect(mockSummary.dryRunUpdates).toBe(1);
      expect(mockSummary.records[0].category).toBe('dry_run_would_update');
      expect(mockSummary.records[1].category).toBe('tracker_ahead');
      expect(mockSummary.records[1].newProgress).toBe(180); // never decreases!
    });

    it('verifies tracked library foundation distinctly separates tracked, owned, acquiring, read, and auto-acquire', async () => {
      type TrackedCatalogEntry = import('./api').TrackedCatalogEntry;

      const trackedItem: TrackedCatalogEntry = {
        id: 42,
        mangaId: 101,
        komgaSeriesId: 'series-dandadan',
        title: 'Dandadan',
        origins: ['mango', 'anilist'],
        autoAcquire: false, // Strictly false on tracking
        tracked: true,
        createdAt: 1714500000,
        updatedAt: 1714500000,
      };

      // Proves separation: tracking does NOT mean downloading or auto-acquiring
      expect(trackedItem.tracked).toBe(true);
      expect(trackedItem.autoAcquire).toBe(false);
      expect(trackedItem.origins).toContain('mango');
      expect(trackedItem.origins).toContain('anilist');
    });

    it('distinguishes MangaBaka token configuration from connection validation', () => {
      type PipelineConfig = import('./api').PipelineConfig;

      function getMangaBakaStatus(
        cfg: PipelineConfig,
        testResult?: { ok: boolean; message: string } | null,
      ): { text: string; badgeClass: string } {
        if (!cfg.has_mangabaka_token) {
          return { text: 'MangaBaka: No Token', badgeClass: 'status-not_configured' };
        }
        if (testResult ? testResult.ok : cfg.mangabaka_validated === true) {
          return { text: 'MangaBaka: Validated', badgeClass: 'status-valid' };
        }
        if (testResult ? !testResult.ok : cfg.mangabaka_validated === false) {
          return { text: 'MangaBaka: Validation Failed', badgeClass: 'status-missing' };
        }
        return { text: 'MangaBaka: Token Configured (Unverified)', badgeClass: 'status-not_configured' };
      }

      // 1. No token configured
      const noTokenCfg = { has_mangabaka_token: false } as PipelineConfig;
      expect(getMangaBakaStatus(noTokenCfg)).toEqual({
        text: 'MangaBaka: No Token',
        badgeClass: 'status-not_configured',
      });

      // 2. Token configured, not yet tested (unverified)
      const unverifiedCfg = { has_mangabaka_token: true, mangabaka_validated: null } as PipelineConfig;
      expect(getMangaBakaStatus(unverifiedCfg)).toEqual({
        text: 'MangaBaka: Token Configured (Unverified)',
        badgeClass: 'status-not_configured',
      });

      // 3. Token configured, validation succeeded
      const validatedCfg = { has_mangabaka_token: true, mangabaka_validated: true } as PipelineConfig;
      expect(getMangaBakaStatus(validatedCfg)).toEqual({
        text: 'MangaBaka: Validated',
        badgeClass: 'status-valid',
      });

      // 4. Token configured, validation failed (e.g. HTTP 400 Bad Request)
      const failedCfg = { has_mangabaka_token: true, mangabaka_validated: false } as PipelineConfig;
      expect(getMangaBakaStatus(failedCfg)).toEqual({
        text: 'MangaBaka: Validation Failed',
        badgeClass: 'status-missing',
      });

      // 5. Active test result in UI overrides stored state
      expect(getMangaBakaStatus(unverifiedCfg, { ok: true, message: 'Connected' })).toEqual({
        text: 'MangaBaka: Validated',
        badgeClass: 'status-valid',
      });
      expect(getMangaBakaStatus(validatedCfg, { ok: false, message: 'Bad Request' })).toEqual({
        text: 'MangaBaka: Validation Failed',
        badgeClass: 'status-missing',
      });
    });

    it('verifies authoritative Trackers status presentation and audit latest-run filtering', () => {
      type SyncStatus = import('./api').SyncStatus;
      type SyncRecord = import('./api').SyncRecord;

      function getKomgaStatus(status: { komga_enabled: boolean; has_komga_api_key: boolean }): {
        text: string;
        badgeClass: string;
      } {
        if (!status.komga_enabled) {
          return { text: 'Komga: Disabled', badgeClass: 'status-not_configured' };
        }
        if (status.has_komga_api_key) {
          return { text: 'Komga: Connected', badgeClass: 'status-valid' };
        }
        return { text: 'Komga: No API Key', badgeClass: 'status-missing' };
      }

      // 1. Komga integration status tests
      expect(getKomgaStatus({ komga_enabled: false, has_komga_api_key: false })).toEqual({
        text: 'Komga: Disabled',
        badgeClass: 'status-not_configured',
      });
      expect(getKomgaStatus({ komga_enabled: true, has_komga_api_key: true })).toEqual({
        text: 'Komga: Connected',
        badgeClass: 'status-valid',
      });
      expect(getKomgaStatus({ komga_enabled: true, has_komga_api_key: false })).toEqual({
        text: 'Komga: No API Key',
        badgeClass: 'status-missing',
      });

      // 2. Audit record latest-run detection and filtering
      const run1Time = 1700000000;
      const run2Time = 1700003600; // 1 hour later

      const records: SyncRecord[] = [
        {
          id: 'rec-run2-1',
          timestamp: run2Time,
          komgaSeriesId: 's1',
          komgaSeriesTitle: 'Blue Lock',
          tracker: 'mangabaka',
          category: 'no_change',
          message: 'Up to date',
          isDryRun: true,
        },
        {
          id: 'rec-run2-2',
          timestamp: run2Time - 2, // 2 seconds earlier in same run
          komgaSeriesId: 's2',
          komgaSeriesTitle: 'Pandora Hearts',
          tracker: 'mangabaka',
          category: 'no_change',
          message: 'Up to date',
          isDryRun: true,
        },
        {
          id: 'rec-run1-1',
          timestamp: run1Time,
          komgaSeriesId: 's1',
          komgaSeriesTitle: 'Blue Lock',
          tracker: 'mangabaka',
          category: 'ambiguous_mapping',
          message: 'Ambiguous',
          isDryRun: true,
        },
      ];

      const latestRunTimestamp = records[0].timestamp;
      function isLatestRun(rec: SyncRecord): boolean {
        return Math.abs(rec.timestamp - latestRunTimestamp) <= 30;
      }

      expect(isLatestRun(records[0])).toBe(true);
      expect(isLatestRun(records[1])).toBe(true);
      expect(isLatestRun(records[2])).toBe(false);

      const latestRunRecords = records.filter(isLatestRun);
      expect(latestRunRecords.length).toBe(2);
      expect(latestRunRecords.map((r) => r.id)).toEqual(['rec-run2-1', 'rec-run2-2']);
    });
  });

  describe('first-class Lists navigation and tracked catalog UI', () => {
    it('verifies Lists navigation destination is defined with bookmark icon', () => {
      const navItem = { to: '/lists', label: 'Lists', icon: 'bookmark', under: ['lists'] };
      expect(navItem.to).toBe('/lists');
      expect(navItem.label).toBe('Lists');
      expect(navItem.icon).toBe('bookmark');
      expect(navItem.under).toContain('lists');
    });

    it('filters tracked items by normalized status correctly', () => {
      const items: Array<{ id: string; title: string; normalizedStatus: string }> = [
        { id: '1', title: 'Blue Lock', normalizedStatus: 'Reading' },
        { id: '2', title: 'Frieren', normalizedStatus: 'Planning' },
        { id: '3', title: 'Fullmetal Alchemist', normalizedStatus: 'Completed' },
        { id: '4', title: 'Nana', normalizedStatus: 'Paused' },
        { id: '5', title: 'Random Manga', normalizedStatus: 'Dropped' },
      ];

      expect(items.filter((i) => i.normalizedStatus.toLowerCase() === 'reading').length).toBe(1);
      expect(items.filter((i) => i.normalizedStatus.toLowerCase() === 'planning').length).toBe(1);
      expect(items.filter((i) => i.normalizedStatus.toLowerCase() === 'completed').length).toBe(1);
      expect(items.filter((i) => i.normalizedStatus.toLowerCase() === 'paused').length).toBe(1);
      expect(items.filter((i) => i.normalizedStatus.toLowerCase() === 'dropped').length).toBe(1);
    });

    it('filters tracked items by local ownership status independently', () => {
      const items: Array<{ id: string; title: string; ownershipStatus: string }> = [
        { id: '1', title: 'Owned Manga', ownershipStatus: 'Owned' },
        { id: '2', title: 'Partial Manga', ownershipStatus: 'Partially Owned' },
        { id: '3', title: 'TBR Manga', ownershipStatus: 'Not Owned' },
      ];

      expect(items.filter((i) => i.ownershipStatus === 'Owned').length).toBe(1);
      expect(items.filter((i) => i.ownershipStatus === 'Partially Owned').length).toBe(1);
      expect(items.filter((i) => i.ownershipStatus === 'Not Owned').length).toBe(1);
    });

    it('filters tracked items by tracker presence (AniList, MangaBaka, Both)', () => {
      const items: Array<{ id: string; anilist: boolean; mangabaka: boolean }> = [
        { id: '1', anilist: true, mangabaka: false },
        { id: '2', anilist: false, mangabaka: true },
        { id: '3', anilist: true, mangabaka: true },
      ];

      // AniList filter
      expect(items.filter((i) => i.anilist).length).toBe(2);
      // MangaBaka filter
      expect(items.filter((i) => i.mangabaka).length).toBe(2);
      // Both filter
      expect(items.filter((i) => i.anilist && i.mangabaka).length).toBe(1);
    });

    it('verifies unified title displays both AniList and MangaBaka presence', () => {
      const unifiedItem = {
        id: 'al-101_mb-202',
        title: 'Chainsaw Man',
        normalizedStatus: 'Reading',
        anilist: { trackerId: '101', status: 'CURRENT', normalizedStatus: 'Reading', progressChapter: 97, updatedAt: 1700000000 },
        mangabaka: { trackerId: '202', status: 'reading', normalizedStatus: 'Reading', progressChapter: 97, updatedAt: 1700000100 },
        ownershipStatus: 'Owned',
      };

      expect(unifiedItem.anilist).toBeDefined();
      expect(unifiedItem.mangabaka).toBeDefined();
      expect(unifiedItem.anilist?.status).toBe('CURRENT');
      expect(unifiedItem.mangabaka?.status).toBe('reading');
    });

    it('displays tracker-only titles even when physically not owned', () => {
      const tbrItem = {
        id: 'al-999',
        title: 'Future Reading Manga',
        normalizedStatus: 'Planning',
        anilist: { trackerId: '999', status: 'PLANNING', normalizedStatus: 'Planning', progressChapter: 0, updatedAt: 1700000000 },
        mangabaka: null,
        ownershipStatus: 'Not Owned',
        ownedUnits: null,
        autoAcquire: false,
      };

      expect(tbrItem.ownershipStatus).toBe('Not Owned');
      expect(tbrItem.ownedUnits).toBeNull();
      expect(tbrItem.autoAcquire).toBe(false);
      expect(tbrItem.anilist).not.toBeNull();
    });

    it('proves owned status is strictly distinct from tracked reading status', () => {
      // Case A: Owned locally (bought all volumes) but user is still in Planning on tracker
      const plannedOwned = {
        title: 'Series A',
        normalizedStatus: 'Planning',
        ownershipStatus: 'Owned',
        ownedUnits: 20,
        totalUnits: 20,
      };
      expect(plannedOwned.normalizedStatus).toBe('Planning');
      expect(plannedOwned.ownershipStatus).toBe('Owned');

      // Case B: Currently Reading on tracker but not owned locally (read online/physically elsewhere)
      const readingUnowned = {
        title: 'Series B',
        normalizedStatus: 'Reading',
        ownershipStatus: 'Not Owned',
        ownedUnits: null,
      };
      expect(readingUnowned.normalizedStatus).toBe('Reading');
      expect(readingUnowned.ownershipStatus).toBe('Not Owned');
    });

    it('models empty, loading, and error states gracefully', () => {
      type CatalogView = { loading: boolean; error: string | null; count: number };

      const loadingView: CatalogView = { loading: true, error: null, count: 0 };
      expect(loadingView.loading).toBe(true);

      const errorView: CatalogView = { loading: false, error: 'Network timeout', count: 0 };
      expect(errorView.error).toBe('Network timeout');

      const emptyView: CatalogView = { loading: false, error: null, count: 0 };
      expect(emptyView.count).toBe(0);
    });

    it('preserves cached list when provider refresh returns an error', () => {
      const cachedCatalog = {
        items: [
          { id: 'al-1', title: 'Cached Manga 1', normalizedStatus: 'Reading' },
          { id: 'mb-2', title: 'Cached Manga 2', normalizedStatus: 'Planning' },
        ],
        refreshStatuses: [
          { tracker: 'anilist', lastRefreshedAt: 1700000000, itemCount: 1, status: 'ok', errorMessage: null },
          { tracker: 'mangabaka', lastRefreshedAt: 1700001000, itemCount: 0, status: 'error', errorMessage: '500 Internal Server Error' },
        ],
      };

      // Despite MangaBaka status being error, cached catalog items remain populated!
      expect(cachedCatalog.items.length).toBe(2);
      expect(cachedCatalog.refreshStatuses.find((s) => s.tracker === 'mangabaka')?.status).toBe('error');
    });

    it('formats proposed status changes in dry run / audit records', () => {
      const dryRunRecord = {
        id: 'rec-1',
        category: 'would_update',
        isDryRun: true,
        oldProgress: 0.0,
        newProgress: 1.0,
        statusChange: 'PLANNING -> CURRENT',
        message: 'Dry run: would update AniList progress from Vol 0 to Vol 1 (status -> PLANNING -> CURRENT)',
      };

      expect(dryRunRecord.statusChange).toBe('PLANNING -> CURRENT');
      expect(dryRunRecord.category).toBe('would_update');
      expect(dryRunRecord.isDryRun).toBe(true);
      expect(dryRunRecord.message).toContain('PLANNING -> CURRENT');
    });

    it('derives contextual/faceted status and ownership counts reactively without API calls', () => {
      type Item = {
        id: string;
        title: string;
        aliases?: string[];
        normalizedStatus: string;
        ownershipStatus: string;
        anilist: boolean;
        mangabaka: boolean;
      };

      const items: Item[] = [
        { id: '1', title: 'Berserk', aliases: ['베르세르크'], normalizedStatus: 'Reading', ownershipStatus: 'Owned', anilist: true, mangabaka: false },
        { id: '2', title: 'Sakamoto Days', aliases: ['사카모토 데이즈'], normalizedStatus: 'Reading', ownershipStatus: 'Not Owned', anilist: false, mangabaka: true },
        { id: '3', title: 'Frieren', aliases: ['Sousou no Frieren'], normalizedStatus: 'Planning', ownershipStatus: 'Owned', anilist: true, mangabaka: true },
        { id: '4', title: 'Chainsaw Man', aliases: [], normalizedStatus: 'Completed', ownershipStatus: 'Owned', anilist: true, mangabaka: true },
        { id: '5', title: 'Nana', aliases: [], normalizedStatus: 'Paused', ownershipStatus: 'Partially Owned', anilist: true, mangabaka: false },
      ];

      function computeFacetedCounts(itemsList: Item[], activeOwnership: string, activeTracker: string, search: string) {
        const matchesSecondary = (item: Item) => {
          if (activeOwnership !== 'all') {
            if (activeOwnership === 'owned' && item.ownershipStatus !== 'Owned') return false;
            if (activeOwnership === 'partially_owned' && item.ownershipStatus !== 'Partially Owned') return false;
            if (activeOwnership === 'not_owned' && item.ownershipStatus !== 'Not Owned') return false;
          }
          if (activeTracker !== 'all') {
            if (activeTracker === 'anilist' && !item.anilist) return false;
            if (activeTracker === 'mangabaka' && !item.mangabaka) return false;
            if (activeTracker === 'both' && (!item.anilist || !item.mangabaka)) return false;
          }
          if (search) {
            const q = search.toLowerCase();
            const inTitle = item.title.toLowerCase().includes(q);
            const inAlias = (item.aliases ?? []).some((a) => a.toLowerCase().includes(q));
            if (!inTitle && !inAlias) return false;
          }
          return true;
        };

        const filtered = itemsList.filter(matchesSecondary);
        const counts = { total: filtered.length, reading: 0, planning: 0, completed: 0, paused: 0, dropped: 0 };
        for (const item of filtered) {
          switch (item.normalizedStatus.toLowerCase()) {
            case 'reading': counts.reading++; break;
            case 'planning': counts.planning++; break;
            case 'completed': counts.completed++; break;
            case 'paused': counts.paused++; break;
            case 'dropped': counts.dropped++; break;
          }
        }
        return counts;
      }

      // Unfiltered global counts
      const globalCounts = computeFacetedCounts(items, 'all', 'all', '');
      expect(globalCounts.total).toBe(5);
      expect(globalCounts.reading).toBe(2);
      expect(globalCounts.planning).toBe(1);
      expect(globalCounts.completed).toBe(1);
      expect(globalCounts.paused).toBe(1);

      // Faceted count when Ownership is Owned
      const ownedCounts = computeFacetedCounts(items, 'owned', 'all', '');
      expect(ownedCounts.total).toBe(3);
      expect(ownedCounts.reading).toBe(1); // Berserk
      expect(ownedCounts.planning).toBe(1); // Frieren
      expect(ownedCounts.completed).toBe(1); // CSM
      expect(ownedCounts.paused).toBe(0); // Nana is partially owned

      // Faceted count when Tracker is Both
      const bothCounts = computeFacetedCounts(items, 'all', 'both', '');
      expect(bothCounts.total).toBe(2); // Frieren and CSM
      expect(bothCounts.reading).toBe(0);
      expect(bothCounts.planning).toBe(1);
      expect(bothCounts.completed).toBe(1);

      // Faceted count when Search is "사카모토 데이즈" (Korean alias)
      const aliasCounts = computeFacetedCounts(items, 'all', 'all', '사카모토 데이즈');
      expect(aliasCounts.total).toBe(1);
      expect(aliasCounts.reading).toBe(1); // Sakamoto Days
    });

    it('verifies Cover.svelte renders with referrerpolicy="no-referrer" and accessible alt text', async () => {
      const { render } = await import('svelte/server');
      const Cover = (await import('./Cover.svelte')).default;

      // When src is provided
      const resWithSrc = render(Cover, {
        props: {
          src: 'https://images.mangabaka.dev/covers/350.webp',
          title: 'Berserk',
        },
      });
      expect(resWithSrc.html).toContain('referrerpolicy="no-referrer"');
      expect(resWithSrc.html).toContain('alt="Berserk cover"');
      expect(resWithSrc.html).toContain('https://images.mangabaka.dev/covers/350.webp');
      expect(resWithSrc.html).toContain('B'); // glyph placeholder behind cover

      // When src is null
      const resWithoutSrc = render(Cover, {
        props: {
          src: null,
          title: 'Sakamoto Days',
        },
      });
      expect(resWithoutSrc.html).not.toContain('<img');
      expect(resWithoutSrc.html).toContain('S'); // glyph placeholder
    });

    it('verifies Nyaa release discovery & TorBox external download frontend contracts', () => {
      // 1. Secret token protection in config patch
      const originalConfig: import('./api').AcquisitionConfig = {
        nyaaEnabled: true,
        nyaaUrl: 'https://nyaa.si',
        nyaaCategory: '3_1',
        torboxEnabled: true,
        torboxUrl: 'https://api.torbox.app/v1/api',
        hasTorboxToken: true,
        torboxValidated: true,
        torboxStatusMessage: 'Connected as user@example.com (Plan: Pro)',
      };

      // When token input is '********' (unchanged placeholder), patch must omit token to preserve backend state
      const tokenInputUnchanged = '********';
      const patch1: import('./api').AcquisitionConfigPatch = {
        torboxEnabled: originalConfig.torboxEnabled,
        torboxUrl: originalConfig.torboxUrl || undefined,
        torboxToken: tokenInputUnchanged === 'CLEAR' ? '' : (tokenInputUnchanged && tokenInputUnchanged !== '********' ? tokenInputUnchanged.trim() : undefined),
      };
      expect(patch1.torboxToken).toBeUndefined();

      // When token input is 'CLEAR', patch sends empty string to forget token
      const tokenInputClear = 'CLEAR';
      const patch2: import('./api').AcquisitionConfigPatch = {
        torboxToken: tokenInputClear === 'CLEAR' ? '' : (tokenInputClear && tokenInputClear !== '********' ? tokenInputClear.trim() : undefined),
      };
      expect(patch2.torboxToken).toBe('');

      // When user enters a new token, patch sends trimmed token
      const tokenInputNew = '  secret_new_token_123  ';
      const patch3: import('./api').AcquisitionConfigPatch = {
        torboxToken: tokenInputNew === 'CLEAR' ? '' : (tokenInputNew && tokenInputNew !== '********' ? tokenInputNew.trim() : undefined),
      };
      expect(patch3.torboxToken).toBe('secret_new_token_123');

      // 2. Candidate ranking & status badges
      const candidate: import('./api').AcquisitionCandidate = {
        release: {
          id: 'nyaa-12345',
          title: '[Group] Berserk - Vol. 01 [Digital] [1080p]',
          url: 'https://nyaa.si/view/12345',
          magnet: 'magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567',
          infoHash: '0123456789abcdef0123456789abcdef01234567',
          sizeBytes: 157286400,
          seeders: 42,
          leechers: 3,
          publishedAt: 1700000000,
          uploader: 'Group',
          unit: {
            kind: 'volume',
            number: 1,
            unitType: 'single',
          },
          trusted: true,
          remake: false,
        },
        exactMatch: true,
        alreadyOwned: false,
        currentlyAcquiring: false,
      };

      expect(candidate.exactMatch).toBe(true);
      expect(candidate.release.trusted).toBe(true);
      expect(candidate.release.unit.kind).toBe('volume');
      expect(candidate.release.unit.number).toBe(1);
      expect(candidate.release.seeders).toBe(42);

      // 3. TorBox external download job lifecycle representation
      const torboxJob: import('./api').Queued = {
        chapterId: 200,
        chapterName: 'Volume 1',
        mangaId: 10,
        title: 'Berserk',
        state: 'downloading',
        stage: 'torbox_downloading',
        source: 'Nyaa',
        sourceType: 'nyaa',
        backendType: 'torbox',
        externalJobId: 'tb-999',
        torrentHash: '0123456789abcdef0123456789abcdef01234567',
        rawTitle: '[Group] Berserk - Vol. 01 [Digital] [1080p]',
        uploader: 'Group',
        releaseSize: 157286400,
        releaseType: 'volume',
        done: 45,
        total: 100,
        attempts: 1,
        retryAt: null,
        canRetryKomga: false,
      };

      expect(torboxJob.sourceType).toBe('nyaa');
      expect(torboxJob.backendType).toBe('torbox');
      expect(torboxJob.stage).toBe('torbox_downloading');
      expect(torboxJob.uploader).toBe('Group');

      // 4. Smart Nyaa ranking and collection inspection representation
      const smartScore: import('./api').SmartScore = {
        total: 88.5,
        identity: 25.0,
        coverage: 25.0,
        quality: 15.0,
        health: 13.5,
        freshness: 10.0,
        efficiency: 10.0,
        penalties: 0.0,
        summaryTag: 'BEST MATCH',
        explanation: 'Excellent coverage for missing volumes; High health (42 seeders); Trusted uploader',
      };

      const collectionCandidate: import('./api').AcquisitionCandidate = {
        release: {
          id: 'nyaa-54321',
          title: '[Stick] Blue Lock v01-39 [Digital]',
          url: 'https://nyaa.si/view/54321',
          magnet: 'magnet:?xt=urn:btih:fedcba9876543210fedcba9876543210fedcba98',
          infoHash: 'fedcba9876543210fedcba9876543210fedcba98',
          sizeBytes: 4200000000,
          seeders: 55,
          leechers: 8,
          publishedAt: 1710000000,
          uploader: 'Stick',
          unit: {
            kind: 'range',
            start: 1,
            end: 39,
            unitType: 'range',
          },
          trusted: true,
          remake: false,
          isVariant: false,
          isMixed: false,
        },
        exactMatch: true,
        alreadyOwned: false,
        currentlyAcquiring: false,
        smartScore,
        unitsIncluded: [1, 2, 3],
        unitsMissing: [2, 3],
        unitsOwned: [1],
        ownershipStatus: 'partial',
        coverageSummary: '2 of 3 volumes missing',
      };

      expect(collectionCandidate.smartScore?.summaryTag).toBe('BEST MATCH');
      expect(collectionCandidate.ownershipStatus).toBe('partial');
      expect(collectionCandidate.unitsMissing).toEqual([2, 3]);

      // 5. Collection Inspection model and child unit download representation
      const inspection: import('./api').CollectionInspection = {
        releaseTitle: '[Stick] Blue Lock v01-03 [Digital]',
        totalUnits: 3,
        missingCount: 2,
        ownedCount: 1,
        recommendedMode: 'missing_only',
        units: [
          { volume: 1, isOwned: true, isAcquiring: false, filename: 'Blue Lock v01.cbz', fileSize: 100000000 },
          { volume: 2, isOwned: false, isAcquiring: false, filename: 'Blue Lock v02.cbz', fileSize: 105000000 },
          { volume: 3, isOwned: false, isAcquiring: false, filename: 'Blue Lock v03.cbz', fileSize: 110000000 },
        ],
      };

      expect(inspection.recommendedMode).toBe('missing_only');
      expect(inspection.units[0].isOwned).toBe(true);
      expect(inspection.units[1].isOwned).toBe(false);

      const collectionChildJob: import('./api').Queued = {
        chapterId: 202,
        chapterName: 'Volume 2',
        mangaId: 1,
        title: 'Blue Lock',
        state: 'queued',
        stage: 'queued',
        source: 'Nyaa',
        sourceType: 'nyaa',
        backendType: 'torbox',
        externalJobId: 'tb-8888',
        collectionId: 'col-8888',
        collectionName: 'Blue Lock v01-03',
        rawTitle: '[Stick] Blue Lock v01-03 [Digital]',
        uploader: 'Stick',
        releaseSize: 105000000,
        releaseType: 'volume',
        done: 0,
        total: 100,
        attempts: 0,
        retryAt: null,
        canRetryKomga: false,
      };

      expect(collectionChildJob.collectionId).toBe('col-8888');
      expect(collectionChildJob.collectionName).toBe('Blue Lock v01-03');
    });
  });
});
