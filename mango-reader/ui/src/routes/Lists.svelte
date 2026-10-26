<script lang="ts">
  import {
    getLists,
    refreshLists,
    planAllMissingContent,
    getAcquisitionSources,
    planByTitle,
    createExecution,
    message,
    type UnifiedTrackedItem,
    type UnifiedCatalogResponse,
    type TrackerListRefreshStatus,
    type GlobalPlanSummary,
    type AcquisitionPlan,
    type AcquisitionSourceOption,
  } from '../lib/api';
  import Cover from '../lib/Cover.svelte';
  import Icon from '../lib/Icon.svelte';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { keep, recall } from '../lib/keep';

  type StatusFilter = 'all' | 'reading' | 'planning' | 'completed' | 'paused' | 'dropped';
  type OwnershipFilter = 'all' | 'owned' | 'partially_owned' | 'not_owned';
  type TrackerFilter = 'all' | 'anilist' | 'mangabaka' | 'both';
  type SortOption = 'updated' | 'title' | 'progress';

  const cached = recall<UnifiedCatalogResponse>('tracked_lists');
  let data = $state<UnifiedCatalogResponse | null>(cached);
  let loading = $state(!cached);
  let refreshing = $state(false);
  let error = $state<string | null>(null);

  // Filters and state
  let statusFilter = $state<StatusFilter>('all');
  let ownershipFilter = $state<OwnershipFilter>('all');
  let trackerFilter = $state<TrackerFilter>('all');
  let searchQuery = $state('');
  let sortOption = $state<SortOption>('updated');

  // Selected title for modal
  let selectedItem = $state<UnifiedTrackedItem | null>(null);

  // Title-level Acquisition Planning Modal state
  let planModalOpen = $state(false);
  let planModalItem = $state<UnifiedTrackedItem | null>(null);
  let sources = $state<AcquisitionSourceOption[]>([]);
  let selectedSourceId = $state<string>('auto');
  let contentMode = $state<'missing' | 'complete' | 'specific'>('missing');
  let specificVolumesInput = $state<string>('');
  let titlePlan = $state<AcquisitionPlan | null>(null);
  let generatingPlan = $state<boolean>(false);
  let approvingPlan = $state<boolean>(false);
  let planError = $state<string | null>(null);

  async function openPlanningModal(item: UnifiedTrackedItem, mode?: 'missing' | 'complete' | 'specific') {
    planModalItem = item;
    contentMode = mode ?? (item.ownershipStatus === 'Partially Owned' ? 'missing' : 'complete');
    selectedSourceId = 'auto';
    specificVolumesInput = '';
    titlePlan = null;
    planError = null;
    planModalOpen = true;

    if (sources.length === 0) {
      try {
        sources = await getAcquisitionSources();
      } catch (e) {
        console.warn('Failed to load acquisition sources:', e);
      }
    }

    generateTitlePlan();
  }

  async function generateTitlePlan() {
    if (!planModalItem) return;
    generatingPlan = true;
    planError = null;
    titlePlan = null;

    let requestedVolumes: number[] | null = null;
    if (contentMode === 'specific' && specificVolumesInput.trim()) {
      const vols: number[] = [];
      const parts = specificVolumesInput.split(',');
      for (const p of parts) {
        const trimmed = p.trim();
        if (trimmed.includes('-')) {
          const [start, end] = trimmed.split('-').map(Number);
          if (!isNaN(start) && !isNaN(end)) {
            for (let v = start; v <= end; v++) vols.push(v);
          }
        } else {
          const num = Number(trimmed);
          if (!isNaN(num)) vols.push(num);
        }
      }
      requestedVolumes = vols;
    }

    try {
      const plan = await planByTitle({
        title: planModalItem.title,
        komgaSeriesId: planModalItem.komgaSeriesId,
        sourceId: selectedSourceId,
        contentMode,
        requestedVolumes,
        totalVolumes: planModalItem.catalog?.totalVolumes ?? planModalItem.totalUnits ?? null,
      });
      titlePlan = plan;
    } catch (e) {
      planError = message(e);
    } finally {
      generatingPlan = false;
    }
  }

  async function approveTitlePlan() {
    if (!titlePlan || !planModalItem) return;
    approvingPlan = true;
    try {
      await createExecution(titlePlan.mangaId, {
        planId: titlePlan.id,
        approved: true,
      });
      toast.success(`Acquisition approved and started for ${titlePlan.title}!`);
      planModalItem.acquisitionStatus = 'Acquiring';
      planModalOpen = false;
      if (selectedItem && selectedItem.id === planModalItem.id) {
        selectedItem.acquisitionStatus = 'Acquiring';
      }
    } catch (e) {
      toast.error(`Failed to approve acquisition: ${message(e)}`);
    } finally {
      approvingPlan = false;
    }
  }

  // Global Plan All Dry-Run state
  let planningAll = $state(false);
  let globalPlanModalOpen = $state(false);
  let globalPlanSummary = $state<GlobalPlanSummary | null>(null);

  async function handlePlanAll() {
    planningAll = true;
    try {
      const summary = await planAllMissingContent();
      globalPlanSummary = summary;
      globalPlanModalOpen = true;
    } catch (e) {
      toast.error(`Plan All failed: ${message(e)}`);
    } finally {
      planningAll = false;
    }
  }

  async function loadData() {
    error = null;
    try {
      const res = await getLists({});
      data = res;
      keep('tracked_lists', res);
    } catch (e) {
      error = message(e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    // Fetch initial list once on mount
    loadData();
  });

  // All tracked items from response
  const allItems = $derived(data?.items ?? []);

  // Items filtered by secondary filters: ownership, tracker, and search (ignoring status)
  // This is the faceted base used to calculate the contextual counts on the status tabs!
  const statusFacetedItems = $derived.by(() => {
    return allItems.filter((item) => {
      // Ownership filter
      if (ownershipFilter !== 'all') {
        const owNorm = ownershipFilter.replace('_', ' ').toLowerCase();
        if (item.ownershipStatus.toLowerCase() !== owNorm) {
          return false;
        }
      }
      // Tracker filter
      if (trackerFilter === 'anilist' && !item.anilist) {
        return false;
      }
      if (trackerFilter === 'mangabaka' && !item.mangabaka) {
        return false;
      }
      if (trackerFilter === 'both' && (!item.anilist || !item.mangabaka)) {
        return false;
      }
      // Search query (matches display title or any alias)
      if (searchQuery.trim()) {
        const q = searchQuery.trim().toLowerCase();
        const matchTitle = item.title.toLowerCase().includes(q);
        const matchAlias = item.aliases?.some((a) => a.toLowerCase().includes(q)) ?? false;
        if (!matchTitle && !matchAlias) {
          return false;
        }
      }
      return true;
    });
  });

  // Faceted counts for status tabs based on statusFacetedItems
  const facetedStatusCounts = $derived.by(() => {
    const counts = {
      all: statusFacetedItems.length,
      reading: 0,
      planning: 0,
      completed: 0,
      paused: 0,
      dropped: 0,
    };
    for (const item of statusFacetedItems) {
      switch (item.normalizedStatus.toLowerCase()) {
        case 'reading':
          counts.reading++;
          break;
        case 'planning':
          counts.planning++;
          break;
        case 'completed':
          counts.completed++;
          break;
        case 'paused':
          counts.paused++;
          break;
        case 'dropped':
          counts.dropped++;
          break;
      }
    }
    return counts;
  });

  // Items filtered by status, tracker, and search (used to calculate faceted ownership counts)
  const ownershipFacetedItems = $derived.by(() => {
    return allItems.filter((item) => {
      if (statusFilter !== 'all' && item.normalizedStatus.toLowerCase() !== statusFilter) {
        return false;
      }
      if (trackerFilter === 'anilist' && !item.anilist) return false;
      if (trackerFilter === 'mangabaka' && !item.mangabaka) return false;
      if (trackerFilter === 'both' && (!item.anilist || !item.mangabaka)) return false;
      if (searchQuery.trim()) {
        const q = searchQuery.trim().toLowerCase();
        const matchTitle = item.title.toLowerCase().includes(q);
        const matchAlias = item.aliases?.some((a) => a.toLowerCase().includes(q)) ?? false;
        if (!matchTitle && !matchAlias) return false;
      }
      return true;
    });
  });

  const facetedOwnershipCounts = $derived.by(() => {
    const counts = {
      all: ownershipFacetedItems.length,
      owned: 0,
      partially_owned: 0,
      not_owned: 0,
    };
    for (const item of ownershipFacetedItems) {
      switch (item.ownershipStatus.toLowerCase()) {
        case 'owned':
          counts.owned++;
          break;
        case 'partially owned':
          counts.partially_owned++;
          break;
        case 'not owned':
          counts.not_owned++;
          break;
      }
    }
    return counts;
  });

  // Final displayed items: statusFacetedItems filtered by statusFilter, then sorted
  const displayItems = $derived.by(() => {
    let items = statusFacetedItems;
    if (statusFilter !== 'all') {
      items = items.filter((item) => item.normalizedStatus.toLowerCase() === statusFilter);
    }
    const sorted = [...items];
    switch (sortOption) {
      case 'title':
        sorted.sort((a, b) => a.title.localeCompare(b.title));
        break;
      case 'progress':
        sorted.sort((a, b) => {
          const ap =
            a.anilist?.progressVolume ??
            a.anilist?.progressChapter ??
            a.mangabaka?.progressVolume ??
            a.mangabaka?.progressChapter ??
            0;
          const bp =
            b.anilist?.progressVolume ??
            b.anilist?.progressChapter ??
            b.mangabaka?.progressVolume ??
            b.mangabaka?.progressChapter ??
            0;
          return bp - ap;
        });
        break;
      default: // 'updated'
        sorted.sort((a, b) => b.updatedAt - a.updatedAt);
        break;
    }
    return sorted;
  });

  async function handleRefresh() {
    refreshing = true;
    try {
      const rep = await refreshLists(null);
      if (rep.errors.length > 0) {
        toast.error(`Refresh had issues: ${rep.errors.join('; ')}`);
      } else {
        toast.success(`Refreshed ${rep.refreshedTrackers.join(' & ')} successfully`);
      }
      await loadData();
    } catch (e) {
      toast.error(message(e));
    } finally {
      refreshing = false;
    }
  }

  function formatProgress(item: UnifiedTrackedItem): string {
    const al = item.anilist;
    const mb = item.mangabaka;

    const vol = al?.progressVolume ?? mb?.progressVolume;
    const ch = al?.progressChapter ?? mb?.progressChapter;

    if (vol && vol > 0) {
      return `Vol ${vol}`;
    }
    if (ch && ch > 0) {
      return `Ch ${ch}`;
    }
    return 'Not started';
  }

  function getStatusClass(status: string): string {
    switch (status.toLowerCase()) {
      case 'reading':
        return 'status-reading';
      case 'planning':
        return 'status-planning';
      case 'completed':
        return 'status-completed';
      case 'paused':
        return 'status-paused';
      case 'dropped':
        return 'status-dropped';
      default:
        return 'status-reading';
    }
  }

  function getOwnershipClass(status: string): string {
    switch (status.toLowerCase()) {
      case 'owned':
        return 'ownership-owned';
      case 'partially owned':
        return 'ownership-partial';
      case 'not owned':
        return 'ownership-not-owned';
      default:
        return 'ownership-unknown';
    }
  }

  function getDistinctAliases(item: UnifiedTrackedItem | null): string[] {
    if (!item || !item.aliases) return [];
    const titleLower = item.title.toLowerCase();
    return item.aliases.filter((a) => a.toLowerCase() !== titleLower);
  }
</script>

<div class="lists-page">
  <header class="header">
    <div class="title-row">
      <div class="title-wrap">
        <h1 class="display title">Lists</h1>
        {#if data?.summary}
          <span class="count-pill">{data.summary.total} tracked</span>
        {/if}
      </div>

      <div class="actions">
        <button
          class="btn pill icon-text"
          onclick={handlePlanAll}
          disabled={planningAll}
          aria-label="Plan All Missing Content (Dry Run)"
        >
          <Icon name={planningAll ? 'spinner' : 'puzzle'} size={18} />
          <span>{planningAll ? 'Planning…' : 'Plan All (Dry Run)'}</span>
        </button>
        <button
          class="btn refresh-btn"
          onclick={handleRefresh}
          disabled={refreshing}
          aria-label="Refresh Lists from AniList and MangaBaka"
        >
          <Icon name={refreshing ? 'spinner' : 'refresh'} size={18} />
          <span>{refreshing ? 'Refreshing...' : 'Refresh Lists'}</span>
        </button>
      </div>
    </div>

    <!-- Provider Refresh Notices if stale/error -->
    {#if data?.refreshStatuses}
      {#each data.refreshStatuses as st}
        {#if st.status !== 'ok'}
          <div class="provider-warning" role="alert">
            <Icon name="info" size={16} />
            <span>
              <strong>{st.tracker === 'anilist' ? 'AniList' : 'MangaBaka'}:</strong>
              {st.errorMessage || 'Provider unavailable'} (showing cached catalog).
            </span>
          </div>
        {/if}
      {/each}
    {/if}

    <!-- Search & Sort Row -->
    <div class="toolbar">
      <div class="search-box">
        <Icon name="search" size={18} />
        <input
          type="search"
          placeholder="Filter tracked titles..."
          bind:value={searchQuery}
          aria-label="Filter tracked titles"
        />
        {#if searchQuery}
          <button class="clear-btn" onclick={() => (searchQuery = '')} aria-label="Clear search">
            <Icon name="close" size={16} />
          </button>
        {/if}
      </div>

      <div class="sort-box">
        <label for="sort-select" class="sr">Sort by</label>
        <select id="sort-select" bind:value={sortOption}>
          <option value="updated">Recently updated</option>
          <option value="title">Title (A-Z)</option>
          <option value="progress">Highest progress</option>
        </select>
      </div>
    </div>

    <!-- Filter Tabs -->
    <div class="filters-container">
      <!-- Status Filters -->
      <div class="pill-group" role="tablist" aria-label="Status filter">
        <button
          class="pill"
          class:active={statusFilter === 'all'}
          onclick={() => (statusFilter = 'all')}
          role="tab"
          aria-selected={statusFilter === 'all'}
        >
          All {data ? `(${facetedStatusCounts.all})` : ''}
        </button>
        <button
          class="pill"
          class:active={statusFilter === 'reading'}
          onclick={() => (statusFilter = 'reading')}
          role="tab"
          aria-selected={statusFilter === 'reading'}
        >
          Reading {data ? `(${facetedStatusCounts.reading})` : ''}
        </button>
        <button
          class="pill"
          class:active={statusFilter === 'planning'}
          onclick={() => (statusFilter = 'planning')}
          role="tab"
          aria-selected={statusFilter === 'planning'}
        >
          Planning {data ? `(${facetedStatusCounts.planning})` : ''}
        </button>
        <button
          class="pill"
          class:active={statusFilter === 'completed'}
          onclick={() => (statusFilter = 'completed')}
          role="tab"
          aria-selected={statusFilter === 'completed'}
        >
          Completed {data ? `(${facetedStatusCounts.completed})` : ''}
        </button>
        <button
          class="pill"
          class:active={statusFilter === 'paused'}
          onclick={() => (statusFilter = 'paused')}
          role="tab"
          aria-selected={statusFilter === 'paused'}
        >
          Paused {data ? `(${facetedStatusCounts.paused})` : ''}
        </button>
        <button
          class="pill"
          class:active={statusFilter === 'dropped'}
          onclick={() => (statusFilter = 'dropped')}
          role="tab"
          aria-selected={statusFilter === 'dropped'}
        >
          Dropped {data ? `(${facetedStatusCounts.dropped})` : ''}
        </button>
      </div>

      <!-- Ownership & Tracker Secondary Filters -->
      <div class="secondary-filters">
        <div class="filter-dropdown">
          <span class="filter-label">Ownership:</span>
          <select bind:value={ownershipFilter} aria-label="Filter by local ownership">
            <option value="all">All ownership ({facetedOwnershipCounts.all})</option>
            <option value="owned">Owned ({facetedOwnershipCounts.owned})</option>
            <option value="partially_owned">Partially Owned ({facetedOwnershipCounts.partially_owned})</option>
            <option value="not_owned">Not Owned ({facetedOwnershipCounts.not_owned})</option>
          </select>
        </div>

        <div class="filter-dropdown">
          <span class="filter-label">Trackers:</span>
          <select bind:value={trackerFilter} aria-label="Filter by tracker presence">
            <option value="all">All trackers</option>
            <option value="anilist">AniList</option>
            <option value="mangabaka">MangaBaka</option>
            <option value="both">Both trackers</option>
          </select>
        </div>
      </div>
    </div>
  </header>

  <!-- Error or Loading Notices -->
  {#if error}
    <Notice error={error} retry={loadData} />
  {:else if loading && !data}
    <div class="loading-state">
      <Icon name="spinner" size={32} />
      <p>Loading tracked catalog...</p>
    </div>
  {:else if !data || data.items.length === 0}
    <div class="empty-state">
      <div class="empty-icon"><Icon name="bookmark" size={48} /></div>
      <h2 class="display">No tracked titles found</h2>
      <p class="empty-msg">
        Your tracked catalog is empty. Click "Refresh Lists" to import your manga lists from AniList and MangaBaka.
      </p>
      <button class="btn primary" onclick={handleRefresh} disabled={refreshing}>
        <Icon name={refreshing ? 'spinner' : 'refresh'} size={18} />
        <span>Refresh Lists</span>
      </button>
    </div>
  {:else if displayItems.length === 0}
    <div class="empty-state">
      <div class="empty-icon"><Icon name="bookmark" size={48} /></div>
      <h2 class="display">No matching titles</h2>
      <p class="empty-msg">
        No titles matched the selected filters.
      </p>
    </div>
  {:else}
    <!-- Grid of Tracked Cards -->
    <div class="grid" role="list">
      {#each displayItems as item (item.id)}
        <button
          type="button"
          class="card-wrap"
          onclick={() => (selectedItem = item)}
          aria-label={`View details for ${item.title}`}
        >
          <div class="art">
            <Cover src={item.coverUrl ?? null} title={item.title} />
            <div class="badges-overlay">
              <span class="badge status-badge {getStatusClass(item.normalizedStatus)}">
                {item.normalizedStatus}
              </span>
              <span class="badge ownership-badge {getOwnershipClass(item.ownershipStatus)}">
                {item.ownershipStatus}
              </span>
            </div>
          </div>

          <div class="card-info">
            <h3 class="card-title" title={item.title}>{item.title}</h3>

            <div class="card-meta">
              <span class="progress-info">{formatProgress(item)}</span>
              <div class="tracker-tags">
                <span class="tracker-tag" class:active={!!item.anilist} title="AniList tracking status">
                  AL {item.anilist ? '✓' : '—'}
                </span>
                <span class="tracker-tag" class:active={!!item.mangabaka} title="MangaBaka tracking status">
                  MB {item.mangabaka ? '✓' : '—'}
                </span>
              </div>
            </div>
          </div>
        </button>
      {/each}
    </div>
  {/if}
</div>

<!-- Title Detail Modal (4 Distinct Sections) -->
{#if selectedItem}
  <div
    class="modal-backdrop"
    onclick={() => (selectedItem = null)}
    onkeydown={(e) => {
      if (e.key === 'Escape') selectedItem = null;
    }}
    role="presentation"
  >
    <div
      class="modal four-section-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="modal-title"
      tabindex="-1"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => {
        if (e.key === 'Escape') selectedItem = null;
      }}
    >
      <div class="modal-header">
        <h2 id="modal-title" class="display modal-title">{selectedItem.title}</h2>
        <button class="btn icon close-btn" onclick={() => (selectedItem = null)} aria-label="Close dialog">
          <Icon name="close" size={20} />
        </button>
      </div>

      <div class="modal-body">
        <div class="modal-top">
          <div class="modal-cover">
            <Cover src={selectedItem.coverUrl ?? null} title={selectedItem.title} />
          </div>
          <div class="modal-summary">
            <div class="summary-row">
              <span class="summary-label">Status</span>
              <span class="badge {getStatusClass(selectedItem.normalizedStatus)}">
                {selectedItem.normalizedStatus}
              </span>
            </div>
            <div class="summary-row">
              <span class="summary-label">Local Ownership</span>
              <span class="badge {getOwnershipClass(selectedItem.ownershipStatus)}">
                {selectedItem.ownershipStatus}
                {#if selectedItem.ownedUnits !== undefined && selectedItem.ownedUnits !== null}
                  ({selectedItem.ownedUnits} {selectedItem.totalUnits ? `/ ${selectedItem.totalUnits}` : 'units'})
                {/if}
              </span>
            </div>
            {#if selectedItem.komgaSeriesId}
              <div class="summary-row">
                <span class="summary-label">Komga Series</span>
                <span class="summary-value monospace">{selectedItem.komgaSeriesId}</span>
              </div>
            {/if}
            {#if getDistinctAliases(selectedItem).length > 0}
              {@const distinctAliases = getDistinctAliases(selectedItem)}
              <div class="summary-row">
                <span class="summary-label">Aliases</span>
                <span class="summary-value dim">
                  {distinctAliases.slice(0, 4).join(' · ')}
                  {#if distinctAliases.length > 4}
                    (+{distinctAliases.length - 4} more)
                  {/if}
                </span>
              </div>
            {/if}
            <div class="summary-row">
              <span class="summary-label">Acquisition</span>
              <span class="summary-value">
                {selectedItem.acquisitionStatus}
                {selectedItem.autoAcquire ? '(Auto-acquire enabled)' : '(No auto-acquire)'}
              </span>
            </div>
          </div>
        </div>

        <div class="modal-sections-four">
          <!-- 1. CATALOG (MangaBaka) -->
          <div class="section-box catalog-box">
            <div class="section-box-header">
              <span class="section-title">
                <Icon name="bookmark" size={16} />
                1. CATALOG (MangaBaka)
              </span>
              {#if selectedItem.catalog?.webUrl}
                <a href={selectedItem.catalog.webUrl} target="_blank" rel="noreferrer" class="external-link">
                  MangaBaka Entry ↗
                </a>
              {/if}
            </div>
            <div class="catalog-details">
              {#if selectedItem.catalog?.title && selectedItem.catalog.title.toLowerCase() !== selectedItem.title.toLowerCase()}
                <div class="detail-row"><strong>Canonical Title:</strong> {selectedItem.catalog.title}</div>
              {/if}
              {#if selectedItem.catalog?.authors && selectedItem.catalog.authors.length > 0}
                <div class="detail-row"><strong>Authors:</strong> {selectedItem.catalog.authors.join(', ')}</div>
              {/if}
              {#if selectedItem.catalog?.artists && selectedItem.catalog.artists.length > 0}
                <div class="detail-row"><strong>Artists:</strong> {selectedItem.catalog.artists.join(', ')}</div>
              {/if}
              {#if selectedItem.catalog?.publisher}
                <div class="detail-row"><strong>Publisher:</strong> {selectedItem.catalog.publisher}</div>
              {/if}
              <div class="detail-row">
                <strong>Total Volumes:</strong> {selectedItem.catalog?.totalVolumes ?? selectedItem.totalUnits ?? 'Unknown'}
              </div>
              {#if selectedItem.catalog?.genres && selectedItem.catalog.genres.length > 0}
                <div class="detail-tags-row">
                  <strong>Genres:</strong>
                  <div class="tag-list">
                    {#each selectedItem.catalog.genres as genre}
                      <span class="genre-pill">{genre}</span>
                    {/each}
                  </div>
                </div>
              {/if}
              {#if selectedItem.catalog?.tags && selectedItem.catalog.tags.length > 0}
                <div class="detail-tags-row">
                  <strong>Tags:</strong>
                  <div class="tag-list">
                    {#each selectedItem.catalog.tags.slice(0, 8) as tag}
                      <span class="tag-pill">{tag}</span>
                    {/each}
                  </div>
                </div>
              {/if}
              {#if selectedItem.catalog?.summary}
                <div class="detail-summary">
                  <strong>Summary:</strong>
                  <p class="summary-text">{selectedItem.catalog.summary}</p>
                </div>
              {/if}
            </div>
          </div>

          <!-- 2. LOCAL OWNERSHIP (Komga) -->
          <div class="section-box ownership-box">
            <div class="section-box-header">
              <span class="section-title">
                <Icon name="book" size={16} />
                2. LOCAL OWNERSHIP (Komga)
              </span>
              <span class="badge {getOwnershipClass(selectedItem.ownershipStatus)}">
                {selectedItem.ownershipStatus}
              </span>
            </div>
            <div class="ownership-details">
              <div class="detail-row">
                <strong>Physical Media:</strong>
                <span>
                  {selectedItem.ownedUnits ?? 0}
                  {selectedItem.totalUnits ? `/ ${selectedItem.totalUnits} volumes owned` : 'volumes owned'}
                </span>
              </div>
              {#if selectedItem.komgaSeriesId}
                <div class="detail-row">
                  <strong>Komga Series ID:</strong>
                  <span class="monospace">{selectedItem.komgaSeriesId}</span>
                </div>
              {/if}
              <div class="detail-row">
                <strong>Source of Truth:</strong> Canonical Komga library on disk
              </div>
            </div>
          </div>

          <!-- 3. TRACKERS (AniList & MangaBaka) -->
          <div class="section-box trackers-box">
            <div class="section-box-header">
              <span class="section-title">
                <Icon name="link" size={16} />
                3. TRACKERS (Personal Progress)
              </span>
            </div>
            <div class="trackers-columns">
              <!-- AniList -->
              <div class="tracker-subcol">
                <div class="tracker-subcol-title">
                  <strong>AniList</strong>
                  <span class="pill-badge {selectedItem.anilist ? 'active' : 'inactive'}">
                    {selectedItem.anilist ? 'Tracked' : 'Not in list'}
                  </span>
                </div>
                {#if selectedItem.anilist}
                  <div class="tracker-meta">
                    <div>Status: <strong>{selectedItem.anilist.status}</strong></div>
                    <div>Progress: <strong>Vol {selectedItem.anilist.progressVolume ?? 0} / Ch {selectedItem.anilist.progressChapter ?? 0}</strong></div>
                    <div>Known: <strong>{selectedItem.anilist.totalVolumes ? `${selectedItem.anilist.totalVolumes} vols` : 'Unknown'}</strong></div>
                  </div>
                {:else}
                  <p class="dim">Not tracked on AniList</p>
                {/if}
              </div>

              <!-- MangaBaka -->
              <div class="tracker-subcol">
                <div class="tracker-subcol-title">
                  <strong>MangaBaka</strong>
                  <span class="pill-badge {selectedItem.mangabaka ? 'active' : 'inactive'}">
                    {selectedItem.mangabaka ? 'Tracked' : 'Not in library'}
                  </span>
                </div>
                {#if selectedItem.mangabaka}
                  <div class="tracker-meta">
                    <div>Status: <strong>{selectedItem.mangabaka.status}</strong></div>
                    <div>Progress: <strong>Vol {selectedItem.mangabaka.progressVolume ?? 0} / Ch {selectedItem.mangabaka.progressChapter ?? 0}</strong></div>
                    <div>Known: <strong>{selectedItem.mangabaka.totalVolumes ? `${selectedItem.mangabaka.totalVolumes} vols` : 'Unknown'}</strong></div>
                  </div>
                {:else}
                  <p class="dim">Not in MangaBaka personal library</p>
                {/if}
              </div>
            </div>
          </div>

          <!-- 4. ACQUISITION -->
          <div class="section-box acquisition-box">
            <div class="section-box-header">
              <span class="section-title">
                <Icon name="download" size={16} />
                4. ACQUISITION
              </span>
              <span class="pill-badge {selectedItem.acquisitionStatus === 'Acquiring' ? 'active' : 'idle'}">
                {selectedItem.acquisitionStatus}
              </span>
            </div>
            <div class="acquisition-actions">
              {#if selectedItem.ownershipStatus === 'Owned'}
                <div class="acquisition-status-note">
                  ✓ Library Complete: All known volumes are owned in Komga.
                </div>
                <button
                  class="btn small"
                  onclick={() => {
                    const it = selectedItem;
                    selectedItem = null;
                    if (it) openPlanningModal(it, 'specific');
                  }}
                >
                  <Icon name="search" size={16} />
                  Re-check / Re-acquire Specific Volumes
                </button>
              {:else if selectedItem.acquisitionStatus === 'Acquiring'}
                <div class="acquisition-status-note">
                  ⏳ Acquisition is currently in progress via execution pipeline.
                </div>
              {:else if selectedItem.ownershipStatus === 'Partially Owned'}
                <div class="acquisition-status-note">
                  Missing {selectedItem.totalUnits ? `${selectedItem.totalUnits - (selectedItem.ownedUnits ?? 0)} volumes` : 'volumes'}.
                </div>
                <button
                  class="btn spot"
                  onclick={() => {
                    const it = selectedItem;
                    selectedItem = null;
                    if (it) openPlanningModal(it, 'missing');
                  }}
                >
                  <Icon name="download" size={16} />
                  Add Missing to Library
                </button>
              {:else}
                <div class="acquisition-status-note">
                  Not currently in Komga library.
                </div>
                <button
                  class="btn spot"
                  onclick={() => {
                    const it = selectedItem;
                    selectedItem = null;
                    if (it) openPlanningModal(it, 'complete');
                  }}
                >
                  <Icon name="download" size={16} />
                  Add to Library
                </button>
              {/if}
            </div>
          </div>
        </div>
      </div>

      <div class="modal-footer">
        <button class="btn" onclick={() => (selectedItem = null)}>Close</button>
      </div>
    </div>
  </div>
{/if}

<!-- Acquisition Planning Modal -->
{#if planModalOpen && planModalItem}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={() => (planModalOpen = false)}>
    <div class="modal-content planning-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-header">
        <div>
          <h2 class="modal-title">Acquisition Plan — {planModalItem.title}</h2>
          <span class="modal-subtitle">Review release candidates, target missing content, and approve safe execution</span>
        </div>
        <button class="btn icon close-btn" onclick={() => (planModalOpen = false)} aria-label="Close dialog">
          <Icon name="close" size={20} />
        </button>
      </div>

      <div class="modal-body-scroll">
        <!-- Configuration Controls -->
        <div class="plan-controls-grid">
          <!-- Content Selection -->
          <div class="control-group">
            <span class="control-label"><strong>Content to Acquire:</strong></span>
            <div class="segmented-control">
              <button
                class="segment-btn"
                class:active={contentMode === 'missing'}
                onclick={() => {
                  contentMode = 'missing';
                  generateTitlePlan();
                }}
              >
                Missing Volumes
              </button>
              <button
                class="segment-btn"
                class:active={contentMode === 'complete'}
                onclick={() => {
                  contentMode = 'complete';
                  generateTitlePlan();
                }}
              >
                Complete Series
              </button>
              <button
                class="segment-btn"
                class:active={contentMode === 'specific'}
                onclick={() => {
                  contentMode = 'specific';
                }}
              >
                Specific Volumes
              </button>
            </div>
            {#if contentMode === 'specific'}
              <div class="specific-input-wrap">
                <input
                  type="text"
                  class="specific-input"
                  bind:value={specificVolumesInput}
                  placeholder="e.g. 1, 2, 5-8, 39"
                />
                <button class="btn small" onclick={generateTitlePlan} disabled={generatingPlan}>
                  Apply
                </button>
              </div>
            {/if}
          </div>

          <!-- Source Selection -->
          <div class="control-group">
            <label class="control-label" for="plan-source-select"><strong>Acquisition Source:</strong></label>
            <select
              id="plan-source-select"
              class="source-select"
              bind:value={selectedSourceId}
              onchange={generateTitlePlan}
            >
              {#if sources.length === 0}
                <option value="auto">Auto (Recommended)</option>
                <option value="nyaa">Nyaa (BitTorrent / TorBox)</option>
              {:else}
                {#each sources as src}
                  <option value={src.id} disabled={!src.available}>
                    {src.name} {!src.available ? '(Unavailable)' : src.isDefault ? '(Default)' : ''}
                  </option>
                {/each}
              {/if}
            </select>
          </div>
        </div>

        <!-- Plan Status / Feedback -->
        {#if generatingPlan}
          <div class="loading-state">
            <Icon name="spinner" size={28} />
            <p>Discovering candidates and generating dry-run plan…</p>
          </div>
        {:else if planError}
          <div class="plan-error-box">
            <Icon name="alert-triangle" size={20} />
            <div>
              <strong>Plan Generation Warning</strong>
              <p>{planError}</p>
            </div>
          </div>
        {:else if titlePlan}
          <!-- Plan Summary Cards -->
          <div class="plan-summary-metrics">
            <div class="metric-card">
              <span class="metric-val">{titlePlan.missingUnits.length}</span>
              <span class="metric-lbl">Missing Units</span>
            </div>
            <div class="metric-card">
              <span class="metric-val">{titlePlan.missingUnitsCoveredCount}</span>
              <span class="metric-lbl">Covered Units</span>
            </div>
            <div class="metric-card">
              <span class="metric-val">{titlePlan.selectedItems.length}</span>
              <span class="metric-lbl">Releases to Acquire</span>
            </div>
            <div class="metric-card">
              <span class="metric-val">{titlePlan.unresolvedUnits.length}</span>
              <span class="metric-lbl">Unresolved Units</span>
            </div>
          </div>

          <!-- Candidates Preview -->
          {#if titlePlan.selectedItems.length === 0}
            <div class="plan-empty-notice">
              <Icon name="info" size={24} />
              <div>
                <strong>No Candidate Releases Selected</strong>
                <p>
                  {#if titlePlan.missingUnits.length === 0}
                    All volumes in the requested range are already present in your Komga library.
                  {:else}
                    No releases matched the quality and profile criteria for the missing content.
                  {/if}
                </p>
              </div>
            </div>
          {:else}
            <div class="planned-items-list">
              <h4>Matched Candidate Releases ({titlePlan.selectedItems.length})</h4>
              {#each titlePlan.selectedItems as item, idx}
                <div class="planned-candidate-row">
                  <div class="cand-header">
                    <span class="cand-idx">#{idx + 1}</span>
                    <strong class="cand-title">{item.candidate.releaseTitle}</strong>
                    <span class="cand-score-badge">
                      Score: {item.candidate.smartScore.total.toFixed(0)}/100
                    </span>
                  </div>
                  <div class="cand-meta">
                    <span>Source: <strong>{item.candidate.source}</strong></span>
                    {#if item.candidate.uploader}
                      <span>Group: <strong>{item.candidate.uploader}</strong></span>
                    {/if}
                    <span>Seeders: <strong>{item.candidate.seeders}</strong></span>
                    <span>Size: <strong>{((item.candidate.fileSizeBytes ?? 0) / 1024 / 1024).toFixed(1)} MB</strong></span>
                    <span>Units Covered: <strong>v{item.expectedUnits.join(', v')}</strong></span>
                  </div>
                </div>
              {/each}
            </div>
          {/if}

          <!-- Warnings -->
          {#if titlePlan.warnings.length > 0}
            <div class="plan-warnings-box">
              <div class="warning-title">
                <Icon name="alert-triangle" size={16} />
                <strong>Pre-flight Warnings:</strong>
              </div>
              <ul>
                {#each titlePlan.warnings as w}
                  <li>{w}</li>
                {/each}
              </ul>
            </div>
          {/if}
        {/if}
      </div>

      <div class="modal-footer">
        <button class="btn" onclick={() => (planModalOpen = false)}>Cancel</button>
        <button
          class="btn primary"
          onclick={approveTitlePlan}
          disabled={!titlePlan || titlePlan.selectedItems.length === 0 || approvingPlan}
        >
          <Icon name={approvingPlan ? 'spinner' : 'check'} size={18} />
          <span>{approvingPlan ? 'Approving…' : 'Approve Acquisition'}</span>
        </button>
      </div>
    </div>
  </div>
{/if}

{#if globalPlanModalOpen && globalPlanSummary}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={() => (globalPlanModalOpen = false)}>
    <div class="modal-content global-plan-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-header">
        <div>
          <h3>Plan All Missing Content (Dry Run)</h3>
          <span class="muted">{globalPlanSummary.totalSeriesAnalyzed} series analyzed across library & tracked catalog</span>
        </div>
        <button class="btn icon close-btn" onclick={() => (globalPlanModalOpen = false)} aria-label="Close">
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body">
        <div class="summary-stats-grid">
          <div class="stat-card">
            <span class="stat-num">{globalPlanSummary.totalVolumesToAcquire}</span>
            <span class="stat-label">Volumes Would Acquire</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">{globalPlanSummary.totalCollectionsCount}</span>
            <span class="stat-label">Collections</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">{globalPlanSummary.totalIndividualCount}</span>
            <span class="stat-label">Individual Releases</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">{(globalPlanSummary.estimatedTotalBytes / (1024 * 1024 * 1024)).toFixed(1)} GiB</span>
            <span class="stat-label">Estimated Size</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">{globalPlanSummary.alreadySatisfiedCount}</span>
            <span class="stat-label">Already Satisfied</span>
          </div>
          <div class="stat-card">
            <span class="stat-num">{globalPlanSummary.needsReviewCount}</span>
            <span class="stat-label">Needs Review</span>
          </div>
        </div>

        <div class="dry-run-safety-box" style="margin-top: 1rem;">
          <Icon name="check" size={16} />
          <div>
            <strong>Dry Run Mode:</strong>
            <span>No external downloads are initiated. No torrents submitted to TorBox. Execution remains strictly manual.</span>
          </div>
        </div>

        <h4 style="margin-top: 1.25rem; margin-bottom: 0.5rem;">Series Plans ({globalPlanSummary.seriesPlanned.length})</h4>
        {#if globalPlanSummary.seriesPlanned.length === 0}
          <p class="muted">No series currently have unfulfilled missing content plans.</p>
        {:else}
          <div class="series-planned-list">
            {#each globalPlanSummary.seriesPlanned as plan (plan.id)}
              <div class="series-plan-item">
                <div class="plan-item-top">
                  <strong>{plan.seriesTitle}</strong>
                  <span class="tag {plan.confidence === 'high' ? 'spot' : 'warn'}">
                    {plan.confidence === 'high' ? 'HIGH CONFIDENCE' : 'REVIEW RECOMMENDED'}
                  </span>
                </div>
                <div class="plan-item-sub">
                  <span>Missing: <strong>{plan.missingUnits.length} volumes</strong></span>
                  <span>Would acquire: <strong>{plan.selectedItems.length} releases</strong> ({plan.missingUnitsCoveredCount} vols covered)</span>
                  {#if plan.totalEstimatedSize > 0}
                    <span>Est: <strong>{(plan.totalEstimatedSize / (1024 * 1024 * 1024)).toFixed(2)} GiB</strong></span>
                  {/if}
                  <span>Profile: <em>{plan.profileName}</em></span>
                </div>
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <div class="modal-footer">
        <button class="btn spot" onclick={() => (globalPlanModalOpen = false)}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .lists-page {
    padding: 24px;
    max-width: 1400px;
    margin: 0 auto;
  }

  .header {
    display: flex;
    flex-direction: column;
    gap: 16px;
    margin-bottom: 24px;
  }

  .title-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .title-wrap {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .title {
    font-size: 2rem;
    font-weight: 800;
    margin: 0;
  }

  .count-pill {
    font-size: 0.85rem;
    padding: 4px 10px;
    background: var(--shade);
    border-radius: 9999px;
    color: var(--dim);
    font-weight: 600;
  }

  .refresh-btn {
    display: flex;
    align-items: center;
    gap: 8px;
    font-weight: 600;
    padding: 8px 16px;
  }

  .provider-warning {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 14px;
    background: rgba(232, 69, 44, 0.1);
    color: #e8452c;
    border-radius: 8px;
    font-size: 0.88rem;
  }

  .toolbar {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
  }

  .search-box {
    display: flex;
    align-items: center;
    gap: 8px;
    background: var(--shade);
    padding: 0 12px;
    border-radius: 8px;
    flex: 1;
    min-width: 240px;
    height: 40px;
  }

  .search-box input {
    border: none;
    background: transparent;
    color: inherit;
    width: 100%;
    outline: none;
    font-size: 0.95rem;
  }

  .clear-btn {
    background: transparent;
    border: none;
    color: var(--dim);
    cursor: pointer;
    display: flex;
  }

  .sort-box select,
  .filter-dropdown select {
    background: var(--shade);
    color: inherit;
    border: 1px solid transparent;
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 0.9rem;
    cursor: pointer;
    outline: none;
  }

  .filters-container {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .pill-group {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }

  .pill {
    border: none;
    background: var(--shade);
    color: var(--dim);
    padding: 6px 14px;
    border-radius: 9999px;
    font-size: 0.88rem;
    font-weight: 600;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .pill:hover {
    color: var(--ink);
  }

  .pill.active {
    background: var(--ink);
    color: var(--paper);
  }

  .secondary-filters {
    display: flex;
    gap: 16px;
    flex-wrap: wrap;
    align-items: center;
  }

  .filter-dropdown {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.88rem;
  }

  .filter-label {
    color: var(--dim);
    font-weight: 500;
  }

  /* Grid & Cards */
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 20px;
  }

  .card-wrap {
    display: flex;
    flex-direction: column;
    gap: 8px;
    cursor: pointer;
    border-radius: 8px;
    outline: none;
    transition: transform 0.15s ease;
    background: transparent;
    border: none;
    padding: 0;
    text-align: left;
    color: inherit;
    font-family: inherit;
  }

  .card-wrap:hover {
    transform: translateY(-2px);
  }

  .card-wrap:focus-visible {
    box-shadow: 0 0 0 2px var(--brand, #2f6fde);
  }

  .art {
    position: relative;
    aspect-ratio: 2 / 3;
    border-radius: 8px;
    overflow: hidden;
    background: var(--shade);
  }

  .badges-overlay {
    position: absolute;
    bottom: 6px;
    left: 6px;
    right: 6px;
    display: flex;
    flex-direction: column;
    gap: 4px;
    pointer-events: none;
  }

  .badge {
    font-size: 0.72rem;
    font-weight: 700;
    padding: 3px 6px;
    border-radius: 4px;
    text-transform: uppercase;
    letter-spacing: 0.03em;
    width: fit-content;
    box-shadow: 0 2px 4px rgba(0, 0, 0, 0.4);
  }

  .status-reading {
    background: #2f6fde;
    color: #fff;
  }

  .status-planning {
    background: #0284c7;
    color: #fff;
  }

  .status-completed {
    background: #16a34a;
    color: #fff;
  }

  .status-paused {
    background: #d97706;
    color: #fff;
  }

  .status-dropped {
    background: #64748b;
    color: #fff;
  }

  .ownership-owned {
    background: rgba(22, 163, 74, 0.9);
    color: #fff;
  }

  .ownership-partial {
    background: rgba(217, 119, 6, 0.9);
    color: #fff;
  }

  .ownership-not-owned {
    background: rgba(15, 23, 42, 0.8);
    color: #cbd5e1;
  }

  .card-info {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .card-title {
    font-size: 0.92rem;
    font-weight: 700;
    margin: 0;
    line-height: 1.3;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .card-meta {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 0.8rem;
    color: var(--dim);
  }

  .tracker-tags {
    display: flex;
    gap: 4px;
  }

  .tracker-tag {
    font-size: 0.7rem;
    font-weight: 700;
    padding: 1px 4px;
    border-radius: 3px;
    background: var(--shade);
    color: var(--dim);
  }

  .tracker-tag.active {
    background: rgba(47, 111, 222, 0.15);
    color: #2f6fde;
  }

  /* Empty & Loading states */
  .loading-state,
  .empty-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 64px 16px;
    text-align: center;
    color: var(--dim);
  }

  .empty-icon {
    margin-bottom: 16px;
    opacity: 0.6;
  }

  .empty-msg {
    max-width: 480px;
    margin-bottom: 24px;
    line-height: 1.5;
  }

  /* Modal */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 1000;
    padding: 16px;
  }

  .modal {
    background: var(--paper);
    color: var(--ink);
    border-radius: 12px;
    width: 100%;
    max-width: 600px;
    max-height: 90vh;
    overflow-y: auto;
    box-shadow: 0 10px 25px rgba(0, 0, 0, 0.3);
    display: flex;
    flex-direction: column;
  }

  .modal-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    padding: 16px 20px;
    border-bottom: 1px solid var(--shade);
  }

  .modal-title {
    font-size: 1.25rem;
    font-weight: 800;
    margin: 0;
  }

  .close-btn {
    background: transparent;
    border: none;
    cursor: pointer;
  }

  .modal-body {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 20px;
  }

  .modal-top {
    display: flex;
    gap: 16px;
  }

  .modal-cover {
    width: 110px;
    aspect-ratio: 2 / 3;
    flex-shrink: 0;
    border-radius: 6px;
    overflow: hidden;
  }

  .modal-summary {
    display: flex;
    flex-direction: column;
    gap: 8px;
    flex: 1;
  }

  .summary-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 0.9rem;
  }

  .summary-label {
    width: 120px;
    color: var(--dim);
    font-weight: 500;
  }

  .summary-value.monospace {
    font-family: monospace;
    font-size: 0.85rem;
  }

  .modal-sections {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .tracker-box {
    background: var(--shade);
    border-radius: 8px;
    padding: 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .tracker-box-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .tracker-title {
    font-weight: 700;
    font-size: 0.95rem;
  }

  .pill-badge {
    font-size: 0.75rem;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 9999px;
  }

  .pill-badge.active {
    background: rgba(22, 163, 74, 0.15);
    color: #16a34a;
  }

  .pill-badge.inactive {
    background: rgba(100, 116, 139, 0.15);
    color: #64748b;
  }

  .tracker-details {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 0.88rem;
  }

  .tracker-empty {
    font-size: 0.85rem;
    color: var(--dim);
    margin: 0;
  }

  .modal-footer {
    display: flex;
    justify-content: flex-end;
    padding: 12px 20px;
    border-top: 1px solid var(--shade);
  }

  /* Global Plan Modal */
  .global-plan-modal {
    max-width: 720px;
  }

  .summary-stats-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(105px, 1fr));
    gap: 0.75rem;
    margin-bottom: 1rem;
  }

  .stat-card {
    background: var(--card-bg, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 8px;
    padding: 0.75rem 0.5rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 0.25rem;
  }

  .stat-num {
    font-size: 1.25rem;
    font-weight: 700;
    color: var(--spot, #ff7a00);
  }

  .stat-label {
    font-size: 0.75rem;
    color: var(--muted, #888);
  }

  .dry-run-safety-box {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.75rem 1rem;
    background: rgba(255, 122, 0, 0.08);
    border: 1px solid rgba(255, 122, 0, 0.25);
    border-radius: 6px;
    font-size: 0.82rem;
  }

  .series-planned-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    max-height: 340px;
    overflow-y: auto;
  }

  .series-plan-item {
    background: var(--card-bg, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 6px;
    padding: 0.75rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .plan-item-top {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .plan-item-sub {
    display: flex;
    flex-wrap: wrap;
    gap: 1rem;
    font-size: 0.82rem;
    color: var(--muted, #888);
  }

  /* 4-Section Detail Modal Styles */
  .four-section-modal {
    max-width: 680px;
    width: 95%;
    max-height: 90vh;
    overflow-y: auto;
  }

  .modal-sections-four {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    margin-top: 1rem;
  }

  .section-box {
    background: var(--shade);
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  }

  .section-box-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    padding-bottom: 0.5rem;
  }

  .section-title {
    font-weight: 700;
    font-size: 0.95rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .external-link {
    font-size: 0.8rem;
    color: var(--spot, #ff7a00);
    text-decoration: none;
  }

  .external-link:hover {
    text-decoration: underline;
  }

  .detail-row {
    font-size: 0.88rem;
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
  }

  .detail-tags-row {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    font-size: 0.85rem;
  }

  .tag-list {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }

  .genre-pill {
    font-size: 0.75rem;
    background: rgba(255, 122, 0, 0.12);
    color: var(--spot, #ff7a00);
    padding: 2px 8px;
    border-radius: 4px;
  }

  .tag-pill {
    font-size: 0.72rem;
    background: rgba(255, 255, 255, 0.08);
    color: var(--text, #eee);
    padding: 2px 6px;
    border-radius: 4px;
  }

  .detail-summary {
    font-size: 0.85rem;
    margin-top: 0.25rem;
  }

  .summary-text {
    font-size: 0.82rem;
    line-height: 1.4;
    color: var(--muted, #aaa);
    margin: 0.25rem 0 0;
    max-height: 120px;
    overflow-y: auto;
  }

  .trackers-columns {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 1rem;
  }

  @media (max-width: 540px) {
    .trackers-columns {
      grid-template-columns: 1fr;
    }
  }

  .tracker-subcol {
    background: rgba(0, 0, 0, 0.15);
    border-radius: 6px;
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .tracker-subcol-title {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 0.85rem;
  }

  .tracker-meta {
    font-size: 0.82rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .acquisition-actions {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .acquisition-status-note {
    font-size: 0.88rem;
    color: var(--muted, #aaa);
  }

  /* Planning Modal Styles */
  .planning-modal {
    max-width: 650px;
    width: 95%;
  }

  .modal-subtitle {
    font-size: 0.8rem;
    color: var(--muted, #888);
  }

  .modal-body-scroll {
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
    padding: 1.25rem;
    max-height: 70vh;
    overflow-y: auto;
  }

  .plan-controls-grid {
    display: flex;
    flex-direction: column;
    gap: 1rem;
    background: var(--shade);
    border-radius: 8px;
    padding: 1rem;
  }

  .control-group {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .control-label {
    font-size: 0.85rem;
  }

  .segmented-control {
    display: flex;
    background: rgba(0, 0, 0, 0.2);
    border-radius: 6px;
    padding: 3px;
    gap: 3px;
  }

  .segment-btn {
    flex: 1;
    background: transparent;
    border: none;
    color: var(--muted, #888);
    padding: 6px 12px;
    font-size: 0.82rem;
    font-weight: 600;
    border-radius: 4px;
    cursor: pointer;
    transition: background 0.15s, color 0.15s;
  }

  .segment-btn.active {
    background: var(--card-bg, #222);
    color: var(--text, #fff);
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.3);
  }

  .specific-input-wrap {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.25rem;
  }

  .specific-input {
    flex: 1;
    background: var(--card-bg, #1a1a1a);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.15));
    border-radius: 4px;
    padding: 6px 10px;
    color: var(--text, #fff);
    font-size: 0.85rem;
  }

  .source-select {
    background: var(--card-bg, #1a1a1a);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.15));
    border-radius: 4px;
    padding: 8px 10px;
    color: var(--text, #fff);
    font-size: 0.88rem;
  }

  .plan-summary-metrics {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 0.5rem;
  }

  .metric-card {
    background: var(--shade);
    border-radius: 6px;
    padding: 0.75rem;
    text-align: center;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .metric-val {
    font-size: 1.3rem;
    font-weight: 700;
  }

  .metric-lbl {
    font-size: 0.72rem;
    color: var(--muted, #888);
  }

  .planned-items-list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .planned-items-list h4 {
    margin: 0 0 0.25rem;
    font-size: 0.9rem;
  }

  .planned-candidate-row {
    background: var(--shade);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .cand-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
  }

  .cand-idx {
    font-size: 0.8rem;
    color: var(--muted, #888);
  }

  .cand-title {
    font-size: 0.85rem;
    flex: 1;
  }

  .cand-score-badge {
    font-size: 0.75rem;
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
    padding: 2px 8px;
    border-radius: 4px;
    font-weight: 600;
  }

  .cand-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
    font-size: 0.78rem;
    color: var(--muted, #888);
  }

  .plan-empty-notice {
    display: flex;
    gap: 0.75rem;
    background: rgba(255, 255, 255, 0.03);
    border-radius: 6px;
    padding: 1rem;
    font-size: 0.85rem;
  }

  .plan-warnings-box {
    background: rgba(234, 179, 8, 0.1);
    border: 1px solid rgba(234, 179, 8, 0.3);
    border-radius: 6px;
    padding: 0.75rem 1rem;
    font-size: 0.82rem;
  }

  .warning-title {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    color: #eab308;
    margin-bottom: 0.35rem;
  }

  .plan-warnings-box ul {
    margin: 0;
    padding-left: 1.25rem;
  }

  .plan-error-box {
    display: flex;
    gap: 0.75rem;
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.3);
    border-radius: 6px;
    padding: 0.75rem 1rem;
    font-size: 0.85rem;
  }
</style>
