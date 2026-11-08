<script lang="ts">
  import {
    get,
    message,
    send,
    getAllBacklogJobs,
    cancelBacklogJob,
    getLibraryReceipts,
    verifyPublicationReceipt,
    getAcquisitionOperationsSummary,
    getDownloadRecoveryInfo,
    getDownloadFallbackCandidates,
    switchDownloadFallback,
    type DownloadRecoveryInfo,
    type FallbackEvaluation,
    type PipelineEvent,
    type Queue,
    type Queued,
    type BacklogJobDto,
    type PublicationReceiptDto,
    type AcquisitionOperationsSummaryDto,
  } from '../lib/api';
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

  // Operations Center Tabs (M7.4)
  let activeTab = $state<'queue' | 'backlog' | 'receipts' | 'health'>('queue');

  // Backlog state
  let allBacklogJobs = $state<BacklogJobDto[]>([]);
  let backlogLoading = $state(false);
  let backlogError = $state<string | null>(null);

  async function loadBacklogJobs() {
    backlogLoading = true;
    backlogError = null;
    try {
      allBacklogJobs = await getAllBacklogJobs(50);
    } catch (e) {
      backlogError = message(e);
    } finally {
      backlogLoading = false;
    }
  }

  // Recovery Inspection State (M8.3 / M10 Fallback)
  let inspectingRecovery = $state<DownloadRecoveryInfo | null>(null);
  let loadingRecovery = $state(false);
  let fallbackEval = $state<FallbackEvaluation | null>(null);
  let switchingFallback = $state(false);

  async function openRecoveryInspector(chapterId: number) {
    loadingRecovery = true;
    fallbackEval = null;
    try {
      inspectingRecovery = await getDownloadRecoveryInfo(chapterId);
      fallbackEval = await getDownloadFallbackCandidates(chapterId).catch(() => null);
    } catch (e) {
      toast.error(message(e));
    } finally {
      loadingRecovery = false;
    }
  }

  async function handleSwitchFallback(chapterId: number, candidateId: string) {
    switchingFallback = true;
    try {
      const res = await switchDownloadFallback(chapterId, candidateId);
      toast.success(res.message);
      inspectingRecovery = null;
      fallbackEval = null;
      await load();
    } catch (e) {
      toast.error(message(e));
    } finally {
      switchingFallback = false;
    }
  }

  async function handleCancelBacklog(jobId: string) {
    try {
      await cancelBacklogJob(jobId);
      toast.success('Backlog job cancelled.');
      await loadBacklogJobs();
    } catch (e) {
      toast.error(message(e));
    }
  }

  // Receipts state
  let allReceipts = $state<PublicationReceiptDto[]>([]);
  let receiptsLoading = $state(false);
  let receiptsError = $state<string | null>(null);
  let verifyingReceipts = $state<Record<string, boolean>>({});

  async function loadReceipts() {
    receiptsLoading = true;
    receiptsError = null;
    try {
      allReceipts = await getLibraryReceipts(50);
    } catch (e) {
      receiptsError = message(e);
    } finally {
      receiptsLoading = false;
    }
  }

  async function handleVerifyReceipt(receiptId: string) {
    verifyingReceipts[receiptId] = true;
    try {
      const res = await verifyPublicationReceipt(receiptId);
      if (res.isValid) {
        toast.success(`Verified CBZ integrity and SHA-256 for receipt ${receiptId.slice(0, 8)}…`);
      } else {
        toast.error(`Verification issue: ${res.error || 'Invalid CBZ'}`);
      }
      await loadReceipts();
    } catch (e) {
      toast.error(message(e));
    } finally {
      verifyingReceipts[receiptId] = false;
    }
  }

  // Health summary state
  let healthSummary = $state<AcquisitionOperationsSummaryDto | null>(null);
  let healthLoading = $state(false);
  let healthError = $state<string | null>(null);

  async function loadHealthSummary() {
    healthLoading = true;
    healthError = null;
    try {
      healthSummary = await getAcquisitionOperationsSummary();
    } catch (e) {
      healthError = message(e);
    } finally {
      healthLoading = false;
    }
  }

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
    <div class="head-left">
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
      <div class="pill-group view-switcher" role="tablist" aria-label="Downloads view">
        <button
          class="pill"
          class:active={activeTab === 'queue'}
          onclick={() => (activeTab = 'queue')}
          role="tab"
          aria-selected={activeTab === 'queue'}
        >
          <Icon name="download" size={14} /> Active Queue
        </button>
        <button
          class="pill"
          class:active={activeTab === 'backlog'}
          onclick={() => {
            activeTab = 'backlog';
            if (allBacklogJobs.length === 0) loadBacklogJobs();
          }}
          role="tab"
          aria-selected={activeTab === 'backlog'}
        >
          <Icon name="list" size={14} /> Backlog Jobs
        </button>
        <button
          class="pill"
          class:active={activeTab === 'receipts'}
          onclick={() => {
            activeTab = 'receipts';
            if (allReceipts.length === 0) loadReceipts();
          }}
          role="tab"
          aria-selected={activeTab === 'receipts'}
        >
          <Icon name="check" size={14} /> Publication Receipts
        </button>
        <button
          class="pill"
          class:active={activeTab === 'health'}
          onclick={() => {
            activeTab = 'health';
            if (!healthSummary) loadHealthSummary();
          }}
          role="tab"
          aria-selected={activeTab === 'health'}
        >
          <Icon name="sliders" size={14} /> Operations Health
        </button>
      </div>
    </div>
    {#if activeTab === 'queue' && data}
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
    {:else if activeTab === 'backlog'}
      <div class="tools">
        <button class="btn" disabled={backlogLoading} onclick={loadBacklogJobs}>
          {#if backlogLoading}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={18} />
          {/if}
          Refresh
        </button>
      </div>
    {:else if activeTab === 'receipts'}
      <div class="tools">
        <button class="btn" disabled={receiptsLoading} onclick={loadReceipts}>
          {#if receiptsLoading}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={18} />
          {/if}
          Refresh
        </button>
      </div>
    {:else if activeTab === 'health'}
      <div class="tools">
        <button class="btn" disabled={healthLoading} onclick={loadHealthSummary}>
          {#if healthLoading}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={18} />
          {/if}
          Refresh
        </button>
      </div>
    {/if}
  </div>

  {#if activeTab === 'queue'}
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
                  <button
                    class="btn pill"
                    disabled={busyItems[entry.chapterId]}
                    onclick={() => act('POST', `/downloads/${entry.chapterId}/recover`)}
                    title="Recover published file from receipt without re-downloading"
                  >
                    <Icon name="check" size={14} /> Recover
                  </button>
                  <button
                    class="btn pill"
                    disabled={busyItems[entry.chapterId] || loadingRecovery}
                    onclick={() => openRecoveryInspector(entry.chapterId)}
                    title="Inspect crash-safe recovery details"
                  >
                    <Icon name="info" size={14} /> Recovery Details
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
                    {:else if entry.sourceType === 'madokami'}
                      <span class="tag spot">Madokami</span>
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

  {:else if activeTab === 'backlog'}
    <Notice loading={backlogLoading} error={backlogError} retry={loadBacklogJobs} />

    {#if !backlogLoading && !backlogError}
      {#if allBacklogJobs.length === 0}
        <Notice>
          No historical backlog jobs recorded. Backlog jobs can be planned and reviewed from any series' title page or diagnostics modal.
        </Notice>
      {:else}
        <section class="section-card">
          <div class="section-header">
            <h2 class="display section-title">HISTORICAL BACKLOG JOBS ({allBacklogJobs.length})</h2>
          </div>
          <ul class="rows backlog-rows">
            {#each allBacklogJobs as job (job.id)}
              <li class="backlog-row">
                <div class="main">
                  <div class="row-header">
                    <strong>{job.seriesTitle}</strong>
                    <span class="tag" class:spot={job.state === 'COMPLETED'} class:danger={job.state === 'FAILED'} class:warning={job.state === 'PLANNED'}>
                      {job.state}
                    </span>
                    <span class="tag">{job.scope}</span>
                    {#if job.isDryRun}
                      <span class="tag muted">DRY-RUN</span>
                    {/if}
                  </div>
                  <div class="job-meta">
                    <span>Requested: {job.totalRequested}</span>
                    <span>·</span>
                    <span>Eligible: {job.totalEligible}</span>
                    <span>·</span>
                    <span>Already Owned: {job.totalAlreadyOwned}</span>
                    <span>·</span>
                    <span>Queued: {job.totalQueued}</span>
                    {#if job.totalFailed > 0}
                      <span>·</span>
                      <span class="attention-badge">Failed: {job.totalFailed}</span>
                    {/if}
                    <span>·</span>
                    <span class="muted">{ago(job.createdAt > 1e11 ? Math.floor(job.createdAt / 1000) : job.createdAt)}</span>
                  </div>
                  {#if job.requestedVolumes && job.requestedVolumes.length > 0}
                    <div class="volume-tags">
                      <span class="muted-label">Volumes:</span>
                      {#each job.requestedVolumes.slice(0, 15) as v}
                        <span class="vol-chip">Vol. {v}</span>
                      {/each}
                      {#if job.requestedVolumes.length > 15}
                        <span class="muted">+{job.requestedVolumes.length - 15} more</span>
                      {/if}
                    </div>
                  {/if}
                  {#if job.error}
                    <div class="error-desc">{job.error}</div>
                  {/if}
                </div>
                {#if job.state === 'PLANNED' || job.state === 'executing'}
                  <div class="row-actions">
                    <button class="btn pill danger" onclick={() => handleCancelBacklog(job.id)}>
                      Cancel Job
                    </button>
                  </div>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    {/if}

  {:else if activeTab === 'receipts'}
    <Notice loading={receiptsLoading} error={receiptsError} retry={loadReceipts} />

    {#if !receiptsLoading && !receiptsError}
      {#if allReceipts.length === 0}
        <Notice>
          No publication receipts recorded yet. Receipts are generated when acquisitions complete the native pipeline and are verified into canonical storage.
        </Notice>
      {:else}
        <section class="section-card">
          <div class="section-header">
            <div>
              <h2 class="display section-title">PUBLICATION RECEIPTS ({allReceipts.length})</h2>
              <span class="queue-status-text">Immutable audit trail of pipeline publications and canonical storage verification</span>
            </div>
          </div>
          <ul class="rows receipt-rows">
            {#each allReceipts as r (r.id)}
              <li class="receipt-row">
                <div class="main">
                  <div class="row-header">
                    <strong>{r.seriesTitle}</strong>
                    {#if r.volumeNumber != null}
                      <span class="tag">Vol. {r.volumeNumber}</span>
                    {:else if r.chapterNumber != null}
                      <span class="tag">Ch. {r.chapterNumber}</span>
                    {/if}
                    {#if r.verifiedOwnership}
                      <span class="tag spot">Verified in Library</span>
                    {:else}
                      <span class="tag warning">Unverified</span>
                    {/if}
                    {#if r.komgaNotified}
                      <span class="tag">Komga Synced</span>
                    {/if}
                    {#if r.metadataStatus === 'complete'}
                      <span class="tag">Metadata Enriched</span>
                    {/if}
                  </div>
                  <div class="receipt-path mono muted">
                    {r.canonicalPath}
                  </div>
                  <div class="receipt-meta">
                    <span>Size: {formatBytes(r.fileSizeBytes)}</span>
                    <span>·</span>
                    <span class="mono">SHA: {r.fileSha256 ? r.fileSha256.slice(0, 16) + '…' : 'None'}</span>
                    <span>·</span>
                    <span class="muted">{ago(r.publishedAt > 1e11 ? Math.floor(r.publishedAt / 1000) : r.publishedAt)}</span>
                  </div>
                  {#if r.metadataError}
                    <div class="error-desc">Metadata error: {r.metadataError}</div>
                  {/if}
                </div>
                <div class="row-actions">
                  <button
                    class="btn pill"
                    disabled={verifyingReceipts[r.id]}
                    onclick={() => handleVerifyReceipt(r.id)}
                  >
                    {#if verifyingReceipts[r.id]}
                      <span class="spin"><Icon name="spinner" size={14} /></span>
                    {:else}
                      <Icon name="check" size={14} />
                    {/if}
                    Verify CBZ
                  </button>
                </div>
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    {/if}

  {:else if activeTab === 'health'}
    <Notice loading={healthLoading} error={healthError} retry={loadHealthSummary} />

    {#if !healthLoading && !healthError}
      {#if healthSummary}
        <!-- CRITICAL SAFETY LOCK BANNER -->
        <div class="safety-card">
          <div class="safety-header">
            <span class="status-indicator active"></span>
            <strong>ACQUISITION SAFETY LOCK ACTIVE</strong>
          </div>
          <p class="safety-desc">
            Mango operates under strict fail-closed safety constraints. Autonomous downloads without explicit user approval are completely locked out.
          </p>
          <div class="safety-grid">
            <div class="safety-item">
              <span class="safety-label">Global Auto-Acquire Switch</span>
              <span class="status-badge" class:locked={!healthSummary.autoAcquireEnabled}>
                {healthSummary.autoAcquireEnabled ? 'ENABLED (Risk)' : 'DISABLED (Safety Locked)'}
              </span>
            </div>
            <div class="safety-item">
              <span class="safety-label">Auto-Acquire Dry-Run Mode</span>
              <span class="status-badge" class:active={healthSummary.autoAcquireDryRun}>
                {healthSummary.autoAcquireDryRun ? 'ENFORCED (Dry-Run)' : 'MUTATING'}
              </span>
            </div>
            <div class="safety-item">
              <span class="safety-label">Titles Opted-In to Auto-Acquire</span>
              <span class="safety-val">
                <strong>{healthSummary.autoAcquireOptedInCount}</strong> titles (0 expected in safe operation)
              </span>
            </div>
          </div>
        </div>

        <!-- OPERATIONAL METRICS -->
        <div class="metrics-grid">
          <div class="metric-card">
            <div class="metric-val">{healthSummary.queueTotal}</div>
            <div class="metric-label">Active Downloads Queue</div>
            <div class="metric-sub">{healthSummary.queuePaused ? 'Queue is Paused' : 'Queue is Processing'}</div>
          </div>
          <div class="metric-card">
            <div class="metric-val">{healthSummary.backlogJobsTotal}</div>
            <div class="metric-label">Backlog Jobs</div>
            <div class="metric-sub">{healthSummary.backlogJobsPending} planned · {healthSummary.backlogJobsRunning} executing</div>
          </div>
          <div class="metric-card">
            <div class="metric-val">{healthSummary.receiptsTotal}</div>
            <div class="metric-label">Published Receipts</div>
            <div class="metric-sub">{healthSummary.receiptsVerified} verified · {healthSummary.receiptsUnverified} pending check</div>
          </div>
          <div class="metric-card">
            <div class="metric-val">{formatBytes(healthSummary.freeDiskBytes)}</div>
            <div class="metric-label">Staging Free Disk Space</div>
            <div class="metric-sub">Download directory storage</div>
          </div>
        </div>

        <!-- OPERATIONS CONTROL & LINKS -->
        <div class="operations-links-card">
          <h3 class="display sub-title">STORAGE & RELEASE CONTROLS</h3>
          <div class="links-row">
            <a class="btn" href="#/settings">
              <Icon name="sliders" size={16} />
              Staging Storage Hygiene & Manifest
            </a>
            <a class="btn" href="#/updates">
              <Icon name="bell" size={16} />
              Availability Watchers & Release Calendar
            </a>
          </div>
        </div>
      {/if}
    {/if}
  {/if}
</div>

{#if inspectingRecovery}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="recovery-modal-backdrop" onclick={() => (inspectingRecovery = null)}>
    <div class="recovery-modal-card" onclick={(e) => e.stopPropagation()}>
      <div class="recovery-modal-header">
        <div>
          <h3 class="display">RECOVERY DIAGNOSTICS</h3>
          <p class="recovery-modal-subtitle">
            {inspectingRecovery.title} · {inspectingRecovery.chapterName}
          </p>
        </div>
        <button class="btn icon subtle" onclick={() => (inspectingRecovery = null)}>
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="recovery-modal-body">
        <div class="recovery-status-strip">
          <div class="strip-item">
            <span class="strip-label">Error Category</span>
            <span class="strip-val danger">{inspectingRecovery.errorCategory.toUpperCase()}</span>
          </div>
          <div class="strip-item">
            <span class="strip-label">Failed Stage</span>
            <span class="strip-val">{inspectingRecovery.failedStage}</span>
          </div>
          <div class="strip-item">
            <span class="strip-label">Last Succeeded</span>
            <span class="strip-val">{inspectingRecovery.lastSuccessfulStage}</span>
          </div>
        </div>

        {#if inspectingRecovery.errorMessage}
          <div class="recovery-error-banner">
            <Icon name="info" size={16} />
            <span class="error-text">{inspectingRecovery.errorMessage}</span>
          </div>
        {/if}

        <div class="recovery-details-grid">
          <div class="detail-box">
            <h4>Canonical Media State</h4>
            <div class="detail-line">
              <span class="dt-k">Publication Receipt:</span>
              <span class="dt-v">{inspectingRecovery.canonicalPublicationOccurred ? 'Recorded' : 'Not Published'}</span>
            </div>
            {#if inspectingRecovery.canonicalReceiptId}
              <div class="detail-line">
                <span class="dt-k">Receipt ID:</span>
                <span class="dt-v mono">{inspectingRecovery.canonicalReceiptId}</span>
              </div>
            {/if}
            {#if inspectingRecovery.canonicalPath}
              <div class="detail-line">
                <span class="dt-k">Canonical Path:</span>
                <span class="dt-v mono">{inspectingRecovery.canonicalPath}</span>
              </div>
            {/if}
            <div class="detail-line">
              <span class="dt-k">Komga Status:</span>
              <span class="dt-v tag {inspectingRecovery.komgaStatus === 'komga_verified' ? 'spot' : 'warning'}">{inspectingRecovery.komgaStatus}</span>
            </div>
            <div class="detail-line">
              <span class="dt-k">Metadata Status:</span>
              <span class="dt-v tag">{inspectingRecovery.metadataStatus}</span>
            </div>
          </div>

          <div class="detail-box">
            <h4>Staging & Artifact State</h4>
            <div class="detail-line">
              <span class="dt-k">Artifact on Disk:</span>
              <span class="dt-v">{inspectingRecovery.artifactExists ? 'Verified Present' : 'None Found'}</span>
            </div>
            {#if inspectingRecovery.artifactPath}
              <div class="detail-line">
                <span class="dt-k">Artifact Path:</span>
                <span class="dt-v mono">{inspectingRecovery.artifactPath}</span>
              </div>
            {/if}
            <div class="detail-line">
              <span class="dt-k">Retry Safety:</span>
              <span class="dt-v">{inspectingRecovery.retryIsSafe ? 'Safe (Idempotent)' : 'Blocked'}</span>
            </div>
          </div>
        </div>

        <div class="recommendation-card">
          <div class="rec-title">
            <Icon name="check" size={16} />
            <strong>Recommended Action:</strong>
          </div>
          <p class="rec-desc">
            {#if inspectingRecovery.recommendedAction === 'verify_komga'}
              Canonical file exists and receipt is recorded. Triggering a targeted Komga scan will verify ownership without re-downloading media.
            {:else if inspectingRecovery.recommendedAction === 'republish_staged_archive'}
              Valid archive was found in staging. Recovery can validate and publish without initiating a new external download.
            {:else if inspectingRecovery.recommendedAction === 'already_verified_complete'}
              This acquisition is already published and verified in Komga canonical library. No recovery action needed.
            {:else if inspectingRecovery.recommendedAction === 'retry_metadata_only'}
              Media archive is published. Only metadata delivery needs to be retried. Canonical file will not be modified.
            {:else}
              Download failed before completing staging. A safe download restart is recommended.
            {/if}
          </p>
        </div>

        {#if fallbackEval && fallbackEval.hasFallbackCandidates}
          <div class="fallback-section">
            <div class="fallback-header">
              <Icon name="search" size={16} />
              <strong>Compatible Fallback Releases Across Sources (M10):</strong>
            </div>
            <p class="fallback-desc">
              If this release remains unavailable or failing, you can safely switch to a verified alternative release below without losing diagnostic history:
            </p>
            <div class="fallback-list">
              {#each fallbackEval.fallbackCandidates as cand}
                <div class="fallback-card">
                  <div class="fallback-info">
                    <div class="fallback-badges">
                      <span class="tag spot">{cand.provider.toUpperCase()}</span>
                      {#if cand.confidence}
                        <span class="tag {cand.confidence === 'high' ? 'spot' : 'warning'}">{cand.confidence.toUpperCase()} CONFIDENCE</span>
                      {/if}
                      {#if cand.isDigital}
                        <span class="tag">DIGITAL</span>
                      {/if}
                      {#if cand.seeders !== undefined && cand.seeders !== null}
                        <span class="tag muted">{cand.seeders} seeds</span>
                      {/if}
                    </div>
                    <div class="fallback-title" title={cand.rawTitle}>{cand.title}</div>
                    <div class="fallback-meta">Score: {cand.score.toFixed(0)} · {cand.scoreExplanation}</div>
                  </div>
                  <button
                    class="btn spot small"
                    disabled={switchingFallback}
                    onclick={() => handleSwitchFallback(inspectingRecovery!.chapterId, cand.candidateId)}
                  >
                    Switch & Re-queue
                  </button>
                </div>
              {/each}
            </div>
          </div>
        {/if}
      </div>

      <div class="recovery-modal-footer">
        <button class="btn subtle" onclick={() => (inspectingRecovery = null)}>
          Close
        </button>
        {#if inspectingRecovery.canRecover}
          <button
            class="btn spot"
            onclick={async () => {
              const cid = inspectingRecovery!.chapterId;
              inspectingRecovery = null;
              await act('POST', `/downloads/${cid}/recover`);
            }}
          >
            Apply Recommended Recovery
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

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

  .head-left {
    display: flex;
    align-items: center;
    gap: 16px;
    flex-wrap: wrap;
  }

  .view-switcher {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .pill-group {
    display: flex;
    gap: 4px;
    background: var(--shade);
    padding: 3px;
    border-radius: 9999px;
  }

  .pill {
    border: none;
    background: transparent;
    color: var(--dim);
    padding: 5px 12px;
    border-radius: 9999px;
    font-size: 0.85rem;
    font-weight: 600;
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    transition: background-color 0.15s, color 0.15s;
  }

  .pill:hover {
    color: var(--ink);
  }

  .pill.active {
    background: var(--surface);
    color: var(--ink);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1);
  }

  /* Safety card */
  .safety-card {
    background: #f0fdf4;
    border: 2px solid #16a34a;
    border-radius: var(--radius, 6px);
    box-shadow: 2px 2px 0 var(--ink);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .safety-header {
    display: flex;
    align-items: center;
    gap: 8px;
    color: #15803d;
    font-size: 0.95rem;
  }

  .safety-desc {
    margin: 0;
    font-size: 0.88rem;
    color: var(--ink);
  }

  .safety-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(240px, 1fr));
    gap: 12px;
  }

  .safety-item {
    background: var(--surface);
    border: 1px solid var(--ink);
    border-radius: 4px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .safety-label {
    font-size: 0.78rem;
    color: var(--dim);
    text-transform: uppercase;
    font-weight: 700;
  }

  .safety-val {
    font-size: 0.9rem;
    color: var(--ink);
  }

  .status-badge {
    display: inline-block;
    padding: 2px 8px;
    border-radius: 4px;
    font-size: 0.82rem;
    font-weight: 700;
  }

  .status-badge.locked {
    background: #dcfce7;
    color: #166534;
    border: 1px solid #16a34a;
  }

  .status-badge.active {
    background: #e0f2fe;
    color: #0369a1;
    border: 1px solid #0284c7;
  }

  /* Operational metrics */
  .metrics-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 12px;
  }

  .metric-card {
    background: var(--surface);
    border: 2px solid var(--ink);
    border-radius: var(--radius, 6px);
    box-shadow: 2px 2px 0 var(--ink);
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .metric-val {
    font-size: 1.6rem;
    font-weight: 800;
    line-height: 1.1;
  }

  .metric-label {
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--ink);
  }

  .metric-sub {
    font-size: 0.78rem;
    color: var(--dim);
  }

  .operations-links-card {
    background: var(--surface);
    border: 2px solid var(--ink);
    border-radius: var(--radius, 6px);
    box-shadow: 2px 2px 0 var(--ink);
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .links-row {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }

  /* Backlog rows & Receipt rows */
  .backlog-rows, .receipt-rows {
    background: var(--surface);
    border: 1px solid var(--ink);
    border-radius: 4px;
    overflow: hidden;
  }

  .backlog-row, .receipt-row {
    padding: 12px 14px;
    border-bottom: 1px solid var(--muted);
  }

  .backlog-row:last-child, .receipt-row:last-child {
    border-bottom: none;
  }

  .job-meta, .receipt-meta {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.82rem;
    color: var(--dim);
    margin-top: 4px;
    flex-wrap: wrap;
  }

  .volume-tags {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
    flex-wrap: wrap;
  }

  .vol-chip {
    background: var(--shade);
    padding: 2px 6px;
    border-radius: 3px;
    font-size: 0.78rem;
    font-weight: 600;
  }

  .muted-label {
    font-size: 0.78rem;
    color: var(--dim);
  }

  .receipt-path {
    font-size: 0.8rem;
    margin-top: 4px;
    word-break: break-all;
  }

  .tag.warning {
    background: #fef3c7;
    color: #92400e;
    border: 1px solid #f59e0b;
  }

  .recovery-modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.75);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 1rem;
  }

  .recovery-modal-card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    max-width: 680px;
    width: 100%;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.5);
  }

  .recovery-modal-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    padding: 1.25rem 1.5rem;
    border-bottom: 1px solid var(--border);
  }

  .recovery-modal-subtitle {
    margin: 0.25rem 0 0 0;
    font-size: 0.85rem;
    color: var(--muted);
  }

  .recovery-modal-body {
    padding: 1.25rem 1.5rem;
    overflow-y: auto;
    flex: 1;
  }

  .recovery-status-strip {
    display: flex;
    gap: 1rem;
    background: var(--surface-alt, rgba(0, 0, 0, 0.04));
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.75rem 1rem;
    margin-bottom: 1rem;
  }

  .strip-item {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    flex: 1;
  }

  .strip-label {
    font-size: 0.7rem;
    text-transform: uppercase;
    color: var(--muted);
    letter-spacing: 0.05em;
  }

  .strip-val {
    font-size: 0.9rem;
    font-weight: 700;
  }

  .strip-val.danger {
    color: #ef4444;
  }

  .recovery-error-banner {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.3);
    color: #ef4444;
    padding: 0.75rem 1rem;
    border-radius: 6px;
    font-size: 0.85rem;
    margin-bottom: 1rem;
  }

  .recovery-details-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1rem;
    margin-bottom: 1rem;
  }

  .detail-box {
    background: var(--surface-alt, rgba(0, 0, 0, 0.02));
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.85rem 1rem;
  }

  .detail-box h4 {
    margin: 0 0 0.5rem 0;
    font-size: 0.85rem;
    letter-spacing: 0.03em;
  }

  .detail-line {
    display: flex;
    justify-content: space-between;
    font-size: 0.8rem;
    margin-bottom: 0.35rem;
  }

  .dt-k {
    color: var(--muted);
  }

  .recommendation-card {
    background: rgba(34, 197, 94, 0.08);
    border: 1px solid rgba(34, 197, 94, 0.3);
    border-radius: 6px;
    padding: 0.85rem 1rem;
  }

  .rec-title {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    color: #22c55e;
    font-size: 0.85rem;
    margin-bottom: 0.25rem;
  }

  .rec-desc {
    font-size: 0.85rem;
    color: var(--fg);
    margin: 0;
    line-height: 1.4;
  }

  .recovery-modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    padding: 1rem 1.5rem;
    border-top: 1px solid var(--border);
  }

  .fallback-section {
    margin-top: 1rem;
    padding: 0.85rem 1rem;
    background: var(--surface-alt, rgba(0, 0, 0, 0.02));
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .fallback-header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
    color: var(--fg);
    margin-bottom: 0.25rem;
  }

  .fallback-desc {
    font-size: 0.8rem;
    color: var(--muted);
    margin: 0 0 0.75rem 0;
    line-height: 1.4;
  }

  .fallback-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .fallback-card {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
    padding: 0.6rem 0.75rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  .fallback-info {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    min-width: 0;
    flex: 1;
  }

  .fallback-badges {
    display: flex;
    gap: 0.35rem;
    align-items: center;
    flex-wrap: wrap;
  }

  .fallback-title {
    font-size: 0.85rem;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .fallback-meta {
    font-size: 0.75rem;
    color: var(--muted);
  }
</style>
