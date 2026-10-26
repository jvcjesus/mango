<script lang="ts">
  import { get, message, send, type PipelineEvent, type Queue, type Queued } from '../lib/api';
  import { confirm } from '../lib/confirm.svelte';
  import Icon from '../lib/Icon.svelte';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { ago, formatBytes } from '../lib/util';

  let data = $state<Queue | null>(null);
  let events = $state<PipelineEvent[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let togglingPause = $state(false);
  let clearing = $state(false);
  let reordering = $state(false);
  let busyItems = $state<Record<number, boolean>>({});
  let expandedCollections = $state<Record<string, boolean>>({});

  type CollectionGroup = {
    collectionId: string;
    collectionName: string;
    title: string;
    mangaId: number;
    items: Queued[];
  };

  const queue = $derived(data?.queue ?? []);
  // Only chapters still waiting can be moved; active or verifying items stay where they are.
  const movable = $derived(
    queue.filter(
      (q) =>
        q.state !== 'downloading' &&
        q.stage !== 'downloading' &&
        q.state !== 'verifying' &&
        q.stage !== 'verifying'
    )
  );

  const groupedQueue = $derived.by(() => {
    const collectionsMap = new Map<string, CollectionGroup>();
    const standalone: Queued[] = [];

    for (const entry of queue) {
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
      data = freshQueue;
      events = freshEvents;
      if (!isBackground) error = null;
    } catch (e) {
      if (!isBackground || !data) error = message(e);
    }
    loading = false;
  }
  load();

  // The queue changes by itself: chapters finish, and updates add new ones. So the
  // screen looks again every two seconds for as long as it is open.
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

  function describe(entry: Queued): string {
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
      case 'torbox_checking': return 'TorBox Wait';
      case 'torbox_downloading': return 'TorBox Downloading';
      case 'torbox_stalled': return 'TorBox Stalled';
      case 'torbox_selecting': return 'TorBox Select';
      case 'torbox_retrieving': return 'Retrieving';
      case 'downloading': return 'Downloading';
      case 'identifying': return 'Identifying';
      case 'metadata': return 'Metadata';
      case 'optimizing': return 'Optimizing';
      case 'validating': return 'Validating';
      case 'publishing': return 'Publishing';
      case 'komga': return 'Komga';
      case 'verifying': return 'Verifying';
      case 'complete': return 'Complete';
      case 'failed': return 'Failed';
      default: return 'Queued';
    }
  }
</script>

<div class="page">
  <div class="page-head">
    <div class="heading">
      <h1 class="display">DOWNLOADS</h1>
      <strong>{queue.length} in the queue{data?.paused ? ', paused' : ''}</strong>
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
        {#if queue.length}
          <button class="btn" disabled={clearing} onclick={clear}>
            {#if clearing}
              <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
            {:else}
              <Icon name="trash" size={18} />
            {/if}
            Clear queue
          </button>
        {/if}
      </div>
    {/if}
  </div>

  <Notice {loading} {error} retry={() => load(false)} />

  {#if data}
    {#if queue.length === 0}
      <Notice>
        Nothing is waiting. Chapters are queued from a title's page, from Updates, or by themselves for titles set to download new chapters.
      </Notice>
    {:else}
      <div class="downloads-container">
        <!-- Grouped Collections if any -->
        {#if groupedQueue.collections.length > 0}
          <div class="collections-section">
            {#each groupedQueue.collections as col (col.collectionId)}
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
                            <span class="tag" class:spot={entry.stage !== 'queued' && entry.stage !== 'failed' && !entry.isStalled} class:error={entry.stage === 'failed' || entry.isStalled}>
                              {stageLabel(entry)}
                            </span>
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
                          {#if entry.canRetryKomga}
                            <button class="btn pill" disabled={busyItems[entry.chapterId]} onclick={() => act('POST', `/downloads/${entry.chapterId}/retry-komga`)}>
                              <Icon name="refresh" size={16} />Retry Komga
                            </button>
                          {/if}
                          {#if entry.state === 'failed'}
                            <button class="btn" disabled={busyItems[entry.chapterId]} onclick={() => act('POST', `/downloads/${entry.chapterId}/retry`)}>
                              <Icon name="refresh" size={18} />Try again
                            </button>
                          {/if}
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

        <!-- Standalone items if any -->
        {#if groupedQueue.standalone.length > 0}
          <ul class="rows">
            {#each groupedQueue.standalone as entry (entry.chapterId)}
              {@const at = movable.indexOf(entry)}
              <li>
                <a class="main" href="#/title/{entry.mangaId}">
                  <div class="row-header">
                    <strong>{entry.title}</strong>
                    <span class="tag" class:spot={entry.stage !== 'queued' && entry.stage !== 'failed' && !entry.isStalled} class:error={entry.stage === 'failed' || entry.isStalled}>
                      {stageLabel(entry)}
                    </span>
                    {#if entry.sourceType === 'nyaa'}
                      <span class="tag spot">Nyaa</span>
                    {/if}
                    {#if entry.uploader}
                      <span class="tag">{entry.uploader}</span>
                    {/if}
                    {#if entry.releaseType === 'volume' || (entry.volume !== undefined && entry.volume !== null)}
                      <span class="tag">Vol. {entry.volume ?? 'Special'}</span>
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
                  {#if entry.canRetryKomga}
                    <button class="btn pill" disabled={busyItems[entry.chapterId]} onclick={() => act('POST', `/downloads/${entry.chapterId}/retry-komga`)}>
                      <Icon name="refresh" size={16} />Retry Komga Scan
                    </button>
                  {/if}
                  {#if entry.state === 'failed'}
                    <button class="btn" disabled={busyItems[entry.chapterId]} onclick={() => act('POST', `/downloads/${entry.chapterId}/retry`)}>
                      <Icon name="refresh" size={18} />Try again
                    </button>
                  {/if}
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
    {/if}

    {#if events.length > 0}
      <div class="acquisitions-section">
        <div class="heading">
          <h2 class="display">RECENT ACQUISITIONS</h2>
          <strong>{events.length} recorded</strong>
        </div>
        <ul class="rows">
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
  {/if}
</div>

<style>
  .heading {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 6px 18px;
  }

  .row-header {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .acquisitions-section {
    margin-top: 36px;
  }

  .event-details {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    margin-top: 4px;
    font-size: 0.85rem;
  }

  .error-text {
    color: var(--error, #e53935);
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    flex-wrap: wrap;
  }

  .meter {
    width: min(320px, 100%);
    margin-top: 6px;
  }

  .meter span {
    background: var(--spot);
  }

  .downloads-container {
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .collections-section {
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
    padding: 12px 16px;
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
    font-size: 15px;
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
    padding: 10px 16px;
    border-bottom: 1px solid var(--ink);
  }

  .collection-unit-rows li:last-child {
    border-bottom: none;
  }

  .filename-tag {
    font-size: 11px;
    max-width: 280px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
