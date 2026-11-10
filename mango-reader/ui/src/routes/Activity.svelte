<script lang="ts">
  import { onMount } from 'svelte';
  import {
    get,
    getWatchersDashboard,
    refreshWatcherById,
    executeWatchersBulkAction,
    getReviewQueue,
    approveReviewQueueItem,
    rejectReviewQueueItem,
    skipReviewQueueItem,
    batchApproveReviewQueue,
    getMonitoringActivity,
    triggerMonitoringRefresh,
    message,
    type WatchersDashboardResponseDto,
    type TitleAvailabilityWatcherDto,
    type ReviewQueueItem,
    type MonitoringActivityEntry,
  } from '../lib/api';
  import CandidateInspectorModal from '../lib/CandidateInspectorModal.svelte';
  import LifecycleTimeline from '../lib/LifecycleTimeline.svelte';
  import Icon from '../lib/Icon.svelte';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { ago, formatBytes, formatLastChecked, formatNextCheck } from '../lib/util';
  import {
    groupReviewQueueBySeries,
    type SeriesGroupedReviewQueue,
  } from '../lib/volumeStatus';
  import Downloads from './Downloads.svelte';
  import { go, route } from '../lib/router.svelte';

  // Active tab state: synchronizes with ?tab= query parameter
  type ActivityTab = 'inbox' | 'downloads' | 'monitoring' | 'history';

  let activeTab = $state<ActivityTab>('inbox');

  $effect(() => {
    const qTab = route.query.get('tab');
    if (qTab === 'downloads' || qTab === 'monitoring' || qTab === 'history' || qTab === 'inbox') {
      activeTab = qTab;
    }
  });

  function setTab(tab: ActivityTab) {
    activeTab = tab;
    go(`/activity?tab=${tab}`);
  }

  // ---------------------------------------------------------------------------
  // 1. INBOX: Series-Grouped Acquisition Inbox
  // ---------------------------------------------------------------------------
  let reviewItems = $state<ReviewQueueItem[]>([]);
  let reviewLoading = $state(false);
  let reviewError = $state<string | null>(null);
  let reviewStatusFilter = $state<string>('pending');
  let selectedReviewIds = $state(new Set<number>());
  let reviewActionBusy = $state(false);
  let rejectingItemId = $state<number | null>(null);
  let rejectReasonInput = $state('');
  let expandedSeriesGroups = $state<Record<string, boolean>>({});
  let inspectingVolumeTarget = $state<{ mangaId: number; volume: number } | null>(null);

  async function loadReviewQueue() {
    reviewLoading = true;
    reviewError = null;
    try {
      reviewItems = await getReviewQueue(reviewStatusFilter === 'all' ? undefined : reviewStatusFilter);
    } catch (e) {
      reviewError = message(e);
      toast.error(reviewError);
    } finally {
      reviewLoading = false;
    }
  }

  const groupedSeriesInbox = $derived.by<SeriesGroupedReviewQueue[]>(() => {
    if (!reviewItems.length) return [];
    return groupReviewQueueBySeries(reviewItems);
  });

  const totalPendingProposals = $derived.by(() => {
    return reviewItems.filter((i) => i.status === 'pending').length;
  });

  async function handleApproveReviewItem(id: number) {
    reviewActionBusy = true;
    try {
      const res = await approveReviewQueueItem(id);
      toast.success(res.queued ? 'Approved and queued for download' : 'Approved');
      await loadReviewQueue();
    } catch (e) {
      toast.error(message(e));
    } finally {
      reviewActionBusy = false;
    }
  }

  async function handleRejectReviewItem(id: number, reason?: string) {
    reviewActionBusy = true;
    try {
      await rejectReviewQueueItem(id, reason);
      toast.info('Release candidate rejected');
      rejectingItemId = null;
      rejectReasonInput = '';
      await loadReviewQueue();
    } catch (e) {
      toast.error(message(e));
    } finally {
      reviewActionBusy = false;
    }
  }

  async function handleSkipReviewItem(id: number, reason?: string) {
    reviewActionBusy = true;
    try {
      await skipReviewQueueItem(id, reason);
      toast.info('Release candidate skipped');
      await loadReviewQueue();
    } catch (e) {
      toast.error(message(e));
    } finally {
      reviewActionBusy = false;
    }
  }

  async function handleBatchApprove(ids: number[]) {
    if (ids.length === 0) return;
    reviewActionBusy = true;
    try {
      const res = await batchApproveReviewQueue(ids);
      toast.success(`Approved ${res.approved} of ${res.total} proposals`);
      selectedReviewIds.clear();
      await loadReviewQueue();
    } catch (e) {
      toast.error(message(e));
    } finally {
      reviewActionBusy = false;
    }
  }

  function toggleReviewSelect(id: number, checked: boolean) {
    const next = new Set(selectedReviewIds);
    if (checked) next.add(id);
    else next.delete(id);
    selectedReviewIds = next;
  }

  function toggleSeriesExpand(seriesId: string) {
    expandedSeriesGroups = {
      ...expandedSeriesGroups,
      [seriesId]: !expandedSeriesGroups[seriesId],
    };
  }

  // ---------------------------------------------------------------------------
  // 2. MONITORING: Availability Watchers & Discovery
  // ---------------------------------------------------------------------------
  let watchersData = $state<WatchersDashboardResponseDto | null>(null);
  let watchersLoading = $state<boolean>(false);
  let watchersError = $state<string | null>(null);
  let watchersFilter = $state<string>('ALL');
  let watchersSearch = $state<string>('');
  let watchersProviderFilter = $state<string>('ALL');
  let watchersSortField = $state<'title' | 'volume' | 'state' | 'lastChecked' | 'nextCheck'>('title');
  let watchersSortDir = $state<'asc' | 'desc'>('asc');
  let watchersPage = $state<number>(1);
  let watchersPageSize = $state<number>(25);
  let watchersViewMode = $state<'grouped' | 'flat'>('grouped');
  let expandedWatcherSeries = $state<Record<string, boolean>>({});
  let selectedWatcherIds = $state<Set<number>>(new Set());
  let refreshingWatchers = $state<Record<number, boolean>>({});
  let bulkBusy = $state(false);

  async function loadWatchers() {
    watchersLoading = true;
    watchersError = null;
    try {
      watchersData = await getWatchersDashboard();
    } catch (e) {
      watchersError = message(e);
      toast.error(watchersError);
    } finally {
      watchersLoading = false;
    }
  }

  async function handleRefreshWatcher(id: number) {
    if (refreshingWatchers[id]) return;
    refreshingWatchers[id] = true;
    try {
      const updated = await refreshWatcherById(id);
      if (updated && watchersData) {
        const idx = watchersData.watchers.findIndex((w) => w.id === id);
        if (idx !== -1) {
          watchersData.watchers[idx] = updated;
        }
        toast.success(`Checked Vol ${updated.volumeNumber}: ${updated.state}`);
      }
    } catch (e) {
      toast.error(message(e));
    } finally {
      refreshingWatchers[id] = false;
    }
  }

  async function handleCheckAllWatchers() {
    if (bulkBusy) return;
    bulkBusy = true;
    try {
      const res = await triggerMonitoringRefresh();
      toast.info(res.message);
      await loadWatchers();
    } catch (e) {
      toast.error(message(e));
    } finally {
      bulkBusy = false;
    }
  }

  const processedWatchers = $derived.by(() => {
    if (!watchersData?.watchers) return [];
    let list = watchersData.watchers.filter((w) => {
      if (watchersSearch.trim()) {
        const q = watchersSearch.toLowerCase();
        const matchTitle = (w.seriesTitle ?? '').toLowerCase().includes(q);
        const matchCand = (w.topCandidateTitle ?? '').toLowerCase().includes(q);
        if (!matchTitle && !matchCand) return false;
      }
      if (watchersFilter !== 'ALL' && w.state !== watchersFilter) {
        return false;
      }
      if (watchersProviderFilter !== 'ALL') {
        const matchLast = w.lastProviderChecked?.toLowerCase() === watchersProviderFilter.toLowerCase();
        const matchCand = w.topCandidateProvider?.toLowerCase() === watchersProviderFilter.toLowerCase();
        if (!matchLast && !matchCand) return false;
      }
      return true;
    });

    list.sort((a, b) => {
      let cmp = 0;
      if (watchersSortField === 'title') {
        cmp = (a.seriesTitle ?? '').localeCompare(b.seriesTitle ?? '');
        if (cmp === 0) cmp = a.volumeNumber - b.volumeNumber;
      } else if (watchersSortField === 'volume') {
        cmp = a.volumeNumber - b.volumeNumber;
      } else if (watchersSortField === 'state') {
        cmp = (a.state ?? '').localeCompare(b.state ?? '');
      } else if (watchersSortField === 'lastChecked') {
        cmp = (a.lastCheckedAt ?? 0) - (b.lastCheckedAt ?? 0);
      } else if (watchersSortField === 'nextCheck') {
        cmp = (a.nextCheckAt ?? 0) - (b.nextCheckAt ?? 0);
      }
      return watchersSortDir === 'asc' ? cmp : -cmp;
    });

    return list;
  });

  const groupedWatchersBySeries = $derived.by(() => {
    const map = new Map<string, TitleAvailabilityWatcherDto[]>();
    for (const w of processedWatchers) {
      const key = w.seriesTitle || `manga:${w.mangaId}` || w.seriesId;
      if (!map.has(key)) map.set(key, []);
      map.get(key)!.push(w);
    }
    return Array.from(map.entries()).map(([title, watchers]) => ({
      title,
      mangaId: watchers[0].mangaId,
      watchers,
      totalCount: watchers.length,
      candidateFoundCount: watchers.filter((w) => w.state === 'ELIGIBLE_CANDIDATE_FOUND').length,
      awaitingCount: watchers.filter((w) => w.state === 'RELEASED_AWAITING_PROVIDER').length,
      ownedCount: watchers.filter((w) => w.state === 'OWNED').length,
    }));
  });

  const paginatedWatchers = $derived.by(() => {
    const start = (watchersPage - 1) * watchersPageSize;
    return processedWatchers.slice(start, start + watchersPageSize);
  });

  const totalWatchersPages = $derived(Math.max(1, Math.ceil(processedWatchers.length / watchersPageSize)));

  // ---------------------------------------------------------------------------
  // 3. HISTORY: Past Acquisitions & Activity Log
  // ---------------------------------------------------------------------------
  let activityEntries = $state<MonitoringActivityEntry[]>([]);
  let activityLoading = $state(false);
  let activityError = $state<string | null>(null);

  async function loadActivityHistory() {
    activityLoading = true;
    activityError = null;
    try {
      activityEntries = await getMonitoringActivity(100);
    } catch (e) {
      activityError = message(e);
      toast.error(activityError);
    } finally {
      activityLoading = false;
    }
  }

  onMount(() => {
    loadReviewQueue();
    loadWatchers();
    loadActivityHistory();
  });
</script>

<div class="page activity-page">
  <!-- Activity Header -->
  <header class="activity-header">
    <div class="activity-title-row">
      <div class="activity-title-col">
        <h1 class="display title-main">ACTIVITY &amp; ACQUISITIONS</h1>
        <p class="title-sub">
          Centralized acquisition inbox, in-flight transfers, release monitoring, and audit log.
        </p>
      </div>

      <!-- Quick Action Controls -->
      <div class="header-actions">
        {#if activeTab === 'inbox'}
          <button class="btn pill" onclick={loadReviewQueue} disabled={reviewLoading}>
            <Icon name="refresh" size={14} /> Refresh Inbox
          </button>
        {:else if activeTab === 'monitoring'}
          <button class="btn pill" onclick={handleCheckAllWatchers} disabled={bulkBusy}>
            <Icon name="refresh" size={14} /> Check All Watchers
          </button>
        {:else if activeTab === 'history'}
          <button class="btn pill" onclick={loadActivityHistory} disabled={activityLoading}>
            <Icon name="refresh" size={14} /> Refresh History
          </button>
        {/if}
      </div>
    </div>

    <!-- Primary Subnavigation Tabs (M13.2) -->
    <div class="activity-tabs" role="tablist" aria-label="Activity Subnavigation">
      <button
        class="activity-tab-btn"
        class:active={activeTab === 'inbox'}
        role="tab"
        aria-selected={activeTab === 'inbox'}
        onclick={() => setTab('inbox')}
      >
        <Icon name="bookmark" size={16} />
        <span>Acquisition Inbox</span>
        {#if totalPendingProposals > 0}
          <span class="tab-badge">{totalPendingProposals}</span>
        {/if}
      </button>

      <button
        class="activity-tab-btn"
        class:active={activeTab === 'downloads'}
        role="tab"
        aria-selected={activeTab === 'downloads'}
        onclick={() => setTab('downloads')}
      >
        <Icon name="download" size={16} />
        <span>Downloads &amp; Queue</span>
      </button>

      <button
        class="activity-tab-btn"
        class:active={activeTab === 'monitoring'}
        role="tab"
        aria-selected={activeTab === 'monitoring'}
        onclick={() => setTab('monitoring')}
      >
        <Icon name="refresh" size={16} />
        <span>Monitoring &amp; Watchers</span>
      </button>

      <button
        class="activity-tab-btn"
        class:active={activeTab === 'history'}
        role="tab"
        aria-selected={activeTab === 'history'}
        onclick={() => setTab('history')}
      >
        <Icon name="book" size={16} />
        <span>History &amp; Audit</span>
      </button>
    </div>
  </header>

  <!-- TAB 1: ACQUISITION INBOX -->
  {#if activeTab === 'inbox'}
    <div class="tab-pane inbox-pane" data-testid="acquisition-inbox">
      <!-- Filter Toolbar -->
      <div class="inbox-toolbar">
        <div class="filter-pills" role="tablist" aria-label="Proposal Status">
          {#each ['pending', 'approved', 'rejected', 'skipped', 'all'] as st}
            <button
              class="btn pill sm"
              class:spot={reviewStatusFilter === st}
              onclick={() => {
                reviewStatusFilter = st;
                loadReviewQueue();
              }}
            >
              {st.toUpperCase()}
            </button>
          {/each}
        </div>

        {#if selectedReviewIds.size > 0}
          <div class="batch-bar">
            <span class="batch-count">{selectedReviewIds.size} selected</span>
            <button
              class="btn spot sm"
              onclick={() => handleBatchApprove(Array.from(selectedReviewIds))}
              disabled={reviewActionBusy}
            >
              <Icon name="check" size={14} /> Approve Selected
            </button>
            <button class="btn subtle sm" onclick={() => selectedReviewIds.clear()}>
              Clear
            </button>
          </div>
        {/if}
      </div>

      <Notice loading={reviewLoading} error={reviewError} retry={loadReviewQueue} />

      {#if !reviewLoading && !reviewError}
        {#if groupedSeriesInbox.length === 0}
          <div class="panel empty-inbox-panel">
            <Icon name="check" size={28} />
            <h3 class="display">NO PROPOSALS IN INBOX</h3>
            <p class="muted">
              {reviewStatusFilter === 'pending'
                ? 'All release candidates have been reviewed or acquired.'
                : `No proposals match status "${reviewStatusFilter}".`}
            </p>
          </div>
        {:else}
          <!-- SERIES-GROUPED INBOX LIST (M13.2) -->
          <div class="series-groups-list">
            {#each groupedSeriesInbox as group (group.seriesId)}
              {@const isExpanded = expandedSeriesGroups[group.seriesId]}
              <div class="series-group-card">
                <!-- Series Group Header -->
                <div class="group-header">
                  <div class="group-title-col">
                    <div class="group-title-row">
                      {#if group.mangaId}
                        <a href={`#/title/${group.mangaId}`} class="group-title-link">
                          <h2 class="display group-title">{group.seriesTitle}</h2>
                        </a>
                      {:else}
                        <h2 class="display group-title">{group.seriesTitle}</h2>
                      {/if}
                      <span class="tag spot">
                        {group.ownedCount}{group.totalOfficialCount ? ` / ${group.totalOfficialCount}` : ''} OWNED IN KOMGA
                      </span>
                      {#if group.nextMissingVolume != null}
                        <span class="tag next-missing-tag">
                          <Icon name="arrow" size={12} /> NEXT MISSING: VOL {group.nextMissingVolume}
                        </span>
                      {/if}
                    </div>

                    <div class="group-meta-row">
                      <span class="group-meta-pill">
                        <strong>{group.pendingCount}</strong> pending proposal{group.pendingCount === 1 ? '' : 's'}
                      </span>
                      {#if group.outOfOrderCount > 0}
                        <span class="group-meta-pill warn">
                          {group.outOfOrderCount} out-of-order
                        </span>
                      {/if}
                      {#if group.mangaId}
                        <a href={`#/title/${group.mangaId}`} class="view-series-link">
                          <Icon name="book" size={13} /> View Series Workspace
                        </a>
                      {/if}
                    </div>
                  </div>

                  <div class="group-header-actions">
                    {#if group.primaryProposals.length > 0}
                      <button
                        class="btn sm spot"
                        onclick={() => handleApproveReviewItem(group.primaryProposals[0].id)}
                        disabled={reviewActionBusy}
                        title={`Approve Volume ${group.primaryProposals[0].volumeNumber}`}
                      >
                        <Icon name="check" size={14} /> Approve Next (Vol {group.primaryProposals[0].volumeNumber})
                      </button>
                    {/if}
                  </div>
                </div>

                <!-- Primary Actionable Proposals (Next Missing in Sequence) -->
                <div class="proposals-container">
                  {#if group.primaryProposals.length > 0}
                    <div class="primary-proposals-section">
                      <div class="section-label-row">
                        <span class="section-label primary-label">
                          <Icon name="check" size={13} /> Next Eligible Volumes (Recommended for Acquisition)
                        </span>
                      </div>

                      <div class="proposals-list">
                        {#each group.primaryProposals as prop (prop.id)}
                          <div class="proposal-card primary-card">
                            <div class="prop-check-col">
                              <input
                                type="checkbox"
                                checked={selectedReviewIds.has(prop.id)}
                                onchange={(e) => toggleReviewSelect(prop.id, e.currentTarget.checked)}
                                aria-label={`Select Volume ${prop.volumeNumber}`}
                              />
                            </div>

                            <div class="prop-info-col">
                              <div class="prop-top-line">
                                <strong class="prop-vol">Volume {prop.volumeNumber}</strong>
                                <span class="provider-pill {prop.provider}">{prop.provider.toUpperCase()}</span>
                                <span class="score-pill">{Math.round(prop.qualityScore)}% Match</span>
                                <span class="conf-pill">{prop.confidence} Confidence</span>
                                {#if prop.fileSizeBytes}
                                  <span class="size-pill">{formatBytes(prop.fileSizeBytes)}</span>
                                {/if}
                              </div>

                              <p class="prop-title" title={prop.rawTitle || prop.title}>
                                {prop.title || prop.rawTitle}
                              </p>

                              <p class="prop-ordering-note">
                                <Icon name="info" size={13} />
                                {prop.orderingExplanation || 'Next sequential volume in library'}
                              </p>
                            </div>

                            <div class="prop-actions-col">
                              <button
                                class="btn sm subtle"
                                onclick={() => {
                                  inspectingVolumeTarget = {
                                    mangaId: prop.mangaId || 0,
                                    volume: prop.volumeNumber,
                                  };
                                }}
                                title="Inspect candidate file details"
                              >
                                <Icon name="search" size={13} /> Inspect
                              </button>
                              <button
                                class="btn sm spot"
                                onclick={() => handleApproveReviewItem(prop.id)}
                                disabled={reviewActionBusy}
                                title="Approve candidate for acquisition"
                              >
                                <Icon name="check" size={13} /> Approve
                              </button>
                              <button
                                class="btn sm subtle"
                                onclick={() => handleSkipReviewItem(prop.id)}
                                disabled={reviewActionBusy}
                                title="Skip for now"
                              >
                                Skip
                              </button>
                              <button
                                class="btn sm subtle danger-text"
                                onclick={() => (rejectingItemId = prop.id)}
                                disabled={reviewActionBusy}
                                title="Reject candidate"
                              >
                                Reject
                              </button>
                            </div>
                          </div>
                        {/each}
                      </div>
                    </div>
                  {/if}

                  <!-- Out of Order Proposals Section (Collapsible) -->
                  {#if group.outOfOrderProposals.length > 0}
                    <div class="secondary-proposals-section">
                      <div class="expand-header-row">
                        <button
                          class="btn text sm expand-toggle-btn"
                          onclick={() => toggleSeriesExpand(group.seriesId)}
                        >
                          <Icon name={isExpanded ? 'arrow' : 'forward'} size={13} />
                          <span>
                            {isExpanded ? 'Hide' : 'Show'} {group.outOfOrderCount} Out-of-Order Volume{group.outOfOrderCount === 1 ? '' : 's'}
                            (Volumes {group.outOfOrderProposals.map((p) => p.volumeNumber).join(', ')})
                          </span>
                        </button>
                      </div>

                      {#if isExpanded}
                        <div class="proposals-list out-of-order-list">
                          {#each group.outOfOrderProposals as prop (prop.id)}
                            <div class="proposal-card out-of-order-card">
                              <div class="prop-check-col">
                                <input
                                  type="checkbox"
                                  checked={selectedReviewIds.has(prop.id)}
                                  onchange={(e) => toggleReviewSelect(prop.id, e.currentTarget.checked)}
                                  aria-label={`Select Volume ${prop.volumeNumber}`}
                                />
                              </div>

                              <div class="prop-info-col">
                                <div class="prop-top-line">
                                  <strong class="prop-vol">Volume {prop.volumeNumber}</strong>
                                  <span class="provider-pill {prop.provider}">{prop.provider.toUpperCase()}</span>
                                  <span class="score-pill">{Math.round(prop.qualityScore)}% Match</span>
                                  <span class="tag warn">Out of Order</span>
                                </div>

                                <p class="prop-title">{prop.title || prop.rawTitle}</p>

                                <p class="prop-ordering-warning">
                                  <Icon name="alert" size={13} />
                                  {prop.orderingExplanation || 'Earlier missing volumes detected'}
                                </p>
                              </div>

                              <div class="prop-actions-col">
                                <button
                                  class="btn sm subtle"
                                  onclick={() => {
                                    inspectingVolumeTarget = {
                                      mangaId: prop.mangaId || 0,
                                      volume: prop.volumeNumber,
                                    };
                                  }}
                                >
                                  <Icon name="search" size={13} /> Inspect
                                </button>
                                <button
                                  class="btn sm subtle"
                                  onclick={() => handleApproveReviewItem(prop.id)}
                                  disabled={reviewActionBusy}
                                >
                                  Approve
                                </button>
                                <button
                                  class="btn sm subtle"
                                  onclick={() => handleSkipReviewItem(prop.id)}
                                  disabled={reviewActionBusy}
                                >
                                  Skip
                                </button>
                              </div>
                            </div>
                          {/each}
                        </div>
                      {/if}
                    </div>
                  {/if}
                </div>
              </div>
            {/each}
          </div>
        {/if}
      {/if}
    </div>

  <!-- TAB 2: DOWNLOADS & QUEUE -->
  {:else if activeTab === 'downloads'}
    <div class="tab-pane downloads-pane" data-testid="downloads-center">
      <Downloads />
    </div>

  <!-- TAB 3: MONITORING & WATCHERS -->
  {:else if activeTab === 'monitoring'}
    <div class="tab-pane monitoring-pane" data-testid="watchers-dashboard">
      <Notice loading={watchersLoading} error={watchersError} retry={loadWatchers} />

      {#if !watchersLoading && !watchersError && watchersData}
        <!-- Stats Summary Bar -->
        <div class="stat-grid watchers-stat-grid">
          <button type="button" class="stat-card" class:selected={watchersFilter === 'ALL'} onclick={() => { watchersFilter = 'ALL'; watchersPage = 1; }}>
            <span class="stat-val">{watchersData.summary.totalWatchers}</span>
            <span class="stat-lbl">Total Watchers</span>
          </button>
          <button type="button" class="stat-card" class:selected={watchersFilter === 'UPCOMING'} onclick={() => { watchersFilter = 'UPCOMING'; watchersPage = 1; }}>
            <span class="stat-val" style="color: var(--status-info, #2563eb);">{watchersData.summary.upcomingCount}</span>
            <span class="stat-lbl">Upcoming</span>
          </button>
          <button type="button" class="stat-card" class:selected={watchersFilter === 'RELEASED_AWAITING_PROVIDER'} onclick={() => { watchersFilter = 'RELEASED_AWAITING_PROVIDER'; watchersPage = 1; }}>
            <span class="stat-val" style="color: var(--status-warn, #d97706);">{watchersData.summary.releasedAwaitingCount}</span>
            <span class="stat-lbl">Awaiting Upload</span>
          </button>
          <button type="button" class="stat-card" class:selected={watchersFilter === 'ELIGIBLE_CANDIDATE_FOUND'} onclick={() => { watchersFilter = 'ELIGIBLE_CANDIDATE_FOUND'; watchersPage = 1; }}>
            <span class="stat-val" style="color: var(--spot, #ff7a00);">{watchersData.summary.candidateFoundCount}</span>
            <span class="stat-lbl">Candidates Found</span>
          </button>
          <button type="button" class="stat-card" class:selected={watchersFilter === 'OWNED'} onclick={() => { watchersFilter = 'OWNED'; watchersPage = 1; }}>
            <span class="stat-val" style="color: var(--status-ok, #16a34a);">{watchersData.summary.ownedCount}</span>
            <span class="stat-lbl">Owned / Complete</span>
          </button>
        </div>

        <!-- Filter & Search Toolbar -->
        <div class="table-toolbar">
          <div class="toolbar-left">
            <input
              type="search"
              class="field search-field"
              placeholder="Search series or candidates…"
              bind:value={watchersSearch}
              oninput={() => watchersPage = 1}
            />

            <!-- Group by Series Toggle (M13.2) -->
            <div class="view-toggle" role="group" aria-label="Watchers grouping layout">
              <button
                class="btn pill sm"
                class:spot={watchersViewMode === 'grouped'}
                onclick={() => (watchersViewMode = 'grouped')}
              >
                Group by Series
              </button>
              <button
                class="btn pill sm"
                class:spot={watchersViewMode === 'flat'}
                onclick={() => (watchersViewMode = 'flat')}
              >
                All Volumes
              </button>
            </div>
          </div>

          <div class="bulk-actions-group">
            <button class="btn mini" onclick={handleCheckAllWatchers} disabled={bulkBusy}>
              <Icon name="refresh" size={12} /> Check All
            </button>
          </div>
        </div>

        <!-- Grouped Series View (M13.2) -->
        {#if watchersViewMode === 'grouped'}
          <div class="watchers-series-list">
            {#each groupedWatchersBySeries as item (item.title)}
              {@const isExp = expandedWatcherSeries[item.title]}
              <div class="watcher-series-card">
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <!-- svelte-ignore a11y_no_static_element_interactions -->
                <div class="wsc-header" onclick={() => {
                  expandedWatcherSeries = {
                    ...expandedWatcherSeries,
                    [item.title]: !expandedWatcherSeries[item.title],
                  };
                }}>
                  <div class="wsc-title-col">
                    <strong class="wsc-title">{item.title}</strong>
                    <span class="wsc-stats">
                      {item.totalCount} tracked volume{item.totalCount === 1 ? '' : 's'} · {item.ownedCount} owned · {item.candidateFoundCount} candidates
                    </span>
                  </div>

                  <div class="wsc-actions">
                    <button class="btn pill sm">
                      <Icon name={isExp ? 'arrow' : 'forward'} size={12} />
                      {isExp ? 'Collapse' : 'Expand Volumes'}
                    </button>
                  </div>
                </div>

                {#if isExp}
                  <div class="wsc-volumes-table-wrap">
                    <table class="data-table font-small">
                      <thead>
                        <tr>
                          <th>Volume</th>
                          <th>Status</th>
                          <th>Best Candidate</th>
                          <th>Last Checked</th>
                          <th>Actions</th>
                        </tr>
                      </thead>
                      <tbody>
                        {#each item.watchers as w (w.id)}
                          <tr>
                            <td><strong>Vol {w.volumeNumber}</strong></td>
                            <td><span class="status-pill {w.state}">{w.state}</span></td>
                            <td>
                              {#if w.topCandidateTitle}
                                <span>{w.topCandidateTitle}</span>
                              {:else}
                                <span class="muted">None discovered</span>
                              {/if}
                            </td>
                            <td><span class="muted">{formatLastChecked(w.lastCheckedAt)}</span></td>
                            <td>
                              <button
                                class="btn mini subtle"
                                onclick={() => handleRefreshWatcher(w.id)}
                                disabled={refreshingWatchers[w.id]}
                              >
                                Check
                              </button>
                            </td>
                          </tr>
                        {/each}
                      </tbody>
                    </table>
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {:else}
          <!-- Flat Table View -->
          <div class="data-table-wrap">
            <table class="data-table font-small">
              <thead>
                <tr>
                  <th>Series Title</th>
                  <th>Vol</th>
                  <th>Status</th>
                  <th>Best Candidate</th>
                  <th>Last Checked</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {#each paginatedWatchers as w (w.id)}
                  <tr>
                    <td><strong>{w.seriesTitle || `Manga #${w.mangaId}`}</strong></td>
                    <td>Vol {w.volumeNumber}</td>
                    <td><span class="status-pill {w.state}">{w.state}</span></td>
                    <td>{w.topCandidateTitle || 'None'}</td>
                    <td>{formatLastChecked(w.lastCheckedAt)}</td>
                    <td>
                      <button
                        class="btn mini subtle"
                        onclick={() => handleRefreshWatcher(w.id)}
                        disabled={refreshingWatchers[w.id]}
                      >
                        Check
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

  <!-- TAB 4: HISTORY & AUDIT -->
  {:else if activeTab === 'history'}
    <div class="tab-pane history-pane" data-testid="activity-history">
      <Notice loading={activityLoading} error={activityError} retry={loadActivityHistory} />

      {#if !activityLoading && !activityError}
        {#if activityEntries.length === 0}
          <div class="panel empty-history-panel">
            <p class="muted">No monitoring activity recorded yet.</p>
          </div>
        {:else}
          <div class="data-table-wrap">
            <table class="data-table font-small">
              <thead>
                <tr>
                  <th>Timestamp</th>
                  <th>Series</th>
                  <th>Vol</th>
                  <th>Action</th>
                  <th>Source</th>
                  <th>Details</th>
                </tr>
              </thead>
              <tbody>
                {#each activityEntries as entry (entry.id)}
                  <tr>
                    <td><span class="muted">{ago(entry.createdAt * 1000)}</span></td>
                    <td><strong>{entry.seriesTitle}</strong></td>
                    <td>Vol {entry.volumeNumber}</td>
                    <td><span class="tag">{entry.action}</span></td>
                    <td>{entry.source}</td>
                    <td>{entry.details || '—'}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {/if}
    </div>
  {/if}
</div>

{#if inspectingVolumeTarget}
  <CandidateInspectorModal
    open={true}
    mangaId={inspectingVolumeTarget.mangaId}
    volume={inspectingVolumeTarget.volume}
    onClose={() => { inspectingVolumeTarget = null; }}
  />
{/if}

<style>
  .activity-page {
    max-width: var(--page-max-width, 1560px);
    margin: 0 auto;
    padding: 1.5rem var(--gutter);
  }

  .activity-header {
    border-bottom: 3px solid var(--ink);
    padding-bottom: 1rem;
    margin-bottom: 1.5rem;
  }

  .activity-title-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1.25rem;
  }

  .title-main {
    font-size: 2rem;
    line-height: 1;
    margin: 0 0 0.4rem 0;
  }

  .title-sub {
    font-size: 0.9rem;
    color: var(--muted);
    margin: 0;
  }

  .activity-tabs {
    display: flex;
    gap: 0.5rem;
    overflow-x: auto;
    padding-bottom: 2px;
  }

  .activity-tab-btn {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 0.55rem 1.1rem;
    border: 2px solid var(--ink);
    background: var(--surface);
    color: var(--ink);
    font-size: 0.85rem;
    font-weight: 800;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    cursor: pointer;
    box-shadow: var(--sh-3);
    transition: all 0.12s ease;
  }

  .activity-tab-btn:hover {
    transform: translate(-1px, -1px);
    box-shadow: var(--sh-4);
  }

  .activity-tab-btn.active {
    background: var(--ink);
    color: var(--paper);
  }

  .tab-badge {
    background: var(--spot);
    color: var(--ink);
    font-size: 10px;
    font-weight: 900;
    padding: 1px 6px;
    border-radius: 999px;
    border: 1px solid var(--ink);
  }

  /* Inbox Styles */
  .inbox-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1.25rem;
  }

  .filter-pills {
    display: flex;
    gap: 0.35rem;
    flex-wrap: wrap;
  }

  .batch-bar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .batch-count {
    font-size: 0.85rem;
    font-weight: 700;
  }

  .series-groups-list {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  .series-group-card {
    border: 2px solid var(--ink);
    background: var(--surface);
    box-shadow: var(--sh-4);
  }

  .group-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 1.1rem 1.25rem;
    background: var(--shade);
    border-bottom: 2px solid var(--ink);
  }

  .group-title-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    margin-bottom: 0.35rem;
  }

  .group-title-link {
    text-decoration: none;
    color: var(--ink);
  }

  .group-title-link:hover {
    text-decoration: underline;
  }

  .group-title {
    font-size: 1.35rem;
    margin: 0;
  }

  .next-missing-tag {
    background: var(--spot);
    color: var(--ink);
    border: 1.5px solid var(--ink);
    font-weight: 900;
    font-size: 11px;
    letter-spacing: 0.05em;
  }

  .group-meta-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    font-size: 0.82rem;
  }

  .group-meta-pill {
    color: var(--muted);
  }

  .group-meta-pill.warn {
    color: var(--status-warn, #d97706);
    font-weight: 700;
  }

  .view-series-link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    color: var(--ink);
    font-weight: 700;
    text-decoration: underline;
  }

  .proposals-container {
    padding: 1rem 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .section-label-row {
    margin-bottom: 0.65rem;
  }

  .primary-label {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 0.8rem;
    font-weight: 800;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--status-ok, #166534);
  }

  .proposals-list {
    display: flex;
    flex-direction: column;
    gap: 0.65rem;
  }

  .proposal-card {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.85rem;
    padding: 0.85rem 1rem;
    border: 1.5px solid var(--ink);
    background: var(--paper);
  }

  .primary-card {
    border-left: 5px solid var(--status-ok, #16a34a);
  }

  .out-of-order-card {
    border-left: 5px solid var(--status-warn, #d97706);
    opacity: 0.9;
  }

  .prop-check-col {
    display: flex;
    align-items: center;
  }

  .prop-info-col {
    flex: 1 1 350px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .prop-top-line {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
  }

  .prop-vol {
    font-size: 1rem;
    color: var(--ink);
  }

  .provider-pill {
    font-size: 10px;
    font-weight: 800;
    padding: 1px 6px;
    border-radius: 2px;
    border: 1px solid var(--ink);
    background: var(--surface);
  }

  .score-pill {
    font-size: 10px;
    font-weight: 800;
    padding: 1px 6px;
    border-radius: 2px;
    background: var(--shade-2);
  }

  .conf-pill, .size-pill {
    font-size: 10px;
    color: var(--muted);
  }

  .prop-title {
    font-size: 0.88rem;
    font-weight: 600;
    margin: 0;
    color: var(--ink);
  }

  .prop-ordering-note {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 0.78rem;
    color: var(--muted);
    margin: 0;
  }

  .prop-ordering-warning {
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 0.78rem;
    color: var(--status-warn, #d97706);
    font-weight: 700;
    margin: 0;
  }

  .prop-actions-col {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
  }

  .expand-header-row {
    padding-top: 0.5rem;
    border-top: 1px solid var(--line);
  }

  .expand-toggle-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 700;
    cursor: pointer;
  }

  .empty-inbox-panel {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.75rem;
    padding: 3rem 1rem;
    text-align: center;
  }

  /* Watchers Tab Styles */
  .watchers-stat-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(140px, 1fr));
    gap: 0.75rem;
    margin-bottom: 1.25rem;
  }

  .table-toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-bottom: 1rem;
  }

  .toolbar-left {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.75rem;
    flex: 1;
  }

  .search-field {
    min-width: 220px;
  }

  .view-toggle {
    display: inline-flex;
    gap: 4px;
  }

  .watchers-series-list {
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
  }

  .watcher-series-card {
    border: 2px solid var(--ink);
    background: var(--surface);
  }

  .wsc-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.85rem 1rem;
    cursor: pointer;
  }

  .wsc-header:hover {
    background: var(--shade);
  }

  .wsc-title-col {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .wsc-title {
    font-size: 1.05rem;
    color: var(--ink);
  }

  .wsc-stats {
    font-size: 0.8rem;
    color: var(--muted);
  }

  .wsc-volumes-table-wrap {
    border-top: 1.5px solid var(--ink);
    overflow-x: auto;
  }

  .data-table-wrap {
    border: 2px solid var(--ink);
    overflow-x: auto;
    background: var(--surface);
  }

  .data-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.82rem;
  }

  .data-table th, .data-table td {
    padding: 8px 12px;
    border-bottom: 1px solid var(--line);
    text-align: left;
  }

  .data-table th {
    background: var(--shade);
    font-weight: 800;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .status-pill {
    display: inline-block;
    padding: 1px 6px;
    border-radius: 999px;
    font-size: 10px;
    font-weight: 800;
    border: 1px solid var(--ink);
  }

  .status-pill.ELIGIBLE_CANDIDATE_FOUND {
    background: var(--spot);
    color: var(--ink);
  }

  .status-pill.OWNED {
    background: #dcfce7;
    color: #166534;
  }

  .status-pill.RELEASED_AWAITING_PROVIDER {
    background: #fef3c7;
    color: #92400e;
  }

  .danger-text {
    color: var(--status-error, #c62828);
  }
</style>
