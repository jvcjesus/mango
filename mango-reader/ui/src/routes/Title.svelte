<script lang="ts">
  import { get, message, send, type Category, type Chapter, type OwnershipStatus, type Title, type TitleOwnership } from '../lib/api';
  import { confirm } from '../lib/confirm.svelte';
  import Cover from '../lib/Cover.svelte';
  import Icon from '../lib/Icon.svelte';
  import { keep, recall } from '../lib/keep';
  import Notice from '../lib/Notice.svelte';
  import { pref, setPref } from '../lib/prefs.svelte';
  import { back } from '../lib/router.svelte';
  import { toast } from '../lib/toast.svelte';
  import Tracking from '../lib/Tracking.svelte';
  import { ago, chapterShort, langName, parseVolume, volumeHeading, STATUS } from '../lib/util';

  const WEEK = 7 * 86400;

  type Sort = 'source' | 'number' | 'date';
  type View = { sort: Sort; down: boolean; volumes: boolean };

  const SORTS: [Sort, string][] = [
    ['source', 'Source order'],
    ['number', 'Number'],
    ['date', 'Date'],
  ];
  const DEFAULT: View = { sort: 'source', down: true, volumes: false };

  type Show = 'all' | 'unread' | 'downloaded' | 'bookmarked';

  const SHOWS: [Show, string][] = [
    ['all', 'All chapters'],
    ['unread', 'Unread'],
    ['downloaded', 'Downloaded'],
    ['bookmarked', 'Bookmarked'],
  ];

  let { id }: { id: number } = $props();

  /** The title last open, if it is this one: come back to from the reader, it shows at once. */
  function before(): Title | null {
    const was = recall<Title>('title');
    return was?.id === id ? was : null;
  }

  let title = $state<Title | null>(before());
  let loading = $state(true);
  let error = $state<string | null>(null);
  // A failed refresh keeps what is on screen and says so, instead of replacing it.
  let refreshing = $state(false);
  let refreshError = $state<string | null>(null);
  let menu = $state(false);
  let wholeText = $state(false);
  let filters = $state(false);
  // The chapter whose own menu is open.
  let opened = $state<number | null>(null);
  // What the list is narrowed to lasts as long as the page: the next title opened shows whole.
  let show = $state<Show>('all');
  // Selected uploader filter: '' means All, '__unknown__' means chapters with no uploader/scanlator,
  // or a specific scanlator/uploader string.
  function initialUploader(): string {
    return recall<string>(`title.${id}.uploader`) ?? '';
  }
  let uploader = $state(initialUploader());

  $effect(() => {
    // Whenever the title id changes, recall that title's uploader preference
    uploader = recall<string>(`title.${id}.uploader`) ?? '';
  });

  function setUploader(val: string) {
    uploader = val;
    keep(`title.${id}.uploader`, val);
  }

  const chapters = $derived(title?.chapters ?? []);
  const unread = $derived(chapters.filter((c) => !c.read).length);
  const saved = $derived(chapters.filter((c) => c.downloaded).length);
  const hasVolumes = $derived(
    chapters.some((c) => (c.volume !== undefined && c.volume !== null) || parseVolume(c.name) !== null)
  );
  // The order is kept for every title, on the server like the library's.
  // When trustworthy volumes exist, default to volume view unless user explicitly chose otherwise.
  // For chapter-only/manhwa titles without volume metadata, never force volume grouping.
  const savedView = $derived(pref<Partial<View>>('title.view', {}));
  const view = $derived<View>({
    sort: savedView.sort ?? 'source',
    down: savedView.down ?? true,
    volumes: hasVolumes && (savedView.volumes !== undefined ? savedView.volumes : true),
  });

  const hasUnknown = $derived(chapters.some((c) => !c.scanlator?.trim()));
  const namedGroups = $derived(
    [...new Set(chapters.map((c) => c.scanlator?.trim() ?? '').filter(Boolean))].sort((a, b) => a.localeCompare(b))
  );
  // Show uploader filter when there are multiple uploaders or a named uploader mixed with unknown releases
  const hasMultipleUploaders = $derived(namedGroups.length > 1 || (namedGroups.length >= 1 && hasUnknown));

  // If a refresh took away the selected uploader, safely fall back to ''
  const currentUploader = $derived(
    uploader && (uploader === '__unknown__' ? hasUnknown : namedGroups.includes(uploader)) ? uploader : ''
  );

  type UploaderChoice = { value: string; label: string };
  const uploaderChoices = $derived.by<UploaderChoice[]>(() => {
    const list: UploaderChoice[] = [{ value: '', label: 'All' }];
    for (const g of namedGroups) {
      list.push({ value: g, label: g });
    }
    if (hasUnknown && namedGroups.length > 0) {
      list.push({ value: '__unknown__', label: 'Unknown' });
    }
    return list;
  });

  const filtering = $derived(show !== 'all' || currentUploader !== '');
  const listed = $derived.by(() => {
    const shown: Record<Show, (c: Chapter) => boolean> = { all: () => true, unread: (c) => !c.read, downloaded: (c) => c.downloaded, bookmarked: (c) => c.bookmark };
    const list = chapters.filter((c) => {
      if (!shown[show](c)) return false;
      if (!currentUploader) return true;
      if (currentUploader === '__unknown__') return !c.scanlator?.trim();
      return c.scanlator?.trim() === currentUploader;
    });
    // Sources list the newest first, and so does every order here until it is turned round.
    if (view.sort !== 'source') {
      const key = view.sort === 'number' ? (c: Chapter) => c.number : (c: Chapter) => c.dateUpload || c.fetchedAt * 1000;
      list.sort((a, b) => key(b) - key(a));
    }
    return view.down ? list : list.reverse();
  });
  const filteredUnread = $derived(listed.filter((c) => !c.read && !c.downloaded));
  let ownership = $state<TitleOwnership | null>(null);
  let refreshingOwnership = $state(false);
  let showOwnershipDetails = $state(false);

  async function loadOwnership() {
    try {
      ownership = await get<TitleOwnership>(`/manga/${id}/ownership`);
    } catch {
      ownership = null;
    }
  }

  async function refreshOwnership() {
    refreshingOwnership = true;
    try {
      ownership = await send<TitleOwnership>('POST', `/manga/${id}/refresh-ownership`);
      toast.success('Ownership refreshed from Komga');
    } catch (e) {
      toast.error(`Ownership refresh failed: ${message(e)}`);
    } finally {
      refreshingOwnership = false;
    }
  }

  const ownedVolumeNumbers = $derived.by<number[]>(() => {
    if (!ownership?.volumes) return [];
    return ownership.volumes.filter((v) => v.status === 'owned').map((v) => v.volume).sort((a, b) => a - b);
  });

  const missingVolumeNumbers = $derived.by<number[]>(() => {
    if (!ownership?.summary?.totalSourceVolumes || !ownership.volumes) return [];
    const ownedSet = new Set(ownership.volumes.filter((v) => v.status === 'owned').map((v) => Math.round(v.volume)));
    const missing: number[] = [];
    for (let i = 1; i <= ownership.summary.totalSourceVolumes; i++) {
      if (!ownedSet.has(i)) missing.push(i);
    }
    return missing;
  });

  function formatVolumeRanges(nums: number[]): string {
    if (!nums.length) return 'None';
    const sorted = [...nums].sort((a, b) => a - b);
    const ranges: string[] = [];
    let start = sorted[0];
    let end = sorted[0];
    for (let i = 1; i < sorted.length; i++) {
      if (sorted[i] === end + 1) {
        end = sorted[i];
      } else {
        ranges.push(start === end ? `v${start}` : `v${start}–${end}`);
        start = end = sorted[i];
      }
    }
    ranges.push(start === end ? `v${start}` : `v${start}–${end}`);
    return ranges.join(', ');
  }

  type VolumeGroup = {
    volume: number | null;
    label: string;
    chapters: Chapter[];
    downloadedCount: number;
    missingCount: number;
    isFullyDownloaded: boolean;
    isPartiallyDownloaded: boolean;
    isBusy: boolean;
    ownershipStatus: OwnershipStatus | null;
    ownedBookName: string | null;
  };
  const volumeGroups = $derived.by<VolumeGroup[]>(() => {
    if (!view.volumes) {
      const downloadedCount = listed.filter((c) => c.downloaded).length;
      const missingCount = listed.filter((c) => !c.downloaded).length;
      return [{
        volume: null,
        label: '',
        chapters: listed,
        downloadedCount,
        missingCount,
        isFullyDownloaded: listed.length > 0 && missingCount === 0,
        isPartiallyDownloaded: downloadedCount > 0 && missingCount > 0,
        isBusy: listed.some((c) => busyChapter[c.id]),
        ownershipStatus: null,
        ownedBookName: null,
      }];
    }
    const map = new Map<number | null, Chapter[]>();
    for (const c of listed) {
      const vol = c.volume !== undefined && c.volume !== null ? c.volume : parseVolume(c.name);
      const list = map.get(vol) ?? [];
      list.push(c);
      map.set(vol, list);
    }
    const entries = Array.from(map.entries());
    entries.sort(([a], [b]) => {
      if (a === null) return 1;
      if (b === null) return -1;
      return view.down ? b - a : a - b;
    });
    return entries.map(([vol, chs]) => {
      const downloadedCount = chs.filter((c) => c.downloaded).length;
      const missingCount = chs.filter((c) => !c.downloaded).length;
      const volOwnership = vol !== null ? ownership?.volumes.find((v) => Math.abs(v.volume - vol) < 0.001) : null;
      return {
        volume: vol,
        label: volumeHeading(vol),
        chapters: chs,
        downloadedCount,
        missingCount,
        isFullyDownloaded: chs.length > 0 && missingCount === 0,
        isPartiallyDownloaded: downloadedCount > 0 && missingCount > 0,
        isBusy: chs.some((c) => busyChapter[c.id]),
        ownershipStatus: volOwnership?.status ?? null,
        ownedBookName: volOwnership?.bookName ?? null,
      };
    });
  });
  const nothing = $derived(
    !chapters.length
      ? 'No chapters yet.'
      : show === 'unread' && !currentUploader
        ? 'Every chapter is read.'
        : 'No chapter fits the filter.',
  );
  // Reading goes from the bottom of the list up: the next one to read is the last unread.
  const resume = $derived(chapters.findLast((c) => !c.read) ?? null);
  const started = $derived(chapters.some((c) => c.read || c.lastPageRead > 0));

  async function load() {
    loading = !title;
    error = null;
    try {
      const fresh = await get<Title>(`/manga/${id}`);
      keep('title', fresh);
      title = fresh;
      loadOwnership();
      // A title only seen in a list has no details or chapters yet.
      if (!title.initialized || title.chapters.length === 0) refresh();
    } catch (e) {
      error = message(e);
    }
    loading = false;
  }
  load();

  let savingLibrary = $state(false);
  let downloading = $state(false);
  let busyChapter = $state<Record<number, boolean>>({});

  async function refresh() {
    menu = false;
    refreshing = true;
    refreshError = null;
    try {
      title = await send<Title>('POST', `/manga/${id}/refresh`);
      await loadOwnership();
      toast.success('Updated title from source');
    } catch (e) {
      refreshError = message(e);
      toast.error(refreshError);
    }
    refreshing = false;
  }

  let allCategories = $state<Category[]>([]);
  get<{ categories: Category[] }>('/categories')
    .then((answer) => (allCategories = answer.categories))
    .catch(() => {});

  function toggleCategory(category: number) {
    if (!title) return;
    const t = title;
    t.categories = t.categories.includes(category) ? t.categories.filter((c) => c !== category) : [...t.categories, category];
    change(() => send('PUT', `/manga/${id}/categories`, { ids: t.categories }), true);
  }

  let note = $state('');

  async function download(list: Chapter[]) {
    if (!list.length || downloading) return;
    const ownedVols = new Set(ownership?.volumes.filter((v) => v.status === 'owned').map((v) => v.volume) ?? []);
    const downloadingOwned = list.some((c) => {
      const vol = c.volume !== undefined && c.volume !== null ? c.volume : parseVolume(c.name);
      return vol !== null && ownedVols.has(vol);
    });
    if (downloadingOwned && list.length === 1) {
      const ok = await confirm({
        title: 'Already Owned in Komga',
        message: `This release belongs to a volume already present in your canonical Komga library. Acquire anyway?`,
        confirmText: 'Acquire anyway',
        danger: false,
      });
      if (!ok) return;
    }
    downloading = true;
    for (const c of list) busyChapter[c.id] = true;
    try {
      const answer = await send<{ queued: number }>('POST', '/downloads', { chapters: list.map((c) => c.id) });
      const queuedCount = answer.queued;
      const text = queuedCount ? `${queuedCount} ${queuedCount === 1 ? 'chapter' : 'chapters'} queued for download.` : 'Already saved or queued.';
      note = text;
      toast.info(text, { action: { label: 'See downloads', run: () => (location.hash = '#/downloads') } });
    } catch (e) {
      refreshError = message(e);
      toast.error(refreshError);
    } finally {
      downloading = false;
      for (const c of list) busyChapter[c.id] = false;
    }
  }

  async function downloadVolume(grp: VolumeGroup) {
    if (downloading) return;
    if (grp.ownershipStatus === 'owned') {
      const ok = await confirm({
        title: 'Already Owned in Komga',
        message: `${grp.label} is already present in your canonical Komga library${grp.ownedBookName ? ` ("${grp.ownedBookName}")` : ''}. Are you sure you want to acquire it again?`,
        confirmText: 'Acquire anyway',
        danger: false,
      });
      if (!ok) return;
    }
    const missing = grp.chapters.filter((c) => !c.downloaded && !busyChapter[c.id]);
    if (!missing.length) return;
    for (const c of missing) busyChapter[c.id] = true;
    try {
      const answer = await send<{ queued: number }>('POST', '/downloads', { chapters: missing.map((c) => c.id) });
      const queuedCount = answer.queued;
      const volName = grp.volume !== null ? `Volume ${grp.volume}` : 'Chapters';
      if (queuedCount === missing.length) {
        const text = `Queued ${volName} (${queuedCount} ${queuedCount === 1 ? 'chapter' : 'chapters'}) for download.`;
        note = text;
        toast.info(text, { action: { label: 'See downloads', run: () => (location.hash = '#/downloads') } });
      } else if (queuedCount > 0) {
        const text = `Queued ${queuedCount} of ${missing.length} chapters for ${volName}.`;
        note = text;
        toast.info(text, { action: { label: 'See downloads', run: () => (location.hash = '#/downloads') } });
      } else {
        const text = `All chapters for ${volName} already saved or queued.`;
        note = text;
        toast.info(text);
      }
    } catch (e) {
      refreshError = message(e);
      toast.error(`Failed to queue ${grp.label}: ${refreshError}`);
    } finally {
      for (const c of missing) busyChapter[c.id] = false;
    }
  }

  async function deleteFile(chapter: Chapter) {
    const ok = await confirm({
      title: 'Delete Download?',
      message: `Delete the saved file for ${chapter.name}? You will need to download it again to read offline.`,
      confirmText: 'Delete',
      danger: true,
    });
    if (!ok) return;
    busyChapter[chapter.id] = true;
    chapter.downloaded = false;
    try {
      await change(() => send('DELETE', `/chapters/${chapter.id}/download`));
      toast.info(`Deleted saved file for ${chapter.name}`);
    } finally {
      busyChapter[chapter.id] = false;
    }
  }

  function toggleAuto() {
    if (!title) return;
    const t = title;
    t.autoDownload = !t.autoDownload;
    change(() => send('PUT', `/manga/${id}/auto-download`, { on: t.autoDownload }), true);
  }

  async function change(work: () => Promise<unknown>, keepMenu = false) {
    menu = keepMenu && menu;
    opened = null;
    refreshError = null;
    try {
      await work();
    } catch (e) {
      refreshError = message(e);
      toast.error(refreshError);
      title = await get<Title>(`/manga/${id}`).catch(() => title);
    }
  }

  async function toggleLibrary() {
    if (!title || savingLibrary) return;
    savingLibrary = true;
    const nextState = !title.inLibrary;
    title.inLibrary = nextState;
    try {
      await send('PUT', `/manga/${id}/library`, { inLibrary: nextState });
      toast.success(nextState ? `Added "${title.title}" to library` : `Removed "${title.title}" from library`);
    } catch (e) {
      title.inLibrary = !nextState;
      refreshError = message(e);
      toast.error(refreshError);
    } finally {
      savingLibrary = false;
    }
  }

  function mark(list: Chapter[], read: boolean) {
    for (const chapter of list) {
      chapter.read = read;
      chapter.lastPageRead = 0;
    }
    change(() => send('PUT', `/manga/${id}/chapters`, { ids: list.map((c) => c.id), read }));
  }

  function bookmark(chapter: Chapter) {
    chapter.bookmark = !chapter.bookmark;
    change(() => send('PUT', `/manga/${id}/chapters`, { ids: [chapter.id], bookmark: chapter.bookmark }));
  }

  function setView(patch: Partial<View>) {
    setPref('title.view', { ...view, ...patch }).catch((e) => (refreshError = message(e)));
  }

  /** "Weekly", from the days a title usually leaves between two chapters. */
  function pace(days: number): string {
    return days === 1 ? 'Daily' : days === 7 ? 'Weekly' : days >= 28 ? 'Monthly or slower' : `Every ${days} days`;
  }

  function about(chapter: Chapter): string {
    const when = chapter.dateUpload > 0 ? ago(chapter.dateUpload / 1000) : chapter.fetchedAt > 0 ? `found ${ago(chapter.fetchedAt)}` : '';
    const where = chapter.read
      ? 'read'
      : chapter.lastPageRead > 0
        ? `page ${chapter.lastPageRead + 1}${chapter.pageCount ? ` of ${chapter.pageCount}` : ''}`
        : chapter.pageCount
          ? `${chapter.pageCount} pages`
          : '';
    return [when, chapter.scanlator, where, chapter.downloaded ? 'saved' : ''].filter(Boolean).join(' · ');
  }

  function isNew(chapter: Chapter): boolean {
    return !chapter.read && chapter.lastPageRead === 0 && !!title?.addedAt && chapter.fetchedAt > title.addedAt && Date.now() / 1000 - chapter.fetchedAt < WEEK;
  }
</script>

<!-- Menus close when clicking outside or pressing Escape. -->
<svelte:window
  onclick={(e) => {
    const el = e.target as Element;
    if (opened !== null && !el.closest?.('.own')) opened = null;
    if (menu && !el.closest?.('.menu')) menu = false;
  }}
  onkeydown={(e) => {
    if (e.key === 'Escape') {
      opened = null;
      menu = false;
    }
  }}
/>

<div class="page">
  <Notice {loading} {error} retry={load} />

  {#if title}
    <div class="bar">
      <a class="btn icon lift" href="#/" aria-label="Back" onclick={(e) => (e.preventDefault(), back())}>
        <Icon name="back" stroke={3} />
      </a>
      <div class="tools">
        {#if title.webUrl}
          <a
            class="btn lift"
            href={title.webUrl}
            target="_blank"
            rel="noreferrer noopener"
            aria-label="Open on the source site"
            title="Open in WebView / Website"
          >
            <Icon name="globe" size={18} stroke={3} />
            <span class="site-label">Website</span>
          </a>
        {/if}
        <button
          class="btn lift spot-on"
          aria-pressed={title.inLibrary}
          disabled={savingLibrary}
          onclick={toggleLibrary}
        >
          {#if savingLibrary}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="bookmark" fill={title.inLibrary} />
          {/if}
          {title.inLibrary ? 'In library' : 'Add to library'}
        </button>
        <div class="menu">
          <button class="btn icon lift" aria-label="More actions" aria-expanded={menu} onclick={() => (menu = !menu)}><Icon name="more" fill /></button>
          {#if menu}
            <div class="panel sheet">
              {#if title.webUrl}
                <a class="btn" href={title.webUrl} target="_blank" rel="noreferrer noopener">
                  <Icon name="globe" size={18} stroke={3} />Open in WebView / Website
                </a>
              {/if}
              <button class="btn" onclick={refresh}><Icon name="refresh" size={18} />Refresh from the source</button>
              <button class="btn" onclick={() => mark(chapters, true)}><Icon name="check" size={18} />Mark all read</button>
              <button class="btn" onclick={() => mark(chapters, false)}>Mark all unread</button>
              <a class="btn" href="#/search?q={encodeURIComponent(title.title)}&for={id}"><Icon name="arrow" size={18} />Move to another source</a>
              {#if title.inLibrary}
                <button class="btn" aria-pressed={title.autoDownload} onclick={toggleAuto}>
                  {#if title.autoDownload}<Icon name="check" size={18} stroke={3} />{/if}Download new chapters
                </button>
              {/if}
              {#if title.inLibrary && allCategories.length}
                <div class="cats" role="group" aria-label="Categories">
                  <span>Categories</span>
                  {#each allCategories as category (category.id)}
                    {@const inIt = title.categories.includes(category.id)}
                    <button class="btn" aria-pressed={inIt} onclick={() => toggleCategory(category.id)}>
                      {#if inIt}<Icon name="check" size={18} stroke={3} />{/if}{category.name}
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
        </div>
      </div>
    </div>

    <div class="columns">
      <div class="about">
        <div class="hero">
          <div class="art"><Cover src={title.cover} title={title.title} tilt /></div>
          <div class="facts">
            <div class="tools">
              <span class="tag ink">{STATUS[title.status] ?? 'Unknown'}</span>
              {#if title.sourceLang}<span class="tag">{langName(title.sourceLang)}</span>{/if}
              {#if title.releaseDays && title.status !== 2 && title.status !== 4}
                <span class="tag" title="How often it gets a chapter, going by the source's dates">{pace(title.releaseDays)}</span>
              {/if}
            </div>
            <h1 class="display">{title.title}</h1>
            <strong>{title.sourceName ?? 'Its extension is not installed'}</strong>
            {#if title.author || title.artist}
              <span>{[...new Set([title.author, title.artist].filter(Boolean))].join(' · ')}</span>
            {/if}
          </div>
        </div>

        <div class="stats">
          <div><span class="display">{chapters.length}</span><span>Chapters</span></div>
          <div class:hot={unread > 0}><span class="display">{unread}</span><span>Unread</span></div>
          <div><span class="display">{saved}</span><span>Saved</span></div>
        </div>

        {#if ownership}
          <div class="ownership-summary" class:unavailable={!ownership.komgaConnected} class:has-err={!!ownership.error}>
            <div class="ownership-top">
              <div class="ownership-badge-title">
                <Icon name="book" size={16} />
                <span class="ownership-heading">KOMGA OWNED</span>
              </div>
              <div class="ownership-actions">
                <button
                  class="btn icon mini"
                  title="Refresh ownership from Komga"
                  disabled={refreshingOwnership}
                  onclick={refreshOwnership}
                  aria-label="Refresh Komga ownership"
                >
                  {#if refreshingOwnership}
                    <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
                  {:else}
                    <Icon name="refresh" size={14} />
                  {/if}
                </button>
                {#if ownership.komgaConnected && ownership.seriesId}
                  <button class="btn mini" onclick={() => (showOwnershipDetails = !showOwnershipDetails)}>
                    {showOwnershipDetails ? 'Hide details' : 'Details'}
                  </button>
                {/if}
              </div>
            </div>

            <div class="ownership-body">
              {#if !ownership.komgaConnected}
                <div class="ownership-line">
                  <span class="tag warn">Unavailable</span>
                  <span class="ownership-expl">{ownership.error ?? 'Komga is offline or unreachable'}</span>
                </div>
              {:else if ownership.error}
                <div class="ownership-line">
                  <span class="tag error">Unknown</span>
                  <span class="ownership-expl">{ownership.error}</span>
                </div>
              {:else if ownership.summary.ownedVolumesCount === 0 && ownership.summary.ownedChaptersCount === 0}
                <div class="ownership-line">
                  <span class="ownership-metric">None owned in Komga</span>
                  {#if ownership.summary.explanation}
                    <span class="ownership-expl">{ownership.summary.explanation}</span>
                  {/if}
                </div>
              {:else}
                <div class="ownership-metrics-row">
                  {#if ownership.summary.ownedVolumesCount > 0}
                    <div class="ownership-metric-col">
                      <span class="ownership-metric">
                        VOLUMES <strong>{ownership.summary.ownedVolumesCount}{#if ownership.summary.totalSourceVolumes} / {ownership.summary.totalSourceVolumes}{:else if ownership.summary.authoritativeTotalVolumes} / {ownership.summary.authoritativeTotalVolumes}{/if}</strong>
                      </span>
                      {#if ownership.summary.explanation}
                        <span class="ownership-expl">{ownership.summary.explanation}</span>
                      {/if}
                    </div>
                  {/if}
                  {#if ownership.summary.ownedChaptersCount > 0}
                    <div class="ownership-metric-col">
                      <span class="ownership-metric">
                        CHAPTERS <strong>{ownership.summary.ownedChaptersCount}{#if ownership.summary.totalSourceChapters} / {ownership.summary.totalSourceChapters}{:else if ownership.summary.authoritativeTotalChapters} / {ownership.summary.authoritativeTotalChapters}{/if}</strong>
                      </span>
                    </div>
                  {/if}
                </div>
              {/if}
            </div>

            {#if showOwnershipDetails && ownership.komgaConnected}
              <div class="ownership-details-box">
                {#if ownership.seriesName}
                  <div class="detail-row">
                    <span class="detail-k">Matched Series:</span>
                    <span class="detail-v"><strong>{ownership.seriesName}</strong> <code class="detail-id">{ownership.seriesId}</code></span>
                  </div>
                {/if}
                {#if ownership.libraryName || ownership.libraryId}
                  <div class="detail-row">
                    <span class="detail-k">Komga Library:</span>
                    <span class="detail-v">{ownership.libraryName ?? ownership.libraryId}</span>
                  </div>
                {/if}
                {#if ownedVolumeNumbers.length > 0}
                  <div class="detail-row">
                    <span class="detail-k">Owned Volumes:</span>
                    <span class="detail-v">{formatVolumeRanges(ownedVolumeNumbers)}</span>
                  </div>
                {/if}
                {#if missingVolumeNumbers.length > 0}
                  <div class="detail-row">
                    <span class="detail-k">Missing Volumes:</span>
                    <span class="detail-v">{formatVolumeRanges(missingVolumeNumbers)}</span>
                  </div>
                {/if}
                <div class="detail-row">
                  <span class="detail-k">Source Releases:</span>
                  <span class="detail-v capitalize">{ownership.summary.sourceReleaseType} ({chapters.length} releases)</span>
                </div>
                {#if ownership.lastRefreshed}
                  <div class="detail-row">
                    <span class="detail-k">Last Checked:</span>
                    <span class="detail-v">{ago(ownership.lastRefreshed)}</span>
                  </div>
                {/if}
              </div>
            {/if}
          </div>
        {/if}

        {#if title.description}
          <p class="text" class:clamp={!wholeText}>{title.description}</p>
          {#if title.description.length > 280}
            <button class="btn more" aria-expanded={wholeText} onclick={() => (wholeText = !wholeText)}>{wholeText ? 'Less' : 'More'}</button>
          {/if}
        {/if}
        {#if title.genres.length}
          <div class="tools genres">
            {#each title.genres as genre}<span class="tag">{genre}</span>{/each}
          </div>
        {/if}
        <Tracking {id} title={title.title} />
      </div>

      <div class="chapters">
        <div class="section-head">
          <h2 class="display">CHAPTERS</h2>
          <div class="tools">
            <button class="btn" aria-expanded={filters} onclick={() => (filters = !filters)}>
              <Icon name="filter" size={18} />Sort &amp; filter{filtering ? ' · on' : ''}
            </button>
            <button
              class="btn icon"
              aria-label="Download the unread chapters"
              disabled={downloading || !filteredUnread.length}
              onclick={() => download(filteredUnread)}
            >
              {#if downloading}
                <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
              {:else}
                <Icon name="download" />
              {/if}
            </button>
            <button
              class="btn icon"
              aria-label="Refresh from the source"
              disabled={refreshing}
              onclick={refresh}
            >
              {#if refreshing}
                <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
              {:else}
                <Icon name="refresh" />
              {/if}
            </button>
          </div>
        </div>

        {#if hasMultipleUploaders}
          <div class="uploader-filter" role="group" aria-label="Filter by uploader">
            <span class="uploader-label">Uploader</span>
            {#if uploaderChoices.length <= 4}
              <div class="pills">
                {#each uploaderChoices as choice (choice.value)}
                  <button
                    class="btn pill"
                    class:spot={currentUploader === choice.value}
                    aria-pressed={currentUploader === choice.value}
                    onclick={() => setUploader(choice.value)}
                  >
                    {choice.label}
                  </button>
                {/each}
              </div>
            {:else}
              <label class="sorter">
                <span class="sr">Filter by uploader</span>
                <select class="btn" value={currentUploader} onchange={(e) => setUploader(e.currentTarget.value)}>
                  {#each uploaderChoices as choice (choice.value)}
                    <option value={choice.value}>{choice.label}</option>
                  {/each}
                </select>
              </label>
            {/if}
          </div>
        {/if}

        {#if filters}
          <div class="tools" role="group" aria-label="Sort and filter">
            <label class="sorter">
              <span class="sr">Sort by</span>
              <select class="btn" value={view.sort} onchange={(e) => setView({ sort: e.currentTarget.value as Sort })}>
                {#each SORTS as [value, label] (value)}<option {value}>{label}</option>{/each}
              </select>
            </label>
            <button class="btn icon" aria-label={view.down ? 'Sorted downwards. Sort upwards' : 'Sorted upwards. Sort downwards'} onclick={() => setView({ down: !view.down })}>
              <Icon name={view.down ? 'down' : 'up'} />
            </button>
            <label class="sorter">
              <span class="sr">Show</span>
              <select class="btn" bind:value={show}>
                {#each SHOWS as [value, label] (value)}<option {value}>{label}</option>{/each}
              </select>
            </label>
            {#if hasVolumes}
              <button
                class="btn pill"
                class:spot={view.volumes}
                aria-pressed={view.volumes}
                onclick={() => setView({ volumes: !view.volumes })}
              >
                Volumes
              </button>
            {/if}
          </div>
        {/if}

        <Notice loading={refreshing} error={refreshError} retry={refresh} />
        {#if note}<p role="status"><strong>{note}</strong> <a class="link" href="#/downloads">See downloads</a></p>{/if}

        {#each volumeGroups as grp (grp.volume ?? 'unassigned')}
          {#if grp.label}
            <div class="volume-header">
              <div class="volume-info">
                <h3 class="display">{grp.label}</h3>
                <span class="tag">{grp.chapters.length} {grp.chapters.length === 1 ? 'chapter' : 'chapters'}</span>
                {#if grp.ownershipStatus === 'owned'}
                  <span class="tag spot" title="Present in canonical Komga library"><Icon name="check" size={14} /> Owned in Komga</span>
                {:else if grp.ownershipStatus === 'acquiring'}
                  <span class="tag" title="In acquisition pipeline"><span class="spin"><Icon name="spinner" size={14} stroke={3} /></span> Acquiring</span>
                {/if}
                {#if grp.isFullyDownloaded}
                  <span class="tag spot">Downloaded</span>
                {:else if grp.isPartiallyDownloaded}
                  <span class="tag">{grp.downloadedCount}/{grp.chapters.length} saved</span>
                {/if}
              </div>
              {#if grp.volume !== null}
                <div class="volume-actions">
                  {#if grp.ownershipStatus === 'owned'}
                    <button
                      class="btn pill"
                      aria-label="Re-acquire {grp.label}"
                      disabled={downloading || grp.isBusy || !grp.chapters.length}
                      onclick={() => downloadVolume(grp)}
                      title="Already in Komga library. Click to re-acquire"
                    >
                      {#if grp.isBusy}
                        <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
                      {:else}
                        <Icon name="refresh" size={14} />
                      {/if}
                      Re-acquire
                    </button>
                  {:else if grp.isFullyDownloaded}
                    <span class="tag spot"><Icon name="check" size={14} /> Complete</span>
                  {:else if grp.isPartiallyDownloaded}
                    <button
                      class="btn pill"
                      aria-label="Download missing chapters for {grp.label}"
                      disabled={downloading || grp.isBusy || grp.missingCount === 0}
                      onclick={() => downloadVolume(grp)}
                    >
                      {#if grp.isBusy}
                        <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
                      {:else}
                        <Icon name="download" size={14} />
                      {/if}
                      Missing ({grp.missingCount})
                    </button>
                  {:else}
                    <button
                      class="btn pill spot"
                      aria-label="Download {grp.label}"
                      disabled={downloading || grp.isBusy || !grp.chapters.length}
                      onclick={() => downloadVolume(grp)}
                    >
                      {#if grp.isBusy}
                        <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
                      {:else}
                        <Icon name="download" size={14} />
                      {/if}
                      Download Volume
                    </button>
                  {/if}
                </div>
              {/if}
            </div>
          {/if}
          <ul class="rows">
            {#each grp.chapters as chapter}
              <li class:done={chapter.read}>
                <a class="main" href="#/read/{chapter.id}">
                  <strong>
                    {#if chapter.bookmark}<span class="kept"><Icon name="bookmark" size={14} fill /><span class="sr">Bookmarked.</span></span>{/if}
                    {#if (chapter.volume !== undefined && chapter.volume !== null ? chapter.volume : parseVolume(chapter.name)) !== null && !view.volumes}
                      <span class="tag vol">Vol. {chapter.volume ?? parseVolume(chapter.name)}</span>
                    {/if}
                    {chapter.name}
                  </strong>
                  <span>{about(chapter)}</span>
                </a>
                {#if isNew(chapter)}
                  <span class="tag spot">New</span>
                {:else if !chapter.read && chapter.lastPageRead > 0 && chapter.pageCount}
                  <span class="meter" aria-hidden="true"><span style:width="{Math.round(((chapter.lastPageRead + 1) / chapter.pageCount) * 100)}%"></span></span>
                {/if}
                {#if chapter.downloaded}
                  <button
                    class="btn icon"
                    aria-label="Delete the saved file of {chapter.name}"
                    disabled={busyChapter[chapter.id]}
                    onclick={() => deleteFile(chapter)}
                  >
                    {#if busyChapter[chapter.id]}
                      <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
                    {:else}
                      <Icon name="trash" />
                    {/if}
                  </button>
                {:else}
                  <button
                    class="btn icon"
                    aria-label="Download {chapter.name}"
                    disabled={busyChapter[chapter.id]}
                    onclick={() => download([chapter])}
                  >
                    {#if busyChapter[chapter.id]}
                      <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
                    {:else}
                      <Icon name="download" />
                    {/if}
                  </button>
                {/if}
                <!-- What is seldom wanted waits behind one button, so a row keeps to two. -->
                <div class="menu own">
                  <button class="btn icon" aria-label="More for {chapter.name}" aria-expanded={opened === chapter.id} onclick={() => (opened = opened === chapter.id ? null : chapter.id)}>
                    <Icon name="more" fill />
                  </button>
                  {#if opened === chapter.id}
                    <div class="panel sheet">
                      {#if title.webUrl}
                        <a class="btn" href={title.webUrl} target="_blank" rel="noreferrer noopener">
                          <Icon name="globe" size={18} stroke={3} />Open on site
                        </a>
                      {/if}
                      <button class="btn" onclick={() => mark([chapter], !chapter.read)}>
                        <Icon name="check" size={18} stroke={3} />{chapter.read ? 'Mark unread' : 'Mark read'}
                      </button>
                      <button class="btn" onclick={() => bookmark(chapter)}>
                        <Icon name="bookmark" size={18} fill={chapter.bookmark} />{chapter.bookmark ? 'Take the bookmark off' : 'Bookmark'}
                      </button>
                    </div>
                  {/if}
                </div>
              </li>
            {/each}
          </ul>
        {/each}
        {#if !refreshing && listed.length === 0}
          <Notice>{nothing}</Notice>
        {/if}
      </div>
    </div>

    <div class="dock">
      {#if resume}
        <a class="btn big spot" href="#/read/{resume.id}">
          {started ? 'Resume' : 'Start'} {chapterShort(resume)}<Icon name="arrow" size={18} stroke={3} />
        </a>
      {:else if chapters.length}
        <a class="btn big" href="#/read/{chapters[chapters.length - 1].id}">Read again<Icon name="arrow" size={18} stroke={3} /></a>
      {/if}
      {#if title.webUrl}
        <a class="btn big icon" href={title.webUrl} target="_blank" rel="noreferrer noopener" aria-label="Open on the source's site">
          <Icon name="globe" />
        </a>
      {/if}
    </div>
  {/if}
</div>

<style>
  .page {
    gap: 20px;
    padding-bottom: calc(120px + var(--safe-bottom));
  }

  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  /* With the menu along the bottom this screen stands alone, and its bar is the top of it: always there, for the way back. */
  :global(:root[data-nav='tabs']) .page {
    padding-top: 0;
  }

  :global(:root[data-nav='tabs']) .bar {
    position: sticky;
    top: var(--safe-top);
    z-index: 5;
    margin: 0 calc(-1 * var(--gutter));
    padding: 10px var(--gutter);
    border-bottom: 3px solid var(--ink);
    background: var(--paper);
  }

  .menu {
    position: relative;
  }

  .sheet {
    position: absolute;
    top: 52px;
    right: 0;
    z-index: 5;
    display: flex;
    flex-direction: column;
    min-width: 240px;
    max-height: 70vh;
    overflow-y: auto;
  }

  .sheet .btn {
    justify-content: flex-start;
    border-width: 0 0 2px;
  }

  .sheet .btn:last-child {
    border-bottom: 0;
  }

  .link {
    text-decoration: underline;
  }

  .cats {
    display: flex;
    flex-direction: column;
  }

  .cats span {
    padding: 10px 14px 6px;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .columns {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 28px 48px;
  }

  @media (min-width: 960px) {
    .columns {
      grid-template-columns: minmax(0, 5fr) minmax(0, 7fr);
      align-items: start;
    }
  }

  .about {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }

  .hero {
    display: flex;
    gap: 22px;
    padding: 6px 4px 0;
  }

  .art {
    flex: 0 0 clamp(110px, 30%, 170px);
  }

  .facts {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }

  .facts h1 {
    font-size: clamp(24px, 4.4vw, 40px);
    overflow-wrap: anywhere;
  }

  .facts strong,
  .facts > span {
    font-size: 13px;
    font-weight: 700;
  }

  .stats {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    border: 3px solid var(--ink);
    background: var(--surface);
  }

  .stats div {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 10px 4px;
    border-right: 3px solid var(--ink);
  }

  .stats div:last-child {
    border-right: 0;
  }

  .stats .hot {
    background: var(--spot);
    color: var(--on-spot);
  }

  .stats .display {
    font-size: 22px;
    line-height: 1.15;
  }

  .stats span:last-child {
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  .ownership-summary {
    border: 3px solid var(--ink);
    background: var(--surface);
    padding: 10px 14px;
    margin-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    box-shadow: var(--sh-4);
  }

  .ownership-summary.unavailable {
    background: var(--paper-warm, var(--surface));
  }

  .ownership-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .ownership-badge-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink);
  }

  .ownership-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .btn.mini {
    padding: 2px 8px;
    min-height: 26px;
    font-size: 11px;
    font-weight: 800;
    border-width: 2px;
  }

  .ownership-body {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 12px;
  }

  .ownership-metrics-row {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 16px;
  }

  .ownership-metric-col {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .ownership-metric {
    font-size: 14px;
    font-weight: 800;
    letter-spacing: 0.05em;
  }

  .ownership-metric strong {
    font-size: 18px;
    font-family: var(--display, inherit);
  }

  .ownership-expl {
    font-size: 12px;
    font-weight: 700;
    opacity: 0.75;
  }

  .ownership-details-box {
    margin-top: 6px;
    padding-top: 8px;
    border-top: 2px dashed var(--ink);
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }

  .detail-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .detail-k {
    font-weight: 800;
    min-width: 110px;
    opacity: 0.7;
  }

  .detail-v {
    font-weight: 700;
  }

  .detail-id {
    font-size: 11px;
    opacity: 0.75;
    background: var(--paper, #f5f5f5);
    padding: 1px 4px;
  }

  .capitalize {
    text-transform: capitalize;
  }

  .text {
    font-size: 14px;
    line-height: 1.5;
    white-space: pre-line;
    overflow-wrap: anywhere;
  }

  .clamp {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 5;
    line-clamp: 5;
  }

  .more {
    align-self: flex-start;
  }

  .genres .tag {
    letter-spacing: 0.06em;
  }

  .chapters {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
  }

  .chapters h2 {
    font-size: 20px;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .done .main {
    color: var(--muted);
  }

  .done .main strong {
    font-weight: 700;
  }

  .meter {
    flex: 0 0 56px;
  }

  .sorter select {
    max-width: 220px;
  }

  .uploader-filter {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
    margin-top: 14px;
    padding-bottom: 2px;
  }

  .uploader-label {
    font-size: 13px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
  }

  .uploader-filter .pills {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .own .sheet {
    min-width: 200px;
  }

  .kept {
    display: inline-flex;
    margin-right: 4px;
    vertical-align: -1px;
  }

  .dock {
    position: fixed;
    right: 0;
    bottom: var(--tabs);
    left: 0;
    z-index: 4;
    display: flex;
    justify-content: center;
    gap: 14px;
    padding: 14px max(var(--gutter), var(--safe-right)) calc(14px + var(--safe-bottom)) max(var(--gutter), var(--safe-left));
    border-top: 3px solid var(--ink);
    background: var(--paper);
  }

  .dock .btn:first-child {
    flex: 0 1 420px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .volume-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 12px;
    margin-top: 16px;
    background: var(--surface);
    border: 3px solid var(--ink);
    box-shadow: 2px 2px 0 var(--ink);
  }

  .volume-info {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .volume-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .volume-header h3 {
    margin: 0;
    font-size: 14px;
    letter-spacing: 0.08em;
  }

  .tag.vol {
    display: inline-block;
    margin-right: 6px;
    padding: 1px 6px;
    font-size: 11px;
    font-weight: 900;
    background: var(--surface);
    border: 1.5px solid var(--ink);
  }

  @media (max-width: 640px) {
    .site-label {
      display: none;
    }
  }
</style>
