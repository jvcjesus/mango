<script lang="ts">
  import {
    get,
    getReleaseCalendar,
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
    send,
    type ReleaseCalendarItem,
    type UpdateItem,
    type Updates,
    type WatchersDashboardResponseDto,
    type TitleAvailabilityWatcherDto,
    type ReviewQueueItem,
    type MonitoringActivityEntry,
  } from '../lib/api';
  import CandidateInspectorModal from '../lib/CandidateInspectorModal.svelte';
  import Icon from '../lib/Icon.svelte';
  import { keep, recall } from '../lib/keep';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { ago, dayKey, dayLabel, formatLastChecked, formatNextCheck } from '../lib/util';

  // Navigation mode
  let activeTab = $state<'chapters' | 'calendar' | 'watchers' | 'review' | 'activity'>('chapters');

  // Review Queue state (M10.1 / M11 Mode B)
  let reviewItems = $state<ReviewQueueItem[]>([]);
  let reviewLoading = $state(false);
  let reviewError = $state<string | null>(null);
  let reviewStatusFilter = $state<string>('pending');
  let selectedReviewIds = $state(new Set<number>());
  let reviewActionBusy = $state(false);
  let rejectingItemId = $state<number | null>(null);
  let rejectReasonInput = $state('');

  // Activity History state (M10.1 / M11)
  let activityEntries = $state<MonitoringActivityEntry[]>([]);
  let activityLoading = $state(false);
  let activityError = $state<string | null>(null);

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

  async function handleApproveReviewItem(id: number) {
    reviewActionBusy = true;
    try {
      const res = await approveReviewQueueItem(id);
      toast.success(res.queued ? 'Approved and queued for acquisition' : 'Approved');
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

  async function handleBatchApprove() {
    const ids = Array.from(selectedReviewIds);
    if (ids.length === 0) return;
    reviewActionBusy = true;
    try {
      const res = await batchApproveReviewQueue(ids);
      toast.success(`Approved ${res.approved} of ${res.total} candidates`);
      selectedReviewIds.clear();
      await loadReviewQueue();
    } catch (e) {
      toast.error(message(e));
    } finally {
      reviewActionBusy = false;
    }
  }

  async function handleRefreshMonitoring() {
    reviewActionBusy = true;
    try {
      const res = await triggerMonitoringRefresh();
      toast.success(res.message);
      if (activeTab === 'review') await loadReviewQueue();
      if (activeTab === 'activity') await loadActivityHistory();
    } catch (e) {
      toast.error(message(e));
    } finally {
      reviewActionBusy = false;
    }
  }

  async function loadActivityHistory() {
    activityLoading = true;
    activityError = null;
    try {
      activityEntries = await getMonitoringActivity(50);
    } catch (e) {
      activityError = message(e);
      toast.error(activityError);
    } finally {
      activityLoading = false;
    }
  }

  function toggleReviewSelect(id: number, checked: boolean) {
    if (checked) {
      selectedReviewIds.add(id);
    } else {
      selectedReviewIds.delete(id);
    }
  }

  // Chapters tab state
  const kept = recall<Updates>('updates');
  let data = $state<Updates | null>(kept);
  let loading = $state(!kept);
  let error = $state<string | null>(null);
  let note = $state('');
  let showErrors = $state(false);
  let starting = $state(false);
  let downloadingItems = $state<Record<number, boolean>>({});

  // Release Calendar state
  let calendarItems = $state<ReleaseCalendarItem[]>([]);
  let calendarLoading = $state(false);
  let calendarError = $state<string | null>(null);
  let marketFilter = $state<string>('EN');
  let inspectingVolumeTarget = $state<{ mangaId: number; volume: number } | null>(null);

  // Availability Watchers state (M7.2)
  let watchersData = $state<WatchersDashboardResponseDto | null>(null);
  let watchersLoading = $state<boolean>(false);
  let watchersError = $state<string | null>(null);
  let watchersFilter = $state<string>('ALL');
  let watchersSearch = $state<string>('');
  let refreshingWatchers = $state<Record<number, boolean>>({});

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
        const idx = watchersData.watchers.findIndex(w => w.id === id);
        if (idx !== -1) {
          watchersData.watchers[idx] = updated;
        }
        toast.success(`Checked Vol ${updated.volumeNumber}: ${updated.state}`);
      } else {
        toast.info('Watcher checked.');
      }
    } catch (e) {
      toast.error(message(e));
    } finally {
      refreshingWatchers[id] = false;
    }
  }

  let selectedWatcherIds = $state(new Set<number>());
  let bulkBusy = $state(false);

  function toggleWatcherSelect(id: number, checked: boolean) {
    if (checked) {
      selectedWatcherIds.add(id);
    } else {
      selectedWatcherIds.delete(id);
    }
  }

  async function handleCheckAllWatchers() {
    bulkBusy = true;
    try {
      const res = await executeWatchersBulkAction({ action: 'check_all' });
      toast.success(res.message);
      await loadWatchers();
    } catch (e) {
      toast.error(message(e));
    } finally {
      bulkBusy = false;
    }
  }

  async function handlePauseSelected() {
    if (selectedWatcherIds.size === 0) return;
    bulkBusy = true;
    try {
      const res = await executeWatchersBulkAction({
        action: 'pause_selected',
        watcherIds: Array.from(selectedWatcherIds),
      });
      toast.success(res.message);
      selectedWatcherIds.clear();
      await loadWatchers();
    } catch (e) {
      toast.error(message(e));
    } finally {
      bulkBusy = false;
    }
  }

  async function handleResumeSelected() {
    if (selectedWatcherIds.size === 0) return;
    bulkBusy = true;
    try {
      const res = await executeWatchersBulkAction({
        action: 'resume_selected',
        watcherIds: Array.from(selectedWatcherIds),
      });
      toast.success(res.message);
      selectedWatcherIds.clear();
      await loadWatchers();
    } catch (e) {
      toast.error(message(e));
    } finally {
      bulkBusy = false;
    }
  }

  // Watchers Table sorting, pagination, and expandable details
  let watchersSortField = $state<'title' | 'volume' | 'state' | 'lastChecked' | 'nextCheck'>('title');
  let watchersSortDir = $state<'asc' | 'desc'>('asc');
  let watchersProviderFilter = $state<string>('ALL');
  let watchersPage = $state(1);
  let watchersPageSize = $state(25);
  let expandedWatcherIds = $state(new Set<number>());

  function toggleRowExpanded(id: number) {
    if (expandedWatcherIds.has(id)) {
      expandedWatcherIds.delete(id);
    } else {
      expandedWatcherIds.add(id);
    }
  }

  function toggleSort(field: 'title' | 'volume' | 'state' | 'lastChecked' | 'nextCheck') {
    if (watchersSortField === field) {
      watchersSortDir = watchersSortDir === 'asc' ? 'desc' : 'asc';
    } else {
      watchersSortField = field;
      watchersSortDir = 'asc';
    }
    watchersPage = 1;
  }

  const watcherDuplicates = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const w of watchersData?.watchers ?? []) {
      const key = `${w.seriesTitle || w.mangaId || w.seriesId}_v${w.volumeNumber}`.toLowerCase();
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  });

  function isWatcherDuplicate(w: TitleAvailabilityWatcherDto): boolean {
    const key = `${w.seriesTitle || w.mangaId || w.seriesId}_v${w.volumeNumber}`.toLowerCase();
    return (watcherDuplicates.get(key) ?? 0) > 1;
  }

  const availableProviders = $derived.by(() => {
    const set = new Set<string>();
    for (const w of watchersData?.watchers ?? []) {
      if (w.lastProviderChecked) set.add(w.lastProviderChecked);
      if (w.topCandidateProvider) set.add(w.topCandidateProvider);
    }
    return Array.from(set).sort();
  });

  const processedWatchers = $derived.by(() => {
    if (!watchersData?.watchers) return [];
    let list = watchersData.watchers.filter(w => {
      if (watchersSearch.trim()) {
        const q = watchersSearch.toLowerCase();
        const matchTitle = (w.seriesTitle ?? '').toLowerCase().includes(q);
        const matchCand = (w.topCandidateTitle ?? '').toLowerCase().includes(q);
        if (!matchTitle && !matchCand) return false;
      }
      if (watchersProviderFilter !== 'ALL') {
        const matchP = (w.lastProviderChecked?.toLowerCase() === watchersProviderFilter.toLowerCase()) ||
                       (w.topCandidateProvider?.toLowerCase() === watchersProviderFilter.toLowerCase());
        if (!matchP) return false;
      }
      if (watchersFilter === 'ALL') return true;
      if (watchersFilter === 'OWNED') return w.isOwned;
      if (watchersFilter === 'ERROR') {
        return w.state === 'PROVIDER_UNAVAILABLE' || w.state === 'RATE_LIMITED' || w.state === 'REVIEW_REQUIRED';
      }
      return w.state === watchersFilter;
    });

    list.sort((a, b) => {
      let cmp = 0;
      if (watchersSortField === 'title') {
        cmp = (a.seriesTitle || '').localeCompare(b.seriesTitle || '');
        if (cmp === 0) cmp = a.volumeNumber - b.volumeNumber;
      } else if (watchersSortField === 'volume') {
        cmp = a.volumeNumber - b.volumeNumber;
      } else if (watchersSortField === 'state') {
        cmp = (a.state || '').localeCompare(b.state || '');
      } else if (watchersSortField === 'lastChecked') {
        cmp = (a.lastCheckedAt || 0) - (b.lastCheckedAt || 0);
      } else if (watchersSortField === 'nextCheck') {
        cmp = (a.nextCheckAt || 0) - (b.nextCheckAt || 0);
      }
      return watchersSortDir === 'asc' ? cmp : -cmp;
    });

    return list;
  });

  const totalWatchersCount = $derived(processedWatchers.length);
  const totalWatchersPages = $derived(Math.max(1, Math.ceil(totalWatchersCount / watchersPageSize)));
  const paginatedWatchers = $derived.by(() => {
    const start = (watchersPage - 1) * watchersPageSize;
    return processedWatchers.slice(start, start + watchersPageSize);
  });

  const running = $derived(data?.status.running ?? false);
  const days = $derived.by(() => {
    const groups: { day: number; items: UpdateItem[] }[] = [];
    for (const item of data?.items ?? []) {
      const day = dayKey(item.fetchedAt);
      if (groups.at(-1)?.day === day) groups.at(-1)!.items.push(item);
      else groups.push({ day, items: [item] });
    }
    return groups;
  });

  const calendarGroups = $derived.by(() => {
    const today = calendarItems.filter((i) => i.status === 'ReleasedToday');
    const upcoming = calendarItems.filter((i) => i.status === 'Pending');
    const past = calendarItems.filter((i) => i.status === 'Past');
    return { today, upcoming, past };
  });

  async function load() {
    try {
      const fresh = await get<Updates>('/updates');
      keep('updates', fresh);
      data = fresh;
      error = null;
    } catch (e) {
      error = message(e);
    }
    loading = false;
  }
  load();

  // While a run is going, the screen follows it.
  $effect(() => {
    if (!running) return;
    const timer = setInterval(load, 2000);
    return () => clearInterval(timer);
  });

  async function run() {
    if (starting || running) return;
    starting = true;
    note = '';
    try {
      const status = await send<Updates['status']>('POST', '/updates/run');
      if (data) data.status = status;
      toast.info('Checking library titles for updates…');
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      starting = false;
    }
  }

  async function download(item: UpdateItem) {
    if (downloadingItems[item.chapterId]) return;
    downloadingItems[item.chapterId] = true;
    try {
      await send('POST', '/downloads', { chapters: [item.chapterId] });
      const text = `${item.title}, ${item.chapterName}: queued for download.`;
      note = text;
      toast.info(text, { action: { label: 'See downloads', run: () => (location.hash = '#/downloads') } });
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      downloadingItems[item.chapterId] = false;
    }
  }

  async function loadCalendar() {
    calendarLoading = true;
    calendarError = null;
    try {
      const market = marketFilter === 'ALL' ? undefined : marketFilter;
      calendarItems = await getReleaseCalendar(market);
    } catch (e) {
      calendarError = message(e);
    } finally {
      calendarLoading = false;
    }
  }

  function handleMarketChange(m: string) {
    marketFilter = m;
    loadCalendar();
  }

  function getDaysUntil(dateStr: string): number {
    const target = new Date(dateStr).getTime();
    const now = new Date().setHours(0, 0, 0, 0);
    return Math.round((target - now) / 86400000);
  }

  function formatRelativeDays(days: number): string {
    if (days === 0) return 'Today';
    if (days === 1) return 'Tomorrow';
    if (days > 1) return `in ${days} days`;
    if (days === -1) return 'Yesterday';
    return `${Math.abs(days)} days ago`;
  }
</script>

<div class="page">
  <div class="page-head">
    <div class="head-left">
      <h1 class="display">UPDATES</h1>
      <div class="pill-group view-switcher" role="tablist" aria-label="Updates view">
        <button
          class="pill"
          class:active={activeTab === 'chapters'}
          onclick={() => (activeTab = 'chapters')}
          role="tab"
          aria-selected={activeTab === 'chapters'}
        >
          <Icon name="book" size={14} /> Chapter Updates
        </button>
        <button
          class="pill"
          class:active={activeTab === 'calendar'}
          onclick={() => {
            activeTab = 'calendar';
            if (calendarItems.length === 0) loadCalendar();
          }}
          role="tab"
          aria-selected={activeTab === 'calendar'}
        >
          <Icon name="clock" size={14} /> Release Calendar
        </button>
        <button
          class="pill"
          class:active={activeTab === 'watchers'}
          onclick={() => {
            activeTab = 'watchers';
            if (!watchersData) loadWatchers();
          }}
          role="tab"
          aria-selected={activeTab === 'watchers'}
        >
          <Icon name="bell" size={14} /> Availability Watchers
        </button>
        <button
          class="pill"
          class:active={activeTab === 'review'}
          onclick={() => {
            activeTab = 'review';
            loadReviewQueue();
          }}
          role="tab"
          aria-selected={activeTab === 'review'}
        >
          <Icon name="check" size={14} /> Review Queue
        </button>
        <button
          class="pill"
          class:active={activeTab === 'activity'}
          onclick={() => {
            activeTab = 'activity';
            loadActivityHistory();
          }}
          role="tab"
          aria-selected={activeTab === 'activity'}
        >
          <Icon name="list" size={14} /> Activity History
        </button>
      </div>
    </div>

    {#if activeTab === 'chapters'}
      <div class="tools">
        {#if data?.status.finishedAt && !running}
          <strong class="font-small muted">Checked {ago(data.status.finishedAt)}: {data.status.found} new{data.status.failed ? `, ${data.status.failed} failed` : ''}{data.status.waiting ? `, ${data.status.waiting} not due yet` : ''}</strong>
        {/if}
        <button class="btn big spot" disabled={running || starting} onclick={run}>
          {#if running || starting}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={18} stroke={3} />
          {/if}
          {running && data ? `Checking ${data.status.done} / ${data.status.total}` : starting ? 'Starting…' : 'Update now'}
        </button>
      </div>
    {:else if activeTab === 'calendar'}
      <div class="tools">
        <div class="pill-group" role="radiogroup" aria-label="Market Filter">
          <button
            class="pill mini"
            class:active={marketFilter === 'EN'}
            onclick={() => handleMarketChange('EN')}
          >
            EN
          </button>
          <button
            class="pill mini"
            class:active={marketFilter === 'JA'}
            onclick={() => handleMarketChange('JA')}
          >
            JA
          </button>
          <button
            class="pill mini"
            class:active={marketFilter === 'ALL'}
            onclick={() => handleMarketChange('ALL')}
          >
            All
          </button>
        </div>
        <button class="btn big" disabled={calendarLoading} onclick={loadCalendar}>
          {#if calendarLoading}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={18} stroke={3} />
          {/if}
          Refresh
        </button>
      </div>
    {:else if activeTab === 'watchers'}
      <div class="tools">
        <button class="btn big" disabled={watchersLoading} onclick={loadWatchers}>
          {#if watchersLoading}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={18} stroke={3} />
          {/if}
          Refresh Watchers
        </button>
      </div>
    {:else if activeTab === 'review'}
      <div class="tools">
        <button class="btn big" disabled={reviewLoading} onclick={loadReviewQueue}>
          {#if reviewLoading}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={18} stroke={3} />
          {/if}
          Refresh Queue
        </button>
      </div>
    {:else if activeTab === 'activity'}
      <div class="tools">
        <button class="btn big" disabled={activityLoading} onclick={loadActivityHistory}>
          {#if activityLoading}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={18} stroke={3} />
          {/if}
          Refresh History
        </button>
      </div>
    {/if}
  </div>

  {#if activeTab === 'chapters'}
    <Notice {loading} {error} retry={load} />
    {#if note}<p role="status"><strong>{note}</strong> <a class="link" href="#/downloads">See downloads</a></p>{/if}

    {#if data?.errors.length}
      <section class="panel problems">
        <button class="btn" aria-expanded={showErrors} onclick={() => (showErrors = !showErrors)}>
          {data.errors.length} {data.errors.length === 1 ? 'title' : 'titles'} could not be checked<Icon name={showErrors ? 'up' : 'down'} size={18} />
        </button>
        {#if showErrors}
          <ul class="rows">
            {#each data.errors as failure (failure.mangaId)}
              <li>
                <a class="main" href="#/title/{failure.mangaId}"><strong>{failure.title}</strong><span>{failure.error} · {ago(failure.at)}</span></a>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    {/if}

    {#each days as group (group.day)}
      <section>
        <div class="section-head"><h2 class="display">{dayLabel(group.day)}</h2></div>
        <ul class="rows">
          {#each group.items as item (item.chapterId)}
            <li class:done={item.read}>
              <a class="main" href="#/title/{item.mangaId}">
                <strong>{item.title}</strong>
                <span>{item.chapterName}{item.sourceName ? ` · ${item.sourceName}` : ''}{item.read ? ' · read' : ''}{item.downloaded ? ' · saved' : ''}</span>
              </a>
              <span class="tools">
                {#if !item.downloaded}
                  <button
                    class="btn icon"
                    aria-label="Download {item.chapterName} of {item.title}"
                    disabled={downloadingItems[item.chapterId]}
                    onclick={() => download(item)}
                  >
                    {#if downloadingItems[item.chapterId]}
                      <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                    {:else}
                      <Icon name="download" />
                    {/if}
                  </button>
                {/if}
                <a class="btn" href="#/read/{item.chapterId}">Read<Icon name="arrow" size={18} stroke={3} /></a>
              </span>
            </li>
          {/each}
        </ul>
      </section>
    {/each}

    {#if data && data.items.length === 0 && !loading}
      <Notice>No new chapters yet. Titles in the library are checked on a schedule; “Update now” checks them right away.</Notice>
    {/if}

  {:else if activeTab === 'calendar'}
    <!-- RELEASE CALENDAR VIEW -->
    <Notice loading={calendarLoading} error={calendarError} retry={loadCalendar} />

    {#if !calendarLoading && !calendarError}
      {#if calendarItems.length === 0}
        <div class="panel empty-calendar-panel">
          <h3>No Scheduled Volume Releases Found</h3>
          <p class="muted">
            OpenTome release intelligence tracks upcoming physical volume dates from publisher metadata.
            Make sure your titles are reconciled with MangaBaka and your OpenTome database dump is up to date in Settings → Pipeline.
          </p>
        </div>
      {:else}
        <!-- Releases Today -->
        {#if calendarGroups.today.length > 0}
          <section class="calendar-section releases-today-section">
            <div class="section-head">
              <h2 class="display today-headline">
                <span class="pulse-indicator"></span>
                RELEASES TODAY ({calendarGroups.today.length})
              </h2>
            </div>
            <div class="calendar-grid">
              {#each calendarGroups.today as item}
                {@render calendarCard(item)}
              {/each}
            </div>
          </section>
        {/if}

        <!-- Upcoming Releases -->
        {#if calendarGroups.upcoming.length > 0}
          <section class="calendar-section">
            <div class="section-head">
              <h2 class="display">SCHEDULED & UPCOMING ({calendarGroups.upcoming.length})</h2>
            </div>
            <div class="calendar-grid">
              {#each calendarGroups.upcoming as item}
                {@render calendarCard(item)}
              {/each}
            </div>
          </section>
        {/if}

        <!-- Past Releases -->
        {#if calendarGroups.past.length > 0}
          <section class="calendar-section">
            <div class="section-head">
              <h2 class="display muted-headline">PAST RELEASES ({calendarGroups.past.length})</h2>
            </div>
            <div class="calendar-grid">
              {#each calendarGroups.past as item}
                {@render calendarCard(item)}
              {/each}
            </div>
          </section>
        {/if}
      {/if}
    {/if}

  {:else if activeTab === 'watchers'}
    <!-- AVAILABILITY & WATCHERS DASHBOARD (M7.2 / M12) -->
    <Notice loading={watchersLoading} error={watchersError} retry={loadWatchers} />

    {#if !watchersLoading && !watchersError}
      {#if watchersData}
        <!-- Educational Workflow Explainer -->
        <div class="watchers-explainer-card">
          <div class="explainer-item">
            <span class="exp-title"><Icon name="eye" size={14} /> <strong>Watch</strong></span>
            <p>Tracks official publication dates from OpenTome.</p>
          </div>
          <div class="explainer-item">
            <span class="exp-title"><Icon name="search" size={14} /> <strong>Discover</strong></span>
            <p>Searches Madokami & Nyaa providers as release date arrives.</p>
          </div>
          <div class="explainer-item">
            <span class="exp-title"><Icon name="bell" size={14} /> <strong>Notify</strong></span>
            <p>Alerts user when eligible candidates are discovered.</p>
          </div>
          <div class="explainer-item">
            <span class="exp-title"><Icon name="download" size={14} /> <strong>Auto-Acquire</strong></span>
            <p>Strictly opt-in (default OFF) automated acquisition with hard gates.</p>
          </div>
        </div>

        <!-- Metrics Grid (Stat Cards) -->
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
          <button type="button" class="stat-card" class:selected={watchersFilter === 'NOTIFIED'} onclick={() => { watchersFilter = 'NOTIFIED'; watchersPage = 1; }}>
            <span class="stat-val" style="color: #9333ea;">{watchersData.summary.notifiedCount}</span>
            <span class="stat-lbl">Notified</span>
          </button>
          <button type="button" class="stat-card" class:selected={watchersFilter === 'OWNED'} onclick={() => { watchersFilter = 'OWNED'; watchersPage = 1; }}>
            <span class="stat-val" style="color: var(--status-ok, #16a34a);">{watchersData.summary.ownedCount}</span>
            <span class="stat-lbl">Owned / Complete</span>
          </button>
          <button type="button" class="stat-card" class:selected={watchersFilter === 'ERROR'} onclick={() => { watchersFilter = 'ERROR'; watchersPage = 1; }}>
            <span class="stat-val" style="color: var(--status-error, #dc2626);">{watchersData.summary.errorCount}</span>
            <span class="stat-lbl">Issues / Errors</span>
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

            {#if availableProviders.length > 0}
              <label class="filter-label">
                <span class="font-small muted">Provider:</span>
                <select class="field select-compact" bind:value={watchersProviderFilter} onchange={() => watchersPage = 1}>
                  <option value="ALL">All Providers</option>
                  {#each availableProviders as prov}
                    <option value={prov}>{prov}</option>
                  {/each}
                </select>
              </label>
            {/if}

            <label class="filter-label">
              <span class="font-small muted">Rows:</span>
              <select class="field select-compact" bind:value={watchersPageSize} onchange={() => watchersPage = 1}>
                <option value={25}>25</option>
                <option value={50}>50</option>
                <option value={100}>100</option>
              </select>
            </label>
          </div>

          <div class="bulk-actions-group">
            <button
              class="btn mini"
              onclick={handleCheckAllWatchers}
              disabled={bulkBusy}
              title="Queue all active watchers for immediate check"
            >
              {#if bulkBusy}
                <span class="spin"><Icon name="spinner" size={12} stroke={3} /></span>
              {:else}
                <Icon name="refresh" size={12} />
              {/if}
              <span>Check All</span>
            </button>
            {#if selectedWatcherIds.size > 0}
              <button class="btn mini flat" onclick={handlePauseSelected} disabled={bulkBusy}>
                <Icon name="pause" size={12} /> Pause ({selectedWatcherIds.size})
              </button>
              <button class="btn mini flat" onclick={handleResumeSelected} disabled={bulkBusy}>
                <Icon name="play" size={12} /> Resume ({selectedWatcherIds.size})
              </button>
              <button class="btn mini text" onclick={() => selectedWatcherIds.clear()}>
                Clear ({selectedWatcherIds.size})
              </button>
            {/if}
          </div>
        </div>

        {#if processedWatchers.length === 0}
          <div class="panel empty-watchers-panel">
            <p class="muted">No availability watchers match the current filter or search criteria.</p>
          </div>
        {:else}
          <div class="data-table-wrap">
            <table class="data-table font-small">
              <thead>
                <tr>
                  <th class="th-checkbox">
                    <input
                      type="checkbox"
                      checked={paginatedWatchers.length > 0 && paginatedWatchers.every(w => selectedWatcherIds.has(w.id))}
                      onchange={(e) => {
                        const checked = (e.currentTarget as HTMLInputElement).checked;
                        for (const w of paginatedWatchers) {
                          toggleWatcherSelect(w.id, checked);
                        }
                      }}
                      aria-label="Select all watchers on current page"
                    />
                  </th>
                  <th class="sortable" onclick={() => toggleSort('title')}>
                    Series Title {watchersSortField === 'title' ? (watchersSortDir === 'asc' ? '▲' : '▼') : ''}
                  </th>
                  <th class="sortable th-vol" onclick={() => toggleSort('volume')}>
                    Vol {watchersSortField === 'volume' ? (watchersSortDir === 'asc' ? '▲' : '▼') : ''}
                  </th>
                  <th class="sortable th-status" onclick={() => toggleSort('state')}>
                    Status {watchersSortField === 'state' ? (watchersSortDir === 'asc' ? '▲' : '▼') : ''}
                  </th>
                  <th>Best Candidate</th>
                  <th class="sortable th-time" onclick={() => toggleSort('lastChecked')}>
                    Last Checked {watchersSortField === 'lastChecked' ? (watchersSortDir === 'asc' ? '▲' : '▼') : ''}
                  </th>
                  <th class="sortable th-time" onclick={() => toggleSort('nextCheck')}>
                    Next Check {watchersSortField === 'nextCheck' ? (watchersSortDir === 'asc' ? '▲' : '▼') : ''}
                  </th>
                  <th class="th-actions">Actions</th>
                </tr>
              </thead>
              <tbody>
                {#each paginatedWatchers as watcher (watcher.id)}
                  {@const isExpanded = expandedWatcherIds.has(watcher.id)}
                  {@const isDup = isWatcherDuplicate(watcher)}
                  <tr class="watcher-row" class:row-selected={selectedWatcherIds.has(watcher.id)} class:row-expanded={isExpanded}>
                    <td class="td-checkbox">
                      <input
                        type="checkbox"
                        checked={selectedWatcherIds.has(watcher.id)}
                        onchange={(e) => toggleWatcherSelect(watcher.id, (e.target as HTMLInputElement).checked)}
                        aria-label="Select Volume {watcher.volumeNumber}"
                      />
                    </td>
                    <td class="td-title">
                      <div class="series-name-wrap">
                        {#if watcher.mangaId}
                          <a href="#/title/{watcher.mangaId}" class="series-link">
                            <strong>{watcher.seriesTitle || `Manga #${watcher.mangaId}`}</strong>
                          </a>
                        {:else}
                          <strong>{watcher.seriesTitle}</strong>
                        {/if}
                        {#if watcher.edition && watcher.edition !== 'standard'}
                          <span class="edition-pill">{watcher.edition}</span>
                        {/if}
                        {#if isDup}
                          <span class="status-badge status-warn" title="Multiple watcher records exist for this title and volume">
                            <Icon name="alert" size={11} stroke={2.5} /> Duplicate
                          </span>
                        {/if}
                      </div>
                    </td>
                    <td class="td-vol font-mono">
                      <span class="vol-num-tag">Vol. {watcher.volumeNumber}</span>
                    </td>
                    <td class="td-status">
                      {#if watcher.isOwned}
                        <span class="status-badge status-owned">Owned</span>
                      {:else if watcher.state === 'ELIGIBLE_CANDIDATE_FOUND' || watcher.state === 'CANDIDATE_FOUND'}
                        <span class="status-badge status-candidate">Candidate</span>
                      {:else if watcher.state === 'RELEASED_AWAITING_PROVIDER'}
                        <span class="status-badge status-awaiting">Awaiting</span>
                      {:else if watcher.state === 'UPCOMING'}
                        <span class="status-badge status-upcoming">Upcoming</span>
                      {:else if watcher.state === 'NOTIFIED'}
                        <span class="status-badge status-info">Notified</span>
                      {:else if watcher.state === 'FAILED_PERMANENT' || watcher.state === 'PROVIDER_UNAVAILABLE' || watcher.state === 'RATE_LIMITED' || watcher.state === 'REVIEW_REQUIRED'}
                        <span class="status-badge status-error">{watcher.state}</span>
                      {:else}
                        <span class="status-badge status-neutral">{watcher.state}</span>
                      {/if}
                    </td>
                    <td class="td-cand">
                      {#if watcher.topCandidateTitle}
                        <div class="cand-cell">
                          {#if watcher.topCandidateProvider}
                            <span class="provider-pill">{watcher.topCandidateProvider}</span>
                          {/if}
                          <span class="cand-text font-mono" title={watcher.topCandidateTitle}>{watcher.topCandidateTitle}</span>
                        </div>
                      {:else if watcher.candidateCount && watcher.candidateCount > 0}
                        <span class="muted font-small">{watcher.candidateCount} candidates</span>
                      {:else}
                        <span class="muted font-small">—</span>
                      {/if}
                    </td>
                    <td class="td-time font-small muted">
                      {formatLastChecked(watcher.lastCheckedAt)}
                    </td>
                    <td class="td-time font-small">
                      <span class:text-warn={formatNextCheck(watcher.nextCheckAt) === 'Due now'}>
                        {formatNextCheck(watcher.nextCheckAt)}
                      </span>
                    </td>
                    <td class="td-actions">
                      <div class="action-btn-row">
                        <button
                          class="btn mini icon-text"
                          disabled={refreshingWatchers[watcher.id]}
                          onclick={() => handleRefreshWatcher(watcher.id)}
                          title="Run on-demand check for Volume {watcher.volumeNumber}"
                        >
                          {#if refreshingWatchers[watcher.id]}
                            <span class="spin"><Icon name="spinner" size={12} stroke={3} /></span>
                          {:else}
                            <Icon name="search" size={12} />
                          {/if}
                          <span>Check</span>
                        </button>
                        {#if watcher.mangaId}
                          <button
                            class="btn mini flat icon"
                            onclick={() => {
                              inspectingVolumeTarget = { mangaId: watcher.mangaId!, volume: watcher.volumeNumber };
                            }}
                            title="Inspect candidate transports"
                          >
                            <Icon name="eye" size={12} />
                          </button>
                        {/if}
                        <button
                          class="btn mini flat icon"
                          onclick={() => toggleRowExpanded(watcher.id)}
                          title={isExpanded ? 'Collapse details' : 'Expand details'}
                        >
                          <Icon name={isExpanded ? 'up' : 'down'} size={12} />
                        </button>
                      </div>
                    </td>
                  </tr>

                  {#if isExpanded}
                    <tr class="drawer-row">
                      <td colspan="8">
                        <div class="drawer-content">
                          <div class="drawer-grid">
                            <div class="drawer-field">
                              <span class="drawer-label">Release Date</span>
                              <span class="drawer-value font-mono">{watcher.releaseDate || 'Unknown'}</span>
                            </div>
                            <div class="drawer-field">
                              <span class="drawer-label">Last Provider</span>
                              <span class="drawer-value">{watcher.lastProviderChecked || 'None recorded'}</span>
                            </div>
                            <div class="drawer-field">
                              <span class="drawer-label">Consecutive Failures</span>
                              <span class="drawer-value">{watcher.consecutiveFailures}</span>
                            </div>
                            <div class="drawer-field">
                              <span class="drawer-label">Check Interval</span>
                              <span class="drawer-value font-mono">{Math.round(watcher.checkIntervalSecs / 60)} min</span>
                            </div>
                          </div>

                          {#if watcher.topCandidateTitle}
                            <div class="drawer-cand-box">
                              <strong>Top Candidate:</strong>
                              <span class="font-mono">{watcher.topCandidateTitle}</span>
                              {#if watcher.topCandidateFingerprint}
                                <span class="muted font-small font-mono">Fingerprint: {watcher.topCandidateFingerprint}</span>
                              {/if}
                            </div>
                          {/if}

                          {#if watcher.lastError}
                            <div class="drawer-error-box">
                              <Icon name="alert" size={14} />
                              <span>{watcher.lastError}</span>
                            </div>
                          {/if}
                        </div>
                      </td>
                    </tr>
                  {/if}
                {/each}
              </tbody>
            </table>
          </div>

          <!-- Pagination Bar -->
          <div class="pagination-bar">
            <span class="pagination-info muted">
              Showing {Math.min((watchersPage - 1) * watchersPageSize + 1, totalWatchersCount)}–{Math.min(watchersPage * watchersPageSize, totalWatchersCount)} of {totalWatchersCount} watchers
            </span>
            <div class="pagination-actions">
              <button
                class="btn mini flat page-btn"
                disabled={watchersPage <= 1}
                onclick={() => watchersPage = Math.max(1, watchersPage - 1)}
              >
                Prev
              </button>
              <span class="pagination-pages font-small">
                Page {watchersPage} of {totalWatchersPages}
              </span>
              <button
                class="btn mini flat page-btn"
                disabled={watchersPage >= totalWatchersPages}
                onclick={() => watchersPage = Math.min(totalWatchersPages, watchersPage + 1)}
              >
                Next
              </button>
            </div>
          </div>
        {/if}
      {/if}
    {/if}

  {:else if activeTab === 'review'}
    <!-- ACQUISITION REVIEW QUEUE (M10.1 / M11 Mode B) -->
    <div class="review-queue-header">
      <div class="review-queue-info">
        <h3>Proposed Acquisitions</h3>
        <p class="muted">
          Under <strong>Review Before Acquisition (Mode B)</strong>, releases matching followed series are queued here for operator confirmation before download and publication.
        </p>
      </div>
      <div class="review-queue-actions">
        <button class="btn mini flat icon-text" onclick={handleRefreshMonitoring} disabled={reviewActionBusy}>
          <Icon name="refresh" size={14} /> Refresh Cycle
        </button>
        {#if selectedReviewIds.size > 0}
          <button class="btn mini spot icon-text" onclick={handleBatchApprove} disabled={reviewActionBusy}>
            <Icon name="check" size={14} /> Approve Selected ({selectedReviewIds.size})
          </button>
          <button class="btn mini text" onclick={() => selectedReviewIds.clear()}>
            Clear
          </button>
        {/if}
      </div>
    </div>

    <!-- Status filter tabs -->
    <div class="review-status-filter-bar">
      <div class="pill-group" role="group" aria-label="Review status filter">
        <button
          class="pill mini"
          class:active={reviewStatusFilter === 'pending'}
          onclick={() => { reviewStatusFilter = 'pending'; loadReviewQueue(); }}
        >
          Pending
        </button>
        <button
          class="pill mini"
          class:active={reviewStatusFilter === 'approved'}
          onclick={() => { reviewStatusFilter = 'approved'; loadReviewQueue(); }}
        >
          Approved
        </button>
        <button
          class="pill mini"
          class:active={reviewStatusFilter === 'rejected'}
          onclick={() => { reviewStatusFilter = 'rejected'; loadReviewQueue(); }}
        >
          Rejected
        </button>
        <button
          class="pill mini"
          class:active={reviewStatusFilter === 'skipped'}
          onclick={() => { reviewStatusFilter = 'skipped'; loadReviewQueue(); }}
        >
          Skipped
        </button>
        <button
          class="pill mini"
          class:active={reviewStatusFilter === 'all'}
          onclick={() => { reviewStatusFilter = 'all'; loadReviewQueue(); }}
        >
          All
        </button>
      </div>
    </div>

    <Notice loading={reviewLoading} error={reviewError} retry={loadReviewQueue} />

    {#if !reviewLoading && !reviewError}
      {#if reviewItems.length === 0}
        <div class="panel empty-watchers-panel">
          <p class="muted">No proposed acquisitions found in status "{reviewStatusFilter}".</p>
        </div>
      {:else}
        <div class="review-items-list">
          {#each reviewItems as item (item.id)}
            <div class="review-card status-{item.status}">
              <div class="review-main-row">
                {#if item.status === 'pending'}
                  <label class="watcher-checkbox-label" title="Select for batch approval">
                    <input
                      type="checkbox"
                      checked={selectedReviewIds.has(item.id)}
                      onchange={(e) => toggleReviewSelect(item.id, e.currentTarget.checked)}
                    />
                  </label>
                {/if}
                <div class="review-target-info">
                  <div class="review-title-row">
                    <span class="series-title"><strong>{item.seriesTitle}</strong></span>
                    <span class="vol-num font-mono">Vol. {item.volumeNumber}</span>
                    <span class="status-pill status-{item.status}">{item.status.toUpperCase()}</span>
                    {#if item.confidence}
                      <span class="conf-pill conf-{item.confidence.toLowerCase()}">{item.confidence.toUpperCase()} CONFIDENCE</span>
                    {/if}
                  </div>
                  <div class="review-candidate-title font-mono font-small">
                    {item.rawTitle || item.title}
                  </div>
                  <div class="review-meta-row font-small muted">
                    <span class="provider-pill">{item.provider}</span>
                    <span>Quality Score: <strong>{Math.round(item.qualityScore)}/100</strong></span>
                    {#if item.fileSizeBytes}
                      <span>Size: {Math.round(item.fileSizeBytes / (1024 * 1024))} MB</span>
                    {/if}
                    {#if item.publicationDestination}
                      <span class="dest-path font-mono">Destination: {item.publicationDestination}</span>
                    {/if}
                  </div>
                  {#if item.statusReason}
                    <div class="review-reason-note font-small muted">
                      Note: {item.statusReason}
                    </div>
                  {/if}
                </div>

                <div class="review-actions-cell">
                  {#if item.status === 'pending'}
                    <button
                      class="btn mini spot icon-text"
                      onclick={() => handleApproveReviewItem(item.id)}
                      disabled={reviewActionBusy}
                      title="Approve candidate and queue for acquisition"
                    >
                      <Icon name="check" size={13} /> Approve
                    </button>
                    {#if rejectingItemId === item.id}
                      <div class="inline-reject-box">
                        <input
                          type="text"
                          class="field mini"
                          placeholder="Rejection reason…"
                          bind:value={rejectReasonInput}
                          onkeydown={(e) => { if (e.key === 'Enter') handleRejectReviewItem(item.id, rejectReasonInput); }}
                        />
                        <button class="btn mini flat" onclick={() => handleRejectReviewItem(item.id, rejectReasonInput)}>
                          Confirm
                        </button>
                        <button class="btn mini text" onclick={() => { rejectingItemId = null; rejectReasonInput = ''; }}>
                          Cancel
                        </button>
                      </div>
                    {:else}
                      <button
                        class="btn mini flat"
                        onclick={() => { rejectingItemId = item.id; rejectReasonInput = ''; }}
                        disabled={reviewActionBusy}
                        title="Reject candidate"
                      >
                        Reject
                      </button>
                    {/if}
                    <button
                      class="btn mini text"
                      onclick={() => handleSkipReviewItem(item.id, 'Operator skipped')}
                      disabled={reviewActionBusy}
                      title="Skip candidate"
                    >
                      Skip
                    </button>
                  {/if}
                  {#if item.mangaId}
                    <button
                      class="btn mini flat icon-text"
                      onclick={() => { inspectingVolumeTarget = { mangaId: item.mangaId!, volume: item.volumeNumber }; }}
                      title="Inspect candidates and transports"
                    >
                      <Icon name="eye" size={13} /> Inspect
                    </button>
                    <a class="btn mini flat" href="#/title/{item.mangaId}">
                      Title <Icon name="arrow" size={13} />
                    </a>
                  {/if}
                </div>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    {/if}

  {:else if activeTab === 'activity'}
    <!-- MONITORING & AUTOMATION ACTIVITY HISTORY (M10.1 / M11) -->
    <div class="review-queue-header">
      <div class="review-queue-info">
        <h3>Monitoring Activity History</h3>
        <p class="muted">
          Chronological audit trail of release discovery, review queue proposals, operator approvals, simulation decisions, and background checks.
        </p>
      </div>
      <div class="review-queue-actions">
        <button class="btn mini flat icon-text" onclick={loadActivityHistory} disabled={activityLoading}>
          <Icon name="refresh" size={14} /> Refresh
        </button>
      </div>
    </div>

    <Notice loading={activityLoading} error={activityError} retry={loadActivityHistory} />

    {#if !activityLoading && !activityError}
      {#if activityEntries.length === 0}
        <div class="panel empty-watchers-panel">
          <p class="muted">No monitoring activity recorded yet. Run a monitoring check cycle or refresh followed series.</p>
        </div>
      {:else}
        <div class="activity-table-wrapper">
          <table class="activity-table font-small">
            <thead>
              <tr>
                <th>Time</th>
                <th>Series & Target</th>
                <th>Action</th>
                <th>Provider</th>
                <th>Confidence</th>
                <th>Decision Code</th>
                <th>Details</th>
              </tr>
            </thead>
            <tbody>
              {#each activityEntries as entry (entry.id)}
                <tr>
                  <td class="nowrap muted">{ago(entry.createdAt)}</td>
                  <td>
                    {#if entry.mangaId}
                      <a href="#/title/{entry.mangaId}"><strong>{entry.seriesTitle}</strong></a>
                    {:else}
                      <strong>{entry.seriesTitle}</strong>
                    {/if}
                    <span class="muted"> Vol. {entry.volumeNumber}</span>
                  </td>
                  <td>
                    <span class="action-badge action-{entry.action}">{entry.action}</span>
                  </td>
                  <td><span class="provider-pill">{entry.source}</span></td>
                  <td>
                    <span class="conf-pill conf-{entry.confidence.toLowerCase()}">{entry.confidence}</span>
                  </td>
                  <td>
                    {#if entry.decisionCode}
                      <code class="decision-code font-mono font-small">{entry.decisionCode}</code>
                    {:else}
                      <span class="muted">-</span>
                    {/if}
                  </td>
                  <td class="activity-details-cell">{entry.details}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {/if}
    {/if}
  {/if}
</div>

{#snippet calendarCard(item: ReleaseCalendarItem)}
  {@const daysUntil = getDaysUntil(item.releaseDate)}
  {@const autoMode = item.autoAcquireEnabled ? (item.autoAcquireDryRun ? 'Dry-Run' : 'Active') : 'Disabled'}
  <div class="cal-card" class:card-owned={item.isOwned} class:card-today={item.status === 'ReleasedToday'}>
    <div class="cal-card-head">
      <div class="cal-badges">
        <span class="badge badge-market">{item.market}</span>
        {#if item.status === 'ReleasedToday'}
          <span class="badge badge-today">Today</span>
        {:else if item.status === 'Pending'}
          <span class="badge badge-pending">{formatRelativeDays(daysUntil)}</span>
        {:else}
          <span class="badge badge-past">{formatRelativeDays(daysUntil)}</span>
        {/if}
        <span class="badge badge-own {item.isOwned ? 'owned' : 'missing'}">
          {item.isOwned ? 'Owned' : 'Missing'}
        </span>
      </div>
      <span class="cal-date">{item.releaseDate}</span>
    </div>

    <div class="cal-card-body">
      <h3 class="cal-title">
        {#if item.mangaId}
          <a href="#/title/{item.mangaId}" class="cal-link">{item.seriesTitle}</a>
        {:else}
          <span>{item.seriesTitle}</span>
        {/if}
      </h3>
      <div class="cal-sub">
        <strong>Vol. {item.volumeNumber}</strong>
        {#if item.edition && item.edition !== 'standard'}
          <span class="cal-edition">({item.edition})</span>
        {/if}
        {#if item.candidatesCount > 0}
          <span class="cal-candidates font-small muted">{item.candidatesCount} candidate{item.candidatesCount === 1 ? '' : 's'}</span>
        {/if}
      </div>
    </div>

    <div class="cal-card-foot">
      <div class="cal-policy">
        <span class="policy-label">Policy:</span>
        <span class="policy-val policy-{autoMode.toLowerCase().replace(/[^a-z]/g, '')}">{autoMode}</span>
      </div>
      <div class="cal-actions">
        {#if item.mangaId}
          <button
            class="btn mini flat"
            onclick={() => {
              inspectingVolumeTarget = { mangaId: item.mangaId!, volume: item.volumeNumber };
            }}
            title="Inspect candidate transports and gate evaluation for Volume {item.volumeNumber}"
          >
            <Icon name="eye" size={13} /> Inspect
          </button>
          <a class="btn mini" href="#/title/{item.mangaId}">
            Title <Icon name="arrow" size={13} />
          </a>
        {:else}
          <span class="muted font-small">Target #{item.targetId}</span>
        {/if}
      </div>
    </div>
  </div>
{/snippet}

{#if inspectingVolumeTarget}
  <CandidateInspectorModal
    open={true}
    mangaId={inspectingVolumeTarget.mangaId}
    volume={inspectingVolumeTarget.volume}
    onClose={() => { inspectingVolumeTarget = null; }}
  />
{/if}

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 16px;
  }

  .section-head h2 {
    font-size: 20px;
  }

  .problems {
    align-items: flex-start;
    padding: 12px;
    box-shadow: var(--sh-4);
  }

  .problems .rows {
    align-self: stretch;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    flex-wrap: wrap;
  }

  .done .main {
    color: var(--muted);
  }

  .link {
    text-decoration: underline;
  }

  .head-left {
    display: flex;
    align-items: baseline;
    gap: 20px;
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
    transition: all 0.15s ease;
  }

  .pill:hover {
    color: var(--ink);
  }

  .pill.active {
    background: var(--ink);
    color: var(--paper);
    box-shadow: var(--sh-1);
  }

  /* Release Calendar Grid & Cards */
  .calendar-section {
    margin-top: 24px;
  }

  .calendar-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 14px;
  }

  .cal-card {
    background: var(--paper);
    border: 2px solid var(--shade);
    border-radius: 8px;
    padding: 14px 16px;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    gap: 12px;
    box-shadow: var(--sh-1);
    transition: transform 0.15s ease, border-color 0.15s ease;
  }

  .cal-card:hover {
    border-color: var(--dim);
  }

  .card-today {
    border-color: var(--spot, #d9534f);
    background: color-mix(in srgb, var(--spot, #d9534f) 4%, var(--paper));
  }

  .cal-card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .cal-badges {
    display: flex;
    gap: 6px;
    align-items: center;
    flex-wrap: wrap;
  }

  .badge {
    font-size: 11px;
    font-weight: 700;
    padding: 2px 7px;
    border-radius: 4px;
    text-transform: uppercase;
  }

  .badge-market {
    background: var(--shade);
    color: var(--ink);
  }

  .badge-today {
    background: var(--spot, #d9534f);
    color: #fff;
  }

  .badge-pending {
    background: #0284c7;
    color: #fff;
  }

  .badge-past {
    background: var(--shade);
    color: var(--dim);
  }

  .badge-own.owned {
    background: #16a34a;
    color: #fff;
  }

  .badge-own.missing {
    background: var(--dim);
    color: var(--paper);
  }

  .cal-date {
    font-size: 12px;
    font-weight: 600;
    color: var(--dim);
    font-variant-numeric: tabular-nums;
  }

  .cal-card-body {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .cal-title {
    font-size: 16px;
    font-weight: 700;
    margin: 0;
    line-height: 1.3;
  }

  .cal-link {
    color: var(--ink);
    text-decoration: none;
  }

  .cal-link:hover {
    text-decoration: underline;
  }

  .cal-sub {
    display: flex;
    align-items: baseline;
    gap: 8px;
    font-size: 13px;
    color: var(--muted);
    flex-wrap: wrap;
  }

  .cal-isbn {
    font-size: 11px;
  }

  .cal-card-foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-top: 1px solid var(--shade);
    padding-top: 10px;
    gap: 8px;
  }

  .cal-policy {
    display: flex;
    gap: 4px;
    align-items: center;
    font-size: 12px;
  }

  .policy-label {
    color: var(--muted);
  }

  .policy-val {
    font-weight: 600;
  }

  .policy-val.policy-enabled {
    color: #16a34a;
  }

  .policy-val.policy-dryrun {
    color: #eab308;
  }

  .policy-val.policy-disabled {
    color: var(--dim);
  }

  .cal-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .today-headline {
    color: var(--spot, #d9534f);
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .muted-headline {
    color: var(--muted);
  }

  .pulse-indicator {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--spot, #d9534f);
    display: inline-block;
    animation: pulse 1.8s infinite;
  }

  @keyframes pulse {
    0% { transform: scale(0.95); opacity: 0.8; }
    50% { transform: scale(1.3); opacity: 1; }
    100% { transform: scale(0.95); opacity: 0.8; }
  }

  .empty-calendar-panel {
    padding: 32px 24px;
    text-align: center;
    background: var(--paper);
    border: 2px dashed var(--shade);
    border-radius: 8px;
    margin-top: 20px;
  }

  .monospace {
    font-family: monospace;
  }

  .font-small {
    font-size: 12px;
  }

  /* Availability Watchers Dashboard & Operational Table Styles (M12) */
  .watchers-explainer-card {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 0.75rem;
    background: var(--shade);
    border: 1px solid var(--line);
    border-radius: 8px;
    padding: 0.85rem 1rem;
    margin-bottom: 0.75rem;
  }

  .explainer-item {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .exp-title {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.82rem;
    color: var(--ink);
  }

  .explainer-item p {
    margin: 0;
    font-size: 0.76rem;
    color: var(--muted);
    line-height: 1.35;
  }

  .watchers-stat-grid {
    grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
    margin-bottom: 1rem;
  }

  .table-toolbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.75rem;
    margin-bottom: 0.75rem;
  }

  .toolbar-left {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    flex-wrap: wrap;
  }

  .search-field {
    min-width: 220px;
    padding: 0.4rem 0.75rem;
    font-size: 13px;
  }

  .filter-label {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .select-compact {
    padding: 0.35rem 0.6rem;
    font-size: 12px;
  }

  .bulk-actions-group {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
  }

  .th-checkbox, .td-checkbox {
    width: 36px;
    text-align: center;
  }

  .td-checkbox input, .th-checkbox input {
    cursor: pointer;
    width: 15px;
    height: 15px;
    accent-color: var(--ink);
  }

  .sortable {
    cursor: pointer;
    user-select: none;
  }

  .sortable:hover {
    color: var(--ink);
    background: var(--shade-2);
  }

  .th-vol, .td-vol {
    width: 70px;
    text-align: center;
  }

  .th-status, .td-status {
    width: 100px;
  }

  .th-time, .td-time {
    width: 110px;
  }

  .th-actions, .td-actions {
    width: 140px;
    text-align: right;
  }

  .series-name-wrap {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .series-link {
    color: var(--ink);
    text-decoration: none;
    font-size: 14px;
  }

  .series-link:hover {
    text-decoration: underline;
  }

  .vol-num-tag {
    font-weight: 700;
    color: var(--ink);
  }

  .edition-pill {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 4px;
    background: var(--shade);
    color: var(--dim);
  }

  .cand-cell {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    max-width: 320px;
  }

  .provider-pill {
    background: var(--spot, #ff7a00);
    color: var(--ink);
    font-size: 10px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 3px;
    text-transform: uppercase;
    flex-shrink: 0;
  }

  .cand-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }

  .action-btn-row {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 0.35rem;
  }

  .watcher-row.row-selected {
    background: color-mix(in srgb, var(--spot, #ff7a00) 10%, var(--paper));
  }

  .watcher-row.row-expanded {
    background: var(--shade);
  }

  .drawer-row td {
    background: var(--shade);
    padding: 0.75rem 1rem !important;
    border-bottom: 2px solid var(--line) !important;
  }

  .drawer-content {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .drawer-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 0.75rem;
  }

  .drawer-field {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .drawer-label {
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    font-weight: 600;
  }

  .drawer-value {
    font-size: 13px;
    font-weight: 600;
  }

  .drawer-cand-box {
    background: var(--paper);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .drawer-error-box {
    background: var(--status-error-bg);
    border: 1px solid var(--status-error);
    color: var(--status-error);
    border-radius: 6px;
    padding: 0.5rem 0.75rem;
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-weight: 600;
  }

  .empty-watchers-panel {
    padding: 32px 24px;
    text-align: center;
    background: var(--paper);
    border: 2px dashed var(--line);
    border-radius: 8px;
    margin-top: 1rem;
  }

  .text-warn {
    color: var(--status-warn, #d97706);
    font-weight: 700;
  }

  /* Review Queue Styles (M10.1 / M12) */
  .review-queue-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    margin-bottom: 1rem;
    flex-wrap: wrap;
  }

  .review-queue-info h3 {
    margin: 0 0 0.25rem 0;
    font-size: 1.1rem;
  }

  .review-queue-info p {
    margin: 0;
    font-size: 0.85rem;
  }

  .review-queue-actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .review-status-filter-bar {
    margin-bottom: 1rem;
  }

  .review-items-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .review-card {
    background: var(--paper);
    border: 2px solid var(--shade);
    border-radius: 8px;
    padding: 1rem;
    box-shadow: var(--sh-1);
    transition: border-color 0.15s ease;
  }

  .review-card:hover {
    border-color: var(--dim);
  }

  .review-card.status-approved {
    border-left: 4px solid var(--status-ok, #16a34a);
  }

  .review-card.status-rejected {
    border-left: 4px solid var(--status-error, #dc2626);
    opacity: 0.8;
  }

  .review-card.status-skipped {
    border-left: 4px solid var(--status-warn, #d97706);
    opacity: 0.8;
  }

  .review-main-row {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
  }

  .review-target-info {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .review-title-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .review-title-row .series-title {
    font-size: 1rem;
    color: var(--ink);
  }

  .status-pill {
    font-size: 0.65rem;
    font-weight: 700;
    padding: 0.15rem 0.4rem;
    border-radius: 4px;
    letter-spacing: 0.05em;
  }

  .status-pill.status-pending {
    background: var(--status-warn-bg);
    color: var(--status-warn);
    border: 1px solid var(--status-warn);
  }

  .status-pill.status-approved {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1px solid var(--status-ok);
  }

  .status-pill.status-rejected {
    background: var(--status-error-bg);
    color: var(--status-error);
    border: 1px solid var(--status-error);
  }

  .status-pill.status-skipped {
    background: var(--shade);
    color: var(--dim);
    border: 1px solid var(--line);
  }

  .conf-pill {
    font-size: 0.65rem;
    font-weight: 700;
    padding: 0.15rem 0.4rem;
    border-radius: 4px;
  }

  .conf-pill.conf-high {
    background: var(--status-ok-bg);
    color: var(--status-ok);
  }

  .conf-pill.conf-medium {
    background: var(--status-warn-bg);
    color: var(--status-warn);
  }

  .conf-pill.conf-low {
    background: var(--status-error-bg);
    color: var(--status-error);
  }

  .review-candidate-title {
    word-break: break-all;
    color: var(--ink);
  }

  .review-meta-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex-wrap: wrap;
  }

  .dest-path {
    max-width: 300px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .review-reason-note {
    background: var(--shade);
    padding: 0.25rem 0.5rem;
    border-radius: 4px;
    border-left: 2px solid var(--dim);
  }

  .review-actions-cell {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
  }

  .inline-reject-box {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .inline-reject-box input {
    width: 140px;
  }

  /* Activity History Table Styles (M10.1 / M12) */
  .activity-table-wrapper {
    overflow-x: auto;
    background: var(--paper);
    border: 2px solid var(--shade);
    border-radius: 8px;
    box-shadow: var(--sh-1);
  }

  .activity-table {
    width: 100%;
    border-collapse: collapse;
  }

  .activity-table th,
  .activity-table td {
    padding: 0.6rem 0.8rem;
    text-align: left;
    border-bottom: 1px solid var(--line);
  }

  .activity-table th {
    background: var(--shade);
    color: var(--ink);
    font-weight: 700;
  }

  .action-badge {
    font-size: 0.7rem;
    font-weight: 700;
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    text-transform: uppercase;
  }

  .action-badge.action-discovered {
    background: var(--status-info-bg);
    color: var(--status-info);
  }

  .action-badge.action-plan_proposed {
    background: var(--status-warn-bg);
    color: var(--status-warn);
  }

  .action-badge.action-approved {
    background: var(--status-ok-bg);
    color: var(--status-ok);
  }

  .action-badge.action-rejected {
    background: var(--status-error-bg);
    color: var(--status-error);
  }

  .action-badge.action-simulated {
    background: #f3e8ff;
    color: #9333ea;
  }

  .action-badge.action-auto_acquired {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    font-weight: 800;
  }

  .decision-code {
    background: var(--shade);
    padding: 0.15rem 0.4rem;
    border-radius: 3px;
    font-size: 0.75rem;
    color: var(--ink);
  }

  .activity-details-cell {
    max-width: 350px;
    word-break: break-word;
  }
</style>
