<script lang="ts">
  import {
    getDashboardSummary,
    message,
    type DashboardSummaryDto,
  } from '../lib/api';
  import Icon from '../lib/Icon.svelte';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { formatBytes } from '../lib/util';
  import { go } from '../lib/router.svelte';

  let summary = $state<DashboardSummaryDto | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let refreshing = $state(false);

  async function load(isRefresh = false) {
    if (isRefresh) refreshing = true;
    else loading = true;
    error = null;
    try {
      summary = await getDashboardSummary();
      if (isRefresh) {
        toast.success('System telemetry updated');
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
</script>

<div class="home-screen">
  <!-- Editorial Header -->
  <header class="home-hero">
    <div class="hero-brand">
      <div class="brand-tag">
        <span class="dot-live"></span>
        <span>CONTROL PLANE · MANGO v1.0</span>
      </div>
      <h1 class="hero-title display">MANGA OPERATIONS DASHBOARD</h1>
      <p class="hero-subtitle">
        Unified acquisition, canonical library intelligence, and provider telemetry across Komga, MangaBaka, OpenTome, Madokami, and TorBox.
      </p>
    </div>
    <div class="hero-actions">
      <button class="btn refresh-btn icon-text" onclick={() => load(true)} disabled={refreshing || loading}>
        <span class:spin={refreshing}><Icon name="refresh" size={16} /></span>
        <span>{refreshing ? 'Refreshing…' : 'Refresh Telemetry'}</span>
      </button>
    </div>
  </header>

  <!-- Quick Actions Bar -->
  <nav class="quick-actions-bar" aria-label="Quick Actions">
    <div class="quick-actions-label">
      <Icon name="sliders" size={14} />
      <span>QUICK ACTIONS</span>
    </div>
    <div class="actions-group">
      <button class="btn mini flat icon-text" onclick={() => go('/activity?tab=inbox')}>
        <Icon name="bookmark" size={14} />
        <span>Acquisition Inbox</span>
      </button>
      <button class="btn mini flat icon-text" onclick={() => go('/activity?tab=downloads')}>
        <Icon name="download" size={14} />
        <span>Downloads &amp; Queue</span>
      </button>
      <button class="btn mini flat icon-text" onclick={() => go('/activity?tab=monitoring')}>
        <Icon name="bell" size={14} />
        <span>Release Watchers</span>
      </button>
      <button class="btn mini flat icon-text" onclick={() => go('/library')}>
        <Icon name="book" size={14} />
        <span>Canonical Library</span>
      </button>
      <button class="btn mini flat icon-text" onclick={() => go('/settings/pipeline')}>
        <Icon name="search" size={14} />
        <span>Scan &amp; Maintenance</span>
      </button>
    </div>
  </nav>

  {#if loading && !summary}
    <div class="loading-state">
      <span class="spin"><Icon name="spinner" size={28} stroke={3} /></span>
      <span>Gathering operational telemetry across systems…</span>
    </div>
  {:else if error && !summary}
    <Notice error={error} retry={() => load()} />
  {:else if summary}
    <!-- Main Dashboard Grid -->
    <div class="dashboard-grid">
      <!-- 1. Library Overview -->
      <section class="dash-card library-card">
        <div class="card-head">
          <div class="card-head-title">
            <Icon name="book" size={18} />
            <h2>Canonical Library</h2>
          </div>
          <button class="btn icon-link" onclick={() => go('/library')} title="Open Library">
            <Icon name="forward" size={16} />
          </button>
        </div>
        <div class="metrics-row">
          <button type="button" class="metric-block metric-btn" onclick={() => go('/lists')} title="View tracked catalog">
            <span class="m-val">{summary.library.totalTrackedTitles}</span>
            <span class="m-lbl">Tracked Titles</span>
          </button>
          <button type="button" class="metric-block metric-btn highlight-owned" onclick={() => go('/library')} title="Browse owned volumes">
            <span class="m-val">{summary.library.totalOwnedVolumes}</span>
            <span class="m-lbl">Owned Volumes</span>
          </button>
          <button type="button" class="metric-block metric-btn highlight-missing" onclick={() => go('/lists')} title="Review missing volumes">
            <span class="m-val">{summary.library.missingReleasedVolumes}</span>
            <span class="m-lbl">Missing Released</span>
          </button>
          {#if summary.library.titlesNeedingReview > 0}
            <button type="button" class="metric-block metric-btn highlight-review" onclick={() => go('/updates')} title="Inspect titles needing review">
              <span class="m-val">{summary.library.titlesNeedingReview}</span>
              <span class="m-lbl">Needs Review</span>
            </button>
          {/if}
        </div>
        <div class="card-foot">
          <span class="status-indicator">
            <span class="dot-ok"></span> Komga Canonical Authority Active
          </span>
          <button class="btn mini flat" onclick={() => go('/library')}>Browse Library</button>
        </div>
      </section>

      <!-- 2. Acquisition Pipeline -->
      <section class="dash-card acquisition-card">
        <div class="card-head">
          <div class="card-head-title">
            <Icon name="download" size={18} />
            <h2>Acquisition Pipeline</h2>
          </div>
          <button class="btn icon-link" onclick={() => go('/activity?tab=downloads')} title="Open Downloads &amp; Queue">
            <Icon name="forward" size={16} />
          </button>
        </div>
        <div class="metrics-row">
          <button type="button" class="metric-block metric-btn" onclick={() => go('/activity?tab=downloads')} title="View active downloads">
            <span class="m-val">{summary.acquisition.activeDownloads}</span>
            <span class="m-lbl">In-Flight Downloads</span>
          </button>
          {#if summary.acquisition.pendingApprovals > 0}
            <button type="button" class="metric-block metric-btn highlight-review" onclick={() => go('/activity?tab=inbox')} title="Review proposed acquisitions">
              <span class="m-val">{summary.acquisition.pendingApprovals}</span>
              <span class="m-lbl">Pending Review</span>
            </button>
          {/if}
          <button type="button" class="metric-block metric-btn highlight-warn" onclick={() => go('/activity?tab=downloads')} title="Inspect failed downloads">
            <span class="m-val">{summary.acquisition.failedJobs}</span>
            <span class="m-lbl">Failed Jobs</span>
          </button>
          <button type="button" class="metric-block metric-btn highlight-recov" onclick={() => go('/activity?tab=downloads')} title="Inspect recoverable downloads">
            <span class="m-val">{summary.acquisition.recoverableJobs}</span>
            <span class="m-lbl">Recoverable</span>
          </button>
        </div>
        <div class="card-foot">
          {#if summary.acquisition.failedJobs > 0}
            <span class="status-indicator warn">
              <span class="dot-warn"></span> {summary.acquisition.failedJobs} job(s) require attention
            </span>
          {:else}
            <span class="status-indicator ok">
              <span class="dot-ok"></span> All downloads healthy
            </span>
          {/if}
          <button class="btn mini flat" onclick={() => go('/activity?tab=downloads')}>Manage Queue</button>
        </div>
      </section>

      <!-- 3. Release Intelligence -->
      <section class="dash-card release-card">
        <div class="card-head">
          <div class="card-head-title">
            <Icon name="bell" size={18} />
            <h2>Release Watchers</h2>
          </div>
          <button class="btn icon-link" onclick={() => go('/activity?tab=monitoring')} title="Open Watchers &amp; Monitoring">
            <Icon name="forward" size={16} />
          </button>
        </div>
        <div class="metrics-row">
          <button type="button" class="metric-block metric-btn" onclick={() => go('/activity?tab=monitoring')} title="View scheduled releases">
            <span class="m-val">{summary.releaseIntelligence.upcomingReleasesCount}</span>
            <span class="m-lbl">Upcoming Releases</span>
          </button>
          <button type="button" class="metric-block metric-btn highlight-awaiting" onclick={() => go('/activity?tab=monitoring')} title="View releases awaiting upload">
            <span class="m-val">{summary.releaseIntelligence.releasedAwaitingCount}</span>
            <span class="m-lbl">Awaiting Upload</span>
          </button>
          <button type="button" class="metric-block metric-btn highlight-candidate" onclick={() => go('/activity?tab=monitoring')} title="View discovered candidates">
            <span class="m-val">{summary.releaseIntelligence.candidateFoundCount}</span>
            <span class="m-lbl">Candidates Found</span>
          </button>
        </div>
        <div class="card-foot">
          <span class="status-indicator ok">
            <span class="dot-ok"></span> OpenTome Tracking Active
          </span>
          <button class="btn mini flat" onclick={() => go('/activity?tab=monitoring')}>View Watchers</button>
        </div>
      </section>

      <!-- 4. Storage & Quality -->
      <section class="dash-card storage-card">
        <div class="card-head">
          <div class="card-head-title">
            <Icon name="sliders" size={18} />
            <h2>Storage & Quality</h2>
          </div>
          <button class="btn icon-link" onclick={() => go('/settings/pipeline')} title="Open Storage Settings">
            <Icon name="forward" size={16} />
          </button>
        </div>
        <div class="metrics-row">
          <button type="button" class="metric-block metric-btn" onclick={() => go('/settings/pipeline')} title="Inspect staging area">
            <span class="m-val">{formatBytes(summary.storage.stagingBytes)}</span>
            <span class="m-lbl">Staging Area</span>
          </button>
          <button type="button" class="metric-block metric-btn highlight-reclaim" onclick={() => go('/settings/pipeline')} title="Inspect reclaimable space">
            <span class="m-val">{formatBytes(summary.storage.reclaimableBytes)}</span>
            <span class="m-lbl">Reclaimable Space</span>
          </button>
          <button type="button" class="metric-block metric-btn" onclick={() => go('/settings/pipeline')} title="Inspect duplicate issues">
            <span class="m-val">{summary.storage.duplicateGroupsCount}</span>
            <span class="m-lbl">Duplicate Issues</span>
          </button>
        </div>
        <div class="card-foot">
          <span class="status-indicator">
            {#if summary.storage.optimizerAvailable}
              <span class="dot-ok"></span> RustyLibraryShrinker Active
            {:else}
              <span class="dot-dim"></span> Optimizer Unconfigured
            {/if}
          </span>
          <button class="btn mini flat" onclick={() => go('/settings/pipeline')}>Storage Settings</button>
        </div>
      </section>
    </div>

    <!-- 5. Provider Telemetry Strip -->
    <section class="provider-telemetry-section">
      <div class="sec-head">
        <div class="sec-title">
          <Icon name="globe" size={16} />
          <h3>Provider Status & Telemetry</h3>
        </div>
        <button class="btn mini flat" onclick={() => go('/settings')}>Configure Providers</button>
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
</div>

<style>
  .home-screen {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
    padding: 1.5rem 1rem;
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
    border-bottom: 2px solid var(--ink);
    padding-bottom: 1.5rem;
  }

  .brand-tag {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.08em;
    color: var(--spot, #ff7a00);
    margin-bottom: 0.35rem;
  }

  .dot-live {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--status-ok, #16a34a);
    box-shadow: 0 0 6px var(--status-ok, #16a34a);
  }

  .hero-title {
    font-size: 1.85rem;
    margin: 0 0 0.5rem;
    letter-spacing: -0.02em;
    color: var(--ink);
  }

  .hero-subtitle {
    margin: 0;
    font-size: 0.92rem;
    color: var(--muted);
    max-width: 720px;
    line-height: 1.5;
  }

  .hero-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .refresh-btn {
    font-weight: 700;
    font-size: 0.85rem;
  }

  /* Quick Actions Bar */
  .quick-actions-bar {
    display: flex;
    align-items: center;
    gap: 1rem;
    background: var(--shade);
    border: 2px solid var(--ink);
    border-radius: 8px;
    padding: 0.65rem 1rem;
    box-shadow: var(--sh-2);
    overflow-x: auto;
  }

  .quick-actions-label {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.75rem;
    font-weight: 900;
    letter-spacing: 0.08em;
    color: var(--ink);
    white-space: nowrap;
    border-right: 2px solid var(--ink);
    padding-right: 0.75rem;
  }

  .actions-group {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: nowrap;
  }

  /* Dashboard Cards Grid */
  .dashboard-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 1.25rem;
  }

  .dash-card {
    background: var(--paper);
    border: 3px solid var(--ink);
    border-radius: 8px;
    padding: 1.25rem;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    gap: 1rem;
    box-shadow: var(--sh-4);
    transition: transform 0.15s ease, box-shadow 0.15s ease;
  }

  .dash-card:hover {
    transform: translate(-1px, -1px);
    box-shadow: var(--sh-5);
  }

  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-bottom: 2px solid var(--ink);
    padding-bottom: 0.65rem;
  }

  .card-head-title {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: var(--ink);
  }

  .card-head-title h2 {
    font-size: 1.05rem;
    font-weight: 800;
    margin: 0;
    color: var(--ink);
    letter-spacing: -0.01em;
  }

  .icon-link {
    background: transparent;
    border: none;
    color: var(--muted);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    display: flex;
    align-items: center;
  }

  .icon-link:hover {
    color: var(--ink);
    background: var(--shade);
  }

  .metrics-row {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(80px, 1fr));
    gap: 0.75rem;
  }

  .metric-block {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    text-align: left;
    background: transparent;
    border: none;
    padding: 0.25rem 0.4rem;
    border-radius: 6px;
    transition: background 0.12s ease;
  }

  .metric-btn {
    cursor: pointer;
  }

  .metric-btn:hover {
    background: var(--shade);
  }

  .m-val {
    font-size: 1.4rem;
    font-weight: 800;
    line-height: 1.1;
    color: var(--ink);
  }

  .m-lbl {
    font-size: 0.74rem;
    color: var(--muted);
    line-height: 1.3;
  }

  .highlight-owned .m-val {
    color: var(--status-ok, #16a34a);
  }

  .highlight-missing .m-val {
    color: var(--status-error, #dc2626);
  }

  .highlight-review .m-val {
    color: var(--status-warn, #d97706);
  }

  .highlight-warn .m-val {
    color: var(--status-error, #dc2626);
  }

  .highlight-recov .m-val {
    color: var(--status-info, #2563eb);
  }

  .highlight-awaiting .m-val {
    color: var(--status-warn, #d97706);
  }

  .highlight-candidate .m-val {
    color: var(--spot, #ff7a00);
  }

  .highlight-reclaim .m-val {
    color: var(--status-ok, #16a34a);
  }

  .card-foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-top: 1px solid var(--line);
    padding-top: 0.65rem;
    font-size: 0.78rem;
  }

  .status-indicator {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    color: var(--muted);
  }

  .dot-ok {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--status-ok, #16a34a);
  }

  .dot-warn {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--status-error, #dc2626);
  }

  .dot-dim {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--dim, #64748b);
  }

  /* Provider Telemetry Section */
  .provider-telemetry-section {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    background: var(--paper);
    border: 3px solid var(--ink);
    border-radius: 8px;
    box-shadow: var(--sh-4);
    padding: 1.25rem;
  }

  .sec-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-bottom: 2px solid var(--ink);
    padding-bottom: 0.65rem;
  }

  .sec-title {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--ink);
  }

  .sec-title h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 800;
    color: var(--ink);
    letter-spacing: -0.01em;
  }

  .provider-cards-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 0.75rem;
  }

  .provider-pill-card {
    background: var(--shade);
    border: 1px solid var(--line);
    border-radius: 6px;
    padding: 0.6rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .prov-top {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .prov-name {
    font-weight: 700;
    font-size: 0.85rem;
    color: var(--ink);
  }

  .prov-details {
    margin: 0;
    font-size: 0.74rem;
    color: var(--muted);
    line-height: 1.35;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 3rem;
    gap: 0.75rem;
    color: var(--muted);
  }

  @media (max-width: 640px) {
    .home-hero {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
