<script lang="ts">
  import { onMount } from 'svelte';
  import {
    getUpcomingReleases,
    type UpcomingReleaseItem,
    type UndatedReleaseItem,
    type UpcomingReleasesResponse,
  } from '../lib/api';
  import Cover from '../lib/Cover.svelte';
  import Icon from '../lib/Icon.svelte';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { go } from '../lib/router.svelte';

  type ScopeOption = 'mango_library_only' | 'tracked_lists' | 'all_available';
  type HorizonOption = 'all' | '7' | '30' | '90';
  type ViewMode = 'list' | 'calendar';
  type FormatOption = 'all' | 'physical' | 'digital';

  let scope = $state<ScopeOption>('mango_library_only');
  let horizon = $state<HorizonOption>('all');
  let viewMode = $state<ViewMode>('list');
  let formatFilter = $state<FormatOption>('all');

  let data = $state<UpcomingReleasesResponse | null>(null);
  let loading = $state(true);
  let refreshing = $state(false);
  let error = $state<string | null>(null);
  let showUndated = $state(false);

  // Calendar state: current displayed year and month (0-indexed)
  const now = new Date();
  let calYear = $state(now.getFullYear());
  let calMonth = $state(now.getMonth());

  async function loadReleases(isManualRefresh = false) {
    if (isManualRefresh) refreshing = true;
    else loading = true;
    error = null;

    try {
      const days = horizon === 'all' ? undefined : parseInt(horizon, 10);
      const res = await getUpcomingReleases({
        scope,
        days,
        format: formatFilter === 'all' ? undefined : formatFilter,
      });
      data = res;
      if (isManualRefresh) {
        toast.info('Release schedules refreshed');
      }
    } catch (e: any) {
      error = e?.message || 'Failed to load upcoming releases';
    } finally {
      loading = false;
      refreshing = false;
    }
  }

  onMount(() => {
    loadReleases();
  });

  function handleScopeChange(newScope: ScopeOption) {
    scope = newScope;
    loadReleases();
  }

  function handleHorizonChange(newHorizon: HorizonOption) {
    horizon = newHorizon;
    loadReleases();
  }

  function handleFormatChange(newFmt: FormatOption) {
    formatFilter = newFmt;
    loadReleases();
  }

  function formatRelativeDays(days: number): string {
    if (days === 0) return 'Today';
    if (days === 1) return 'Tomorrow';
    return `In ${days} days`;
  }

  function formatDateHuman(dateStr: string): string {
    if (!dateStr) return '';
    try {
      const parts = dateStr.split('-');
      if (parts.length === 3) {
        const d = new Date(parseInt(parts[0], 10), parseInt(parts[1], 10) - 1, parseInt(parts[2], 10));
        return d.toLocaleDateString(undefined, {
          weekday: 'short',
          month: 'short',
          day: 'numeric',
          year: 'numeric',
        });
      }
    } catch {
      // Fall through
    }
    return dateStr;
  }

  function navToSeries(mangaId?: number | null, seriesId?: string) {
    if (mangaId) {
      go(`/title/${mangaId}`);
    } else if (seriesId?.startsWith('manga:')) {
      const id = seriesId.replace('manga:', '');
      go(`/title/${id}`);
    } else {
      go('/library');
    }
  }

  // Calendar Calculations
  const monthNames = [
    'January', 'February', 'March', 'April', 'May', 'June',
    'July', 'August', 'September', 'October', 'November', 'December',
  ];

  function prevMonth() {
    if (calMonth === 0) {
      calMonth = 11;
      calYear -= 1;
    } else {
      calMonth -= 1;
    }
  }

  function nextMonth() {
    if (calMonth === 11) {
      calMonth = 0;
      calYear += 1;
    } else {
      calMonth += 1;
    }
  }

  function todayMonth() {
    const cur = new Date();
    calYear = cur.getFullYear();
    calMonth = cur.getMonth();
  }

  interface CalendarDay {
    dayNumber: number;
    dateStr: string;
    isCurrentMonth: boolean;
    isToday: boolean;
    releases: UpcomingReleaseItem[];
  }

  const calendarGrid = $derived.by((): CalendarDay[] => {
    if (!data) return [];

    const firstDayIndex = new Date(calYear, calMonth, 1).getDay(); // 0 is Sunday
    const daysInMonth = new Date(calYear, calMonth + 1, 0).getDate();
    const daysInPrevMonth = new Date(calYear, calMonth, 0).getDate();

    const todayDate = new Date();
    const todayYMD = `${todayDate.getFullYear()}-${String(todayDate.getMonth() + 1).padStart(2, '0')}-${String(todayDate.getDate()).padStart(2, '0')}`;

    // Map releaseDate -> items
    const dateMap = new Map<string, UpcomingReleaseItem[]>();
    for (const rel of data.releases) {
      const d = rel.releaseDate;
      if (!dateMap.has(d)) dateMap.set(d, []);
      dateMap.get(d)!.push(rel);
    }

    const grid: CalendarDay[] = [];

    // Prev month padding
    for (let i = firstDayIndex - 1; i >= 0; i--) {
      const dayNum = daysInPrevMonth - i;
      const prevM = calMonth === 0 ? 12 : calMonth;
      const prevY = calMonth === 0 ? calYear - 1 : calYear;
      const dateStr = `${prevY}-${String(prevM).padStart(2, '0')}-${String(dayNum).padStart(2, '0')}`;
      grid.push({
        dayNumber: dayNum,
        dateStr,
        isCurrentMonth: false,
        isToday: dateStr === todayYMD,
        releases: dateMap.get(dateStr) ?? [],
      });
    }

    // Current month days
    for (let dayNum = 1; dayNum <= daysInMonth; dayNum++) {
      const dateStr = `${calYear}-${String(calMonth + 1).padStart(2, '0')}-${String(dayNum).padStart(2, '0')}`;
      grid.push({
        dayNumber: dayNum,
        dateStr,
        isCurrentMonth: true,
        isToday: dateStr === todayYMD,
        releases: dateMap.get(dateStr) ?? [],
      });
    }

    // Next month padding to fill complete weeks (multiples of 7)
    const remaining = (7 - (grid.length % 7)) % 7;
    for (let dayNum = 1; dayNum <= remaining; dayNum++) {
      const nextM = calMonth === 11 ? 1 : calMonth + 2;
      const nextY = calMonth === 11 ? calYear + 1 : calYear;
      const dateStr = `${nextY}-${String(nextM).padStart(2, '0')}-${String(dayNum).padStart(2, '0')}`;
      grid.push({
        dayNumber: dayNum,
        dateStr,
        isCurrentMonth: false,
        isToday: dateStr === todayYMD,
        releases: dateMap.get(dateStr) ?? [],
      });
    }

    return grid;
  });

  // Grouped by date for Chronological List view
  const groupedReleases = $derived.by((): { dateStr: string; items: UpcomingReleaseItem[] }[] => {
    if (!data) return [];
    const groups: { dateStr: string; items: UpcomingReleaseItem[] }[] = [];
    let curDate = '';
    let curList: UpcomingReleaseItem[] = [];

    for (const r of data.releases) {
      if (r.releaseDate !== curDate) {
        if (curList.length > 0) {
          groups.push({ dateStr: curDate, items: curList });
        }
        curDate = r.releaseDate;
        curList = [r];
      } else {
        curList.push(r);
      }
    }
    if (curList.length > 0) {
      groups.push({ dateStr: curDate, items: curList });
    }
    return groups;
  });
</script>

<div class="releases-view">
  <!-- Header & Title -->
  <div class="page-header">
    <div class="title-cluster">
      <div class="title-top-row">
        <div class="badge-cluster">
          <span class="icon-wrap">
            <Icon name="calendar" size={24} />
          </span>
          <h1>Upcoming Releases</h1>
        </div>
        <nav class="collection-nav" aria-label="Collection View">
          <a href="#/library" class="btn pill" title="View physically owned manga in Komga">
            <Icon name="book" size={16} />
            <span>Owned in Komga</span>
          </a>
          <a href="#/lists" class="btn pill" title="View tracked catalog from AniList and MangaBaka">
            <Icon name="grid" size={16} />
            <span>Tracked Lists</span>
          </a>
          <a href="#/releases" class="btn pill spot" aria-current="page">
            <Icon name="calendar" size={16} />
            <span>Releases</span>
            {#if data}<strong>{data.totalUpcoming}</strong>{/if}
          </a>
        </nav>
      </div>
      <p class="subtitle">
        Official publisher schedule, upcoming volumes, and release calendar synchronized with your library.
      </p>
    </div>

    <!-- Quick Metrics Summary -->
    {#if data}
      <div class="metrics-row">
        <button
          type="button"
          class="metric-chip"
          class:active={horizon === '7'}
          onclick={() => handleHorizonChange('7')}
        >
          <span class="m-val">{data.next7DaysCount}</span>
          <span class="m-lbl">Next 7 Days</span>
        </button>

        <button
          type="button"
          class="metric-chip"
          class:active={horizon === '30'}
          onclick={() => handleHorizonChange('30')}
        >
          <span class="m-val">{data.next30DaysCount}</span>
          <span class="m-lbl">Next 30 Days</span>
        </button>

        <button
          type="button"
          class="metric-chip"
          class:active={horizon === '90'}
          onclick={() => handleHorizonChange('90')}
        >
          <span class="m-val">{data.next90DaysCount}</span>
          <span class="m-lbl">Next 90 Days</span>
        </button>

        <button
          type="button"
          class="metric-chip"
          class:active={horizon === 'all'}
          onclick={() => handleHorizonChange('all')}
        >
          <span class="m-val">{data.totalUpcoming}</span>
          <span class="m-lbl">All Upcoming</span>
        </button>

        <button
          type="button"
          class="metric-chip undated-chip"
          class:active={showUndated}
          onclick={() => (showUndated = !showUndated)}
        >
          <span class="m-val">{data.undatedCount}</span>
          <span class="m-lbl">Undated / Pending</span>
        </button>
      </div>
    {/if}
  </div>

  <!-- Filters & Controls Bar -->
  <div class="controls-bar">
    <!-- Scope Selector (Default: Mango Library Only) -->
    <div class="control-group">
      <span class="control-label">Scope:</span>
      <div class="pill-group" role="tablist">
        <button
          type="button"
          class="pill-btn"
          class:active={scope === 'mango_library_only'}
          onclick={() => handleScopeChange('mango_library_only')}
        >
          Mango Library Only
        </button>
        <button
          type="button"
          class="pill-btn"
          class:active={scope === 'tracked_lists'}
          onclick={() => handleScopeChange('tracked_lists')}
        >
          Tracked Lists
        </button>
        <button
          type="button"
          class="pill-btn"
          class:active={scope === 'all_available'}
          onclick={() => handleScopeChange('all_available')}
        >
          All Available
        </button>
      </div>
    </div>

    <!-- View Mode Switcher -->
    <div class="control-group right-cluster">
      <div class="pill-group">
        <button
          type="button"
          class="pill-btn icon-pill"
          class:active={viewMode === 'list'}
          onclick={() => (viewMode = 'list')}
          title="Chronological List View"
        >
          <Icon name="list" size={16} />
          <span>List</span>
        </button>
        <button
          type="button"
          class="pill-btn icon-pill"
          class:active={viewMode === 'calendar'}
          onclick={() => (viewMode = 'calendar')}
          title="Monthly Calendar Grid"
        >
          <Icon name="calendar" size={16} />
          <span>Calendar</span>
        </button>
      </div>

      <!-- Format Filter -->
      <select
        class="format-select"
        value={formatFilter}
        onchange={(e) => handleFormatChange(e.currentTarget.value as FormatOption)}
        aria-label="Format Filter"
      >
        <option value="all">All Formats</option>
        <option value="physical">Physical Volumes</option>
        <option value="digital">Digital</option>
      </select>

      <!-- Refresh Action -->
      <button
        type="button"
        class="btn-refresh"
        onclick={() => loadReleases(true)}
        disabled={refreshing || loading}
        title="Refresh release dates"
      >
        <span class:spin={refreshing}>
          <Icon name="refresh" size={16} />
        </span>
        <span>Refresh</span>
      </button>
    </div>
  </div>

  <!-- Error or Loading Notices -->
  {#if error}
    <Notice kind="error">{error}</Notice>
  {/if}

  {#if loading}
    <div class="loading-state">
      <Icon name="spinner" size={28} />
      <p>Gathering release intelligence and publisher schedules…</p>
    </div>
  {:else if data}
    <!-- VIEW MODE: Chronological List View -->
    {#if viewMode === 'list'}
      {#if data.releases.length === 0}
        <div class="empty-state">
          <Icon name="calendar" size={40} />
          <h3>No upcoming releases scheduled</h3>
          <p>
            {#if scope === 'mango_library_only'}
              None of the titles in your Mango library have upcoming physical releases announced yet.
            {:else if scope === 'tracked_lists'}
              No tracked series have upcoming releases within the selected time window.
            {:else}
              No upcoming releases found.
            {/if}
          </p>
        </div>
      {:else}
        <div class="chronological-list">
          {#each groupedReleases as group (group.dateStr)}
            <div class="date-group">
              <div class="date-header">
                <div class="date-title">
                  <span class="cal-icon"><Icon name="clock" size={16} /></span>
                  <h3>{formatDateHuman(group.dateStr)}</h3>
                </div>
                <span class="days-countdown">
                  {formatRelativeDays(group.items[0]?.daysUntil ?? 0)}
                </span>
              </div>

              <div class="releases-grid">
                {#each group.items as item (item.seriesId + item.volumeNumber)}
                  <div
                    class="release-card"
                    class:is-owned={item.isOwned}
                    onclick={() => navToSeries(item.mangaId, item.seriesId)}
                    role="button"
                    tabindex="0"
                    onkeydown={(e) => e.key === 'Enter' && navToSeries(item.mangaId, item.seriesId)}
                  >
                    <div class="card-cover">
                      <Cover src={item.coverUrl ?? null} title={item.title} />
                      {#if item.volumeNumber}
                        <div class="vol-tag">Vol. {item.volumeNumber}</div>
                      {/if}
                    </div>

                    <div class="card-body">
                      <div class="card-head">
                        <span class="series-title" title={item.title}>{item.title}</span>
                        {#if item.isOwned}
                          <span class="owned-pill" title="Owned in Komga library">Owned</span>
                        {:else}
                          <span class="missing-pill" title="Missing from Komga library">Missing</span>
                        {/if}
                      </div>

                      <div class="meta-row">
                        {#if item.publisher}
                          <span class="meta-item publisher">{item.publisher}</span>
                        {/if}
                        <span class="meta-item lang-badge">{item.language.toUpperCase()}</span>
                        <span class="meta-item format-badge">{item.format}</span>
                      </div>

                      <div class="status-row">
                        <span
                          class="status-badge"
                          class:confirmed={item.dateStatus === 'confirmed'}
                          class:tentative={item.dateStatus === 'tentative'}
                        >
                          {item.dateStatus}
                        </span>
                        <span class="source-tag">Source: {item.source}</span>
                      </div>
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/each}
        </div>
      {/if}
    {/if}

    <!-- VIEW MODE: Monthly Calendar Grid -->
    {#if viewMode === 'calendar'}
      <div class="calendar-container">
        <!-- Month Navigation -->
        <div class="calendar-nav">
          <button type="button" class="btn-cal-nav" onclick={prevMonth} title="Previous Month">
            <Icon name="up" size={16} /> Previous
          </button>
          <div class="month-label">
            <h2>{monthNames[calMonth]} {calYear}</h2>
            <button type="button" class="btn-today-pill" onclick={todayMonth}>Today</button>
          </div>
          <button type="button" class="btn-cal-nav" onclick={nextMonth} title="Next Month">
            Next <Icon name="down" size={16} />
          </button>
        </div>

        <!-- Days of Week Header -->
        <div class="days-header-row">
          <span>Sun</span>
          <span>Mon</span>
          <span>Tue</span>
          <span>Wed</span>
          <span>Thu</span>
          <span>Fri</span>
          <span>Sat</span>
        </div>

        <!-- 7-column Calendar Grid -->
        <div class="calendar-cells-grid">
          {#each calendarGrid as day (day.dateStr)}
            <div
              class="cal-cell"
              class:other-month={!day.isCurrentMonth}
              class:is-today={day.isToday}
              class:has-releases={day.releases.length > 0}
            >
              <div class="cell-top">
                <span class="day-num">{day.dayNumber}</span>
                {#if day.releases.length > 0}
                  <span class="rel-count-badge">{day.releases.length}</span>
                {/if}
              </div>

              {#if day.releases.length > 0}
                <div class="cell-releases-list">
                  {#each day.releases as rel (rel.seriesId + rel.volumeNumber)}
                    <button
                      type="button"
                      class="cal-release-chip"
                      class:owned={rel.isOwned}
                      onclick={() => navToSeries(rel.mangaId, rel.seriesId)}
                      title="{rel.title} Vol. {rel.volumeNumber ?? ''} ({rel.isOwned ? 'Owned' : 'Missing'})"
                    >
                      <span class="chip-vol">V{rel.volumeNumber ?? ''}</span>
                      <span class="chip-title">{rel.title}</span>
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </div>
    {/if}

    <!-- Collapsible Undated / Unknown Releases Section -->
    <div class="undated-section" class:expanded={showUndated}>
      <button
        type="button"
        class="undated-toggle"
        onclick={() => (showUndated = !showUndated)}
        aria-expanded={showUndated}
      >
        <div class="toggle-left">
          <Icon name="clock" size={18} />
          <span class="toggle-heading">Undated / Pending Releases</span>
          <span class="count-tag">{data.undated.length}</span>
        </div>
        <div class="toggle-right">
          <span class="toggle-hint">{showUndated ? 'Hide details' : 'Show series lacking release dates'}</span>
          <Icon name={showUndated ? 'up' : 'down'} size={16} />
        </div>
      </button>

      {#if showUndated}
        <div class="undated-content">
          <p class="undated-intro">
            These monitored series are active or ongoing, but their publishers have not yet scheduled an official print volume date. They remain tracked and will update automatically once dates are announced.
          </p>

          <div class="undated-grid">
            {#each data.undated as u (u.seriesId + (u.expectedVolume ?? ''))}
              <div
                class="undated-card"
                onclick={() => navToSeries(u.mangaId, u.seriesId)}
                role="button"
                tabindex="0"
                onkeydown={(e) => e.key === 'Enter' && navToSeries(u.mangaId, u.seriesId)}
              >
                <div class="u-cover">
                  <Cover src={u.coverUrl ?? null} title={u.title} />
                </div>
                <div class="u-info">
                  <div class="u-title">{u.title}</div>
                  {#if u.expectedVolume}
                    <div class="u-vol">Next Expected: Volume {u.expectedVolume}</div>
                  {/if}
                  <div class="u-reason">{u.reason}</div>
                  <div class="u-meta">
                    <span class="source-tag">Source: {u.source}</span>
                    <span class="scope-tag">{u.scope}</span>
                  </div>
                </div>
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .releases-view {
    max-width: 1400px;
    margin: 0 auto;
    padding: 1.5rem 1rem 4rem;
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
  }

  .page-header {
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
  }

  .title-cluster {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .title-top-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 1rem;
  }

  .badge-cluster {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .icon-wrap {
    color: var(--accent, #6366f1);
    display: flex;
    align-items: center;
  }

  h1 {
    font-size: 1.85rem;
    font-weight: 700;
    margin: 0;
    color: var(--text, #111827);
  }

  .subtitle {
    font-size: 0.95rem;
    color: var(--text-muted, #6b7280);
    margin: 0;
  }

  .metrics-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
  }

  .metric-chip {
    background: var(--surface, #ffffff);
    border: 1px solid var(--border, #e5e7eb);
    border-radius: 0.75rem;
    padding: 0.6rem 1rem;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .metric-chip:hover {
    border-color: var(--accent, #6366f1);
    transform: translateY(-1px);
  }

  .metric-chip.active {
    background: var(--accent, #6366f1);
    border-color: var(--accent, #6366f1);
    color: #ffffff;
  }

  .metric-chip.active .m-val,
  .metric-chip.active .m-lbl {
    color: #ffffff;
  }

  .m-val {
    font-size: 1.25rem;
    font-weight: 700;
    color: var(--text, #111827);
  }

  .m-lbl {
    font-size: 0.75rem;
    font-weight: 500;
    color: var(--text-muted, #6b7280);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }

  .undated-chip {
    border-style: dashed;
  }

  /* Controls Bar */
  .controls-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.85rem 1rem;
    background: var(--surface, #ffffff);
    border: 1px solid var(--border, #e5e7eb);
    border-radius: 0.75rem;
  }

  .control-group {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .control-label {
    font-size: 0.85rem;
    font-weight: 600;
    color: var(--text-muted, #6b7280);
  }

  .pill-group {
    display: flex;
    background: var(--surface-secondary, #f3f4f6);
    padding: 0.2rem;
    border-radius: 0.5rem;
    gap: 0.2rem;
  }

  .pill-btn {
    border: none;
    background: transparent;
    padding: 0.35rem 0.75rem;
    font-size: 0.85rem;
    font-weight: 500;
    border-radius: 0.4rem;
    color: var(--text, #374151);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .pill-btn:hover {
    color: var(--text, #111827);
  }

  .pill-btn.active {
    background: var(--surface, #ffffff);
    color: var(--accent, #6366f1);
    font-weight: 600;
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08);
  }

  .icon-pill {
    display: flex;
    align-items: center;
    gap: 0.35rem;
  }

  .right-cluster {
    margin-left: auto;
    flex-wrap: wrap;
  }

  .format-select {
    padding: 0.35rem 0.75rem;
    font-size: 0.85rem;
    background: var(--surface, #ffffff);
    border: 1px solid var(--border, #e5e7eb);
    border-radius: 0.5rem;
    color: var(--text, #374151);
    cursor: pointer;
  }

  .btn-refresh {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    background: var(--surface, #ffffff);
    border: 1px solid var(--border, #e5e7eb);
    padding: 0.35rem 0.75rem;
    font-size: 0.85rem;
    font-weight: 500;
    border-radius: 0.5rem;
    cursor: pointer;
    color: var(--text, #374151);
    transition: all 0.15s ease;
  }

  .btn-refresh:hover:not(:disabled) {
    background: var(--surface-secondary, #f3f4f6);
    border-color: var(--accent, #6366f1);
  }

  .spin {
    animation: spin 1s linear infinite;
    display: inline-flex;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }

  /* Chronological List View */
  .chronological-list {
    display: flex;
    flex-direction: column;
    gap: 1.75rem;
  }

  .date-group {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .date-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding-bottom: 0.4rem;
    border-bottom: 2px solid var(--border, #e5e7eb);
  }

  .date-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .cal-icon {
    color: var(--accent, #6366f1);
    display: flex;
  }

  .date-title h3 {
    margin: 0;
    font-size: 1.1rem;
    font-weight: 700;
    color: var(--text, #111827);
  }

  .days-countdown {
    font-size: 0.8rem;
    font-weight: 600;
    padding: 0.2rem 0.6rem;
    background: var(--accent-light, #e0e7ff);
    color: var(--accent, #4338ca);
    border-radius: 9999px;
  }

  .releases-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(320px, 1fr));
    gap: 1rem;
  }

  .release-card {
    background: var(--surface, #ffffff);
    border: 1px solid var(--border, #e5e7eb);
    border-radius: 0.75rem;
    padding: 0.75rem;
    display: flex;
    gap: 0.85rem;
    cursor: pointer;
    transition: all 0.15s ease;
    text-align: left;
  }

  .release-card:hover {
    border-color: var(--accent, #6366f1);
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.05);
    transform: translateY(-2px);
  }

  .release-card.is-owned {
    border-left: 4px solid var(--status-success, #10b981);
  }

  .card-cover {
    width: 68px;
    height: 98px;
    flex-shrink: 0;
    position: relative;
    border-radius: 0.4rem;
    overflow: hidden;
  }

  .vol-tag {
    position: absolute;
    bottom: 0;
    left: 0;
    right: 0;
    background: rgba(0, 0, 0, 0.75);
    color: #ffffff;
    font-size: 0.7rem;
    font-weight: 700;
    text-align: center;
    padding: 0.15rem 0.2rem;
  }

  .card-body {
    flex: 1;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    min-width: 0;
  }

  .card-head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .series-title {
    font-weight: 700;
    font-size: 0.95rem;
    color: var(--text, #111827);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .owned-pill {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    padding: 0.15rem 0.45rem;
    background: #ecfdf5;
    color: #059669;
    border-radius: 9999px;
    flex-shrink: 0;
  }

  .missing-pill {
    font-size: 0.65rem;
    font-weight: 700;
    text-transform: uppercase;
    padding: 0.15rem 0.45rem;
    background: #fef3c7;
    color: #d97706;
    border-radius: 9999px;
    flex-shrink: 0;
  }

  .meta-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.75rem;
    color: var(--text-muted, #6b7280);
    margin: 0.25rem 0;
  }

  .meta-item.publisher {
    font-weight: 500;
  }

  .meta-item.lang-badge,
  .meta-item.format-badge {
    background: var(--surface-secondary, #f3f4f6);
    padding: 0.1rem 0.35rem;
    border-radius: 0.3rem;
    font-size: 0.65rem;
    font-weight: 600;
  }

  .status-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 0.7rem;
    margin-top: 0.25rem;
  }

  .status-badge {
    text-transform: capitalize;
    font-weight: 600;
    padding: 0.1rem 0.4rem;
    border-radius: 0.3rem;
  }

  .status-badge.confirmed {
    background: #d1fae5;
    color: #065f46;
  }

  .status-badge.tentative {
    background: #fef3c7;
    color: #92400e;
  }

  .source-tag {
    color: var(--text-muted, #9ca3af);
    font-size: 0.65rem;
  }

  /* Calendar View */
  .calendar-container {
    background: var(--surface, #ffffff);
    border: 1px solid var(--border, #e5e7eb);
    border-radius: 0.75rem;
    padding: 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .calendar-nav {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .month-label {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .month-label h2 {
    margin: 0;
    font-size: 1.35rem;
    font-weight: 700;
  }

  .btn-today-pill {
    border: 1px solid var(--border, #e5e7eb);
    background: var(--surface-secondary, #f3f4f6);
    font-size: 0.75rem;
    font-weight: 600;
    padding: 0.2rem 0.55rem;
    border-radius: 0.4rem;
    cursor: pointer;
  }

  .btn-cal-nav {
    display: flex;
    align-items: center;
    gap: 0.3rem;
    background: transparent;
    border: 1px solid var(--border, #e5e7eb);
    padding: 0.35rem 0.75rem;
    font-size: 0.85rem;
    font-weight: 600;
    border-radius: 0.5rem;
    cursor: pointer;
  }

  .btn-cal-nav:hover {
    background: var(--surface-secondary, #f3f4f6);
  }

  .days-header-row {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    text-align: center;
    font-weight: 600;
    font-size: 0.8rem;
    color: var(--text-muted, #6b7280);
    padding-bottom: 0.5rem;
    border-bottom: 1px solid var(--border, #e5e7eb);
  }

  .calendar-cells-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 1px;
    background: var(--border, #e5e7eb);
    border: 1px solid var(--border, #e5e7eb);
    border-radius: 0.5rem;
    overflow: hidden;
  }

  .cal-cell {
    background: var(--surface, #ffffff);
    min-height: 110px;
    padding: 0.4rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .cal-cell.other-month {
    background: var(--surface-secondary, #fafafa);
    opacity: 0.5;
  }

  .cal-cell.is-today {
    background: #eef2ff;
  }

  .cal-cell.has-releases {
    border: 1px solid var(--accent, #c7d2fe);
  }

  .cell-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .day-num {
    font-size: 0.8rem;
    font-weight: 700;
    color: var(--text, #374151);
  }

  .rel-count-badge {
    background: var(--accent, #6366f1);
    color: #ffffff;
    font-size: 0.65rem;
    font-weight: 700;
    border-radius: 9999px;
    padding: 0.05rem 0.35rem;
  }

  .cell-releases-list {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    overflow: hidden;
  }

  .cal-release-chip {
    border: 1px solid var(--border, #e5e7eb);
    background: var(--surface, #ffffff);
    padding: 0.2rem 0.35rem;
    border-radius: 0.3rem;
    font-size: 0.7rem;
    display: flex;
    align-items: center;
    gap: 0.3rem;
    cursor: pointer;
    text-align: left;
    white-space: nowrap;
    overflow: hidden;
  }

  .cal-release-chip:hover {
    border-color: var(--accent, #6366f1);
    background: #f8fafc;
  }

  .cal-release-chip.owned {
    border-left: 3px solid var(--status-success, #10b981);
  }

  .chip-vol {
    font-weight: 700;
    color: var(--accent, #4f46e5);
  }

  .chip-title {
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--text, #111827);
  }

  /* Undated Section */
  .undated-section {
    background: var(--surface, #ffffff);
    border: 1px solid var(--border, #e5e7eb);
    border-radius: 0.75rem;
    overflow: hidden;
  }

  .undated-toggle {
    width: 100%;
    padding: 0.85rem 1.25rem;
    background: transparent;
    border: none;
    display: flex;
    align-items: center;
    justify-content: space-between;
    cursor: pointer;
  }

  .undated-toggle:hover {
    background: var(--surface-secondary, #f9fafb);
  }

  .toggle-left {
    display: flex;
    align-items: center;
    gap: 0.6rem;
  }

  .toggle-heading {
    font-weight: 700;
    font-size: 0.95rem;
    color: var(--text, #111827);
  }

  .count-tag {
    background: var(--surface-secondary, #e5e7eb);
    padding: 0.15rem 0.5rem;
    border-radius: 9999px;
    font-size: 0.75rem;
    font-weight: 700;
  }

  .toggle-right {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    color: var(--text-muted, #6b7280);
    font-size: 0.8rem;
  }

  .undated-content {
    padding: 1rem 1.25rem 1.5rem;
    border-top: 1px solid var(--border, #e5e7eb);
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .undated-intro {
    font-size: 0.85rem;
    color: var(--text-muted, #6b7280);
    margin: 0;
  }

  .undated-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
    gap: 0.85rem;
  }

  .undated-card {
    background: var(--surface-secondary, #f9fafb);
    border: 1px solid var(--border, #e5e7eb);
    border-radius: 0.5rem;
    padding: 0.65rem;
    display: flex;
    gap: 0.75rem;
    cursor: pointer;
  }

  .undated-card:hover {
    border-color: var(--accent, #6366f1);
  }

  .u-cover {
    width: 48px;
    height: 70px;
    flex-shrink: 0;
    border-radius: 0.3rem;
    overflow: hidden;
  }

  .u-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    min-width: 0;
  }

  .u-title {
    font-weight: 700;
    font-size: 0.85rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .u-vol {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--accent, #4f46e5);
  }

  .u-reason {
    font-size: 0.75rem;
    color: var(--text-muted, #6b7280);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .u-meta {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-top: 0.2rem;
  }

  .scope-tag {
    font-size: 0.65rem;
    color: var(--text-muted, #9ca3af);
  }

  .empty-state, .loading-state {
    padding: 3.5rem 1rem;
    text-align: center;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 0.75rem;
    color: var(--text-muted, #6b7280);
  }

  @media (max-width: 768px) {
    .controls-bar {
      flex-direction: column;
      align-items: stretch;
    }
    .right-cluster {
      margin-left: 0;
    }
    .calendar-container {
      overflow-x: auto;
    }
  }
</style>
