<script lang="ts">
  import { onMount } from 'svelte';
  import Icon from './Icon.svelte';
  import {
    getVolumeManager,
    planVolumeManagerBatch,
    type VolumeManagerSummary,
    type VolumeManagerEntry,
    type VolumeManagerBatchPlanResponse,
  } from './api';
  import { formatBytes } from './util';

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
  let filter = $state<'all' | 'missing' | 'candidates' | 'owned'>('all');
  let selectedVolumes = $state<Set<number>>(new Set());

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
      default:
        return summary.volumes;
    }
  });

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

  async function handleOpenBatchPlan() {
    if (selectedVolumes.size === 0) return;
    planningBatch = true;
    planError = null;
    showPlanModal = true;
    try {
      const vols = Array.from(selectedVolumes).sort((a, b) => a - b);
      planResponse = await planVolumeManagerBatch(mangaId, {
        selectedVolumes: vols,
        isDryRun: true,
      });
    } catch (e: any) {
      planError = e?.message || 'Failed to generate batch acquisition plan';
    } finally {
      planningBatch = false;
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
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="vm-card"
            class:is-owned={vol.isOwned}
            class:is-selected={isSelected}
            class:has-candidate={vol.candidateCount > 0 && !vol.isOwned}
            onclick={() => toggleVolumeSelection(vol.volumeNumber)}
          >
            <div class="card-top-row">
              <label class="card-checkbox">
                <input
                  type="checkbox"
                  checked={isSelected}
                  onclick={(e) => e.stopPropagation()}
                  onchange={() => toggleVolumeSelection(vol.volumeNumber)}
                  aria-label={`Select ${vol.label}`}
                />
              </label>
              <span class="card-vol-num">{vol.label}</span>
              <span class="card-badge {statusBadgeClass(vol.displayStatus)}">
                {vol.isOwned ? 'OWNED' : vol.candidateCount > 0 ? 'CANDIDATE' : vol.displayStatus.toUpperCase()}
              </span>
            </div>

            <div class="card-body">
              <p class="card-status-label">{vol.statusLabel}</p>

              {#if vol.topCandidateTitle}
                <div class="card-candidate-box">
                  <span class="cand-provider-tag {vol.topProvider || ''}">{vol.topProvider?.toUpperCase() || 'RELEASE'}</span>
                  <span class="cand-score">{Math.round(vol.topCandidateScore || 0)}% match</span>
                </div>
              {:else if vol.isOwned && vol.canonicalPath}
                <span class="card-storage-info" title={vol.canonicalPath}>
                  Canonical CBZ: {vol.fileSizeBytes ? formatBytes(vol.fileSizeBytes) : 'Owned'}
                </span>
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
                  {vol.candidateCount} candidate{vol.candidateCount === 1 ? '' : 's'}
                </button>
              {:else if !vol.isOwned}
                <button
                  class="btn sm subtle cand-btn"
                  onclick={() => onInspectCandidate?.(vol.volumeNumber)}
                  title="Search for candidates"
                >
                  <Icon name="search" size={14} />
                  Find Releases
                </button>
              {:else}
                <span class="owned-verified-check" title="Verified in Komga">
                  <Icon name="check" size={14} /> Komga Verified
                </span>
              {/if}
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
                  checked={selectedVolumes.size > 0 && selectedVolumes.size === filteredVolumes.length}
                  indeterminate={selectedVolumes.size > 0 && selectedVolumes.size < filteredVolumes.length}
                  onchange={(e) => {
                    if (e.currentTarget.checked) {
                      selectedVolumes = new Set(filteredVolumes.map((v) => v.volumeNumber));
                    } else {
                      clearSelection();
                    }
                  }}
                  aria-label="Select all displayed volumes"
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
              <tr class:row-selected={isSelected} class:row-owned={vol.isOwned}>
                <td class="td-check">
                  <input
                    type="checkbox"
                    checked={isSelected}
                    onchange={() => toggleVolumeSelection(vol.volumeNumber)}
                    aria-label={`Select ${vol.label}`}
                  />
                </td>
                <td class="td-vol">
                  <strong>{vol.label}</strong>
                </td>
                <td>
                  <span class="card-badge {statusBadgeClass(vol.displayStatus)}">
                    {vol.statusLabel}
                  </span>
                </td>
                <td class="td-storage">
                  {#if vol.isOwned}
                    <div class="storage-meta">
                      <span class="storage-tag">KOMGA OWNED</span>
                      {#if vol.canonicalPath}
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
                  <button
                    class="btn sm pill"
                    onclick={() => onInspectCandidate?.(vol.volumeNumber)}
                    title="Inspect candidates for this volume"
                  >
                    <Icon name="search" size={14} /> Inspect
                  </button>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {/if}
  {/if}
</div>

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
        <div class="dry-run-banner">
          <Icon name="info" size={16} />
          <span>DRY-RUN PREVIEW ACTIVE: No real media downloads or TorBox jobs will be executed.</span>
        </div>

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
          Draft Plan ID: {planResponse?.draftJobId || 'Pending'}
        </span>
        <div class="modal-buttons">
          <button class="btn subtle" onclick={() => (showPlanModal = false)}>
            Close
          </button>
          <button class="btn spot" disabled={!planResponse?.canExecute} title="Batch execution is review-first only in M8.2">
            Confirm &amp; Save Draft Plan
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
    border: 1px solid var(--border);
    border-radius: 8px;
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
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }

  .view-toggle .btn {
    border-radius: 0;
    border: none;
  }

  .vm-alert {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.85rem 1rem;
    border-radius: 6px;
    margin-bottom: 1rem;
    font-size: 0.9rem;
  }

  .vm-alert.warn {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.3);
    color: #ef4444;
  }

  .vm-alert.edition-warning {
    background: rgba(245, 158, 11, 0.1);
    border: 1px solid rgba(245, 158, 11, 0.4);
    color: #f59e0b;
  }

  .vm-stats-bar {
    display: flex;
    align-items: center;
    gap: 1.25rem;
    background: var(--surface-alt, rgba(0, 0, 0, 0.05));
    border: 1px solid var(--border);
    padding: 0.75rem 1.25rem;
    border-radius: 6px;
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
    font-weight: 700;
  }

  .stat-label {
    font-size: 0.75rem;
    text-transform: uppercase;
    color: var(--muted);
    letter-spacing: 0.05em;
  }

  .stat-divider {
    width: 1px;
    height: 1.75rem;
    background: var(--border);
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
    grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
    gap: 0.75rem;
  }

  .vm-card {
    background: var(--surface-raised, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 0.75rem;
    cursor: pointer;
    transition: transform 0.15s, border-color 0.15s, background 0.15s;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    min-height: 130px;
  }

  .vm-card:hover {
    border-color: var(--accent);
  }

  .vm-card.is-selected {
    border-color: var(--accent);
    background: rgba(var(--accent-rgb, 255, 225, 53), 0.08);
  }

  .vm-card.is-owned {
    border-color: rgba(34, 197, 94, 0.4);
    background: rgba(34, 197, 94, 0.04);
  }

  .card-top-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.25rem;
  }

  .card-vol-num {
    font-weight: 700;
    font-size: 0.95rem;
    flex: 1;
    margin-left: 0.35rem;
  }

  .card-badge {
    font-size: 0.65rem;
    font-weight: 700;
    padding: 0.15rem 0.4rem;
    border-radius: 3px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .badge-owned {
    background: rgba(34, 197, 94, 0.2);
    color: #22c55e;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .badge-published {
    background: rgba(168, 85, 247, 0.2);
    color: #a855f7;
    border: 1px solid rgba(168, 85, 247, 0.3);
  }

  .badge-downloading {
    background: rgba(234, 179, 8, 0.2);
    color: #eab308;
    border: 1px solid rgba(234, 179, 8, 0.3);
  }

  .badge-candidate {
    background: rgba(6, 182, 212, 0.2);
    color: #06b6d4;
    border: 1px solid rgba(6, 182, 212, 0.3);
  }

  .badge-missing {
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .badge-unreleased {
    background: rgba(148, 163, 184, 0.15);
    color: #94a3b8;
    border: 1px solid rgba(148, 163, 184, 0.25);
  }

  .card-body {
    margin: 0.5rem 0;
    font-size: 0.8rem;
  }

  .card-status-label {
    margin: 0 0 0.35rem 0;
    font-size: 0.8rem;
    color: var(--fg);
  }

  .card-candidate-box {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.75rem;
  }

  .cand-provider-tag {
    font-size: 0.65rem;
    font-weight: 700;
    padding: 0.1rem 0.35rem;
    border-radius: 3px;
    background: #0ea5e9;
    color: #fff;
  }

  .cand-provider-tag.madokami {
    background: #ec4899;
  }

  .cand-provider-tag.nyaa {
    background: #3b82f6;
  }

  .card-storage-info {
    font-size: 0.75rem;
    color: var(--muted);
  }

  .card-release-date {
    font-size: 0.75rem;
    color: var(--muted);
  }

  .card-actions {
    margin-top: 0.25rem;
  }

  .cand-btn {
    width: 100%;
    justify-content: center;
  }

  .owned-verified-check {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-size: 0.75rem;
    color: #22c55e;
  }

  /* List Table View */
  .vm-list-wrap {
    overflow-x: auto;
  }

  .vm-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.85rem;
  }

  .vm-table th,
  .vm-table td {
    padding: 0.65rem 0.75rem;
    border-bottom: 1px solid var(--border);
    text-align: left;
  }

  .vm-table th {
    background: var(--surface-alt, rgba(0, 0, 0, 0.03));
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
  }

  .vm-table tr:hover {
    background: var(--surface-raised, rgba(255, 255, 255, 0.02));
  }

  .vm-table tr.row-selected {
    background: rgba(var(--accent-rgb, 255, 225, 53), 0.06);
  }

  /* Batch Modal Styles */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.75);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 1rem;
  }

  .modal-card {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    max-width: 800px;
    width: 100%;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    box-shadow: 0 20px 25px -5px rgba(0, 0, 0, 0.5);
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    padding: 1.25rem 1.5rem;
    border-bottom: 1px solid var(--border);
  }

  .modal-subtitle {
    margin: 0.25rem 0 0 0;
    font-size: 0.85rem;
    color: var(--muted);
  }

  .modal-body {
    padding: 1.25rem 1.5rem;
    overflow-y: auto;
    flex: 1;
  }

  .dry-run-banner {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: rgba(59, 130, 246, 0.1);
    border: 1px solid rgba(59, 130, 246, 0.3);
    color: #60a5fa;
    padding: 0.65rem 0.85rem;
    border-radius: 6px;
    font-size: 0.85rem;
    font-weight: 600;
    margin-bottom: 1rem;
  }

  .plan-summary-metrics {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.75rem;
    margin-bottom: 1.25rem;
  }

  .metric {
    background: var(--surface-alt, rgba(0, 0, 0, 0.05));
    border: 1px solid var(--border);
    padding: 0.75rem;
    border-radius: 6px;
    text-align: center;
  }

  .metric-val {
    display: block;
    font-size: 1.25rem;
    font-weight: 700;
  }

  .metric-label {
    font-size: 0.75rem;
    color: var(--muted);
    text-transform: uppercase;
  }

  .plan-items-table-wrap {
    max-height: 320px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .plan-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.8rem;
  }

  .plan-table th,
  .plan-table td {
    padding: 0.5rem 0.75rem;
    border-bottom: 1px solid var(--border);
    text-align: left;
  }

  .plan-status-badge {
    font-size: 0.7rem;
    font-weight: 700;
    padding: 0.15rem 0.4rem;
    border-radius: 3px;
  }

  .plan-status-badge.eligible {
    background: rgba(34, 197, 94, 0.2);
    color: #22c55e;
  }

  .plan-status-badge.blocked {
    background: rgba(239, 68, 68, 0.15);
    color: #ef4444;
  }

  .backend-tag {
    font-size: 0.75rem;
    background: var(--surface-raised, rgba(255, 255, 255, 0.05));
    padding: 0.15rem 0.4rem;
    border-radius: 3px;
    border: 1px solid var(--border);
  }

  .modal-footer {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 1rem 1.5rem;
    border-top: 1px solid var(--border);
    background: var(--surface-alt, rgba(0, 0, 0, 0.02));
  }

  .modal-buttons {
    display: flex;
    gap: 0.5rem;
  }
</style>
