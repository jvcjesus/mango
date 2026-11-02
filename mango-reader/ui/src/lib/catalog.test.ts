import { beforeAll, describe, expect, it } from 'vitest';
import type { Title as TitleType } from './api';

describe('Catalog Titles & Optional Reading Sources (M6.7)', () => {
  let Title: any;
  let AddMangaDialog: any;
  let render: any;
  let keep: any;

  beforeAll(async () => {
    if (typeof (globalThis as any).history === 'undefined') {
      (globalThis as any).history = {
        scrollRestoration: 'auto',
        state: {},
        replaceState: () => {},
        pushState: () => {},
        back: () => {},
      };
    }
    if (typeof (globalThis as any).location === 'undefined') {
      (globalThis as any).location = { hash: '#/manga/9001' };
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

    const svelteServer = await import('svelte/server');
    render = svelteServer.render;
    const keepModule = await import('./keep');
    keep = keepModule.keep;
    Title = (await import('../routes/Title.svelte')).default;
    AddMangaDialog = (await import('./AddMangaDialog.svelte')).default;
  });

  const catalogOnlyTitle: TitleType = {
    id: 9001,
    sourceId: '0',
    sourceName: null,
    sourceLang: null,
    webUrl: null,
    title: 'Blue Lock (Catalog Test)',
    artist: 'Yusuke Nomura',
    author: 'Muneyuki Kaneshiro',
    description: 'A catalog-native title without scraped web chapters.',
    genres: ['Action', 'Sports'],
    status: 1, // Ongoing
    cover: null,
    inLibrary: true,
    addedAt: 1700000000,
    initialized: true,
    readerMode: null,
    readerFit: null,
    autoDownload: false,
    chaptersAt: 0,
    releaseDays: null,
    categories: [],
    chapters: [],
    isCatalog: true,
    mangabakaId: 83,
    sourceMangaId: null,
    catalogMangaId: null,
    attachedSourceName: null,
  };

  it('renders catalog-only title without chapter counters or empty table clutter', () => {
    keep('title', catalogOnlyTitle);
    const result = render(Title, { props: { id: 9001 } });

    // Header has Catalog Title badge and MangaBaka link
    expect(result.html).toContain('Catalog Title');
    expect(result.html).toContain('MangaBaka #83');

    // Does NOT render meaningless 0 Chapters / 0 Unread / 0 Saved counters
    expect(result.html).not.toContain('class="stats');

    // Does NOT render chapter list or empty table clutter
    expect(result.html).not.toContain('class="chapters');

    // Renders clean Reading section with "No reading source attached"
    expect(result.html).toContain('READING');
    expect(result.html).toContain('No reading source attached.');
    expect(result.html).toContain('This work is tracked natively in your Mango catalog.');
  });

  it('renders attached reading source info on catalog title when source is attached', () => {
    const catalogWithSource: TitleType = {
      ...catalogOnlyTitle,
      id: 9002,
      sourceMangaId: 15,
      attachedSourceName: 'MangaDot',
    };
    keep('title', catalogWithSource);
    const result = render(Title, { props: { id: 9002 } });

    expect(result.html).toContain('Reading source attached:');
    expect(result.html).toContain('MangaDot');
    expect(result.html).toContain('View Scraped Chapters');
    expect(result.html).toContain('Revert to Source Primary');
    expect(result.html).toContain('Detach Reading Source');
  });

  it('renders conversion banner for unconverted source-backed manga', () => {
    const sourceTitle: TitleType = {
      id: 15,
      sourceId: '1',
      sourceName: 'MangaDot',
      sourceLang: 'en',
      webUrl: 'https://mangadot.net/manga/bluelock',
      title: 'Blue Lock (Source)',
      artist: 'Yusuke Nomura',
      author: 'Muneyuki Kaneshiro',
      description: 'Source backed version.',
      genres: ['Sports'],
      status: 1,
      cover: null,
      inLibrary: true,
      addedAt: 1700000000,
      initialized: true,
      readerMode: null,
      readerFit: null,
      autoDownload: false,
      chaptersAt: 1700000000,
      releaseDays: 7,
      categories: [],
      chapters: [
        {
          id: 101,
          name: 'Chapter 1',
          number: 1,
          scanlator: 'Pairs',
          dateUpload: 1700000000,
          fetchedAt: 1700000000,
          read: false,
          lastPageRead: 0,
          pageCount: 20,
          downloaded: false,
          bookmark: false,
        },
      ],
      isCatalog: false,
      mangabakaId: null,
      sourceMangaId: null,
      catalogMangaId: null,
      attachedSourceName: null,
    };
    keep('title', sourceTitle);
    const result = render(Title, { props: { id: 15 } });

    // Stats block visible for source-backed title
    expect(result.html).toContain('class="stats');
    expect(result.html).toContain('Chapters');
    expect(result.html).toContain('Unread');

    // Convert to catalog banner visible
    expect(result.html).toContain('Convert to Catalog Title');
    expect(result.html).toContain('Manage this work natively with Komga volume ownership');

    // Chapters section visible
    expect(result.html).toContain('class="chapters');
    expect(result.html).toContain('Chapter 1');
  });

  it('renders converted banner for source-backed manga that has been converted', () => {
    const convertedSourceTitle: TitleType = {
      id: 15,
      sourceId: '1',
      sourceName: 'MangaDot',
      sourceLang: 'en',
      webUrl: 'https://mangadot.net/manga/bluelock',
      title: 'Blue Lock',
      artist: null,
      author: null,
      description: null,
      genres: [],
      status: 1,
      cover: null,
      inLibrary: false,
      addedAt: null,
      initialized: true,
      readerMode: null,
      readerFit: null,
      autoDownload: false,
      chaptersAt: 0,
      releaseDays: null,
      categories: [],
      chapters: [],
      isCatalog: false,
      mangabakaId: 83,
      sourceMangaId: null,
      catalogMangaId: 9001,
      attachedSourceName: null,
    };
    keep('title', convertedSourceTitle);
    const result = render(Title, { props: { id: 15 } });

    expect(result.html).toContain('Catalog Version Available');
    expect(result.html).toContain('Open Catalog Title');
    expect(result.html).toContain('href="#/manga/9001"');
    expect(result.html).toContain('Restore as Primary');
  });

  it('renders AddMangaDialog with search input and MangaBaka attributes', () => {
    const result = render(AddMangaDialog, { props: { open: true, onclose: () => {} } });
    expect(result.html).toContain('ADD TO MANGO');
    expect(result.html).toContain('Search MangaBaka to add manga directly as a catalog title');
    expect(result.html).toContain('Search by title, MangaBaka ID, or URL...');
  });
});
