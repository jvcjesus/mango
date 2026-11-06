<script lang="ts">
  import {
    get,
    getReleaseCalendar,
    getWatchersDashboard,
    refreshWatcherById,
    executeWatchersBulkAction,
    message,
    send,
    type ReleaseCalendarItem,
    type UpdateItem,
    type Updates,
    type WatchersDashboardResponseDto,
  } from '../lib/api';
  import CandidateInspectorModal from '../lib/CandidateInspectorModal.svelte';
  import Icon from '../lib/Icon.svelte';
  import { keep, recall } from '../lib/keep';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { ago, dayKey, dayLabel } from '../lib/util';

  // Navigation mode
  let activeTab = $state<'chapters' | 'calendar' | 'watchers'>('chapters');

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
      </div>
    </div>

    {#if activeTab === 'chapters'}
      <div class="tools">
        {#if data?.status.finishedAt && !running}
          <strong>Checked {ago(data.status.finishedAt)}: {data.status.found} new{data.status.failed ? `, ${data.status.failed} failed` : ''}{data.status.waiting ? `, ${data.status.waiting} not due yet` : ''}</strong>
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
    {:else}
      <div class="tools">
        <div class="pill-group" role="radiogroup" aria-label="Market Filter">
          <button
            class="pill"
            class:active={marketFilter === 'EN'}
            onclick={() => handleMarketChange('EN')}
          >
            EN
          </button>
          <button
            class="pill"
            class:active={marketFilter === 'JA'}
            onclick={() => handleMarketChange('JA')}
          >
            JA
          </button>
          <button
            class="pill"
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
    <!-- AVAILABILITY & WATCHERS DASHBOARD (M7.2) -->
    <Notice loading={watchersLoading} error={watchersError} retry={loadWatchers} />

    {#if !watchersLoading && !watchersError}
      {#if watchersData}
        <!-- Educational Workflow Explainer (M8.5) -->
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

        <!-- Metrics Grid -->
        <div class="watchers-summary-grid">
          <button type="button" class="metric-card" class:selected={watchersFilter === 'ALL'} onclick={() => watchersFilter = 'ALL'}>
            <span class="metric-val">{watchersData.summary.totalWatchers}</span>
            <span class="metric-lbl">Total Watchers</span>
          </button>
          <button type="button" class="metric-card metric-upcoming" class:selected={watchersFilter === 'UPCOMING'} onclick={() => watchersFilter = 'UPCOMING'}>
            <span class="metric-val">{watchersData.summary.upcomingCount}</span>
            <span class="metric-lbl">Upcoming</span>
          </button>
          <button type="button" class="metric-card metric-awaiting" class:selected={watchersFilter === 'RELEASED_AWAITING_PROVIDER'} onclick={() => watchersFilter = 'RELEASED_AWAITING_PROVIDER'}>
            <span class="metric-val">{watchersData.summary.releasedAwaitingCount}</span>
            <span class="metric-lbl">Awaiting Upload</span>
          </button>
          <button type="button" class="metric-card metric-candidate" class:selected={watchersFilter === 'ELIGIBLE_CANDIDATE_FOUND'} onclick={() => watchersFilter = 'ELIGIBLE_CANDIDATE_FOUND'}>
            <span class="metric-val">{watchersData.summary.candidateFoundCount}</span>
            <span class="metric-lbl">Candidates Found</span>
          </button>
          <button type="button" class="metric-card metric-notified" class:selected={watchersFilter === 'NOTIFIED'} onclick={() => watchersFilter = 'NOTIFIED'}>
            <span class="metric-val">{watchersData.summary.notifiedCount}</span>
            <span class="metric-lbl">Notified</span>
          </button>
          <button type="button" class="metric-card metric-owned" class:selected={watchersFilter === 'OWNED'} onclick={() => watchersFilter = 'OWNED'}>
            <span class="metric-val">{watchersData.summary.ownedCount}</span>
            <span class="metric-lbl">Owned / Complete</span>
          </button>
          <button type="button" class="metric-card metric-error" class:selected={watchersFilter === 'ERROR'} onclick={() => watchersFilter = 'ERROR'}>
            <span class="metric-val">{watchersData.summary.errorCount}</span>
            <span class="metric-lbl">Issues / Errors</span>
          </button>
        </div>

        <!-- Filter & Search Toolbar -->
        <div class="watchers-toolbar">
          <div class="filter-pills">
            {#each [
              ['ALL', 'All'],
              ['UPCOMING', 'Upcoming'],
              ['RELEASED_AWAITING_PROVIDER', 'Awaiting Upload'],
              ['ELIGIBLE_CANDIDATE_FOUND', 'Candidates Found'],
              ['NOTIFIED', 'Notified'],
              ['OWNED', 'Owned'],
              ['ERROR', 'Issues']
            ] as [fKey, fLabel]}
              <button
                class="pill mini"
                class:active={watchersFilter === fKey}
                onclick={() => watchersFilter = fKey}
              >
                {fLabel}
              </button>
            {/each}
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
                Clear
              </button>
            {/if}
          </div>
          <div class="search-box">
            <input
              type="search"
              class="field search-field"
              placeholder="Search title…"
              bind:value={watchersSearch}
            />
          </div>
        </div>

        <!-- Filtered Watchers List -->
        {@const filteredWatchers = watchersData.watchers.filter(w => {
          if (watchersSearch.trim()) {
            const q = watchersSearch.toLowerCase();
            if (!w.seriesTitle?.toLowerCase().includes(q)) return false;
          }
          if (watchersFilter === 'ALL') return true;
          if (watchersFilter === 'OWNED') return w.isOwned;
          if (watchersFilter === 'ERROR') {
            return w.state === 'PROVIDER_UNAVAILABLE' || w.state === 'RATE_LIMITED' || w.state === 'REVIEW_REQUIRED';
          }
          return w.state === watchersFilter;
        })}

        {#if filteredWatchers.length === 0}
          <div class="panel empty-watchers-panel">
            <p class="muted">No availability watchers match current filter.</p>
          </div>
        {:else}
          <div class="watchers-list">
            {#each filteredWatchers as watcher (watcher.id)}
              <div class="watcher-card" class:card-highlight={watcher.seriesTitle === 'Kagurabachi' && watcher.volumeNumber === 9}>
                <div class="watcher-main-row">
                  <label class="watcher-checkbox-label" title="Select Volume {watcher.volumeNumber}">
                    <input
                      type="checkbox"
                      checked={selectedWatcherIds.has(watcher.id)}
                      onchange={(e) => toggleWatcherSelect(watcher.id, (e.target as HTMLInputElement).checked)}
                    />
                  </label>
                  <div class="watcher-title-col">
                    <div class="watcher-series-head">
                      {#if watcher.mangaId}
                        <a href="#/title/{watcher.mangaId}" class="watcher-series-link">
                          <strong>{watcher.seriesTitle || `Manga #${watcher.mangaId}`}</strong>
                        </a>
                      {:else}
                        <strong>{watcher.seriesTitle}</strong>
                      {/if}
                      <span class="vol-tag">Vol {watcher.volumeNumber}</span>
                      <span class="pill-badge edition-pill">{watcher.edition}</span>
                      {#if watcher.isOwned}
                        <span class="pill-badge active">Owned</span>
                      {:else}
                        <span class="pill-badge state-badge state-{watcher.state.toLowerCase()}">
                          {watcher.state}
                        </span>
                      {/if}
                    </div>

                    <div class="watcher-meta-row font-small muted">
                      {#if watcher.releaseDate}
                        <span>Released: <strong>{watcher.releaseDate}</strong></span>
                      {/if}
                      {#if watcher.lastCheckedAt}
                        <span>Checked: {ago(watcher.lastCheckedAt * 1000)}</span>
                      {/if}
                      <span>Next Check: {ago(watcher.nextCheckAt * 1000)}</span>
                      {#if watcher.lastProviderChecked}
                        <span>Provider: <code>{watcher.lastProviderChecked}</code></span>
                      {/if}
                    </div>
                  </div>

                  <div class="watcher-actions-col">
                    <button
                      class="btn mini"
                      disabled={refreshingWatchers[watcher.id]}
                      onclick={() => handleRefreshWatcher(watcher.id)}
                      title="Run on-demand check for Volume {watcher.volumeNumber}"
                    >
                      {#if refreshingWatchers[watcher.id]}
                        <span class="spin"><Icon name="spinner" size={13} stroke={3} /></span> Checking…
                      {:else}
                        <Icon name="search" size={13} /> Check Now
                      {/if}
                    </button>

                    {#if watcher.mangaId}
                      <button
                        class="btn mini flat"
                        onclick={() => {
                          inspectingVolumeTarget = { mangaId: watcher.mangaId!, volume: watcher.volumeNumber };
                        }}
                        title="Inspect candidate transports"
                      >
                        <Icon name="eye" size={13} /> Inspect
                      </button>
                    {/if}
                  </div>
                </div>

                <!-- Top Candidate Preview if available -->
                {#if watcher.topCandidateTitle || (watcher.candidateCount && watcher.candidateCount > 0)}
                  <div class="candidate-preview-bar font-small">
                    <span class="cand-badge">
                      <Icon name="download" size={12} />
                      {watcher.candidateCount || 1} Candidate{(watcher.candidateCount || 1) > 1 ? 's' : ''}
                    </span>
                    {#if watcher.topCandidateProvider}
                      <span class="provider-pill">{watcher.topCandidateProvider}</span>
                    {/if}
                    <span class="cand-title font-mono">{watcher.topCandidateTitle || 'Candidate Available'}</span>
                  </div>
                {/if}

                {#if watcher.lastError}
                  <div class="watcher-error-note font-small">
                    <Icon name="close" size={12} /> {watcher.lastError}
                  </div>
                {/if}
              </div>
            {/each}
          </div>
        {/if}
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

  /* Availability Watchers Dashboard Styles (M7.2) */
  .watchers-summary-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
    gap: 0.65rem;
    margin: 1rem 0;
  }

  .watchers-summary-grid .metric-card {
    background: var(--shade);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    padding: 0.75rem;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.25rem;
    text-align: center;
    cursor: pointer;
    transition: border-color 0.15s ease, background 0.15s ease;
  }

  .watchers-summary-grid .metric-card:hover {
    border-color: var(--spot, #ff7a00);
  }

  .watchers-summary-grid .metric-card.selected {
    border-color: var(--spot, #ff7a00);
    background: rgba(255, 122, 0, 0.08);
  }

  .metric-val {
    font-size: 1.4rem;
    font-weight: 700;
  }

  .metric-lbl {
    font-size: 0.72rem;
    color: var(--muted, #888);
  }

  .metric-upcoming .metric-val { color: #3b82f6; }
  .metric-awaiting .metric-val { color: #eab308; }
  .metric-candidate .metric-val { color: var(--spot, #ff7a00); }
  .metric-notified .metric-val { color: #a855f7; }
  .metric-owned .metric-val { color: #22c55e; }
  .metric-error .metric-val { color: #ef4444; }

  .watchers-toolbar {
    display: flex;
    justify-content: space-between;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.75rem;
    margin-bottom: 1rem;
  }

  .filter-pills {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }

  .search-field {
    min-width: 220px;
    padding: 0.4rem 0.75rem;
    font-size: 13px;
  }

  .watchers-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .watcher-card {
    background: var(--shade);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }

  .watcher-card.card-highlight {
    border-color: var(--spot, #ff7a00);
    box-shadow: 0 0 12px rgba(255, 122, 0, 0.15);
  }

  .watcher-main-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    flex-wrap: wrap;
  }

  .watcher-title-col {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
  }

  .watcher-series-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .watcher-series-link {
    color: var(--ink);
    text-decoration: none;
    font-size: 15px;
  }

  .watcher-series-link:hover {
    text-decoration: underline;
  }

  .vol-tag {
    font-weight: 700;
    font-size: 13px;
    color: var(--spot, #ff7a00);
  }

  .edition-pill {
    font-size: 11px;
    padding: 2px 6px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.06);
  }

  .state-badge {
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.08);
  }

  .watcher-meta-row {
    display: flex;
    gap: 1rem;
    flex-wrap: wrap;
  }

  .watcher-actions-col {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .candidate-preview-bar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: rgba(255, 122, 0, 0.08);
    border: 1px solid rgba(255, 122, 0, 0.2);
    border-radius: 6px;
    padding: 0.4rem 0.75rem;
  }

  .cand-badge {
    display: flex;
    align-items: center;
    gap: 0.25rem;
    font-weight: 700;
    color: var(--spot, #ff7a00);
  }

  .provider-pill {
    background: var(--spot, #ff7a00);
    color: #111;
    font-size: 10px;
    font-weight: 700;
    padding: 1px 5px;
    border-radius: 3px;
    text-transform: uppercase;
  }

  .cand-title {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .watcher-error-note {
    color: #ef4444;
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .watchers-explainer-card {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 0.75rem;
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 8px;
    padding: 0.85rem 1rem;
    margin-bottom: 0.5rem;
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
    color: var(--accent, #6366f1);
  }

  .explainer-item p {
    margin: 0;
    font-size: 0.76rem;
    color: var(--muted, #8b949e);
    line-height: 1.35;
  }

  .bulk-actions-group {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex-wrap: wrap;
  }

  .watcher-checkbox-label {
    display: flex;
    align-items: center;
    cursor: pointer;
    padding-right: 0.25rem;
  }

  .watcher-checkbox-label input {
    cursor: pointer;
    width: 15px;
    height: 15px;
    accent-color: var(--accent, #6366f1);
  }
</style>
