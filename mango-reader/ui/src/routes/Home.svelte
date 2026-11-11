<script lang="ts">
  import {
    get,
    getDashboardSummary,
    getFollowedReconciliations,
    message,
    type DashboardSummaryDto,
    type Library,
    type LibraryItem,
    type ReconciliationSummary,
    type Resume,
  } from '../lib/api';
  import Cover from '../lib/Cover.svelte';
  import Icon from '../lib/Icon.svelte';
  import Notice from '../lib/Notice.svelte';
  import TitleCard from '../lib/TitleCard.svelte';
  import { toast } from '../lib/toast.svelte';
  import { formatBytes } from '../lib/util';
  import { go } from '../lib/router.svelte';

  let summary = $state<DashboardSummaryDto | null>(null);
  let resume = $state<Resume | null>(null);
  let recentItems = $state<LibraryItem[]>([]);
  let reconciliations = $state<ReconciliationSummary[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let refreshing = $state(false);

  async function load(isRefresh = false) {
    if (isRefresh) refreshing = true;
    else loading = true;
    error = null;

    try {
      const [sumRes, libRes, recRes] = await Promise.allSettled([
        getDashboardSummary(),
        get<Library>('/library'),
        getFollowedReconciliations(),
      ]);

      if (sumRes.status === 'fulfilled') {
        summary = sumRes.value;
      }
      if (libRes.status === 'fulfilled') {
        resume = libRes.value.resume;
        recentItems = [...libRes.value.items]
          .sort((a, b) => b.addedAt - a.addedAt)
          .slice(0, 6);
      }
      if (recRes.status === 'fulfilled') {
        reconciliations = recRes.value;
      }

      if (isRefresh) {
        toast.success('Manga dashboard updated');
      }
    } catch (e: any) {
      error = message(e);
      if (isRefresh) toast.error(error);
    } finally {
      loading = false;
      refreshing = false;
    }
  }

  $effect(() => {
    load();
  });

  const missingSeries = $derived.by(() => {
    return reconciliations
      .filter((r) => r.missingVolumesCount > 0 && r.mangaId !== null)
      .slice(0, 8);
  });
</script>

<div class="home-screen">
  <!-- Editorial Header -->
  <header class="home-hero">
    <div class="hero-brand">
      <div class="brand-tag">
        <span class="dot-live"></span>
        <span>TANKŌBON CONTROL PLANE · MANGO</span>
      </div>
      <h1 class="hero-title display">MANGO</h1>
      <p class="hero-subtitle">
        Personal manga acquisition, discovery, and canonical Komga library control plane.
      </p>
    </div>
    <div class="hero-actions">
      <button
        class="btn refresh-btn icon-text"
        onclick={() => load(true)}
        disabled={refreshing || loading}
        aria-label="Refresh Dashboard"
      >
        <span class:spin={refreshing}><Icon name="refresh" size={16} /></span>
        <span>{refreshing ? 'Refreshing…' : 'Refresh'}</span>
      </button>
    </div>
  </header>

  <!-- Quick Navigation / Action Bar -->
  <nav class="quick-nav-strip" aria-label="Quick Actions">
    <a href="#/library" class="btn pill flat icon-text">
      <Icon name="book" size={16} />
      <span>Owned Library</span>
    </a>
    <a href="#/lists" class="btn pill flat icon-text">
      <Icon name="grid" size={16} />
      <span>Tracked Lists</span>
    </a>
    <a href="#/browse" class="btn pill flat icon-text">
      <Icon name="globe" size={16} />
      <span>Discover Manga</span>
    </a>
    <a href="#/activity?tab=inbox" class="btn pill flat icon-text">
      <Icon name="bookmark" size={16} />
      <span>Acquisition Inbox</span>
    </a>
    <a href="#/activity?tab=downloads" class="btn pill flat icon-text">
      <Icon name="download" size={16} />
      <span>Downloads &amp; Queue</span>
    </a>
    <a href="#/settings" class="btn pill flat icon-text">
      <Icon name="sliders" size={16} />
      <span>Settings</span>
    </a>
  </nav>

  {#if loading && !summary && !resume}
    <div class="loading-state">
      <span class="spin"><Icon name="spinner" size={28} stroke={3} /></span>
      <span>Opening your manga library…</span>
    </div>
  {:else if error && !summary && !resume}
    <Notice {error} retry={() => load()} />
  {:else}
    <!-- Continue Reading Panel -->
    {#if resume}
      <section class="panel resume-card" aria-label="Continue reading">
        <span class="corner-dots" aria-hidden="true"></span>
        <div class="resume-art">
          <Cover src={resume.cover} title={resume.title} tilt />
        </div>
        <div class="resume-words">
          <span class="status-badge status-candidate">CONTINUE READING</span>
          <h2 class="display resume-title">{resume.title}</h2>
          <p class="resume-chapter">{resume.chapterName}</p>
          {#if resume.pageCount}
            <div class="resume-progress">
              <span class="meter">
                <span style:width="{Math.round(((resume.page + 1) / resume.pageCount) * 100)}%"></span>
              </span>
              <strong>Page {resume.page + 1} of {resume.pageCount}</strong>
            </div>
          {/if}
          <div class="resume-actions">
            <a class="btn big spot icon-text" href="#/read/{resume.chapterId}">
              <span>Resume Reading</span>
              <Icon name="arrow" size={18} stroke={3} />
            </a>
            <a class="btn big icon-text" href="#/title/{resume.mangaId}">
              <Icon name="book" size={16} />
              <span>Series Workspace</span>
            </a>
          </div>
        </div>
      </section>
    {/if}

    <!-- Operational Telemetry Overview Strip -->
    {#if summary}
      <section class="telemetry-strip" aria-label="Operations Overview">
        <button
          type="button"
          class="metric-chip"
          onclick={() => go('/library')}
          title="Browse owned library"
        >
          <div class="metric-icon-wrap"><Icon name="book" size={18} /></div>
          <div class="metric-meta">
            <span class="metric-num">{summary.library.totalOwnedVolumes}</span>
            <span class="metric-label">Owned Volumes</span>
          </div>
        </button>

        <button
          type="button"
          class="metric-chip"
          class:alert={summary.acquisition.activeDownloads > 0}
          onclick={() => go('/activity?tab=downloads')}
          title="View in-flight downloads"
        >
          <div class="metric-icon-wrap"><Icon name="download" size={18} /></div>
          <div class="metric-meta">
            <span class="metric-num">{summary.acquisition.activeDownloads}</span>
            <span class="metric-label">In-Flight Downloads</span>
          </div>
        </button>

        <button
          type="button"
          class="metric-chip"
          class:highlight={summary.acquisition.pendingApprovals > 0}
          onclick={() => go('/activity?tab=inbox')}
          title="Review proposed acquisitions"
        >
          <div class="metric-icon-wrap"><Icon name="bookmark" size={18} /></div>
          <div class="metric-meta">
            <span class="metric-num">{summary.acquisition.pendingApprovals}</span>
            <span class="metric-label">Review Queue</span>
          </div>
        </button>

        <button
          type="button"
          class="metric-chip"
          class:warn={summary.library.missingReleasedVolumes > 0}
          onclick={() => go('/lists')}
          title="Review missing volumes"
        >
          <div class="metric-icon-wrap"><Icon name="bell" size={18} /></div>
          <div class="metric-meta">
            <span class="metric-num">{summary.library.missingReleasedVolumes}</span>
            <span class="metric-label">Missing Released</span>
          </div>
        </button>
      </section>
    {/if}

    <!-- Recommended Next Missing Volumes Section -->
    {#if missingSeries.length > 0}
      <section class="shelf missing-shelf" aria-label="Next missing volumes">
        <div class="section-head">
          <div class="sec-title-wrap">
            <h2 class="display">NEXT MISSING VOLUMES</h2>
            <span class="sec-subtitle">Recommended acquisitions for tracked titles</span>
          </div>
          <a href="#/lists" class="btn pill flat">View All Lists</a>
        </div>

        <div class="missing-cards-grid">
          {#each missingSeries as item (item.seriesId)}
            <div class="missing-series-card panel">
              <div class="msc-head">
                <span class="status-badge status-warn">
                  {#if item.missingVolumes.length === 1}
                    Missing Vol {item.missingVolumes[0]}
                  {:else}
                    {item.missingVolumesCount} Volumes Missing
                  {/if}
                </span>
                <span class="msc-edition">{item.edition || 'Standard'}</span>
              </div>
              <h3 class="msc-title">{item.title}</h3>
              <div class="msc-stats">
                <span class="msc-stat">
                  Owned: <strong>{item.ownedVolumesCount}</strong>
                  {#if item.totalOfficialVolumes} / {item.totalOfficialVolumes}{/if}
                </span>
                {#if item.missingVolumes.length > 0}
                  <span class="msc-next">
                    Next: <strong>Vol {item.missingVolumes[0]}</strong>
                  </span>
                {/if}
              </div>
              <div class="msc-actions">
                <a class="btn spot mini icon-text" href="#/title/{item.mangaId}">
                  <span>Acquire</span>
                  <Icon name="arrow" size={14} stroke={3} />
                </a>
              </div>
            </div>
          {/each}
        </div>
      </section>
    {/if}

    <!-- Recently Added Manga Section -->
    {#if recentItems.length > 0}
      <section class="shelf recent-shelf" aria-label="Recently added manga">
        <div class="section-head">
          <div class="sec-title-wrap">
            <h2 class="display">RECENT IN LIBRARY</h2>
            <span class="sec-subtitle">Titles in your local collection</span>
          </div>
          <a href="#/library" class="btn pill flat">Browse Library</a>
        </div>

        <div class="grid">
          {#each recentItems as item (item.id)}
            <TitleCard
              id={item.id}
              title={item.title}
              cover={item.cover}
              unread={item.unread}
            />
          {/each}
        </div>
      </section>
    {/if}

    <!-- Provider Telemetry & Storage Drawer -->
    {#if summary}
      <section class="provider-telemetry-section panel" aria-label="System status">
        <div class="sec-head">
          <div class="sec-title">
            <Icon name="globe" size={16} />
            <h3 class="display">SERVICES &amp; PROVIDERS</h3>
          </div>
          <span class="storage-info">
            Staging: {formatBytes(summary.storage.stagingBytes)} ·
            Reclaimable: {formatBytes(summary.storage.reclaimableBytes)}
          </span>
        </div>

        <div class="provider-cards-grid">
          {#each summary.providerHealth as prov (prov.name)}
            <div class="provider-pill-card">
              <div class="prov-top">
                <span class="prov-name">{prov.name}</span>
                <span class="status-badge status-{prov.status === 'HEALTHY' || prov.status === 'AVAILABLE' ? 'ok' : prov.status === 'ATTENTION_REQUIRED' ? 'error' : prov.status === 'CONFIGURED' ? 'info' : 'neutral'}">
                  {prov.status}
                </span>
              </div>
              <p class="prov-details muted font-small">{prov.details}</p>
            </div>
          {/each}
        </div>
      </section>
    {/if}
  {/if}
</div>

<style>
  .home-screen {
    display: flex;
    flex-direction: column;
    gap: 28px;
    padding: 28px var(--gutter) 72px;
    max-width: var(--page-max-width, 1560px);
    margin: 0 auto;
    width: 100%;
  }

  /* Editorial Hero Header */
  .home-hero {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1.5rem;
    padding-bottom: 1.5rem;
    border-bottom: 3px solid var(--ink);
  }

  .brand-tag {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--muted);
    margin-bottom: 6px;
  }

  .dot-live {
    width: 8px;
    height: 8px;
    background: var(--spot);
    border: 1.5px solid var(--ink);
    border-radius: 50%;
  }

  .hero-title {
    font-size: clamp(38px, 8vw, 64px);
    margin: 0;
    line-height: 1.05;
    letter-spacing: -0.02em;
    color: var(--ink);
  }

  .hero-subtitle {
    margin: 8px 0 0;
    font-size: 15px;
    font-weight: 600;
    color: var(--muted);
    max-width: 640px;
    line-height: 1.45;
  }

  .refresh-btn {
    flex-shrink: 0;
  }

  /* Quick Navigation Strip */
  .quick-nav-strip {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: 4px;
    box-shadow: var(--sh-3);
    padding: 8px 12px;
  }

  .quick-nav-strip .btn.pill {
    font-size: 13px;
    font-weight: 700;
    padding: 6px 12px;
  }

  /* Continue Reading Card */
  .resume-card {
    position: relative;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 28px;
    overflow: hidden;
    padding: 28px;
  }

  .corner-dots {
    position: absolute;
    top: 0;
    right: 0;
    width: 180px;
    height: 180px;
    background-image: var(--dots);
    background-size: 9px 9px;
    clip-path: polygon(100% 0, 0 0, 100% 100%);
    pointer-events: none;
  }

  .resume-art {
    position: relative;
    flex: 0 0 clamp(110px, 20vw, 160px);
  }

  .resume-words {
    position: relative;
    display: flex;
    flex: 1 1 280px;
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
    min-width: 0;
  }

  .resume-title {
    font-size: clamp(24px, 4vw, 36px);
    margin: 0;
    line-height: 1.15;
    overflow-wrap: anywhere;
  }

  .resume-chapter {
    margin: 0;
    font-size: 16px;
    font-weight: 700;
    color: var(--muted);
  }

  .resume-progress {
    display: flex;
    align-items: center;
    gap: 12px;
    width: min(420px, 100%);
  }

  .resume-progress .meter {
    flex: 1;
    height: 16px;
    border-width: 3px;
  }

  .resume-progress .meter span {
    border-right: 3px solid var(--ink);
    background: var(--spot);
  }

  .resume-progress strong {
    font-size: 13px;
    font-weight: 900;
  }

  .resume-actions {
    display: flex;
    gap: 12px;
    flex-wrap: wrap;
    margin-top: 4px;
  }

  /* Operations Telemetry Strip */
  .telemetry-strip {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 14px;
  }

  .metric-chip {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 14px 18px;
    background: var(--surface);
    border: 2px solid var(--ink);
    border-radius: 4px;
    box-shadow: var(--sh-3);
    cursor: pointer;
    text-align: left;
    transition: transform 0.15s ease, box-shadow 0.15s ease;
  }

  .metric-chip:hover {
    transform: translate(-1px, -1px);
    box-shadow: var(--sh-5);
  }

  .metric-chip.alert {
    background: color-mix(in srgb, var(--status-info-bg) 60%, var(--surface));
  }

  .metric-chip.highlight {
    background: color-mix(in srgb, var(--spot) 20%, var(--surface));
  }

  .metric-chip.warn {
    background: color-mix(in srgb, var(--status-warn-bg) 40%, var(--surface));
  }

  .metric-icon-wrap {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 38px;
    height: 38px;
    border: 2px solid var(--ink);
    border-radius: 4px;
    background: var(--paper);
    flex-shrink: 0;
  }

  .metric-meta {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .metric-num {
    font-size: 22px;
    font-weight: 900;
    line-height: 1.1;
    color: var(--ink);
  }

  .metric-label {
    font-size: 12px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
  }

  /* Next Missing Volumes Shelf */
  .missing-shelf {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .sec-title-wrap {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .sec-subtitle {
    font-size: 13px;
    font-weight: 600;
    color: var(--muted);
  }

  .missing-cards-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 16px;
  }

  .missing-series-card {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 10px;
    background: var(--surface);
    border-radius: 4px;
  }

  .msc-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .msc-edition {
    font-size: 11px;
    font-weight: 700;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .msc-title {
    margin: 0;
    font-size: 16px;
    font-weight: 700;
    line-height: 1.3;
    color: var(--ink);
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }

  .msc-stats {
    display: flex;
    justify-content: space-between;
    align-items: center;
    font-size: 12px;
    color: var(--muted);
    border-top: 1px solid var(--line);
    padding-top: 8px;
  }

  .msc-stats strong {
    color: var(--ink);
  }

  .msc-actions {
    margin-top: auto;
    display: flex;
    justify-content: flex-end;
  }

  /* Provider Telemetry Section */
  .provider-telemetry-section {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    border-radius: 4px;
  }

  .sec-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    border-bottom: 2px solid var(--ink);
    padding-bottom: 10px;
  }

  .sec-title {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .sec-title h3 {
    margin: 0;
    font-size: 16px;
    letter-spacing: -0.02em;
  }

  .storage-info {
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
  }

  .provider-cards-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 12px;
  }

  .provider-pill-card {
    background: var(--paper);
    border: 1.5px solid var(--ink);
    border-radius: 4px;
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .prov-top {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 8px;
  }

  .prov-name {
    font-weight: 700;
    font-size: 13px;
    color: var(--ink);
  }

  .prov-details {
    margin: 0;
    font-size: 11px;
    color: var(--muted);
    line-height: 1.35;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 4rem;
    gap: 12px;
    color: var(--muted);
    font-weight: 600;
  }

  .spin {
    display: inline-flex;
    animation: spin 1s linear infinite;
  }

  @keyframes spin {
    from {
      transform: rotate(0deg);
    }
    to {
      transform: rotate(360deg);
    }
  }
</style>
