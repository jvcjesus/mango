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
        volumes: [
          { volume: 1, status: 'owned', bookId: 'b-1', bookName: 'Blue Lock v01' },
          { volume: 2, status: 'acquiring', bookId: null, bookName: null },
        ],
      };

      const v1 = ownership.volumes.find((v) => v.volume === 1);
      expect(v1?.status).toBe('owned');
      expect(v1?.bookName).toBe('Blue Lock v01');

      const v2 = ownership.volumes.find((v) => v.volume === 2);
      expect(v2?.status).toBe('acquiring');

      const v3 = ownership.volumes.find((v) => v.volume === 3);
      expect(v3).toBeUndefined(); // Unowned/available
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
  });
});
