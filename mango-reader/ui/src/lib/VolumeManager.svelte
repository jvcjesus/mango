<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from './Icon.svelte';
  import { toast } from './toast.svelte';
  import {
    getVolumeManager,
    planVolumeManagerBatch,
    acquireVolumeManagerBatch,
    getAcquisitionSettings,
    type VolumeManagerSummary,
    type VolumeManagerEntry,
    type VolumeManagerBatchPlanResponse,
    type VolumeManagerBatchAcquireResponse,
  } from './api';
  import { formatBytes } from './util';
  import {
    resolveVolumeCollectionState,
    calculateNextMissingVolume,
    type VolumeCollectionStateInfo,
  } from './volumeStatus';
  import LifecycleTimeline from './LifecycleTimeline.svelte';

  interface Props {
    mangaId: number;
    title: string;
    isCatalog?: boolean;
    onInspectCandidate?: (volume: number) => void;
    onAcquireCandidate?: (volume: number, candidateId: string) => void;
  }

  let {
    mangaId,
    title,
    isCatalog = true,
    onInspectCandidate,
    onAcquireCandidate,
  }: Props = $props();

  let summary = $state<VolumeManagerSummary | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let viewMode = $state<'grid' | 'list'>('grid');
  let filter = $state<'all' | 'missing' | 'candidates' | 'owned' | 'upcoming'>('all');
  let selectedVolumes = $state<Set<number>>(new Set());

  // Volume Details inspection modal state
  let inspectingVolume = $state<VolumeManagerEntry | null>(null);

  // Batch plan modal state
  let showPlanModal = $state(false);
  let planningBatch = $state(false);
  let planResponse = $state<VolumeManagerBatchPlanResponse | null>(null);
  let planError = $state<string | null>(null);

  export async function refresh() {
    loading = true;
    error = null;
    try {
      summary = await getVolumeManager(mangaId);
    } catch (e: any) {
      error = e?.message || 'Failed to load official volume manager';
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    refresh();
  });

  const filteredVolumes = $derived.by<VolumeManagerEntry[]>(() => {
    if (!summary?.volumes) return [];
    switch (filter) {
      case 'missing':
        return summary.volumes.filter((v) => !v.isOwned && v.displayStatus !== 'unreleased');
      case 'candidates':
        return summary.volumes.filter((v) => v.candidateCount > 0 && !v.isOwned);
      case 'owned':
        return summary.volumes.filter((v) => v.isOwned);
      case 'upcoming':
        return summary.volumes.filter((v) => v.displayStatus === 'unreleased');
      default:
        return summary.volumes;
    }
  });

  const missingSeq = $derived.by(() => {
    if (!summary?.volumes) return null;
    const owned = summary.volumes.filter((v) => v.isOwned).map((v) => v.volumeNumber);
    return calculateNextMissingVolume(owned, summary.totalOfficialVolumes);
  });

  function volumeState(vol: VolumeManagerEntry): VolumeCollectionStateInfo {
    return resolveVolumeCollectionState(vol);
  }

  function toggleVolumeSelection(vol: number) {
    const next = new Set(selectedVolumes);
    if (next.has(vol)) {
      next.delete(vol);
    } else {
      next.add(vol);
    }
    selectedVolumes = next;
  }

  function selectAllMissingReleased() {
    if (!summary?.volumes) return;
    const next = new Set(selectedVolumes);
    for (const v of summary.volumes) {
      if (!v.isOwned && v.displayStatus !== 'unreleased') {
        next.add(v.volumeNumber);
      }
    }
    selectedVolumes = next;
  }

  function selectAllWithCandidates() {
    if (!summary?.volumes) return;
    const next = new Set(selectedVolumes);
    for (const v of summary.volumes) {
      if (!v.isOwned && v.candidateCount > 0) {
        next.add(v.volumeNumber);
      }
    }
    selectedVolumes = next;
  }

  function clearSelection() {
    selectedVolumes = new Set();
  }

  let executingBatch = $state(false);
  let isSimulationMode = $state(true);

  async function handleOpenBatchPlan() {
    if (selectedVolumes.size === 0) return;
    planningBatch = true;
    planError = null;
    showPlanModal = true;
    try {
      const vols = Array.from(selectedVolumes).sort((a, b) => a - b);
      const [volsPlan, acqCfg] = await Promise.all([
        planVolumeManagerBatch(mangaId, {
          selectedVolumes: vols,
          isDryRun: true,
        }),
        getAcquisitionSettings().catch(() => null),
      ]);
      planResponse = volsPlan;
      if (acqCfg) {
        isSimulationMode = acqCfg.operatingMode !== 'normal' || !!acqCfg.autoAcquireDryRun;
      }
    } catch (e: any) {
      planError = e?.message || 'Failed to generate batch acquisition plan';
    } finally {
      planningBatch = false;
    }
  }

  async function handleExecuteBatch() {
    if (!planResponse || planResponse.eligibleCount === 0 || executingBatch) return;
    executingBatch = true;
    planError = null;
    try {
      const vols = Array.from(selectedVolumes).sort((a, b) => a - b);
      const res = await acquireVolumeManagerBatch(mangaId, {
        selectedVolumes: vols,
      });
      if (res.queuedCount > 0) {
        toast.success(res.message);
      } else if (res.simulatedCount > 0) {
        toast.info(res.message);
      } else {
        toast.warn(res.message);
      }
      showPlanModal = false;
      clearSelection();
      await load();
    } catch (e: any) {
      planError = e?.message || 'Failed to execute batch acquisition';
    } finally {
      executingBatch = false;
    }
  }

  function statusBadgeClass(status: string): string {
    switch (status) {
      case 'owned':
        return 'badge-owned';
      case 'published':
        return 'badge-published';
      case 'downloading':
        return 'badge-downloading';
      case 'candidate_available':
        return 'badge-candidate';
      case 'missing_released':
        return 'badge-missing';
      case 'unreleased':
        return 'badge-unreleased';
      case 'uncertain':
        return 'badge-uncertain';
      default:
        return 'badge-neutral';
    }
  }
</script>

<div class="volume-manager" data-testid="volume-manager">
  <!-- Section Header -->
  <div class="vm-header">
    <div class="vm-title-col">
      <div class="vm-title-row">
        <h2 class="display vm-heading">OFFICIAL VOLUMES</h2>
        {#if summary}
          <span class="tag edition-tag" class:warn={!summary.isAuthoritative} title={summary.uncertaintyReason || 'Official Edition'}>
            <Icon name="book" size={14} />
            {summary.edition.toUpperCase()} EDITION
            {#if !summary.isAuthoritative} · UNCERTAIN AUTHORITY{/if}
          </span>
        {/if}
      </div>
      <p class="vm-subtext">
        Primary manga acquisition unit anchored to MangaBaka official volumes and Komga canonical storage.
      </p>
    </div>

    <div class="vm-header-actions">
      <!-- Grid / List toggle -->
      <div class="view-toggle" role="group" aria-label="View layout">
        <button
          class="btn pill sm icon"
          class:spot={viewMode === 'grid'}
          aria-pressed={viewMode === 'grid'}
          onclick={() => (viewMode = 'grid')}
          title="Compact Grid View"
        >
          <Icon name="grid" size={16} />
        </button>
        <button
          class="btn pill sm icon"
          class:spot={viewMode === 'list'}
          aria-pressed={viewMode === 'list'}
          onclick={() => (viewMode = 'list')}
          title="Detailed List View"
        >
          <Icon name="list" size={16} />
        </button>
      </div>

      <button class="btn pill sm" onclick={refresh} disabled={loading} title="Refresh volume manager state">
        {#if loading}
          <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
        {:else}
          <Icon name="refresh" size={14} />
        {/if}
        Refresh
      </button>
    </div>
  </div>

  {#if error}
    <div class="vm-alert warn">
      <Icon name="info" size={18} />
      <span>{error}</span>
    </div>
  {/if}

  {#if summary && !summary.isAuthoritative}
    <div class="vm-alert edition-warning">
      <Icon name="info" size={18} />
      <div class="alert-content">
        <strong>Edition Uncertainty Detected:</strong>
        <span>{summary.uncertaintyReason || 'Multiple ambiguous MangaBaka collections found. Cross-edition volume merging is disabled to protect canonical media.'}</span>
      </div>
    </div>
  {/if}

  {#if summary}
    <!-- Summary Stats Bar -->
    <div class="vm-stats-bar">
      <div class="stat-item">
        <span class="stat-value">{summary.totalOfficialVolumes}</span>
        <span class="stat-label">Official Vols</span>
      </div>
      <div class="stat-divider"></div>
      <div class="stat-item spot">
        <span class="stat-value">{summary.ownedCount}</span>
        <span class="stat-label">Owned in Komga</span>
      </div>
      {#if summary.publishedCount > 0}
        <div class="stat-divider"></div>
        <div class="stat-item info">
          <span class="stat-value">{summary.publishedCount}</span>
          <span class="stat-label">Published</span>
        </div>
      {/if}
      {#if summary.inProgressCount > 0}
        <div class="stat-divider"></div>
        <div class="stat-item active">
          <span class="stat-value">{summary.inProgressCount}</span>
          <span class="stat-label">Acquiring</span>
        </div>
      {/if}
      <div class="stat-divider"></div>
      <div class="stat-item candidate">
        <span class="stat-value">{summary.candidateAvailableCount}</span>
        <span class="stat-label">Candidates</span>
      </div>
      <div class="stat-divider"></div>
      <div class="stat-item missing">
        <span class="stat-value">{summary.missingReleasedCount}</span>
        <span class="stat-label">Missing</span>
      </div>
      {#if summary.unreleasedCount > 0}
        <div class="stat-divider"></div>
        <div class="stat-item muted">
          <span class="stat-value">{summary.unreleasedCount}</span>
          <span class="stat-label">Unreleased</span>
        </div>
      {/if}
    </div>

    <!-- Next Missing Volume Sequence Banner (M13.1) -->
    {#if missingSeq && !missingSeq.isComplete && missingSeq.nextMissingVolume != null}
      <div class="next-missing-banner" data-testid="next-missing-banner">
        <div class="nmb-content">
          <span class="nmb-badge"><Icon name="arrow" size={13} /> NEXT MISSING IN SEQUENCE</span>
          <strong class="nmb-title">Volume {missingSeq.nextMissingVolume}</strong>
          <span class="nmb-desc">{missingSeq.explanation}</span>
        </div>
        <div class="nmb-actions">
          <button
            class="btn sm spot"
            onclick={() => onInspectCandidate?.(missingSeq.nextMissingVolume!)}
            title={`Search candidates for Volume ${missingSeq.nextMissingVolume}`}
          >
            <Icon name="search" size={14} /> Find Volume {missingSeq.nextMissingVolume}
          </button>
        </div>
      </div>
    {/if}

    <!-- Controls & Selection Bar -->
    <div class="vm-controls-bar">
      <!-- Filter Tabs -->
      <div class="filter-pills" role="tablist">
        <button
          class="btn pill sm"
          class:spot={filter === 'all'}
          onclick={() => (filter = 'all')}
        >
          All ({summary.volumes.length})
        </button>
        <button
          class="btn pill sm"
          class:spot={filter === 'missing'}
          onclick={() => (filter = 'missing')}
        >
          Missing ({summary.missingReleasedCount})
        </button>
        <button
          class="btn pill sm"
          class:spot={filter === 'candidates'}
          onclick={() => (filter = 'candidates')}
        >
          Candidates ({summary.candidateAvailableCount})
        </button>
        <button
          class="btn pill sm"
          class:spot={filter === 'owned'}
          onclick={() => (filter = 'owned')}
        >
          Owned ({summary.ownedCount})
        </button>
        {#if summary.unreleasedCount > 0}
          <button
            class="btn pill sm"
            class:spot={filter === 'upcoming'}
            onclick={() => (filter = 'upcoming')}
          >
            Upcoming ({summary.unreleasedCount})
          </button>
        {/if}
      </div>

      <!-- Multi-Selection Tools -->
      <div class="selection-tools">
        <button class="btn subtle sm" onclick={selectAllMissingReleased} title="Select all missing released volumes">
          Select Missing
        </button>
        <button class="btn subtle sm" onclick={selectAllWithCandidates} title="Select all missing volumes with verified candidates">
          Select with Candidates
        </button>
        {#if selectedVolumes.size > 0}
          <button class="btn subtle sm" onclick={clearSelection}>
            Clear ({selectedVolumes.size})
          </button>
          <button class="btn spot sm" onclick={handleOpenBatchPlan}>
            <Icon name="check" size={14} />
            Review Acquisition Plan ({selectedVolumes.size})
          </button>
        {/if}
      </div>
    </div>

    <!-- Main Content: Compact Grid or Detailed List -->
    {#if filteredVolumes.length === 0}
      <div class="vm-empty-state">
        <p>No volumes match the selected filter.</p>
      </div>
    {:else if viewMode === 'grid'}
      <!-- COMPACT GRID VIEW -->
      <div class="vm-grid">
        {#each filteredVolumes as vol (vol.volumeNumber)}
          {@const isSelected = selectedVolumes.has(vol.volumeNumber)}
          {@const st = volumeState(vol)}
          {@const isNextMissing = missingSeq?.nextMissingVolume === vol.volumeNumber && !vol.isOwned}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="vm-card {st.badgeClass}"
            class:is-owned={vol.isOwned}
            class:is-selected={isSelected}
            class:is-next-missing={isNextMissing}
            class:has-candidate={vol.candidateCount > 0 && !vol.isOwned}
            onclick={() => (inspectingVolume = vol)}
          >
            <div class="card-top-row">
              <label class="card-checkbox">
                <input
                  type="checkbox"
                  checked={isSelected}
                  disabled={vol.isOwned}
                  onclick={(e) => e.stopPropagation()}
                  onchange={() => !vol.isOwned && toggleVolumeSelection(vol.volumeNumber)}
                  aria-label={`Select ${vol.label}`}
                  title={vol.isOwned ? 'Volume already owned in Komga' : `Select ${vol.label}`}
                />
              </label>
              <div class="card-vol-identity">
                <span class="card-vol-num">{vol.label}</span>
                {#if isNextMissing}
                  <span class="next-in-line-pill">Next</span>
                {/if}
              </div>
              <span class="card-badge {st.badgeClass}">
                {st.label.toUpperCase()}
              </span>
            </div>

            <div class="card-body">
              {#if vol.coverUrl}
                <div class="card-thumb-wrap">
                  <img src={vol.coverUrl} alt={vol.label} class="card-thumb" loading="lazy" />
                </div>
              {/if}
              <p class="card-status-label">{vol.statusLabel}</p>

              {#if vol.isOwned}
                <div class="card-storage-box">
                  <span class="storage-tag">KOMGA VERIFIED</span>
                  {#if vol.komgaBookName}
                    <span class="storage-filename" title={vol.komgaBookName}>{vol.komgaBookName}</span>
                  {:else if vol.canonicalPath}
                    <span class="storage-filename" title={vol.canonicalPath}>{vol.canonicalPath.split('/').pop()}</span>
                  {/if}
                </div>
              {:else if vol.topCandidateTitle}
                <div class="card-candidate-box">
                  <span class="cand-provider-tag {vol.topProvider || ''}">{vol.topProvider?.toUpperCase() || 'RELEASE'}</span>
                  <span class="cand-score">{Math.round(vol.topCandidateScore || 0)}% match</span>
                </div>
              {:else if vol.releaseDate}
                <span class="card-release-date">Release: {vol.releaseDate}</span>
              {/if}
            </div>

            <div class="card-actions" onclick={(e) => e.stopPropagation()}>
              {#if !vol.isOwned && vol.candidateCount > 0}
                <button
                  class="btn sm subtle cand-btn"
                  onclick={() => onInspectCandidate?.(vol.volumeNumber)}
                  title="Inspect and compare discovered candidates"
                >
                  <Icon name="search" size={14} />
                  {vol.candidateCount} cand.
                </button>
              {:else if !vol.isOwned}
                <button
                  class="btn sm subtle cand-btn"
                  onclick={() => onInspectCandidate?.(vol.volumeNumber)}
                  title="Search for candidates"
                >
                  <Icon name="search" size={14} />
                  Find
                </button>
              {:else}
                <span class="owned-verified-check" title="Verified in Komga canonical storage">
                  <Icon name="check" size={14} /> Owned
                </span>
              {/if}
              <button
                class="btn sm subtle icon-btn"
                onclick={() => (inspectingVolume = vol)}
                title="View volume details"
                aria-label="View volume details"
              >
                <Icon name="info" size={14} />
              </button>
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <!-- DETAILED LIST VIEW -->
      <div class="vm-list-wrap">
        <table class="vm-table">
          <thead>
            <tr>
              <th class="th-check">
                <input
                  type="checkbox"
                  checked={selectedVolumes.size > 0 && selectedVolumes.size === filteredVolumes.filter((v) => !v.isOwned).length}
                  indeterminate={selectedVolumes.size > 0 && selectedVolumes.size < filteredVolumes.filter((v) => !v.isOwned).length}
                  onchange={(e) => {
                    if (e.currentTarget.checked) {
                      selectedVolumes = new Set(filteredVolumes.filter((v) => !v.isOwned).map((v) => v.volumeNumber));
                    } else {
                      clearSelection();
                    }
                  }}
                  aria-label="Select all unowned volumes"
                />
              </th>
              <th>Volume</th>
              <th>Status</th>
              <th>Ownership / Storage</th>
              <th>Candidates / Releases</th>
              <th>Release Date</th>
              <th>Actions</th>
            </tr>
          </thead>
          <tbody>
            {#each filteredVolumes as vol (vol.volumeNumber)}
              {@const isSelected = selectedVolumes.has(vol.volumeNumber)}
              {@const st = volumeState(vol)}
              {@const isNextMissing = missingSeq?.nextMissingVolume === vol.volumeNumber && !vol.isOwned}
              <tr class:row-selected={isSelected} class:row-owned={vol.isOwned} class:row-next-missing={isNextMissing}>
                <td class="td-check">
                  <input
                    type="checkbox"
                    checked={isSelected}
                    disabled={vol.isOwned}
                    onchange={() => !vol.isOwned && toggleVolumeSelection(vol.volumeNumber)}
                    aria-label={`Select ${vol.label}`}
                    title={vol.isOwned ? 'Volume already owned in Komga' : `Select ${vol.label}`}
                  />
                </td>
                <td class="td-vol">
                  <div class="table-vol-id">
                    <strong>{vol.label}</strong>
                    {#if isNextMissing}
                      <span class="next-in-line-pill">Next</span>
                    {/if}
                  </div>
                </td>
                <td>
                  <span class="card-badge {st.badgeClass}">
                    {st.label.toUpperCase()}
                  </span>
                </td>
                <td class="td-storage">
                  {#if vol.isOwned}
                    <div class="storage-meta">
                      <span class="storage-tag">KOMGA OWNED</span>
                      {#if vol.komgaBookName}
                        <span class="file-path" title={vol.komgaBookName}>{vol.komgaBookName}</span>
                      {:else if vol.canonicalPath}
                        <span class="file-path" title={vol.canonicalPath}>
                          {vol.canonicalPath.split('/').pop()}
                        </span>
                      {/if}
                    </div>
                  {:else if vol.publicationReceiptId}
                    <span class="storage-tag pending">SCAN PENDING</span>
                  {:else if vol.inProgress}
                    <span class="storage-tag acquiring">{vol.activeStage?.toUpperCase() || 'ACQUIRING'}</span>
                  {:else}
                    <span class="muted">Not in library</span>
                  {/if}
                </td>
                <td class="td-candidates">
                  {#if vol.candidateCount > 0}
                    <div class="cand-summary">
                      <span class="cand-provider-tag {vol.topProvider || ''}">{vol.topProvider?.toUpperCase() || 'RELEASE'}</span>
                      <span class="cand-count">{vol.candidateCount} found</span>
                    </div>
                  {:else}
                    <span class="muted">None discovered</span>
                  {/if}
                </td>
                <td class="td-date">
                  <span class="muted">{vol.releaseDate || '—'}</span>
                </td>
                <td class="td-actions">
                  <div class="row-actions">
                    {#if !vol.isOwned}
                      <button
                        class="btn sm pill"
                        onclick={() => onInspectCandidate?.(vol.volumeNumber)}
                        title="Inspect candidates for this volume"
                      >
                        <Icon name="search" size={14} /> Inspect
                      </button>
                    {/if}
                    <button
                      class="btn sm subtle"
                      onclick={() => (inspectingVolume = vol)}
                      title="View volume details"
                    >
                      <Icon name="info" size={14} /> Details
                    </button>
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</div>

<!-- Volume Details Inspection Modal -->
{#if inspectingVolume}
  {@const inspectingState = volumeState(inspectingVolume)}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={() => (inspectingVolume = null)}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="modal-card volume-details-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-header">
        <div>
          <div class="modal-title-row">
            <h3 class="display">{inspectingVolume.label.toUpperCase()} DETAILS</h3>
            <span class="card-badge {inspectingState.badgeClass}">
              {inspectingState.label.toUpperCase()}
            </span>
          </div>
          <p class="modal-subtitle">{title} · {summary?.edition.toUpperCase() || 'STANDARD'} EDITION</p>
        </div>
        <button class="btn icon subtle" onclick={() => (inspectingVolume = null)} aria-label="Close modal">
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body">
        <!-- Metadata & Release Info -->
        <div class="details-section">
          <h4 class="section-heading">Official Edition Metadata</h4>
          <div class="details-grid">
            <div class="detail-field">
              <span class="detail-label">Volume Number</span>
              <span class="detail-val">Volume {inspectingVolume.volumeNumber}</span>
            </div>
            <div class="detail-field">
              <span class="detail-label">Release Date</span>
              <span class="detail-val">{inspectingVolume.releaseDate || 'Not specified'}</span>
            </div>
            <div class="detail-field">
              <span class="detail-label">Edition Authority</span>
              <span class="detail-val">
                {summary?.isAuthoritative ? 'Authoritative MangaBaka' : 'Uncertain Authority'}
              </span>
            </div>
            <div class="detail-field">
              <span class="detail-label">Release Status</span>
              <span class="detail-val">{inspectingVolume.displayStatus.toUpperCase()}</span>
            </div>
          </div>
        </div>

        <!-- Komga Ownership / Storage Info -->
        <div class="details-section">
          <h4 class="section-heading">Komga Canonical Ownership</h4>
          <div class="details-grid">
            <div class="detail-field">
              <span class="detail-label">Ownership Status</span>
              <span class="detail-val">
                {#if inspectingVolume.isOwned}
                  <span class="owned-tag-solid"><Icon name="check" size={14} /> Owned in Komga</span>
                {:else if inspectingVolume.publicationReceiptId}
                  <span class="pending-tag">Publication Receipt Recorded (Scan Pending)</span>
                {:else}
                  <span class="missing-tag">Missing from Canonical Media</span>
                {/if}
              </span>
            </div>
            {#if inspectingVolume.komgaBookId}
              <div class="detail-field">
                <span class="detail-label">Komga Book ID</span>
                <span class="detail-val font-mono">{inspectingVolume.komgaBookId}</span>
              </div>
            {/if}
            {#if inspectingVolume.komgaBookName}
              <div class="detail-field">
                <span class="detail-label">Komga Book Name</span>
                <span class="detail-val">{inspectingVolume.komgaBookName}</span>
              </div>
            {/if}
            {#if summary?.komgaSeriesId}
              <div class="detail-field">
                <span class="detail-label">Komga Series ID</span>
                <span class="detail-val font-mono">{summary.komgaSeriesId}</span>
              </div>
            {/if}
            {#if inspectingVolume.canonicalPath}
              <div class="detail-field full-width">
                <span class="detail-label">Canonical Path</span>
                <span class="detail-val font-mono path-val">{inspectingVolume.canonicalPath}</span>
              </div>
            {/if}
            {#if inspectingVolume.fileSizeBytes}
              <div class="detail-field">
                <span class="detail-label">Archive File Size</span>
                <span class="detail-val">{formatBytes(inspectingVolume.fileSizeBytes)}</span>
              </div>
            {/if}
          </div>
        </div>

        <!-- Acquisition Lifecycle Timeline (M13.1) -->
        <div class="details-section">
          <h4 class="section-heading">Acquisition Lifecycle</h4>
          <LifecycleTimeline
            timelineInputs={{
              discoveredAt: inspectingVolume.lastDiscoveryAt,
              publishedAt: inspectingVolume.publishedAt,
              verifiedAt: inspectingVolume.verifiedAt,
              currentStage: inspectingVolume.activeStage || (inspectingVolume.isOwned ? 'verification' : inspectingVolume.displayStatus),
              provider: inspectingVolume.topProvider,
            }}
          />
        </div>

        <!-- Discovered Candidates & Acquisition Intelligence -->
        <div class="details-section">
          <h4 class="section-heading">Acquisition Candidates &amp; Releases</h4>
          {#if inspectingVolume.candidateCount > 0}
            <div class="candidate-highlight-box">
              <div class="cand-top-info">
                <span class="cand-provider-tag {inspectingVolume.topProvider || ''}">
                  {inspectingVolume.topProvider?.toUpperCase() || 'RELEASE'}
                </span>
                <span class="cand-score-pill">
                  {Math.round(inspectingVolume.topCandidateScore || 0)}% Quality Match
                </span>
                <span class="cand-count-badge">
                  {inspectingVolume.candidateCount} candidate{inspectingVolume.candidateCount === 1 ? '' : 's'} available
                </span>
              </div>
              {#if inspectingVolume.topCandidateTitle}
                <p class="cand-title-full">{inspectingVolume.topCandidateTitle}</p>
              {/if}
            </div>
          {:else if inspectingVolume.isOwned}
            <p class="section-empty-msg">This volume is already physically owned and verified in Komga canonical storage.</p>
          {:else}
            <p class="section-empty-msg">No verified release candidates currently discovered for Volume {inspectingVolume.volumeNumber}.</p>
          {/if}
        </div>
      </div>

      <div class="modal-footer">
        <div class="modal-buttons-left">
          {#if !inspectingVolume.isOwned}
            <button
              class="btn subtle"
              onclick={() => toggleVolumeSelection(inspectingVolume!.volumeNumber)}
            >
              <Icon name="check" size={14} />
              {selectedVolumes.has(inspectingVolume.volumeNumber) ? 'Remove from Selection' : 'Select for Acquisition'}
            </button>
          {/if}
        </div>
        <div class="modal-buttons">
          {#if !inspectingVolume.isOwned}
            <button
              class="btn spot"
              onclick={() => {
                const volNum = inspectingVolume!.volumeNumber;
                inspectingVolume = null;
                onInspectCandidate?.(volNum);
              }}
            >
              <Icon name="search" size={14} />
              {inspectingVolume.candidateCount > 0 ? 'Inspect Candidates' : 'Find Releases'}
            </button>
          {/if}
          <button class="btn subtle" onclick={() => (inspectingVolume = null)}>
            Close
          </button>
        </div>
      </div>
    </div>
  </div>
{/if}

<!-- Batch Acquisition Planning Preview Modal -->
{#if showPlanModal}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={() => (showPlanModal = false)}>
    <!-- svelte-ignore a11y_click_events_have_key_events -->
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="modal-card plan-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-header">
        <div>
          <h3 class="display">BATCH ACQUISITION PLAN</h3>
          <p class="modal-subtitle">Review-first acquisition preview for {selectedVolumes.size} selected volume(s)</p>
        </div>
        <button class="btn icon subtle" onclick={() => (showPlanModal = false)}>
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body">
        {#if isSimulationMode}
          <div class="dry-run-banner">
            <Icon name="info" size={16} />
            <span>SIMULATION MODE ACTIVE: Evaluates candidate matching and plan preview without downloading files.</span>
          </div>
        {:else}
          <div class="dry-run-banner" style="background: rgba(46, 204, 113, 0.1); border-color: rgba(46, 204, 113, 0.4); color: #2ecc71;">
            <Icon name="download" size={16} />
            <span>ONE-CLICK BATCH DOWNLOAD: Eligible releases will be queued into downloads. Owned volumes are protected.</span>
          </div>
        {/if}

        {#if planningBatch}
          <div class="loading-state">
            <span class="spin"><Icon name="spinner" size={24} stroke={3} /></span>
            <p>Evaluating candidate eligibility across providers...</p>
          </div>
        {:else if planError}
          <div class="vm-alert warn">
            <Icon name="info" size={18} />
            <span>{planError}</span>
          </div>
        {:else if planResponse}
          <div class="plan-summary-metrics">
            <div class="metric">
              <span class="metric-val">{planResponse.totalSelected}</span>
              <span class="metric-label">Selected</span>
            </div>
            <div class="metric spot">
              <span class="metric-val">{planResponse.eligibleCount}</span>
              <span class="metric-label">Eligible</span>
            </div>
            <div class="metric warn">
              <span class="metric-val">{planResponse.blockedCount}</span>
              <span class="metric-label">Blocked</span>
            </div>
            <div class="metric">
              <span class="metric-val">{formatBytes(planResponse.estimatedTotalBytes)}</span>
              <span class="metric-label">Estimated Size</span>
            </div>
          </div>

          {#if planResponse.executionBlockedReason}
            <div class="vm-alert warn">
              <Icon name="info" size={16} />
              <span>{planResponse.executionBlockedReason}</span>
            </div>
          {/if}

          <div class="plan-items-table-wrap">
            <table class="plan-table">
              <thead>
                <tr>
                  <th>Volume</th>
                  <th>Status</th>
                  <th>Proposed Backend</th>
                  <th>Candidate Title</th>
                  <th>Reason</th>
                </tr>
              </thead>
              <tbody>
                {#each planResponse.items as item (item.volumeNumber)}
                  <tr class:item-eligible={item.isEligible} class:item-blocked={!item.isEligible}>
                    <td><strong>Volume {item.volumeNumber}</strong></td>
                    <td>
                      <span class="plan-status-badge {item.isEligible ? 'eligible' : 'blocked'}">
                        {item.status}
                      </span>
                    </td>
                    <td>
                      {#if item.proposedBackend}
                        <span class="backend-tag">{item.proposedBackend}</span>
                      {:else}
                        <span class="muted">—</span>
                      {/if}
                    </td>
                    <td class="cand-title-cell">
                      {item.candidateTitle || '—'}
                    </td>
                    <td class="reason-cell">
                      <span class="subtext">{item.reason}</span>
                    </td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      </div>

      <div class="modal-footer">
        <span class="subtext">
          {isSimulationMode ? 'Simulation Mode' : 'Normal Execution Mode'} · {planResponse?.eligibleCount ?? 0} ready to acquire
        </span>
        <div class="modal-buttons">
          <button class="btn subtle" onclick={() => (showPlanModal = false)} disabled={executingBatch}>
            Close
          </button>
          <button
            class="btn spot"
            disabled={!planResponse?.canExecute || executingBatch}
            onclick={handleExecuteBatch}
            title={planResponse?.canExecute ? (isSimulationMode ? `Simulate acquisition of ${planResponse.eligibleCount} volume(s)` : `Download ${planResponse.eligibleCount} volume(s)`) : 'No eligible volumes selected'}
          >
            {#if executingBatch}
              <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
              <span>Queuing Downloads…</span>
            {:else}
              <Icon name={isSimulationMode ? 'eye' : 'download'} size={14} />
              <span>{isSimulationMode ? 'Simulate' : 'Download'} {planResponse?.eligibleCount ?? 0} Volume{planResponse?.eligibleCount === 1 ? '' : 's'}</span>
            {/if}
          </button>
        </div>
      </div>
    </div>
  </div>
{/if}

<style>
  .volume-manager {
    margin-top: 2rem;
    padding: 1.5rem;
    background: var(--surface);
    border: 2px solid var(--ink);
    box-shadow: var(--sh-4);
  }

  .vm-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: 1.25rem;
  }

  .vm-title-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

  .vm-heading {
    font-size: 1.25rem;
    letter-spacing: 0.05em;
    margin: 0;
  }

  .edition-tag {
    font-size: 0.75rem;
    padding: 0.2rem 0.5rem;
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
  }

  .vm-subtext {
    font-size: 0.85rem;
    color: var(--muted);
    margin: 0.25rem 0 0 0;
  }

  .vm-header-actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .view-toggle {
    display: inline-flex;
    border: 2px solid var(--ink);
    overflow: hidden;
  }

  .view-toggle .btn {
    border-radius: 0;
    border: none;
    border-right: 1px solid var(--border);
  }

  .view-toggle .btn:last-child {
    border-right: none;
  }

  .vm-alert {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.85rem 1rem;
    border-radius: 4px;
    margin-bottom: 1rem;
    font-size: 0.9rem;
  }

  .vm-alert.warn {
    background: rgba(239, 68, 68, 0.1);
    border: 1.5px solid rgba(239, 68, 68, 0.5);
    color: #991b1b;
  }

  .vm-alert.edition-warning {
    background: rgba(245, 158, 11, 0.12);
    border: 1.5px solid rgba(245, 158, 11, 0.6);
    color: #92400e;
  }

  .vm-stats-bar {
    display: flex;
    align-items: center;
    gap: 1.25rem;
    background: var(--surface);
    border: 2px solid var(--ink);
    padding: 0.75rem 1.25rem;
    margin-bottom: 1.25rem;
    overflow-x: auto;
  }

  .stat-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    min-width: 4.5rem;
  }

  .stat-value {
    font-size: 1.25rem;
    font-weight: 900;
    color: var(--ink);
  }

  .stat-label {
    font-size: 0.75rem;
    text-transform: uppercase;
    color: var(--muted);
    letter-spacing: 0.05em;
    font-weight: 700;
  }

  .stat-divider {
    width: 2px;
    height: 1.75rem;
    background: var(--ink);
    opacity: 0.2;
  }

  .vm-controls-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.75rem;
    margin-bottom: 1rem;
  }

  .filter-pills {
    display: flex;
    gap: 0.35rem;
    flex-wrap: wrap;
  }

  .selection-tools {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  /* Compact Grid View */
  .vm-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
    gap: 0.85rem;
  }

  .vm-card {
    background: var(--surface);
    border: 2px solid var(--ink);
    box-shadow: var(--sh-3);
    padding: 0.85rem;
    cursor: pointer;
    transition: transform 0.1s, box-shadow 0.1s;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    min-height: 140px;
  }

  .vm-card:hover {
    transform: translate(-1px, -1px);
    box-shadow: var(--sh-4);
  }

  .vm-card.is-selected {
    background: color-mix(in srgb, var(--spot) 16%, var(--surface));
    outline: 2px solid var(--spot);
  }

  .vm-card.is-owned {
    border-color: var(--ink);
  }

  .card-top-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.25rem;
  }

  .card-vol-num {
    font-weight: 800;
    font-size: 0.95rem;
    flex: 1;
    margin-left: 0.35rem;
    color: var(--ink);
  }

  .card-badge {
    font-size: 0.65rem;
    font-weight: 800;
    padding: 0.15rem 0.45rem;
    border-radius: 999px;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  /* 12 Explicit M13 Collection States */
  .state-owned,
  .card-badge.state-owned {
    background: #dcfce7;
    color: #166534;
    border: 1.5px solid #166534;
  }

  .state-missing,
  .card-badge.state-missing {
    background: #fee2e2;
    color: #991b1b;
    border: 1.5px solid #991b1b;
  }

  .state-available,
  .card-badge.state-available {
    background: #e0f2fe;
    color: #0369a1;
    border: 1.5px solid #0369a1;
  }

  .state-pending-review,
  .card-badge.state-pending-review {
    background: #f3e8ff;
    color: #6b21a8;
    border: 1.5px solid #6b21a8;
  }

  .state-queued,
  .card-badge.state-queued {
    background: #fef3c7;
    color: #92400e;
    border: 1.5px solid #92400e;
  }

  .state-downloading,
  .card-badge.state-downloading {
    background: #fef08a;
    color: #854d0e;
    border: 1.5px solid #854d0e;
  }

  .state-validating,
  .card-badge.state-validating {
    background: #e0e7ff;
    color: #3730a3;
    border: 1.5px solid #3730a3;
  }

  .state-staged,
  .card-badge.state-staged {
    background: #ede9fe;
    color: #5b21b6;
    border: 1.5px solid #5b21b6;
  }

  .state-publishing,
  .card-badge.state-publishing {
    background: #ccfbf1;
    color: #115e59;
    border: 1.5px solid #115e59;
  }

  .state-awaiting-verification,
  .card-badge.state-awaiting-verification {
    background: #fae8ff;
    color: #86198f;
    border: 1.5px solid #86198f;
  }

  .state-failed,
  .card-badge.state-failed {
    background: #fecaca;
    color: #991b1b;
    border: 1.5px solid #991b1b;
  }

  .state-unknown,
  .card-badge.state-unknown {
    background: #f1f5f9;
    color: #475569;
    border: 1.5px solid #475569;
  }

  .next-missing-banner {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    margin-bottom: 1rem;
    border: 2px solid var(--ink);
    background: color-mix(in srgb, var(--spot) 16%, var(--surface));
    box-shadow: var(--sh-3);
  }

  .nmb-content {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .nmb-badge {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    font-size: 10px;
    font-weight: 900;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--ink);
    background: var(--spot);
    padding: 2px 6px;
    border: 1.5px solid var(--ink);
    width: fit-content;
  }

  .nmb-title {
    font-size: 16px;
    font-weight: 900;
    color: var(--ink);
  }

  .nmb-desc {
    font-size: 12px;
    color: var(--muted);
  }

  .nmb-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .is-next-missing {
    outline: 3px solid var(--spot) !important;
    border-color: var(--ink) !important;
  }

  .next-in-line-pill {
    font-size: 9px;
    font-weight: 900;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    background: var(--spot);
    color: var(--ink);
    padding: 1px 5px;
    border: 1px solid var(--ink);
    border-radius: 999px;
    margin-left: 4px;
  }

  .card-vol-identity {
    display: flex;
    align-items: center;
    gap: 4px;
    flex: 1;
    margin-left: 0.35rem;
  }

  .table-vol-id {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .card-thumb-wrap {
    margin-bottom: 6px;
    border: 1.5px solid var(--ink);
    border-radius: 2px;
    overflow: hidden;
    max-height: 110px;
  }

  .card-thumb {
    width: 100%;
    height: 100px;
    object-fit: cover;
    display: block;
  }

  .badge-owned {
    background: #dcfce7;
    color: #166534;
    border: 1.5px solid #166534;
  }

  .badge-published {
    background: #f3e8ff;
    color: #6b21a8;
    border: 1.5px solid #6b21a8;
  }

  .badge-downloading {
    background: #fef3c7;
    color: #92400e;
    border: 1.5px solid #92400e;
  }

  .badge-candidate {
    background: #e0f2fe;
    color: #0369a1;
    border: 1.5px solid #0369a1;
  }

  .badge-missing {
    background: #fee2e2;
    color: #991b1b;
    border: 1.5px solid #991b1b;
  }

  .badge-unreleased {
    background: #f1f5f9;
    color: #475569;
    border: 1.5px solid #475569;
  }

  .badge-neutral {
    background: #f1f5f9;
    color: #334155;
    border: 1.5px solid #334155;
  }

  .card-body {
    margin: 0.5rem 0;
    font-size: 0.8rem;
  }

  .card-status-label {
    margin: 0 0 0.35rem 0;
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--ink);
  }

  .card-storage-box {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.75rem;
  }

  .storage-tag {
    font-size: 0.65rem;
    font-weight: 800;
    color: #166534;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .storage-filename {
    color: var(--muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 100%;
  }

  .card-candidate-box {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.75rem;
  }

  .cand-provider-tag {
    font-size: 0.65rem;
    font-weight: 800;
    padding: 0.1rem 0.4rem;
    border-radius: 3px;
    background: var(--ink);
    color: var(--paper);
  }

  .cand-provider-tag.madokami {
    background: #ec4899;
    color: #fff;
  }

  .cand-provider-tag.nyaa {
    background: #2563eb;
    color: #fff;
  }

  .cand-score {
    font-weight: 700;
    color: var(--ink);
  }

  .card-release-date {
    font-size: 0.75rem;
    color: var(--muted);
  }

  .card-actions {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    margin-top: 0.5rem;
  }

  .cand-btn {
    flex: 1;
    font-size: 0.75rem;
    justify-content: center;
  }

  .icon-btn {
    padding: 0 0.5rem;
  }

  .owned-verified-check {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.75rem;
    font-weight: 700;
    color: #166534;
    flex: 1;
  }

  .row-actions {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  /* List Table View */
  .vm-list-wrap {
    overflow-x: auto;
    border: 2px solid var(--ink);
  }

  .vm-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
    background: var(--surface);
  }

  .vm-table th,
  .vm-table td {
    padding: 0.65rem 0.75rem;
    border-bottom: 1px solid var(--border);
    text-align: left;
  }

  .vm-table th {
    background: var(--paper);
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    font-weight: 800;
    color: var(--ink);
    border-bottom: 2px solid var(--ink);
  }

  .vm-table tr:hover {
    background: color-mix(in srgb, var(--paper) 50%, var(--surface));
  }

  .vm-table tr.row-selected {
    background: color-mix(in srgb, var(--spot) 12%, var(--surface));
  }

  /* Modals: Details & Batch */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 1rem;
  }

  .modal-card {
    background: var(--surface);
    border: 2px solid var(--ink);
    box-shadow: var(--sh-5);
    max-width: 800px;
    width: 100%;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
  }

  .volume-details-modal {
    max-width: 680px;
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    padding: 1.25rem 1.5rem;
    border-bottom: 2px solid var(--ink);
    background: var(--paper);
  }

  .modal-title-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

  .modal-subtitle {
    margin: 0.25rem 0 0 0;
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--muted);
  }

  .modal-body {
    padding: 1.25rem 1.5rem;
    overflow-y: auto;
    flex: 1;
  }

  .details-section {
    margin-bottom: 1.25rem;
    padding-bottom: 1rem;
    border-bottom: 1px solid var(--border);
  }

  .details-section:last-child {
    margin-bottom: 0;
    padding-bottom: 0;
    border-bottom: none;
  }

  .section-heading {
    font-size: 0.8rem;
    font-weight: 800;
    text-transform: uppercase;
    letter-spacing: 0.08em;
    color: var(--muted);
    margin: 0 0 0.75rem 0;
  }

  .details-grid {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 0.75rem 1.25rem;
  }

  .detail-field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .detail-field.full-width {
    grid-column: 1 / -1;
  }

  .detail-label {
    font-size: 0.75rem;
    font-weight: 700;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .detail-val {
    font-size: 0.85rem;
    font-weight: 700;
    color: var(--ink);
  }

  .font-mono {
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
    font-size: 0.8rem;
  }

  .path-val {
    word-break: break-all;
    background: var(--paper);
    padding: 0.35rem 0.5rem;
    border: 1px solid var(--ink);
  }

  .owned-tag-solid {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    color: #166534;
    background: #dcfce7;
    border: 1.5px solid #166534;
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    font-size: 0.75rem;
    font-weight: 800;
  }

  .pending-tag {
    color: #6b21a8;
    background: #f3e8ff;
    border: 1.5px solid #6b21a8;
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    font-size: 0.75rem;
    font-weight: 800;
  }

  .missing-tag {
    color: #991b1b;
    background: #fee2e2;
    border: 1.5px solid #991b1b;
    padding: 0.15rem 0.5rem;
    border-radius: 999px;
    font-size: 0.75rem;
    font-weight: 800;
  }

  .candidate-highlight-box {
    background: var(--paper);
    border: 2px solid var(--ink);
    padding: 0.85rem;
  }

  .cand-top-info {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-bottom: 0.5rem;
  }

  .cand-score-pill {
    font-size: 0.75rem;
    font-weight: 800;
    color: #0369a1;
    background: #e0f2fe;
    border: 1px solid #0369a1;
    padding: 0.1rem 0.45rem;
    border-radius: 999px;
  }

  .cand-count-badge {
    font-size: 0.75rem;
    font-weight: 700;
    color: var(--muted);
  }

  .cand-title-full {
    font-size: 0.85rem;
    font-weight: 700;
    color: var(--ink);
    margin: 0;
    word-break: break-word;
  }

  .section-empty-msg {
    font-size: 0.85rem;
    color: var(--muted);
    font-weight: 600;
    margin: 0;
  }

  .modal-buttons-left {
    display: flex;
    gap: 0.5rem;
  }

  .dry-run-banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: rgba(59, 130, 246, 0.1);
    border: 1.5px solid rgba(59, 130, 246, 0.5);
    color: #1d4ed8;
    padding: 0.65rem 0.85rem;
    border-radius: 4px;
    font-size: 0.85rem;
    font-weight: 700;
    margin-bottom: 1rem;
  }

  .plan-summary-metrics {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.75rem;
    margin-bottom: 1.25rem;
  }

  .metric {
    background: var(--paper);
    border: 2px solid var(--ink);
    padding: 0.75rem;
    text-align: center;
  }

  .metric-val {
    display: block;
    font-size: 1.25rem;
    font-weight: 900;
    color: var(--ink);
  }

  .metric-label {
    font-size: 0.75rem;
    color: var(--muted);
    text-transform: uppercase;
    font-weight: 700;
  }

  .plan-items-table-wrap {
    max-height: 320px;
    overflow-y: auto;
    border: 2px solid var(--ink);
  }

  .plan-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.8rem;
    background: var(--surface);
  }

  .plan-table th,
  .plan-table td {
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--border);
    text-align: left;
  }

  .plan-table th {
    background: var(--paper);
    font-weight: 800;
    border-bottom: 2px solid var(--ink);
  }

  .plan-status-badge {
    font-size: 0.7rem;
    font-weight: 800;
    padding: 0.15rem 0.45rem;
    border-radius: 999px;
    text-transform: uppercase;
  }

  .plan-status-badge.eligible {
    background: #dcfce7;
    color: #166534;
    border: 1.5px solid #166534;
  }

  .plan-status-badge.blocked {
    background: #fee2e2;
    color: #991b1b;
    border: 1.5px solid #991b1b;
  }

  .backend-tag {
    font-size: 0.75rem;
    font-weight: 700;
    background: var(--paper);
    padding: 0.15rem 0.4rem;
    border: 1px solid var(--ink);
  }

  .modal-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem 1.5rem;
    border-top: 2px solid var(--ink);
    background: var(--paper);
  }

  .modal-buttons {
    display: flex;
    gap: 0.5rem;
  }
</style>
