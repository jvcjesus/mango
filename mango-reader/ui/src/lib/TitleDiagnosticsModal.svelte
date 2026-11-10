<script lang="ts">
  import {
    cancelBacklogJob,
    clearManualTitleMapping,
    executeBacklog,
    getSeriesTimeline,
    getTitleAvailability,
    getTitleBacklogJobs,
    getTitleDiagnostics,
    getTitleReceipts,
    getTitleMangaBakaCollections,
    setTitleMangaBakaCollection,
    getTitleOpenTomeLines,
    setTitleOpenTomeLine,
    listMangaBakaCollections,
    message,
    planBacklog,
    refreshTitleAvailability,
    rescanPublicationReceipt,
    retryReceiptMetadata,
    send,
    setManualTitleMapping,
    updateTitleAvailability,
    verifyPublicationReceipt,
    type BacklogItemPlanDto,
    type BacklogJobDto,
    type BacklogScope,
    type MangaBakaCollectionInfo,
    type PublicationReceiptDto,
    type SeriesTimelineDto,
    type SeriesTimelineVolumeDto,
    type TitleAvailabilityDto,
    type TitleDiagnostics,
    type TitleMangaBakaCollectionsDto,
    type TitleOpenTomeLinesDto,
    type OpenTomeReleaseLineDto,
  } from './api';
  import CandidateInspectorModal from './CandidateInspectorModal.svelte';
  import Icon from './Icon.svelte';
  import Notice from './Notice.svelte';
  import { toast } from './toast.svelte';
  import { ago } from './util';

  let {
    open = false,
    mangaId,
    mangaTitle,
    onClose,
    onReconciled,
  }: {
    open: boolean;
    mangaId: number;
    mangaTitle: string;
    onClose: () => void;
    onReconciled?: () => void;
  } = $props();

  let loading = $state(false);
  let error = $state<string | null>(null);
  let diag = $state<TitleDiagnostics | null>(null);

  // M6.12 Unified Series Timeline & Gap Detection state
  let timeline = $state<SeriesTimelineDto | null>(null);
  let timelineLoading = $state(false);
  let inspectingVolume = $state<number | null>(null);

  // Manual resolution state
  let showManualSection = $state(false);
  let manualMbIdInput = $state('');
  let manualEditionInput = $state('standard');
  let manualCollectionId = $state('');
  let availableCollections = $state<MangaBakaCollectionInfo[]>([]);
  let loadingCollections = $state(false);
  let savingManual = $state(false);
  let manualSearchQuery = $state('');
  let searchingMb = $state(false);
  let searchResults = $state<Array<{ id: number; title: string; year?: number | null }>>([]);

  // Release availability watcher state (M6.8.4)
  let availability = $state<TitleAvailabilityDto | null>(null);
  let refreshingAvailability = $state(false);
  let updatingAvailability = $state(false);

  async function handleToggleMonitoring() {
    if (!mangaId || !availability) return;
    updatingAvailability = true;
    try {
      availability = await updateTitleAvailability(mangaId, {
        monitoringEnabled: !availability.monitoringEnabled,
      });
      toast.success(`Release monitoring ${availability.monitoringEnabled ? 'enabled' : 'disabled'}`);
    } catch (e) {
      toast.error(`Failed to update monitoring: ${message(e)}`);
    } finally {
      updatingAvailability = false;
    }
  }

  async function handleToggleNotify() {
    if (!mangaId || !availability) return;
    updatingAvailability = true;
    try {
      availability = await updateTitleAvailability(mangaId, {
        notifyEnabled: !availability.notifyEnabled,
      });
      toast.success(`Release notifications ${availability.notifyEnabled ? 'enabled' : 'disabled'}`);
    } catch (e) {
      toast.error(`Failed to update notifications: ${message(e)}`);
    } finally {
      updatingAvailability = false;
    }
  }

  async function handleRefreshAvailability() {
    if (!mangaId) return;
    refreshingAvailability = true;
    try {
      const res = await refreshTitleAvailability(mangaId);
      toast.success(`Availability check triggered (${res.watchersChecked} watchers checked)`);
      availability = await getTitleAvailability(mangaId);
    } catch (e) {
      toast.error(`Availability check failed: ${message(e)}`);
    } finally {
      refreshingAvailability = false;
    }
  }

  // Publication receipts state (M6.9)
  let receipts = $state<PublicationReceiptDto[]>([]);
  let verifyingReceiptId = $state<string | null>(null);
  let rescanningReceiptId = $state<string | null>(null);
  let repairingReceiptId = $state<string | null>(null);

  // Guarded Historical Backlog state (M6.10)
  let backlogJobs = $state<BacklogJobDto[]>([]);
  let backlogScope = $state<BacklogScope>('ALL_MISSING');
  let backlogRangeStart = $state<number | undefined>(undefined);
  let backlogRangeEnd = $state<number | undefined>(undefined);
  let backlogExplicitInput = $state('');
  let backlogBatchSize = $state(5);
  let backlogDryRun = $state(true);
  let planningBacklog = $state(false);
  let executingBacklog = $state(false);
  let plannedJob = $state<BacklogJobDto | null>(null);

  async function handlePlanBacklog() {
    if (!mangaId) return;
    planningBacklog = true;
    try {
      let explicitVols: number[] | undefined = undefined;
      if (backlogScope === 'EXPLICIT_VOLUMES') {
        explicitVols = backlogExplicitInput
          .split(',')
          .map((s) => parseFloat(s.trim()))
          .filter((n) => !isNaN(n) && n > 0);
        if (!explicitVols.length) {
          toast.error('Please enter at least one valid volume number');
          planningBacklog = false;
          return;
        }
      }
      plannedJob = await planBacklog(mangaId, {
        scope: backlogScope,
        rangeStart: backlogRangeStart,
        rangeEnd: backlogRangeEnd,
        explicitVolumes: explicitVols,
        batchSize: backlogBatchSize,
        isDryRun: backlogDryRun,
      });
      toast.success(`Backlog batch planned: ${plannedJob.totalEligible} eligible volume(s)`);
      backlogJobs = await getTitleBacklogJobs(mangaId).catch(() => backlogJobs);
    } catch (e) {
      toast.error(`Backlog planning failed: ${message(e)}`);
    } finally {
      planningBacklog = false;
    }
  }

  async function handleExecuteBacklog(job: BacklogJobDto) {
    if (!mangaId) return;
    if (job.totalEligible > 1 && !confirm(`Confirm batch execution for ${job.totalEligible} volumes?`)) {
      return;
    }
    if (!job.isDryRun && !confirm('WARNING: Dry-run is OFF. This may queue actual downloads. Proceed?')) {
      return;
    }
    executingBacklog = true;
    try {
      const res = await executeBacklog(mangaId, {
        jobId: job.id,
        confirm: true,
        confirmMultiple: true,
        isDryRun: job.isDryRun,
      });
      toast.success(res.message);
      plannedJob = null;
      backlogJobs = await getTitleBacklogJobs(mangaId).catch(() => backlogJobs);
    } catch (e) {
      toast.error(`Backlog execution failed: ${message(e)}`);
    } finally {
      executingBacklog = false;
    }
  }

  async function handleCancelJob(jobId: string) {
    if (!mangaId) return;
    try {
      await cancelBacklogJob(jobId);
      toast.success('Backlog job cancelled');
      if (plannedJob?.id === jobId) plannedJob = null;
      backlogJobs = await getTitleBacklogJobs(mangaId).catch(() => backlogJobs);
    } catch (e) {
      toast.error(`Cancel failed: ${message(e)}`);
    }
  }

  async function handleVerifyReceipt(r: PublicationReceiptDto) {
    verifyingReceiptId = r.id;
    try {
      const res = await verifyPublicationReceipt(r.id);
      if (res.isValid) {
        toast.success(`Verified: ${r.seriesTitle} (${(r.fileSizeBytes / 1024 / 1024).toFixed(1)} MB, CBZ valid, SHA256 matched)`);
      } else {
        toast.error(`Verification issue: ${res.error ?? 'Invalid archive'}`);
      }
      receipts = await getTitleReceipts(mangaId).catch(() => receipts);
    } catch (e) {
      toast.error(`Verification failed: ${message(e)}`);
    } finally {
      verifyingReceiptId = null;
    }
  }

  async function handleRescanReceipt(r: PublicationReceiptDto) {
    rescanningReceiptId = r.id;
    try {
      await rescanPublicationReceipt(r.id);
      toast.success(`Triggered Komga library rescan for ${r.seriesTitle}`);
      receipts = await getTitleReceipts(mangaId).catch(() => receipts);
    } catch (e) {
      toast.error(`Rescan failed: ${message(e)}`);
    } finally {
      rescanningReceiptId = null;
    }
  }

  async function handleRepairReceiptMetadata(r: PublicationReceiptDto) {
    repairingReceiptId = r.id;
    try {
      await retryReceiptMetadata(r.id);
      toast.success(`Metadata repaired for ${r.seriesTitle}`);
      receipts = await getTitleReceipts(mangaId).catch(() => receipts);
    } catch (e) {
      toast.error(`Metadata repair failed: ${message(e)}`);
    } finally {
      repairingReceiptId = null;
    }
  }

  async function load() {
    if (!open || !mangaId) return;
    loading = true;
    error = null;
    try {
      diag = await getTitleDiagnostics(mangaId);
      timeline = await getSeriesTimeline(mangaId).catch(() => null);
      availability = await getTitleAvailability(mangaId).catch(() => null);
      receipts = await getTitleReceipts(mangaId).catch(() => []);
      backlogJobs = await getTitleBacklogJobs(mangaId).catch(() => []);
      if (diag.identity.mangabakaId) {
        manualMbIdInput = String(diag.identity.mangabakaId);
        loadCollections(diag.identity.mangabakaId);
      }
      if (diag.collection.resolvedCollectionId) {
        manualCollectionId = diag.collection.resolvedCollectionId;
      }
      if (diag.collection.edition) {
        manualEditionInput = diag.collection.edition;
      }
      loadOpenTomeLines();
      loadTitleMbCollections();
    } catch (e) {
      error = message(e);
    } finally {
      loading = false;
    }
  }

  let openTomeLinesData = $state<TitleOpenTomeLinesDto | null>(null);
  let loadingOpenTomeLines = $state(false);
  let selectingOpenTomeLine = $state(false);
  let showOpenTomeLineSelector = $state(false);

  async function loadOpenTomeLines() {
    loadingOpenTomeLines = true;
    try {
      openTomeLinesData = await getTitleOpenTomeLines(mangaId);
    } catch {
      openTomeLinesData = null;
    } finally {
      loadingOpenTomeLines = false;
    }
  }

  async function handleSelectOpenTomeLine(gcdSeriesId: number) {
    selectingOpenTomeLine = true;
    try {
      await setTitleOpenTomeLine(mangaId, gcdSeriesId);
      toast.success('OpenTome release line retargeted.');
      showOpenTomeLineSelector = false;
      await load();
    } catch (e) {
      toast.error(message(e));
    } finally {
      selectingOpenTomeLine = false;
    }
  }

  let titleMbCollectionsData = $state<TitleMangaBakaCollectionsDto | null>(null);
  let loadingTitleMbCollections = $state(false);

  async function loadTitleMbCollections() {
    loadingTitleMbCollections = true;
    try {
      titleMbCollectionsData = await getTitleMangaBakaCollections(mangaId);
    } catch {
      titleMbCollectionsData = null;
    } finally {
      loadingTitleMbCollections = false;
    }
  }

  async function handleSelectTitleCollection(collectionId: string, edition?: string) {
    try {
      await setTitleMangaBakaCollection(mangaId, collectionId, edition);
      toast.success('MangaBaka edition collection resolved and saved.');
      await load();
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function loadCollections(seriesId: number) {
    loadingCollections = true;
    try {
      availableCollections = await listMangaBakaCollections(seriesId);
      if (availableCollections.length > 0 && !manualCollectionId) {
        manualCollectionId = availableCollections[0].id;
        manualEditionInput = availableCollections[0].edition || 'standard';
      }
    } catch {
      availableCollections = [];
    } finally {
      loadingCollections = false;
    }
  }

  async function searchMangaBaka() {
    if (!manualSearchQuery.trim()) return;
    searchingMb = true;
    try {
      const res = await send<Array<{ id: number; title: string; year?: number | null }>>(
        'GET',
        `/catalog/mangabaka/search?q=${encodeURIComponent(manualSearchQuery.trim())}`
      );
      searchResults = res;
    } catch (e) {
      toast.error(`MangaBaka search failed: ${message(e)}`);
      searchResults = [];
    } finally {
      searchingMb = false;
    }
  }

  function selectSearchResult(item: { id: number; title: string }) {
    manualMbIdInput = String(item.id);
    searchResults = [];
    loadCollections(item.id);
  }

  async function handleApplyManualMapping() {
    const mbIdNum = manualMbIdInput.trim() ? parseInt(manualMbIdInput.trim(), 10) : undefined;
    if (manualMbIdInput.trim() && isNaN(mbIdNum!)) {
      toast.error('Invalid MangaBaka ID number');
      return;
    }

    savingManual = true;
    try {
      await setManualTitleMapping(mangaId, {
        mangabakaId: mbIdNum,
        collectionId: manualCollectionId.trim() || undefined,
        edition: manualEditionInput.trim() || 'standard',
      });
      toast.success('Manual mapping override applied successfully');
      showManualSection = false;
      await load();
      onReconciled?.();
    } catch (e) {
      toast.error(`Failed to apply manual mapping: ${message(e)}`);
    } finally {
      savingManual = false;
    }
  }

  async function handleClearManualMapping() {
    savingManual = true;
    try {
      await clearManualTitleMapping(mangaId);
      toast.success('Manual mapping cleared; reverted to automatic resolution');
      showManualSection = false;
      await load();
      onReconciled?.();
    } catch (e) {
      toast.error(`Failed to clear manual mapping: ${message(e)}`);
    } finally {
      savingManual = false;
    }
  }

  $effect(() => {
    if (open) {
      load();
    } else {
      diag = null;
      error = null;
      showManualSection = false;
    }
  });
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={onClose}>
    <div class="modal-card diag-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-head">
        <div class="head-title">
          <Icon name="info" size={20} />
          <h2>Diagnostics & Authority Resolution — {mangaTitle}</h2>
        </div>
        <button class="btn icon close-btn" onclick={onClose} aria-label="Close">
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body">
        {#if loading}
          <div class="loading-state">
            <span class="spin"><Icon name="spinner" size={24} stroke={3} /></span>
            <span>Gathering title diagnostics…</span>
          </div>
        {:else if error}
          <Notice error={error} retry={load} />
        {:else if diag}
          <!-- Auto-Acquisition Policy Gate Banner -->
          <div class="gate-eval-banner code-{diag.autoAcquireGates.finalCode}">
            <div class="gate-header">
              <span class="gate-badge">{diag.autoAcquireGates.finalCode}</span>
              <span class="gate-decision-text">{diag.autoAcquireGates.finalReason}</span>
            </div>
            <div class="gate-matrix">
              <div class="matrix-item" class:passed={diag.autoAcquireGates.globalEnabled}>
                <span class="matrix-lbl">Master Switch</span>
                <span class="matrix-val">{diag.autoAcquireGates.globalEnabled ? 'ON' : 'OFF'}</span>
              </div>
              <div class="matrix-item" class:passed={diag.autoAcquireGates.titleOptIn}>
                <span class="matrix-lbl">Title Opt-In</span>
                <span class="matrix-val">{diag.autoAcquireGates.titleOptIn ? 'Opted In' : 'Disabled'}</span>
              </div>
              <div class="matrix-item info">
                <span class="matrix-lbl">Dry-Run Mode</span>
                <span class="matrix-val">{diag.autoAcquireGates.dryRun ? 'ON (Simulate)' : 'OFF (Live)'}</span>
              </div>
              <div class="matrix-item" class:passed={diag.autoAcquireGates.releaseGatePassed}>
                <span class="matrix-lbl">Release Gate</span>
                <span class="matrix-val">{diag.autoAcquireGates.releaseGatePassed ? 'Passed' : 'Blocked'}</span>
              </div>
              <div class="matrix-item" class:passed={diag.autoAcquireGates.ownershipGatePassed}>
                <span class="matrix-lbl">Ownership Gate</span>
                <span class="matrix-val">{diag.autoAcquireGates.ownershipGatePassed ? 'Passed' : 'Owned'}</span>
              </div>
              <div class="matrix-item" class:passed={diag.autoAcquireGates.correspondenceGatePassed}>
                <span class="matrix-lbl">Correspondence</span>
                <span class="matrix-val">{diag.autoAcquireGates.correspondenceGatePassed ? 'Passed' : 'Review'}</span>
              </div>
            </div>
          </div>

          <!-- Identity Conflicts Warning -->
          {#if diag.identity.conflicts.length > 0}
            <div class="conflict-alert">
              <Icon name="close" size={18} />
              <div class="conflict-content">
                <strong>Identity Conflicts Detected:</strong>
                <ul>
                  {#each diag.identity.conflicts as conflict}
                    <li>{conflict}</li>
                  {/each}
                </ul>
              </div>
            </div>
          {/if}

          <!-- Unified Series Timeline & Gap Detection (M6.12) -->
          <div class="diag-card timeline-section">
            <div class="card-head">
              <div class="head-title">
                <Icon name="grid" size={16} />
                <h3>Unified Series Timeline & Gap Detection (M6.12)</h3>
              </div>
              {#if timeline}
                <div class="card-actions">
                  <span class="timeline-summary-pill">
                    {timeline.gaps.ownedCount} / {timeline.gaps.totalVolumes} Owned ({timeline.gaps.missingCount} Missing)
                  </span>
                </div>
              {/if}
            </div>

            <div class="card-content">
              {#if timelineLoading}
                <div class="loading-state">
                  <span class="spin"><Icon name="spinner" size={20} /></span>
                  <span>Synthesizing unified series timeline…</span>
                </div>
              {:else if timeline}
                <!-- Gaps & Continuity Status Banner -->
                <div
                  class="gap-status-banner"
                  class:banner-warning={timeline.gaps.hasInteriorGaps}
                  class:banner-healthy={!timeline.gaps.hasInteriorGaps && timeline.gaps.missingCount === 0}
                  class:banner-info={!timeline.gaps.hasInteriorGaps && timeline.gaps.missingCount > 0}
                >
                  <div class="gap-banner-content">
                    <Icon name={timeline.gaps.hasInteriorGaps ? 'alert' : timeline.gaps.missingCount === 0 ? 'check' : 'info'} size={18} />
                    <div class="gap-banner-text">
                      <strong>{timeline.gaps.summaryText}</strong>
                      <span>
                        {#if timeline.gaps.hasInteriorGaps}
                          Interior gaps detected in library: Volume {timeline.gaps.gapVolumeNumbers.join(', ')} missing between owned volumes.
                        {:else if timeline.gaps.missingCount === 0}
                          All official volumes established by MangaBaka are canonically owned.
                        {:else}
                          Continuous ownership up to volume {timeline.gaps.ownedCount}; remaining volumes awaiting acquisition.
                        {/if}
                      </span>
                    </div>
                  </div>
                </div>

                <!-- Volume Sequence Grid -->
                {#if timeline.volumes.length > 0}
                  <div class="timeline-grid-wrapper">
                    <div class="timeline-volume-grid">
                      {#each timeline.volumes as v}
                        <div
                          class="timeline-vol-card status-{v.status.toLowerCase()}"
                          class:is-owned={v.isOwned}
                          class:is-gap={v.isGap}
                        >
                          <div class="vol-card-head">
                            <span class="vol-number">Vol {v.volumeNumber}</span>
                            <span class="tag vol-tag vol-tag-{v.status.toLowerCase()}">
                              {v.status.replace(/_/g, ' ')}
                            </span>
                          </div>

                          {#if v.isGap}
                            <div class="gap-tag-pill gap-{v.gapType?.toLowerCase()}">
                              {v.gapType} GAP
                            </div>
                          {/if}

                          <div class="vol-card-meta">
                            {#if v.releaseDate}
                              <div class="meta-row">
                                <span class="meta-k">Release:</span>
                                <span class="meta-v">{v.releaseDate}</span>
                              </div>
                            {/if}

                            {#if v.candidateCount > 0}
                              <div class="cand-pill-box">
                                <span class="cand-pill-provider">{v.topCandidateSource || 'Candidate'}</span>
                                <span class="cand-pill-score">{v.topCandidateScore ? `${v.topCandidateScore.toFixed(0)} pts` : `${v.candidateCount} found`}</span>
                              </div>
                            {/if}

                            {#if v.receiptId}
                              <div class="receipt-verified-badge" title="Published with verified receipt">
                                <Icon name="check" size={12} />
                                <span>Verified CBZ</span>
                              </div>
                            {/if}
                          </div>

                          <div class="vol-card-actions">
                            <button
                              type="button"
                              class="btn mini outline vol-inspect-btn"
                              onclick={() => { inspectingVolume = v.volumeNumber; }}
                              title="Inspect discovered candidates and quality ranking"
                            >
                              Inspect Candidates
                            </button>
                            {#if !v.isOwned}
                              <button
                                type="button"
                                class="btn mini spot vol-plan-btn"
                                onclick={() => {
                                  backlogScope = 'EXPLICIT_VOLUMES';
                                  backlogExplicitInput = String(v.volumeNumber);
                                  handlePlanBacklog();
                                }}
                                title="Plan backlog acquisition for Volume {v.volumeNumber}"
                              >
                                Plan Acquisition
                              </button>
                            {/if}
                          </div>
                        </div>
                      {/each}
                    </div>
                  </div>
                {:else}
                  <div class="empty-state font-small muted">
                    No volume records established for this title.
                  </div>
                {/if}

                <!-- Reading-Source Separation Card -->
                <div class="reading-source-separation-box">
                  <div class="subcard-header">
                    <Icon name="book" size={15} />
                    <h4>Reading-Source & Physical Ownership Separation (Section 10.3)</h4>
                  </div>
                  <div class="separation-grid">
                    <div class="sep-stat-box">
                      <span class="stat-num">{timeline.readingSourceSeparation.canonicalKomgaCount}</span>
                      <span class="stat-lbl">Canonical Komga Volumes</span>
                      <span class="stat-desc">Physically published in canonical manga directory</span>
                    </div>
                    <div class="sep-stat-box">
                      <span class="stat-num">{timeline.readingSourceSeparation.localStagingCount}</span>
                      <span class="stat-lbl">Local Download Staging</span>
                      <span class="stat-desc">Historical/incoming download files in downloads directory</span>
                    </div>
                    <div class="sep-stat-box">
                      <span class="stat-num">{timeline.readingSourceSeparation.readerChapterCount}</span>
                      <span class="stat-lbl">Reading Extension Chapters</span>
                      <span class="stat-desc">External reader cache; strictly separate from volume ownership</span>
                    </div>
                  </div>

                  {#if timeline.readingSourceSeparation.storageDistinctionNotes.length > 0}
                    <div class="separation-notes">
                      {#each timeline.readingSourceSeparation.storageDistinctionNotes as note}
                        <div class="note-item">
                          <Icon name="info" size={13} />
                          <span>{note}</span>
                        </div>
                      {/each}
                    </div>
                  {/if}
                </div>

                <!-- Event Timeline Sub-Section (M6.12 Section 10.5) -->
                {#if timeline.events.length > 0}
                  <div class="timeline-events-subcard">
                    <div class="subcard-header">
                      <Icon name="clock" size={15} />
                      <h4>Notification & Lifecycle Event History (Section 10.5)</h4>
                    </div>
                    <div class="events-scroll-container">
                      <table class="events-table">
                        <thead>
                          <tr>
                            <th>Time</th>
                            <th>Event</th>
                            <th>Volume</th>
                            <th>Message</th>
                            <th>Status</th>
                          </tr>
                        </thead>
                        <tbody>
                          {#each timeline.events as ev}
                            <tr>
                              <td class="time-cell">{ago(ev.createdAt * 1000)}</td>
                              <td><span class="tag event-tag">{ev.eventType}</span></td>
                              <td>{ev.volumeNumber != null ? `Vol ${ev.volumeNumber}` : '—'}</td>
                              <td class="msg-cell">{ev.message}</td>
                              <td>
                                {#if ev.resolvedAt}
                                  <span class="badge badge-resolved">Resolved</span>
                                {:else}
                                  <span class="badge badge-active">Active</span>
                                {/if}
                              </td>
                            </tr>
                          {/each}
                        </tbody>
                      </table>
                    </div>
                  </div>
                {/if}
              {:else}
                <div class="empty-state font-small muted">
                  Timeline data unavailable.
                </div>
              {/if}
            </div>
          </div>

          <!-- Grid: Identity & MangaBaka Collection -->
          <div class="diag-grid">
            <!-- 1. Identity & Provenance -->
            <div class="diag-card">
              <div class="card-head">
                <Icon name="link" size={16} />
                <h3>Series Identity & Provenance</h3>
              </div>
              <div class="card-content">
                <div class="diag-field">
                  <span class="field-k">Mango Title ID:</span>
                  <span class="field-v">#{diag.identity.mangaId} ({diag.identity.sourceId === 0 ? 'Catalog Native' : `Source #${diag.identity.sourceId}`})</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">MangaBaka ID:</span>
                  <span class="field-v">
                    {#if diag.identity.mangabakaId}
                      <strong>{diag.identity.mangabakaId}</strong> {diag.identity.mangabakaTitle ? `(${diag.identity.mangabakaTitle})` : ''}
                      {#if diag.identity.manualOverride}
                        <span class="badge manual-badge">MANUAL OVERRIDE</span>
                      {/if}
                    {:else}
                      <span class="muted">Unlinked</span>
                    {/if}
                  </span>
                </div>
                <div class="diag-field">
                  <span class="field-k">AniList Tracker ID:</span>
                  <span class="field-v">{diag.identity.anilistId ? `#${diag.identity.anilistId}` : 'Not Linked'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Komga Series ID:</span>
                  <span class="field-v">{diag.identity.komgaSeriesId ? `<code>${diag.identity.komgaSeriesId}</code>` : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Identity Source:</span>
                  <span class="field-v"><code>{diag.identity.identitySource}</code></span>
                </div>
              </div>
              <div class="card-actions">
                <button
                  class="btn mini outline icon-text"
                  onclick={() => (showManualSection = !showManualSection)}
                >
                  <Icon name="sliders" size={14} />
                  {showManualSection ? 'Hide Resolution Controls' : 'Manual Identity Resolution'}
                </button>
                {#if diag.identity.manualOverride}
                  <button
                    class="btn mini warn icon-text"
                    disabled={savingManual}
                    onclick={handleClearManualMapping}
                    title="Clear manual override and revert to automatic resolution"
                  >
                    Revert to Auto
                  </button>
                {/if}
              </div>
            </div>

            <!-- 2. MangaBaka Collection -->
            <div class="diag-card">
              <div class="card-head">
                <Icon name="book" size={16} />
                <h3>MangaBaka Collection & Edition</h3>
              </div>
              <div class="card-content">
                <div class="diag-field">
                  <span class="field-k">Collection Status:</span>
                  <span class="field-v tag status-{diag.collection.collectionStatus}">
                    {diag.collection.collectionStatus}
                  </span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Resolved Collection:</span>
                  <span class="field-v">{diag.collection.resolvedCollectionId ? `<code>${diag.collection.resolvedCollectionId}</code>` : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Market & Edition:</span>
                  <span class="field-v">{diag.collection.market.toUpperCase()} · {diag.collection.edition}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Total Official Volumes:</span>
                  <span class="field-v">{diag.collection.totalVolumes != null ? diag.collection.totalVolumes : 'Unknown'}</span>
                </div>
                {#if diag.collection.reason}
                  <div class="diag-field">
                    <span class="field-k">Review Reason:</span>
                    <span class="field-v warning-text">{diag.collection.reason}</span>
                  </div>
                {/if}

                {#if titleMbCollectionsData}
                  {#if titleMbCollectionsData.status === 'collection_unavailable'}
                    <div class="edition-notice notice-info font-small">
                      <Icon name="info" size={14} />
                      <span>Collection Unavailable: MangaBaka has not published an edition collection for this title. Physical volumes are tracked canonically via Komga without fabricating fake volume records.</span>
                    </div>
                  {:else if titleMbCollectionsData.status === 'ambiguous'}
                    <div class="edition-notice notice-warning font-small">
                      <Icon name="info" size={14} />
                      <span>Multiple Collections ({titleMbCollectionsData.collections.length}): Please select the authoritative edition:</span>
                      <div class="quick-collections-list">
                        {#each titleMbCollectionsData.collections as col}
                          <div class="quick-col-item">
                            <span><strong>{col.name}</strong> ({col.language || 'en'}) — {col.totalWorks ? `${col.totalWorks} vols` : 'unknown'}</span>
                            <button class="btn mini spot" onclick={() => handleSelectTitleCollection(col.id, col.edition)}>Select</button>
                          </div>
                        {/each}
                      </div>
                    </div>
                  {/if}
                {/if}
              </div>
            </div>
          </div>

          <!-- Manual Resolution Controls Panel (Expandable) -->
          {#if showManualSection}
            <div class="manual-resolution-panel">
              <div class="panel-head">
                <Icon name="sliders" size={16} />
                <strong>Manual Identity & Collection Resolution Controls</strong>
              </div>
              <p class="panel-desc">
                Sets a durable, auditable manual mapping override that will never be replaced by automatic resolution.
              </p>

              <!-- Search / Select MangaBaka Title -->
              <div class="search-mb-row">
                <label class="field grow">
                  <span class="lbl">Search MangaBaka Catalog:</span>
                  <div class="input-with-btn">
                    <input
                      type="text"
                      bind:value={manualSearchQuery}
                      placeholder="Search title by name on MangaBaka…"
                      onkeydown={(e) => e.key === 'Enter' && searchMangaBaka()}
                    />
                    <button class="btn small" disabled={searchingMb || !manualSearchQuery.trim()} onclick={searchMangaBaka}>
                      {searchingMb ? 'Searching…' : 'Search'}
                    </button>
                  </div>
                </label>
              </div>

              {#if searchResults.length > 0}
                <div class="search-results-dropdown">
                  {#each searchResults as item}
                    <!-- svelte-ignore a11y_click_events_have_key_events -->
                    <div class="search-item" onclick={() => selectSearchResult(item)}>
                      <strong>{item.title}</strong>
                      <span class="item-meta">MangaBaka #{item.id} {item.year ? `(${item.year})` : ''}</span>
                    </div>
                  {/each}
                </div>
              {/if}

              <div class="manual-inputs-grid">
                <label class="field">
                  <span class="lbl">MangaBaka Series ID:</span>
                  <input
                    type="text"
                    bind:value={manualMbIdInput}
                    placeholder="e.g. 708"
                    onchange={() => {
                      const idNum = parseInt(manualMbIdInput.trim(), 10);
                      if (!isNaN(idNum)) loadCollections(idNum);
                    }}
                  />
                </label>

                <label class="field">
                  <span class="lbl">Edition:</span>
                  <input type="text" bind:value={manualEditionInput} placeholder="e.g. standard" />
                </label>

                <label class="field">
                  <span class="lbl">Collection / Format:</span>
                  {#if availableCollections.length > 0}
                    <select bind:value={manualCollectionId}>
                      {#each availableCollections as col}
                        <option value={col.id}>{col.name} ({col.language || 'en'}) — {col.totalWorks ? `${col.totalWorks} vols` : 'unknown'}</option>
                      {/each}
                    </select>
                  {:else}
                    <input
                      type="text"
                      bind:value={manualCollectionId}
                      placeholder={loadingCollections ? 'Loading collections…' : 'Collection ID or UUID'}
                    />
                  {/if}
                </label>
              </div>

              <div class="panel-foot">
                <button
                  class="btn spot icon-text"
                  disabled={savingManual || !manualMbIdInput.trim()}
                  onclick={handleApplyManualMapping}
                >
                  <Icon name="check" size={14} />
                  {savingManual ? 'Saving Override…' : 'Apply Manual Override'}
                </button>
              </div>
            </div>
          {/if}

          <!-- Grid: Physical Ownership & OpenTome Intelligence -->
          <div class="diag-grid">
            <!-- 3. Physical Ownership Accounting -->
            <div class="diag-card">
              <div class="card-head">
                <Icon name="bookmark" size={16} />
                <h3>Physical Ownership Breakdown</h3>
              </div>
              <div class="card-content">
                <div class="diag-field">
                  <span class="field-k">Total Owned Count:</span>
                  <span class="field-v"><strong>{diag.ownership.totalOwnedCount} volumes</strong></span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Komga Owned Volumes:</span>
                  <span class="field-v">{diag.ownership.komgaOwnedVolumes.length ? diag.ownership.komgaOwnedVolumes.join(', ') : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Mango Local Downloads:</span>
                  <span class="field-v">{diag.ownership.mangoLocalOwnedVolumes.length ? diag.ownership.mangoLocalOwnedVolumes.join(', ') : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Local DB Query:</span>
                  <span class="field-v">{diag.ownership.localQuerySkipped ? 'Skipped (manga_id is NULL)' : 'Queried'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Missing Released:</span>
                  <span class="field-v {diag.ownership.missingVolumes.length ? 'warning-text' : 'success-text'}">
                    {diag.ownership.missingVolumes.length ? diag.ownership.missingVolumes.join(', ') : 'None (Up to Date)'}
                  </span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Future Scheduled:</span>
                  <span class="field-v">{diag.ownership.futureVolumes.length ? diag.ownership.futureVolumes.join(', ') : 'None'}</span>
                </div>
              </div>
            </div>

            <!-- 4. OpenTome Release Intelligence -->
            <div class="diag-card">
              <div class="card-head">
                <Icon name="clock" size={16} />
                <h3>OpenTome Release Intelligence</h3>
              </div>
              <div class="card-content">
                <div class="diag-field">
                  <span class="field-k">Tracked Schedule:</span>
                  <span class="field-v">{diag.releaseIntelligence.opentomeTracked ? 'Active' : 'Unmapped'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">OpenTome Work ID:</span>
                  <span class="field-v">{diag.releaseIntelligence.workId ? `<code>${diag.releaseIntelligence.workId}</code>` : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Mapping Confidence:</span>
                  <span class="field-v">{diag.releaseIntelligence.mappingConfidence || 'N/A'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Next Expected Release:</span>
                  <span class="field-v">
                    {#if diag.releaseIntelligence.nextReleaseVolume}
                      <strong>Volume {diag.releaseIntelligence.nextReleaseVolume}</strong> ({diag.releaseIntelligence.nextReleaseDate || 'No date'})
                    {:else}
                      None scheduled
                    {/if}
                  </span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Release Discovery Targets:</span>
                  <span class="field-v">{diag.releaseIntelligence.totalReleaseTargets} targets tracked</span>
                </div>

                {#if openTomeLinesData && openTomeLinesData.lines.length > 0}
                  <div class="diag-field">
                    <span class="field-k">Release Line:</span>
                    <span class="field-v">
                      <strong>{openTomeLinesData.lines.find(l => l.gcdSeriesId === openTomeLinesData?.currentGcdSeriesId)?.name || 'Default English Line'}</strong>
                    </span>
                  </div>
                  <div class="line-selector-row">
                    <button
                      class="btn mini outline icon-text"
                      onclick={() => showOpenTomeLineSelector = !showOpenTomeLineSelector}
                    >
                      <Icon name="sliders" size={13} />
                      {showOpenTomeLineSelector ? 'Hide Release Lines' : `Change Release Line (${openTomeLinesData.lines.length})`}
                    </button>
                  </div>

                  {#if showOpenTomeLineSelector}
                    <div class="opentome-lines-dropdown">
                      {#each openTomeLinesData.lines as line}
                        <div class="ot-line-item" class:current={line.gcdSeriesId === openTomeLinesData.currentGcdSeriesId}>
                          <div class="line-info">
                            <strong>{line.name}</strong>
                            <span class="muted font-small">({line.language.toUpperCase()}) · {line.volumeCount} vols · {line.publisher || 'Unknown pub'}</span>
                          </div>
                          {#if line.gcdSeriesId === openTomeLinesData.currentGcdSeriesId}
                            <span class="pill-badge active">Active Line</span>
                          {:else}
                            <button
                              class="btn mini"
                              disabled={selectingOpenTomeLine}
                              onclick={() => handleSelectOpenTomeLine(line.gcdSeriesId)}
                            >
                              Select Line
                            </button>
                          {/if}
                        </div>
                      {/each}
                    </div>
                  {/if}
                {/if}
              </div>
            </div>
          </div>

          <!-- Active Reconciliation Event if present -->
          {#if diag.reconciliation.activeEvent}
            <div class="active-event-card">
              <div class="event-head">
                <Icon name="bell" size={16} />
                <strong>Active Attention Event: {diag.reconciliation.activeEvent.eventType}</strong>
                <span class="event-time">{ago(diag.reconciliation.activeEvent.createdAt * 1000)}</span>
              </div>
              <p class="event-msg">{diag.reconciliation.activeEvent.message}</p>
              {#if diag.reconciliation.activeEvent.outboxStatus}
                <div class="event-outbox-info">
                  Outbox status: <code>{diag.reconciliation.activeEvent.outboxStatus}</code>
                  {diag.reconciliation.activeEvent.outboxAttempts ? `(${diag.reconciliation.activeEvent.outboxAttempts} delivery attempts)` : ''}
                </div>
              {/if}
            </div>
          {/if}

          <!-- 5. Release Availability Watchers (M6.8.4) -->
          <div class="diag-card watcher-section">
            <div class="card-head">
              <div class="head-title">
                <Icon name="search" size={16} />
                <h3>Release Availability Watchers (M6.8.4)</h3>
              </div>
              <div class="card-actions">
                <button
                  class="btn mini outline icon-text"
                  disabled={refreshingAvailability}
                  onclick={handleRefreshAvailability}
                  title="Trigger immediate availability check for missing/upcoming volumes"
                >
                  <Icon name="refresh" size={14} />
                  {refreshingAvailability ? 'Checking…' : 'Check Availability'}
                </button>
              </div>
            </div>
            <div class="card-content">
              <div class="watcher-controls-bar">
                <div class="watcher-status-pill">
                  <span class="control-lbl">Monitoring:</span>
                  <button
                    class="btn mini pill"
                    class:spot={availability?.monitoringEnabled}
                    disabled={updatingAvailability}
                    onclick={handleToggleMonitoring}
                  >
                    {availability?.monitoringEnabled ? 'Enabled' : 'Disabled'}
                  </button>
                </div>
                <div class="watcher-status-pill">
                  <span class="control-lbl">Notifications:</span>
                  <button
                    class="btn mini pill"
                    class:spot={availability?.notifyEnabled}
                    disabled={updatingAvailability}
                    onclick={handleToggleNotify}
                  >
                    {availability?.notifyEnabled ? 'Enabled' : 'Disabled'}
                  </button>
                </div>
                <div class="watcher-status-pill">
                  <span class="control-lbl">Active Watchers:</span>
                  <strong>{availability?.activeWatchersCount ?? 0} volumes</strong>
                </div>
              </div>

              {#if availability && availability.watchers.length > 0}
                <div class="watchers-table-wrapper">
                  <table class="watchers-table">
                    <thead>
                      <tr>
                        <th>Volume</th>
                        <th>Release Date</th>
                        <th>State</th>
                        <th>Provider</th>
                        <th>Interval</th>
                        <th>Next Check</th>
                        <th>Result / Note</th>
                      </tr>
                    </thead>
                    <tbody>
                      {#each availability.watchers as w}
                        <tr class="watcher-row">
                          <td class="vol-cell"><strong>Vol {w.volumeNumber}</strong></td>
                          <td class="date-cell">{w.releaseDate || 'Unknown'}</td>
                          <td>
                            <span class="tag watcher-state watcher-state-{w.state.toLowerCase()}">
                              {w.state}
                            </span>
                          </td>
                          <td class="provider-cell"><code>{w.preferredProvider}</code></td>
                          <td class="interval-cell">{Math.round(w.checkIntervalSecs / 3600)}h</td>
                          <td class="next-cell">
                            {#if w.nextCheckAt > 0}
                              {new Date(w.nextCheckAt * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                            {:else}
                              <span class="muted">—</span>
                            {/if}
                          </td>
                          <td class="result-cell">
                            {#if w.foundCandidateId}
                              <span class="found-text">Found ({w.foundProvider}): {w.foundReleaseTitle || w.foundCandidateId}</span>
                            {:else if w.stopReason}
                              <span class="stop-text">{w.stopReason}</span>
                            {:else if w.lastError}
                              <span class="error-text">{w.lastError}</span>
                            {:else}
                              <span class="muted">Awaiting release</span>
                            {/if}
                          </td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {:else}
                <div class="no-watchers-msg">
                  <p class="muted">No unowned volumes actively monitored. All released volumes are owned or reconciled.</p>
                </div>
              {/if}
            </div>
          </div>

          <!-- 6. Durable Publication Receipts (M6.9) -->
          <div class="diag-card receipts-section">
            <div class="card-head">
              <div class="head-title">
                <Icon name="bookmark" size={16} />
                <h3>Durable Publication Receipts (M6.9)</h3>
              </div>
              <div class="card-actions">
                <span class="receipt-count-pill">{receipts.length} verified publications</span>
              </div>
            </div>
            <div class="card-content">
              {#if receipts.length > 0}
                <div class="watchers-table-wrapper">
                  <table class="watchers-table">
                    <thead>
                      <tr>
                        <th>Unit</th>
                        <th>Canonical Path</th>
                        <th>Size</th>
                        <th>SHA-256</th>
                        <th>Metadata</th>
                        <th>Komga</th>
                        <th>Actions</th>
                      </tr>
                    </thead>
                    <tbody>
                      {#each receipts as r}
                        <tr class="receipt-row">
                          <td class="vol-cell">
                            <strong>{r.isVolume ? `Vol ${r.volumeNumber}` : `Ch ${r.chapterNumber}`}</strong>
                          </td>
                          <td class="path-cell" title={r.canonicalPath}>
                            <code>{r.canonicalPath.split('/').pop() || r.canonicalPath}</code>
                          </td>
                          <td class="size-cell">
                            {(r.fileSizeBytes / (1024 * 1024)).toFixed(1)} MB
                          </td>
                          <td class="sha-cell" title={r.fileSha256}>
                            <code>{r.fileSha256.slice(0, 8)}…</code>
                          </td>
                          <td>
                            <span class="tag metadata-state metadata-state-{r.metadataStatus.toLowerCase()}">
                              {r.metadataStatus}
                            </span>
                          </td>
                          <td>
                            <span class="tag komga-notif-state" class:spot={r.komgaNotified}>
                              {r.komgaNotified ? 'Notified' : 'Pending'}
                            </span>
                          </td>
                          <td class="receipt-actions-cell">
                            <button
                              class="btn mini outline"
                              disabled={verifyingReceiptId === r.id}
                              onclick={() => handleVerifyReceipt(r)}
                              title="Verify CBZ integrity, size, and SHA-256 on disk"
                            >
                              {verifyingReceiptId === r.id ? 'Verifying…' : 'Verify'}
                            </button>
                            <button
                              class="btn mini outline"
                              disabled={rescanningReceiptId === r.id}
                              onclick={() => handleRescanReceipt(r)}
                              title="Trigger Komga library rescan for this publication"
                            >
                              {rescanningReceiptId === r.id ? 'Scanning…' : 'Rescan'}
                            </button>
                            {#if r.metadataStatus === 'failed'}
                              <button
                                class="btn mini spot"
                                disabled={repairingReceiptId === r.id}
                                onclick={() => handleRepairReceiptMetadata(r)}
                                title="Repair ComicInfo.xml metadata without re-downloading archive"
                              >
                                {repairingReceiptId === r.id ? 'Repairing…' : 'Repair Meta'}
                              </button>
                            {/if}
                          </td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {:else}
                <div class="no-receipts-msg">
                  <p class="muted">No publication receipts recorded for this series yet.</p>
                </div>
              {/if}
            </div>

            <!-- Guarded Historical Backlog Card (M6.10) -->
            <div class="diag-card backlog-section">
              <div class="card-head">
                <span class="card-title">Guarded Historical Backlog (M6.10)</span>
                <span class="backlog-count-pill">{backlogJobs.length} job(s) in audit trail</span>
              </div>
              <p class="muted card-desc">
                Controlled, bounded acquisition for past missing volumes. Respects Komga ownership, MangaBaka limits, and defaults to dry-run verification.
              </p>

              <!-- Scope and Controls -->
              <div class="backlog-controls-grid">
                <div class="form-group">
                  <label for="backlog-scope-select">Acquisition Scope</label>
                  <select id="backlog-scope-select" bind:value={backlogScope} class="input select">
                    <option value="ALL_MISSING">All Missing Released Volumes</option>
                    <option value="RANGE">Volume Range</option>
                    <option value="EXPLICIT_VOLUMES">Explicit Volumes</option>
                  </select>
                </div>

                {#if backlogScope === 'RANGE'}
                  <div class="form-group range-group">
                    <label for="backlog-range-start">Range (Start – End)</label>
                    <div class="range-inputs">
                      <input id="backlog-range-start" type="number" min="1" placeholder="Start" bind:value={backlogRangeStart} class="input" />
                      <span class="range-sep">–</span>
                      <input id="backlog-range-end" type="number" min="1" placeholder="End" bind:value={backlogRangeEnd} class="input" />
                    </div>
                  </div>
                {:else if backlogScope === 'EXPLICIT_VOLUMES'}
                  <div class="form-group">
                    <label for="backlog-explicit-input">Volume Numbers (comma-separated)</label>
                    <input id="backlog-explicit-input" type="text" placeholder="e.g. 1, 2, 3, 5" bind:value={backlogExplicitInput} class="input" />
                  </div>
                {/if}

                <div class="form-group">
                  <label for="backlog-batch-size">Batch Size (1–20)</label>
                  <input id="backlog-batch-size" type="number" min="1" max="20" bind:value={backlogBatchSize} class="input" />
                </div>

                <div class="form-group checkbox-group">
                  <label class="checkbox-label">
                    <input type="checkbox" bind:checked={backlogDryRun} />
                    <span>Dry-run verification only (Safe, recommended)</span>
                  </label>
                </div>

                <div class="form-group plan-btn-group">
                  <button
                    class="btn spot"
                    disabled={planningBacklog}
                    onclick={handlePlanBacklog}
                  >
                    {planningBacklog ? 'Analyzing Backlog…' : 'Plan Backlog Batch'}
                  </button>
                </div>
              </div>

              <!-- Planned Batch Preview -->
              {#if plannedJob}
                <div class="planned-job-box">
                  <div class="planned-job-header">
                    <h4>Planned Batch: {plannedJob.id}</h4>
                    <div class="planned-job-summary-badges">
                      <span class="badge info">{plannedJob.totalRequested} requested</span>
                      <span class="badge ok">{plannedJob.totalEligible} eligible</span>
                      <span class="badge muted">{plannedJob.totalAlreadyOwned} already owned</span>
                      {#if plannedJob.isDryRun}
                        <span class="badge warn">Dry-Run Only</span>
                      {/if}
                    </div>
                  </div>

                  {#if plannedJob.items.length > 0}
                    <div class="table-scroll-container">
                      <table class="receipts-table">
                        <thead>
                          <tr>
                            <th>Volume</th>
                            <th>Status</th>
                            <th>Provider</th>
                            <th>Candidate</th>
                            <th>Reason / Confidence</th>
                          </tr>
                        </thead>
                        <tbody>
                          {#each plannedJob.items as item}
                            <tr>
                              <td class="code">Vol. {item.volumeNumber}</td>
                              <td>
                                <span class="badge {item.status === 'ELIGIBLE' ? 'ok' : item.status === 'ALREADY_OWNED' ? 'muted' : 'warn'}">
                                  {item.status}
                                </span>
                              </td>
                              <td class="code">{item.provider ?? '—'}</td>
                              <td class="candidate-title-cell" title={item.candidateTitle ?? ''}>
                                {item.candidateTitle ?? '—'}
                              </td>
                              <td class="reason-cell">{item.reason}</td>
                            </tr>
                          {/each}
                        </tbody>
                      </table>
                    </div>
                  {/if}

                  <div class="planned-job-actions">
                    <button
                      class="btn {plannedJob.isDryRun ? 'spot' : 'warn'}"
                      disabled={executingBacklog || plannedJob.totalEligible === 0}
                      onclick={() => plannedJob && handleExecuteBacklog(plannedJob)}
                    >
                      {executingBacklog ? 'Executing…' : plannedJob.isDryRun ? 'Execute Dry-Run Verification' : 'Execute Real Acquisition Batch'}
                    </button>
                    <button
                      class="btn"
                      disabled={executingBacklog}
                      onclick={() => (plannedJob = null)}
                    >
                      Dismiss Plan
                    </button>
                  </div>
                </div>
              {/if}

              <!-- Audit Trail -->
              {#if backlogJobs.length > 0}
                <div class="audit-trail-container">
                  <h4 class="audit-title">Backlog Job Audit Trail</h4>
                  <div class="table-scroll-container">
                    <table class="receipts-table">
                      <thead>
                        <tr>
                          <th>Job ID</th>
                          <th>Scope</th>
                          <th>Batch</th>
                          <th>Mode</th>
                          <th>State</th>
                          <th>Queued / Failed</th>
                          <th>Created</th>
                          <th>Actions</th>
                        </tr>
                      </thead>
                      <tbody>
                        {#each backlogJobs as j}
                          <tr>
                            <td class="code mono-sm">{j.id}</td>
                            <td>{j.scope}</td>
                            <td>{j.batchSize}</td>
                            <td>
                              <span class="badge {j.isDryRun ? 'info' : 'warn'}">
                                {j.isDryRun ? 'Dry Run' : 'Real'}
                              </span>
                            </td>
                            <td>
                              <span class="badge {j.state === 'COMPLETED' ? 'ok' : j.state === 'PLANNED' ? 'warn' : 'muted'}">
                                {j.state}
                              </span>
                            </td>
                            <td>{j.totalQueued} / {j.totalFailed}</td>
                            <td>{ago(j.createdAt)}</td>
                            <td>
                              {#if j.state === 'PLANNED'}
                                <button
                                  class="btn mini warn"
                                  onclick={() => handleCancelJob(j.id)}
                                >
                                  Cancel
                                </button>
                              {:else}
                                <span class="muted">—</span>
                              {/if}
                            </td>
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                </div>
              {/if}
            </div>
          </div>

        {/if}
      </div>

      <div class="modal-foot">
        <button class="btn" onclick={onClose}>Close</button>
      </div>
    </div>
  </div>

  {#if inspectingVolume !== null}
    <CandidateInspectorModal
      open={inspectingVolume !== null}
      mangaId={mangaId}
      volume={inspectingVolume}
      onClose={() => { inspectingVolume = null; }}
    />
  {/if}
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.65);
    backdrop-filter: blur(4px);
    z-index: 9999;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1rem;
  }

  .diag-modal {
    background: var(--bg-surface, #1e1e24);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
    border-radius: 12px;
    width: 100%;
    max-width: 900px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    box-shadow: var(--sh-4, 0 16px 32px rgba(0, 0, 0, 0.5));
    overflow: hidden;
  }

  .modal-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  }

  .head-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .head-title h2 {
    margin: 0;
    font-size: 1.15rem;
    font-weight: 600;
  }

  .modal-body {
    padding: 1.25rem;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
  }

  .modal-foot {
    display: flex;
    justify-content: flex-end;
    padding: 0.75rem 1.25rem;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    background: var(--bg-surface-subtle, rgba(255, 255, 255, 0.02));
  }

  .gate-eval-banner {
    background: var(--bg-card, rgba(255, 255, 255, 0.04));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .gate-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .gate-badge {
    background: #3b82f6;
    color: #fff;
    font-size: 0.75rem;
    font-weight: 700;
    padding: 0.2rem 0.6rem;
    border-radius: 4px;
    letter-spacing: 0.04em;
  }

  .gate-decision-text {
    font-weight: 600;
    font-size: 0.95rem;
  }

  .gate-matrix {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 0.5rem;
    background: rgba(0, 0, 0, 0.2);
    padding: 0.5rem;
    border-radius: 6px;
  }

  .matrix-item {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    padding: 0.4rem;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.02);
  }

  .matrix-item.passed {
    border-left: 3px solid #10b981;
  }

  .matrix-item:not(.passed):not(.info) {
    border-left: 3px solid #f59e0b;
  }

  .matrix-item.info {
    border-left: 3px solid #8b5cf6;
  }

  .matrix-lbl {
    font-size: 0.7rem;
    color: var(--muted, #8b949e);
    text-transform: uppercase;
  }

  .matrix-val {
    font-size: 0.8rem;
    font-weight: 600;
  }

  .conflict-alert {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.3);
    border-radius: 8px;
    padding: 0.85rem 1rem;
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    color: #f87171;
  }

  .conflict-content ul {
    margin: 0.25rem 0 0;
    padding-left: 1.25rem;
    font-size: 0.85rem;
  }

  .diag-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
    gap: 1rem;
  }

  .diag-card {
    background: var(--bg-card, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.06));
    padding-bottom: 0.5rem;
  }

  .card-head h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 600;
  }

  .card-content {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }

  .diag-field {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 0.75rem;
    font-size: 0.85rem;
  }

  .field-k {
    color: var(--muted, #8b949e);
    flex-shrink: 0;
  }

  .field-v {
    text-align: right;
    word-break: break-all;
  }

  .manual-badge {
    background: rgba(139, 92, 246, 0.2);
    color: #c4b5fd;
    font-size: 0.7rem;
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
    margin-left: 0.3rem;
  }

  .card-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: auto;
    padding-top: 0.5rem;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.04));
  }

  .manual-resolution-panel {
    background: rgba(59, 130, 246, 0.05);
    border: 1px solid rgba(59, 130, 246, 0.2);
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .panel-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.95rem;
  }

  .panel-desc {
    margin: 0;
    font-size: 0.8rem;
    color: var(--muted, #8b949e);
  }

  .input-with-btn {
    display: flex;
    gap: 0.4rem;
  }

  .input-with-btn input {
    flex-grow: 1;
  }

  .search-results-dropdown {
    background: var(--bg-surface, #1e1e24);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.15));
    border-radius: 6px;
    max-height: 180px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }

  .search-item {
    padding: 0.5rem 0.75rem;
    cursor: pointer;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.05));
    display: flex;
    justify-content: space-between;
    font-size: 0.85rem;
  }

  .search-item:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .search-item .item-meta {
    color: var(--muted, #8b949e);
  }

  .manual-inputs-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 0.75rem;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .field .lbl {
    font-size: 0.75rem;
    color: var(--muted, #8b949e);
  }

  .active-event-card {
    background: rgba(245, 158, 11, 0.08);
    border: 1px solid rgba(245, 158, 11, 0.25);
    border-radius: 8px;
    padding: 0.85rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  .event-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
    color: #fde68a;
  }

  .event-time {
    margin-left: auto;
    color: var(--muted, #8b949e);
    font-size: 0.75rem;
  }

  .event-msg {
    margin: 0;
    font-size: 0.85rem;
  }

  .event-outbox-info {
    font-size: 0.75rem;
    color: var(--muted, #8b949e);
  }

  .warning-text {
    color: #fca5a5;
  }

  .success-text {
    color: #86efac;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 3rem;
    gap: 0.75rem;
    color: var(--muted, #8b949e);
  }

  .watcher-section {
    grid-column: 1 / -1;
  }

  .watcher-controls-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 1.25rem;
    padding-bottom: 0.75rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    margin-bottom: 0.75rem;
  }

  .watcher-status-pill {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
  }

  .control-lbl {
    color: var(--muted, #8b949e);
  }

  .watchers-table-wrapper {
    overflow-x: auto;
  }

  .watchers-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.82rem;
    text-align: left;
  }

  .watchers-table th {
    padding: 0.4rem 0.6rem;
    font-weight: 600;
    color: var(--muted, #8b949e);
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  }

  .watchers-table td {
    padding: 0.5rem 0.6rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.04));
    vertical-align: middle;
  }

  .watcher-state {
    font-size: 0.7rem;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: 4px;
    text-transform: uppercase;
  }

  .watcher-state-polling {
    background: rgba(59, 130, 246, 0.2);
    color: #93c5fd;
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .watcher-state-candidate_found {
    background: rgba(34, 197, 94, 0.2);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .watcher-state-ownership_detected {
    background: rgba(168, 85, 247, 0.2);
    color: #d8b4fe;
    border: 1px solid rgba(168, 85, 247, 0.3);
  }

  .watcher-state-stopped {
    background: rgba(107, 114, 128, 0.2);
    color: #9ca3af;
    border: 1px solid rgba(107, 114, 128, 0.3);
  }

  .watcher-state-failed_permanent {
    background: rgba(239, 68, 68, 0.2);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .found-text {
    color: #86efac;
    font-size: 0.78rem;
  }

  .stop-text {
    color: var(--muted, #8b949e);
    font-size: 0.78rem;
  }

  .error-text {
    color: #fca5a5;
    font-size: 0.78rem;
  }

  .no-watchers-msg {
    padding: 0.75rem 0;
  }

  .receipts-section {
    grid-column: 1 / -1;
  }

  .receipt-count-pill {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--muted, #8b949e);
  }

  .receipt-actions-cell {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .metadata-state-complete {
    background: rgba(34, 197, 94, 0.2);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .metadata-state-failed {
    background: rgba(239, 68, 68, 0.2);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .metadata-state-pending {
    background: rgba(245, 158, 11, 0.2);
    color: #fde68a;
    border: 1px solid rgba(245, 158, 11, 0.3);
  }

  .no-receipts-msg {
    padding: 0.75rem 0;
  }

  .backlog-section {
    grid-column: 1 / -1;
  }

  .backlog-count-pill {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--muted, #8b949e);
  }

  .backlog-controls-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 12px;
    align-items: flex-end;
    margin: 12px 0 16px;
    background: rgba(255, 255, 255, 0.02);
    padding: 12px;
    border-radius: 8px;
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.06));
  }

  .range-inputs {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .range-sep {
    color: var(--muted, #8b949e);
  }

  .checkbox-group {
    padding-bottom: 8px;
  }

  .checkbox-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.85rem;
    cursor: pointer;
  }

  .plan-btn-group {
    display: flex;
    align-items: flex-end;
  }

  .planned-job-box {
    background: rgba(59, 130, 246, 0.05);
    border: 1px solid rgba(59, 130, 246, 0.2);
    border-radius: 8px;
    padding: 14px;
    margin-bottom: 16px;
  }

  .planned-job-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }

  .planned-job-summary-badges {
    display: flex;
    gap: 6px;
  }

  .planned-job-actions {
    display: flex;
    gap: 8px;
    margin-top: 14px;
  }

  .audit-trail-container {
    margin-top: 16px;
  }

  .audit-title {
    font-size: 0.9rem;
    font-weight: 600;
    margin-bottom: 8px;
  }

  .mono-sm {
    font-size: 0.75rem;
  }

  /* M6.12 Unified Series Timeline & Gap Detection Styles */
  .timeline-section {
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.1));
    margin-bottom: 16px;
  }

  .timeline-summary-pill {
    background: rgba(59, 130, 246, 0.15);
    border: 1px solid rgba(59, 130, 246, 0.35);
    color: #60a5fa;
    font-size: 0.78rem;
    padding: 3px 8px;
    border-radius: 4px;
    font-weight: 600;
  }

  .gap-status-banner {
    border-radius: 6px;
    padding: 10px 14px;
    margin-bottom: 14px;
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.1));
  }

  .gap-status-banner.banner-warning {
    background: rgba(234, 179, 8, 0.1);
    border-color: rgba(234, 179, 8, 0.35);
    color: #facc15;
  }

  .gap-status-banner.banner-healthy {
    background: rgba(34, 197, 94, 0.1);
    border-color: rgba(34, 197, 94, 0.35);
    color: #4ade80;
  }

  .gap-status-banner.banner-info {
    background: rgba(59, 130, 246, 0.1);
    border-color: rgba(59, 130, 246, 0.35);
    color: #60a5fa;
  }

  .gap-banner-content {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .gap-banner-text {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: 0.85rem;
  }

  .timeline-grid-wrapper {
    margin-bottom: 16px;
  }

  .timeline-volume-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
    gap: 10px;
  }

  .timeline-vol-card {
    background: var(--surface-1, rgba(255, 255, 255, 0.02));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .timeline-vol-card.is-owned {
    border-color: rgba(34, 197, 94, 0.3);
    background: rgba(34, 197, 94, 0.03);
  }

  .timeline-vol-card.is-gap {
    border-color: rgba(234, 179, 8, 0.3);
  }

  .vol-card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .vol-number {
    font-weight: 700;
    font-size: 0.9rem;
    color: var(--ink);
  }

  .vol-tag {
    font-size: 0.7rem;
    padding: 2px 6px;
    border-radius: 4px;
    text-transform: uppercase;
    font-weight: 600;
  }

  .vol-tag-owned {
    background: rgba(34, 197, 94, 0.15);
    color: #4ade80;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .vol-tag-candidate_available {
    background: rgba(168, 85, 247, 0.15);
    color: #c084fc;
    border: 1px solid rgba(168, 85, 247, 0.3);
  }

  .vol-tag-candidate_unavailable {
    background: rgba(239, 68, 68, 0.15);
    color: #f87171;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .vol-tag-acquiring {
    background: rgba(6, 182, 212, 0.15);
    color: #22d3ee;
    border: 1px solid rgba(6, 182, 212, 0.3);
  }

  .vol-tag-unreleased_future {
    background: rgba(59, 130, 246, 0.15);
    color: #60a5fa;
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .vol-tag-released_missing {
    background: rgba(249, 115, 22, 0.15);
    color: #fb923c;
    border: 1px solid rgba(249, 115, 22, 0.3);
  }

  .gap-tag-pill {
    align-self: flex-start;
    font-size: 0.68rem;
    padding: 1px 5px;
    border-radius: 3px;
    font-weight: 600;
  }

  .gap-tag-pill.gap-interior {
    background: rgba(239, 68, 68, 0.15);
    color: #f87171;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .gap-tag-pill.gap-leading {
    background: rgba(234, 179, 8, 0.15);
    color: #facc15;
    border: 1px solid rgba(234, 179, 8, 0.3);
  }

  .gap-tag-pill.gap-trailing {
    background: rgba(59, 130, 246, 0.15);
    color: #60a5fa;
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .vol-card-meta {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 0.78rem;
  }

  .cand-pill-box {
    display: flex;
    justify-content: space-between;
    background: var(--surface-2, rgba(255, 255, 255, 0.04));
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 0.72rem;
  }

  .cand-pill-provider {
    font-weight: 600;
    color: #c084fc;
  }

  .receipt-verified-badge {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 0.72rem;
    color: #4ade80;
  }

  .vol-card-actions {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: auto;
  }

  .vol-inspect-btn, .vol-plan-btn {
    width: 100%;
    justify-content: center;
    font-size: 0.75rem;
  }

  .reading-source-separation-box {
    background: var(--surface-2, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 12px;
    margin-bottom: 14px;
  }

  .subcard-header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 10px;
    font-size: 0.9rem;
    font-weight: 600;
  }

  .subcard-header h4 {
    margin: 0;
    font-size: 0.85rem;
  }

  .separation-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 8px;
    margin-bottom: 8px;
  }

  .sep-stat-box {
    background: var(--surface-1, rgba(255, 255, 255, 0.02));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.05));
    border-radius: 4px;
    padding: 8px 10px;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .stat-num {
    font-size: 1.1rem;
    font-weight: 700;
    color: var(--ink);
  }

  .stat-lbl {
    font-size: 0.78rem;
    font-weight: 600;
    color: var(--dim, #aaa);
  }

  .stat-desc {
    font-size: 0.72rem;
    color: var(--muted, #777);
  }

  .separation-notes {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 6px;
  }

  .note-item {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    font-size: 0.75rem;
    color: var(--dim, #ccc);
    background: rgba(255, 255, 255, 0.02);
    padding: 4px 8px;
    border-radius: 4px;
  }

  .timeline-events-subcard {
    background: var(--surface-2, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 12px;
  }

  .events-scroll-container {
    max-height: 200px;
    overflow-y: auto;
  }

  .events-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.78rem;
  }

  .events-table th, .events-table td {
    padding: 6px 8px;
    text-align: left;
    border-bottom: 1px solid var(--edge, rgba(255, 255, 255, 0.05));
  }

  .event-tag {
    font-size: 0.7rem;
    padding: 1px 5px;
    background: rgba(59, 130, 246, 0.15);
    color: #60a5fa;
    border-radius: 3px;
  }

  .badge-resolved {
    font-size: 0.68rem;
    background: rgba(34, 197, 94, 0.15);
    color: #4ade80;
    padding: 1px 5px;
    border-radius: 3px;
  }

  .badge-active {
    font-size: 0.68rem;
    background: rgba(234, 179, 8, 0.15);
    color: #facc15;
    padding: 1px 5px;
    border-radius: 3px;
  }

  /* MangaBaka Edition Resolution & OpenTome Lines (M7.3) */
  .edition-notice {
    padding: 8px 10px;
    border-radius: 6px;
    margin-top: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .edition-notice.notice-info {
    background: rgba(59, 130, 246, 0.1);
    border: 1px solid rgba(59, 130, 246, 0.25);
    color: #93c5fd;
  }

  .edition-notice.notice-warning {
    background: rgba(234, 179, 8, 0.1);
    border: 1px solid rgba(234, 179, 8, 0.25);
    color: #fde047;
  }

  .quick-collections-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
    margin-top: 4px;
  }

  .quick-col-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    background: rgba(0, 0, 0, 0.2);
    padding: 4px 8px;
    border-radius: 4px;
    gap: 8px;
  }

  .line-selector-row {
    margin-top: 6px;
  }

  .opentome-lines-dropdown {
    display: flex;
    flex-direction: column;
    gap: 6px;
    background: var(--surface-1, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--edge, rgba(255, 255, 255, 0.1));
    border-radius: 6px;
    padding: 8px;
    margin-top: 8px;
  }

  .ot-line-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 6px 8px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.02);
    gap: 8px;
  }

  .ot-line-item.current {
    border-left: 3px solid var(--spot, #ff7a00);
    background: rgba(255, 122, 0, 0.05);
  }

  .line-info {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
</style>
