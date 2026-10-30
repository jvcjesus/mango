<script lang="ts">
  import { get, message, send, type PipelineEvent, type Queue, type Queued } from '../lib/api';
  import { confirm } from '../lib/confirm.svelte';
  import Icon from '../lib/Icon.svelte';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { ago, formatBytes } from '../lib/util';
  import { assignmentFor, unambiguousSuggestions, type ImportBook } from '../lib/importReview';
  import { getStagingSummary } from '../lib/volumeStatus';

  let importBooks = $state<ImportBook[]>([]);
  let importSeries = $state<Record<number, string>>({});
  let importNumbers = $state<Record<number, string>>({});
  let importKinds = $state<Record<number, string>>({});
  let importEditions = $state<Record<number, string>>({});

  async function assignBook(book: ImportBook, ignore = false) {
    try {
      await send(
        'POST',
        `/imports/${book.id}/assignment`,
        assignmentFor(
          book,
          {
            series: importSeries[book.id],
            kind: importKinds[book.id],
            number: importNumbers[book.id],
            edition: importEditions[book.id],
          },
          ignore
        )
      );
      await load(true);
    } catch (e) {
      toast.error(message(e));
    }
  }

  let data = $state<Queue | null>(null);
  let events = $state<PipelineEvent[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let togglingPause = $state(false);
  let clearing = $state(false);
  let cleaningStaging = $state(false);
  let reordering = $state(false);
  let busyItems = $state<Record<number, boolean>>({});
  let expandedCollections = $state<Record<string, boolean>>({});
  let showHistory = $state(false);

  type CollectionGroup = {
    collectionId: string;
    collectionName: string;
    title: string;
    mangaId: number;
    items: Queued[];
  };

  const queue = $derived(data?.queue ?? []);

  // 1. Needs Attention items
  const booksNeedingReview = $derived(importBooks.filter((b) => b.state === 'needs_review'));
  const attentionQueue = $derived(
    queue.filter(
      (q) =>
        q.state === 'failed' ||
        q.isStalled ||
        q.state === 'metadata_failed' ||
        q.state === 'metadata_review' ||
        q.metadataHandoff?.state === 'failed' ||
        q.metadataHandoff?.state === 'needs_review' ||
        q.metadataHandoff?.state === 'rejected' ||
        q.metadataHandoff?.state === 'unavailable' ||
        (q.canRetryKomga && !q.metadataHandoff)
    )
  );
  const totalAttentionCount = $derived(booksNeedingReview.length + attentionQueue.length);

  // 2. Active acquisitions (genuinely in flight or legitimately queued, excluding completed and attention items)
  const activeQueue = $derived(
    queue.filter(
      (q) =>
        q.state !== 'completed' &&
        q.stage !== 'complete' &&
        q.publishedPath == null &&
        !attentionQueue.includes(q)
    )
  );

  // 3. Staging cleanup summary
  const stagingSummary = $derived(getStagingSummary(importBooks));

  // 4. Completed History
  const completedQueue = $derived(
    queue.filter(
      (q) =>
        (q.state === 'completed' || q.stage === 'complete' || (q.publishedPath != null && (q.metadataHandoff == null || q.metadataHandoff.state === 'succeeded'))) &&
        !attentionQueue.includes(q)
    )
  );
  const completedBooks = $derived(
    importBooks.filter((b) => b.state === 'completed' || b.state === 'ignored')
  );
  const totalCompletedCount = $derived(
    events.length + completedQueue.length + completedBooks.length
  );

  // Movable waiting items
  const movable = $derived(
    activeQueue.filter(
      (q) =>
        q.state !== 'downloading' &&
        q.stage !== 'downloading' &&
        q.state !== 'verifying' &&
        q.stage !== 'verifying'
    )
  );

  const groupedActiveQueue = $derived.by(() => {
    const collectionsMap = new Map<string, CollectionGroup>();
    const standalone: Queued[] = [];

    for (const entry of activeQueue) {
      if (entry.collectionId) {
        let group = collectionsMap.get(entry.collectionId);
        if (!group) {
          group = {
            collectionId: entry.collectionId,
            collectionName: entry.collectionName ?? entry.title,
            title: entry.title,
            mangaId: entry.mangaId,
            items: [],
          };
          collectionsMap.set(entry.collectionId, group);
        }
        group.items.push(entry);
      } else {
        standalone.push(entry);
      }
    }

    return {
      collections: Array.from(collectionsMap.values()),
      standalone,
    };
  });

  async function load(isBackground = false) {
    try {
      const [freshQueue, freshEvents] = await Promise.all([
        get<Queue>('/downloads'),
        get<PipelineEvent[]>('/pipeline/events').catch(() => []),
      ]);
      importBooks = await get<ImportBook[]>('/imports').catch(() => []);
      data = freshQueue;
      events = freshEvents;
      if (!isBackground) error = null;
    } catch (e) {
      if (!isBackground || !data) error = message(e);
    }
    loading = false;
  }
  load();

  $effect(() => {
    const timer = setInterval(() => load(true), 2000);
    return () => clearInterval(timer);
  });

  async function act(method: string, path: string, body?: unknown) {
    try {
      await send(method, path, body);
      error = null;
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    }
    await load();
  }

  async function togglePause() {
    if (!data || togglingPause) return;
    togglingPause = true;
    const isPaused = data.paused;
    try {
      await send('POST', isPaused ? '/downloads/resume' : '/downloads/pause');
      toast.info(isPaused ? 'Resumed downloads' : 'Paused downloads');
      error = null;
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      togglingPause = false;
      await load();
    }
  }

  async function move(entry: Queued, by: number) {
    if (reordering) return;
    reordering = true;
    const ids = movable.map((q) => q.chapterId);
    const at = ids.indexOf(entry.chapterId);
    ids.splice(at, 1);
    ids.splice(at + by, 0, entry.chapterId);
    try {
      await send('PUT', '/downloads/order', { ids });
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      reordering = false;
      await load();
    }
  }

  async function clear() {
    const ok = await confirm({
      title: 'Clear Download Queue?',
      message: 'All chapters currently waiting or downloading will be canceled and removed.',
      confirmText: 'Clear queue',
      danger: true,
    });
    if (!ok) return;
    clearing = true;
    try {
      await send('DELETE', '/downloads');
      toast.info('Cleared download queue');
      error = null;
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      clearing = false;
      await load();
    }
  }

  async function cancelDownload(entry: Queued) {
    const ok = await confirm({
      title: 'Cancel Download?',
      message: `Cancel downloading "${entry.chapterName}" of ${entry.title}?`,
      confirmText: 'Cancel download',
      danger: true,
    });
    if (!ok) return;
    busyItems[entry.chapterId] = true;
    try {
      await send('DELETE', `/downloads/${entry.chapterId}`);
      toast.info(`Canceled ${entry.chapterName}`);
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      busyItems[entry.chapterId] = false;
      await load();
    }
  }

  async function cancelCollectionGroup(col: CollectionGroup) {
    const ok = await confirm({
      title: 'Cancel Collection?',
      message: `Cancel all ${col.items.length} queued child volumes of "${col.collectionName}"? Any already completed or published volumes will remain intact.`,
      confirmText: 'Cancel collection',
      danger: true,
    });
    if (!ok) return;
    for (const item of col.items) {
      busyItems[item.chapterId] = true;
    }
    try {
      for (const item of col.items) {
        await send('DELETE', `/downloads/${item.chapterId}`);
      }
      toast.info(`Canceled collection "${col.collectionName}"`);
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      for (const item of col.items) {
        busyItems[item.chapterId] = false;
      }
      await load();
    }
  }

  async function cleanAllCompletedStaging() {
    if (cleaningStaging || !stagingSummary.completedPayloadChapters.length) return;
    cleaningStaging = true;
    const count = stagingSummary.completedPayloadChapters.length;
    try {
      for (const payload of stagingSummary.completedPayloadChapters) {
        await send('POST', `/imports/payload/${payload}/cleanup`);
      }
      toast.success(`Cleaned staging for ${count} completed ${count === 1 ? 'payload' : 'payloads'}`);
      await load();
    } catch (e) {
      toast.error(message(e));
    } finally {
      cleaningStaging = false;
    }
  }

  function describe(entry: Queued): string {
    if (entry.metadataHandoff) return entry.metadataHandoff.message;
    if (entry.statusMessage) return entry.statusMessage;
    if (entry.isStalled) return 'Transfer stalled — waiting for progress';
    if (entry.stage === 'retrying' || entry.transferStatus === 'retrying') {
      const wait = entry.retryAt ? Math.max(0, Math.ceil((entry.retryAt - Date.now() / 1000) / 60)) : 0;
      const att = entry.attempt ?? entry.attempts;
      const maxAtt = entry.maxAttempts ?? 5;
      return `Retrying (Attempt ${att}/${maxAtt})${wait > 0 ? ` in ${wait} min` : ''}`;
    }
    if (entry.state === 'verifying' || entry.stage === 'verifying') return 'Published — waiting for Komga';
    if (entry.backendType === 'torbox' && entry.state === 'downloading') {
      if (entry.bytesDownloaded !== undefined && entry.bytesDownloaded !== null && entry.bytesDownloaded > 0) {
        if (entry.totalBytes) {
          const pct = Math.round((entry.bytesDownloaded / entry.totalBytes) * 100);
          return `Downloading via TorBox: ${formatBytes(entry.bytesDownloaded)} / ${formatBytes(entry.totalBytes)} (${pct}%)`;
        }
        return `Downloading via TorBox: ${formatBytes(entry.bytesDownloaded)}`;
      }
      return 'Downloading via TorBox';
    }
    if (entry.state === 'downloading') {
      if (entry.bytesDownloaded !== undefined && entry.bytesDownloaded !== null && entry.bytesDownloaded > 0) {
        if (entry.totalBytes) {
          const pct = Math.round((entry.bytesDownloaded / entry.totalBytes) * 100);
          return `Downloading: ${formatBytes(entry.bytesDownloaded)} / ${formatBytes(entry.totalBytes)} (${pct}%)`;
        }
        return `Downloading: ${formatBytes(entry.bytesDownloaded)}`;
      }
      return entry.total ? `Downloading, page ${entry.done} of ${entry.total}` : 'Downloading';
    }
    if (entry.state === 'identifying') return 'Identifying release semantics';
    if (entry.state === 'metadata') return 'Querying MangaBaka metadata';
    if (entry.state === 'optimizing') return 'Optimizing image compression';
    if (entry.state === 'validating') return 'Validating archive integrity';
    if (entry.state === 'publishing') return 'Publishing to library';
    if (entry.state === 'komga') return 'Triggering Komga scan';
    if (entry.state === 'importing') return 'Inspection complete · assigned books importing independently';
    if (entry.state === 'needs_review' && entry.error) return entry.error;
    if (entry.state === 'needs_review') return 'Downloaded · inspection complete · book assignments await review';
    if (entry.state === 'failed' && entry.attempts === 0) return `Stopped: ${entry.error ?? 'Source or inspection could not continue'}`;
    if (entry.state === 'failed') return `Gave up after ${entry.attempts} tries: ${entry.error ?? 'unknown error'}`;
    if (entry.error) {
      const wait = entry.retryAt ? Math.max(0, Math.ceil((entry.retryAt - Date.now() / 1000) / 60)) : 0;
      return `Try ${entry.attempts} failed: ${entry.error}. Next try ${wait > 0 ? `in ${wait} min` : 'soon'}`;
    }
    return data?.paused ? 'Waiting, queue paused' : 'Waiting in queue';
  }

  function stageLabel(entry: Queued): string {
    if (entry.isStalled || entry.stage === 'stalled') return 'Stalled';
    if (entry.stage === 'retrying' || entry.transferStatus === 'retrying') {
      const att = entry.attempt ?? entry.attempts;
      const maxAtt = entry.maxAttempts ?? 5;
      return `Retrying (${att}/${maxAtt})`;
    }
    const s = entry.stage ?? entry.state;
    switch (s) {
      case 'torbox_submitting': return 'TorBox Submit';
      case 'torbox_recreating': return 'TorBox Recreating';
      case 'torbox_checking': return 'TorBox Wait';
      case 'torbox_downloading': return 'TorBox Downloading';
      case 'torbox_stalled': return 'TorBox Stalled';
      case 'torbox_selecting': return 'TorBox Select';
      case 'torbox_retrieving': return 'Retrieving';
      case 'downloading': return 'Downloading';
      case 'identifying': return 'Identifying';
      case 'metadata': return 'Metadata';
      case 'metadata_wait': return 'Waiting for KOMF';
      case 'metadata_failed': return 'Metadata retry';
      case 'metadata_review': return 'Metadata Needs Review';
      case 'optimizing': return 'Optimizing';
      case 'validating': return 'Validating';
      case 'publishing': return 'Publishing';
      case 'komga': return 'Komga';
      case 'verifying': return 'Verifying';
      case 'complete':
      case 'completed': return 'Complete';
      case 'failed': return 'Failed';
      default: return 'Queued';
    }
  }
</script>

<div class="page">
  <div class="page-head">
    <div class="heading">
      <h1 class="display">DOWNLOADS</h1>
      <span class="queue-status-text">
        <strong>{activeQueue.length} active</strong>
        {#if totalAttentionCount > 0}
          · <span class="attention-badge">{totalAttentionCount} need attention</span>
        {/if}
        {#if data?.paused}
          · <span class="paused-badge">Paused</span>
        {/if}
      </span>
    </div>
    {#if data}
      <div class="tools">
        {#if data.paused}
          <button class="btn big spot" disabled={togglingPause} onclick={togglePause}>
            {#if togglingPause}
              <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
            {:else}
              <Icon name="play" size={18} fill />
            {/if}
            Resume
          </button>
        {:else}
          <button class="btn big" disabled={togglingPause} onclick={togglePause}>
            {#if togglingPause}
              <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
            {:else}
              <Icon name="pause" size={18} stroke={3} />
            {/if}
            Pause
          </button>
        {/if}
        {#if activeQueue.length}
          <button class="btn" disabled={clearing} onclick={clear}>
            {#if clearing}
              <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
            {:else}
              <Icon name="trash" size={18} />
            {/if}
            Clear active
          </button>
        {/if}
      </div>
    {/if}
  </div>

  <Notice {loading} {error} retry={() => load(false)} />

  <!-- SECTION 1: NEEDS ATTENTION -->
  {#if totalAttentionCount > 0}
    <section class="section-card attention-card" aria-label="Items needing attention">
      <div class="section-header">
        <div class="header-left">
          <span class="status-indicator error"></span>
          <h2 class="display section-title">NEEDS ATTENTION ({totalAttentionCount})</h2>
        </div>
        {#if booksNeedingReview.length && unambiguousSuggestions(importBooks).length}
          <button
            class="btn pill spot action-btn"
            onclick={async () => {
              const books = unambiguousSuggestions(importBooks);
              if (
                await confirm({
                  title: 'Approve book assignments',
                  message: `Import ${books.length} books using the displayed series, edition and unit suggestions? Duplicate alternatives and unknown books stay in review.`,
                  confirmText: 'Assign & Import',
                })
              ) {
                for (const book of books) await assignBook(book);
              }
            }}
          >
            Import numbered suggestions ({unambiguousSuggestions(importBooks).length})
          </button>
        {/if}
      </div>

      <!-- Failed downloads / metadata errors -->
      {#if attentionQueue.length > 0}
        <ul class="rows attention-rows">
          {#each attentionQueue as entry (entry.chapterId)}
            <li class="attention-row">
              <div class="main">
                <div class="row-header">
                  <strong>{entry.title}</strong>
                  <span class="tag danger">{stageLabel(entry)}</span>
                  {#if entry.volume != null}
                    <span class="tag">Vol. {entry.volume}</span>
                  {/if}
                </div>
                <span class="error-desc">{describe(entry)}</span>
              </div>
              <div class="tools">
                {#if entry.metadataHandoff && entry.metadataHandoff.state !== 'succeeded'}
                  <button
                    class="btn pill spot"
                    disabled={busyItems[entry.chapterId] || entry.metadataHandoff.state === 'submitting'}
                    onclick={() => act('POST', `/downloads/${entry.chapterId}/retry-metadata`)}
                  >
                    <Icon name="refresh" size={14} /> Retry metadata
                  </button>
                {/if}
                {#if entry.canRetryKomga && !entry.metadataHandoff}
                  <button
                    class="btn pill spot"
                    disabled={busyItems[entry.chapterId]}
                    onclick={() => act('POST', `/downloads/${entry.chapterId}/retry-komga`)}
                  >
                    <Icon name="refresh" size={14} /> Retry Komga
                  </button>
                {/if}
                {#if entry.state === 'failed'}
                  <button
                    class="btn pill spot"
                    disabled={busyItems[entry.chapterId]}
                    onclick={() => act('POST', `/downloads/${entry.chapterId}/retry`)}
                  >
                    <Icon name="refresh" size={14} /> Try again
                  </button>
                {/if}
                <button
                  class="btn icon"
                  aria-label="Cancel {entry.chapterName}"
                  disabled={busyItems[entry.chapterId]}
                  onclick={() => cancelDownload(entry)}
                  title="Cancel this acquisition"
                >
                  <Icon name="close" size={16} />
                </button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}

      <!-- Inspected books needing review -->
      {#if booksNeedingReview.length > 0}
        <div class="review-books-list">
          <p class="review-intro">Payload inspection discovered books that need series or volume confirmation before importing.</p>
          {#each booksNeedingReview as book (book.id)}
            <div class="import-book-card">
              <div class="book-info">
                <strong>{book.series}</strong>
                <span class="inner-path-tag">{book.innerPath}</span>
              </div>
              <div class="book-inputs-row">
                <label>Series <input aria-label="Assigned series" placeholder={book.series} bind:value={importSeries[book.id]} /></label>
                <label>Unit
                  <select aria-label="Unit kind" bind:value={importKinds[book.id]}>
                    <option value="volume">Volume</option>
                    <option value="chapter">Chapter</option>
                  </select>
                </label>
                <label>Number <input type="number" min="0" step="any" aria-label="Assigned number" placeholder={String(book.suggestedNumber ?? 'Required')} bind:value={importNumbers[book.id]} /></label>
                <label>Edition <input aria-label="Edition label" placeholder={book.suggestedEdition ?? 'Optional'} bind:value={importEditions[book.id]} /></label>
                <div class="review-actions">
                  <button class="btn spot" onclick={() => assignBook(book)}>Assign & Import</button>
                  <button class="btn" onclick={() => assignBook(book, true)}>Ignore</button>
                </div>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </section>
  {/if}

  <!-- SECTION 2: ACTIVE ACQUISITIONS -->
  {#if activeQueue.length > 0}
    <section class="section-card" aria-label="Active acquisitions">
      <div class="section-header">
        <div class="header-left">
          <span class="status-indicator active"></span>
          <h2 class="display section-title">ACTIVE WORK ({activeQueue.length})</h2>
        </div>
      </div>

      <div class="downloads-container">
        <!-- Grouped Collections if any -->
        {#if groupedActiveQueue.collections.length > 0}
          <div class="collections-section">
            {#each groupedActiveQueue.collections as col (col.collectionId)}
              {@const isExpanded = expandedCollections[col.collectionId] ?? true}
              <div class="collection-card">
                <div class="collection-header">
                  <div class="collection-info">
                    <div class="collection-tag-row">
                      <span class="tag spot">COLLECTION</span>
                      <span class="tag">{col.items.length} units</span>
                    </div>
                    <h3 class="display collection-title">{col.title} · {col.collectionName}</h3>
                  </div>
                  <div class="collection-tools">
                    <button
                      class="btn pill icon-text"
                      onclick={() => { expandedCollections[col.collectionId] = !isExpanded; }}
                    >
                      <Icon name={isExpanded ? 'up' : 'down'} size={14} />
                      {isExpanded ? 'Hide units' : `Show units (${col.items.length})`}
                    </button>
                    <button
                      class="btn pill icon-text"
                      onclick={() => cancelCollectionGroup(col)}
                      title="Cancel all queued units in this collection"
                    >
                      <Icon name="close" size={14} /> Cancel collection
                    </button>
                  </div>
                </div>

                {#if isExpanded}
                  <ul class="rows collection-unit-rows">
                    {#each col.items as entry (entry.chapterId)}
                      <li>
                        <div class="main">
                          <div class="row-header">
                            <strong>{entry.chapterName}</strong>
                            <span class="tag spot">{stageLabel(entry)}</span>
                            {#if entry.rawTitle}
                              <span class="tag filename-tag">{entry.rawTitle}</span>
                            {/if}
                          </div>
                          <span>{describe(entry)}</span>
                          {#if entry.state === 'downloading' || entry.stage === 'downloading' || entry.stage === 'torbox_retrieving'}
                            {#if entry.totalBytes && entry.bytesDownloaded}
                              <span class="meter" aria-hidden="true"><span style:width="{Math.min(100, Math.round((entry.bytesDownloaded / entry.totalBytes) * 100))}%"></span></span>
                            {:else if entry.total}
                              <span class="meter" aria-hidden="true"><span style:width="{Math.round((entry.done / entry.total) * 100)}%"></span></span>
                            {/if}
                          {/if}
                        </div>
                        <span class="tools">
                          <button
                            class="btn icon"
                            aria-label="Cancel {entry.chapterName}"
                            disabled={busyItems[entry.chapterId]}
                            onclick={() => cancelDownload(entry)}
                            title="Cancel this child unit only"
                          >
                            {#if busyItems[entry.chapterId]}
                              <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
                            {:else}
                              <Icon name="close" />
                            {/if}
                          </button>
                        </span>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </div>
            {/each}
          </div>
        {/if}

        <!-- Standalone active items -->
        {#if groupedActiveQueue.standalone.length > 0}
          <ul class="rows">
            {#each groupedActiveQueue.standalone as entry (entry.chapterId)}
              {@const at = movable.indexOf(entry)}
              <li>
                <a class="main" href="#/title/{entry.mangaId}">
                  <div class="row-header">
                    <strong>{entry.title}</strong>
                    <span class="tag spot">{stageLabel(entry)}</span>
                    {#if entry.sourceType === 'nyaa'}
                      <span class="tag spot">Nyaa</span>
                    {/if}
                    {#if entry.uploader}
                      <span class="tag">{entry.uploader}</span>
                    {/if}
                    {#if entry.volume != null}
                      <span class="tag">Vol. {entry.volume}</span>
                    {/if}
                  </div>
                  <span>{entry.chapterName} · {describe(entry)}</span>
                  {#if entry.state === 'downloading' || entry.stage === 'downloading' || entry.stage === 'torbox_retrieving'}
                    {#if entry.totalBytes && entry.bytesDownloaded}
                      <span class="meter" aria-hidden="true"><span style:width="{Math.min(100, Math.round((entry.bytesDownloaded / entry.totalBytes) * 100))}%"></span></span>
                    {:else if entry.total}
                      <span class="meter" aria-hidden="true"><span style:width="{Math.round((entry.done / entry.total) * 100)}%"></span></span>
                    {/if}
                  {/if}
                </a>
                <span class="tools">
                  {#if at >= 0}
                    <button class="btn icon" aria-label="Move {entry.chapterName} up" disabled={at === 0 || reordering} onclick={() => move(entry, -1)}>
                      <Icon name="up" />
                    </button>
                    <button class="btn icon" aria-label="Move {entry.chapterName} down" disabled={at === movable.length - 1 || reordering} onclick={() => move(entry, 1)}>
                      <Icon name="down" />
                    </button>
                  {/if}
                  <button
                    class="btn icon"
                    aria-label="Cancel {entry.chapterName} of {entry.title}"
                    disabled={busyItems[entry.chapterId]}
                    onclick={() => cancelDownload(entry)}
                  >
                    {#if busyItems[entry.chapterId]}
                      <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
                    {:else}
                      <Icon name="close" />
                    {/if}
                  </button>
                </span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    </section>
  {/if}

  <!-- SECTION 3: STAGING CLEANUP MAINTENANCE (CONSOLIDATED) -->
  {#if stagingSummary.canClean}
    <div class="staging-maintenance-card">
      <div class="staging-info-group">
        <Icon name="trash" size={18} />
        <div>
          <strong>Staging Cleanup</strong>
          <p class="staging-subtext">
            {stagingSummary.completedPayloadChapters.length} completed staging {stagingSummary.completedPayloadChapters.length === 1 ? 'payload is' : 'payloads are'} safely finished and ready for disk cleanup.
          </p>
        </div>
      </div>
      <button class="btn pill action-btn" disabled={cleaningStaging} onclick={cleanAllCompletedStaging}>
        {#if cleaningStaging}
          <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
        {/if}
        Clean staging ({stagingSummary.completedPayloadChapters.length})
      </button>
    </div>
  {/if}

  <!-- SECTION 4: COMPLETED / HISTORY (COLLAPSED BY DEFAULT) -->
  {#if totalCompletedCount > 0}
    <div class="history-accordion-section">
      <div class="section-header">
        <button
          class="btn pill icon-text history-toggle-btn"
          aria-expanded={showHistory}
          onclick={() => (showHistory = !showHistory)}
        >
          <Icon name={showHistory ? 'up' : 'down'} size={14} />
          Acquisition History ({totalCompletedCount} completed)
        </button>
      </div>

      {#if showHistory}
        <div class="history-content">
          {#if events.length > 0}
            <div class="history-block">
              <h3 class="display sub-title">RECENT CANONICAL PUBLICATIONS</h3>
              <ul class="rows history-rows">
                {#each events as evt (evt.id)}
                  <li class="event-item">
                    <div class="main">
                      <div class="row-header">
                        <strong>{evt.series}</strong>
                        {#if evt.outcome === 'published'}
                          <span class="tag spot">Published</span>
                        {:else if evt.outcome === 'already_published'}
                          <span class="tag">Already in Library</span>
                        {:else if evt.outcome === 'conflict'}
                          <span class="tag error">Conflict</span>
                        {:else if evt.outcome === 'error'}
                          <span class="tag error">Error</span>
                        {/if}
                      </div>
                      <span>
                        {evt.release_name}
                        {#if evt.destination}
                          · <span class="muted" title={evt.destination}>Canonical library</span>
                        {/if}
                        · {ago(evt.timestamp * 1000)}
                      </span>
                      <div class="event-details">
                        {#if evt.optimizer_status && !evt.optimizer_status.startsWith('skipped')}
                          <span class="tag">{evt.optimizer_status}</span>
                        {/if}
                        {#if evt.mangabaka_status && !evt.mangabaka_status.startsWith('skipped')}
                          <span class="tag">{evt.mangabaka_status}</span>
                        {/if}
                        {#if evt.komga_status && !evt.komga_status.startsWith('skipped')}
                          <span class="tag">{evt.komga_status}</span>
                        {/if}
                        {#if evt.error}
                          <span class="muted error-text">{evt.error}</span>
                        {/if}
                      </div>
                    </div>
                  </li>
                {/each}
              </ul>
            </div>
          {/if}

          {#if completedQueue.length > 0}
            <div class="history-block">
              <h3 class="display sub-title">COMPLETED DOWNLOAD JOBS</h3>
              <ul class="rows history-rows">
                {#each completedQueue as q (q.chapterId)}
                  <li class="event-item">
                    <div class="main">
                      <div class="row-header">
                        <strong>{q.title}</strong>
                        <span class="tag spot">Complete</span>
                        {#if q.volume != null}
                          <span class="tag">Vol. {q.volume}</span>
                        {/if}
                      </div>
                      <span class="muted">{q.chapterName} · {q.rawTitle ?? ''}</span>
                    </div>
                  </li>
                {/each}
              </ul>
            </div>
          {/if}
        </div>
      {/if}
    </div>
  {/if}

  <!-- EMPTY STATE -->
  {#if activeQueue.length === 0 && totalAttentionCount === 0 && totalCompletedCount === 0}
    <Notice>
      Nothing is waiting. Acquisitions are triggered from a title's page, from Updates, or by background title rules.
    </Notice>
  {/if}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    gap: 24px;
  }

  .heading {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .queue-status-text {
    font-size: 14px;
    color: var(--muted);
  }

  .attention-badge {
    color: var(--error, #e53935);
    font-weight: 700;
  }

  .paused-badge {
    font-weight: 700;
    color: var(--ink);
  }

  .section-card {
    background: var(--surface);
    border: 2px solid var(--ink);
    border-radius: var(--radius, 6px);
    overflow: hidden;
    box-shadow: 2px 2px 0 var(--ink);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .section-card.attention-card {
    border-color: var(--error, #e53935);
    background: #fff8f8;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }

  .header-left {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .status-indicator {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 1.5px solid var(--ink);
  }

  .status-indicator.error {
    background: var(--error, #e53935);
  }

  .status-indicator.active {
    background: #0284c7;
  }

  .section-title {
    font-size: 16px;
    margin: 0;
  }

  .sub-title {
    font-size: 13px;
    margin: 12px 0 6px;
    color: var(--muted);
  }

  .attention-rows {
    background: var(--surface);
    border: 1px solid var(--ink);
    border-radius: 4px;
  }

  .attention-row {
    padding: 10px 14px;
    border-bottom: 1px solid var(--muted);
  }

  .attention-row:last-child {
    border-bottom: none;
  }

  .error-desc {
    font-size: 13px;
    color: var(--error, #e53935);
    font-weight: 500;
  }

  .review-intro {
    font-size: 13px;
    color: var(--muted);
    margin-bottom: 8px;
  }

  .review-books-list {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .import-book-card {
    background: var(--surface);
    border: 1.5px solid var(--ink);
    border-radius: 4px;
    padding: 12px;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .book-info {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex-wrap: wrap;
  }

  .inner-path-tag {
    font-size: 12px;
    color: var(--muted);
    word-break: break-all;
  }

  .book-inputs-row {
    display: flex;
    align-items: flex-end;
    gap: 8px;
    flex-wrap: wrap;
  }

  .book-inputs-row label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
  }

  .book-inputs-row input,
  .book-inputs-row select {
    padding: 4px 8px;
    border: 1px solid var(--ink);
    border-radius: 4px;
    font-size: 13px;
  }

  .review-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .downloads-container {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .collection-card {
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: var(--radius);
    overflow: hidden;
    box-shadow: 2px 2px 0 var(--ink);
  }

  .collection-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    background: var(--shade);
    border-bottom: 2px solid var(--ink);
    gap: 12px;
    flex-wrap: wrap;
  }

  .collection-info {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .collection-tag-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .collection-title {
    font-size: 14px;
    margin: 0;
    word-break: break-word;
  }

  .collection-tools {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .collection-unit-rows {
    background: var(--paper);
  }

  .collection-unit-rows li {
    padding: 8px 14px;
    border-bottom: 1px solid var(--ink);
  }

  .collection-unit-rows li:last-child {
    border-bottom: none;
  }

  .staging-maintenance-card {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 16px;
    background: var(--surface);
    border: 2px dashed var(--ink);
    border-radius: 6px;
    gap: 16px;
    flex-wrap: wrap;
  }

  .staging-info-group {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .staging-subtext {
    font-size: 12px;
    color: var(--muted);
    margin: 2px 0 0;
  }

  .history-accordion-section {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .history-toggle-btn {
    font-size: 13px;
    font-weight: 700;
  }

  .history-content {
    display: flex;
    flex-direction: column;
    gap: 16px;
    padding: 12px 16px;
    background: var(--surface);
    border: 2px solid var(--ink);
    border-radius: 6px;
  }

  .history-rows li {
    padding: 8px 12px;
    border-bottom: 1px solid var(--muted);
  }

  .history-rows li:last-child {
    border-bottom: none;
  }

  .row-header {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .event-details {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 4px;
    font-size: 0.85rem;
  }

  .meter {
    width: min(320px, 100%);
    margin-top: 4px;
    height: 8px;
  }

  .meter span {
    background: #0284c7;
  }

  .filename-tag {
    font-size: 11px;
    max-width: 260px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }
</style>
