<script lang="ts">
  import {
    getLists,
    refreshLists,
    message,
    type UnifiedTrackedItem,
    type UnifiedCatalogResponse,
    type TrackerListRefreshStatus,
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

<!-- Title Detail Modal -->
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
      class="modal"
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

        <div class="modal-sections">
          <!-- AniList Tracking Breakdown -->
          <div class="tracker-box">
            <div class="tracker-box-header">
              <span class="tracker-title">AniList</span>
              {#if selectedItem.anilist}
                <span class="pill-badge active">Tracked</span>
              {:else}
                <span class="pill-badge inactive">Not in list</span>
              {/if}
            </div>
            {#if selectedItem.anilist}
              <div class="tracker-details">
                {#if selectedItem.anilist.title && selectedItem.anilist.title.toLowerCase() !== selectedItem.title.toLowerCase()}
                  <div><strong>Provider Title:</strong> {selectedItem.anilist.title}</div>
                {/if}
                <div><strong>Native Status:</strong> {selectedItem.anilist.status}</div>
                <div><strong>Progress:</strong> Vol {selectedItem.anilist.progressVolume ?? 0} / Ch {selectedItem.anilist.progressChapter ?? 0}</div>
                <div><strong>Total Known:</strong> {selectedItem.anilist.totalVolumes ? `${selectedItem.anilist.totalVolumes} volumes` : 'Unknown volumes'}</div>
              </div>
            {:else}
              <p class="tracker-empty">This title is not currently in your AniList library.</p>
            {/if}
          </div>

          <!-- MangaBaka Tracking Breakdown -->
          <div class="tracker-box">
            <div class="tracker-box-header">
              <span class="tracker-title">MangaBaka</span>
              {#if selectedItem.mangabaka}
                <span class="pill-badge active">Tracked</span>
              {:else}
                <span class="pill-badge inactive">Not in library</span>
              {/if}
            </div>
            {#if selectedItem.mangabaka}
              <div class="tracker-details">
                {#if selectedItem.mangabaka.title && selectedItem.mangabaka.title.toLowerCase() !== selectedItem.title.toLowerCase()}
                  <div><strong>Provider Title:</strong> {selectedItem.mangabaka.title}</div>
                {/if}
                <div><strong>Native State:</strong> {selectedItem.mangabaka.status}</div>
                <div><strong>Progress:</strong> Vol {selectedItem.mangabaka.progressVolume ?? 0} / Ch {selectedItem.mangabaka.progressChapter ?? 0}</div>
                <div><strong>Total Known:</strong> {selectedItem.mangabaka.totalVolumes ? `${selectedItem.mangabaka.totalVolumes} volumes` : 'Unknown volumes'}</div>
              </div>
            {:else}
              <p class="tracker-empty">This title is not currently in your MangaBaka personal library.</p>
            {/if}
          </div>
        </div>
      </div>

      <div class="modal-footer">
        <button class="btn" onclick={() => (selectedItem = null)}>Close</button>
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
</style>
