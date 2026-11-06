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
      <h1 class="hero-title">Manga Operations Dashboard</h1>
      <p class="hero-subtitle">
        Unified acquisition, canonical library intelligence, and provider telemetry across Komga, MangaBaka, OpenTome, Madokami, and TorBox.
      </p>
    </div>
    <div class="hero-actions">
      <button class="btn refresh-btn" onclick={() => load(true)} disabled={refreshing || loading}>
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
      <button class="action-chip" onclick={() => go('/downloads')}>
        <Icon name="download" size={14} />
        <span>Review Queue & Recovery</span>
      </button>
      <button class="action-chip" onclick={() => go('/updates')}>
        <Icon name="bell" size={14} />
        <span>Check Release Watchers</span>
      </button>
      <button class="action-chip" onclick={() => go('/settings/pipeline')}>
        <Icon name="search" size={14} />
        <span>Scan Duplicates & Storage</span>
      </button>
      <button class="action-chip" onclick={() => go('/lists')}>
        <Icon name="bookmark" size={14} />
        <span>View Tracked Catalog</span>
      </button>
      <button class="action-chip" onclick={() => go('/library')}>
        <Icon name="book" size={14} />
        <span>Browse Canonical Library</span>
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
          <div class="metric-block">
            <span class="m-val">{summary.library.totalTrackedTitles}</span>
            <span class="m-lbl">Tracked Titles</span>
          </div>
          <div class="metric-block highlight-owned">
            <span class="m-val">{summary.library.totalOwnedVolumes}</span>
            <span class="m-lbl">Owned Volumes</span>
          </div>
          <div class="metric-block highlight-missing">
            <span class="m-val">{summary.library.missingReleasedVolumes}</span>
            <span class="m-lbl">Missing Released</span>
          </div>
          {#if summary.library.titlesNeedingReview > 0}
            <div class="metric-block highlight-review">
              <span class="m-val">{summary.library.titlesNeedingReview}</span>
              <span class="m-lbl">Needs Review</span>
            </div>
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
          <button class="btn icon-link" onclick={() => go('/downloads')} title="Open Downloads">
            <Icon name="forward" size={16} />
          </button>
        </div>
        <div class="metrics-row">
          <div class="metric-block">
            <span class="m-val">{summary.acquisition.activeDownloads}</span>
            <span class="m-lbl">In-Flight Downloads</span>
          </div>
          <div class="metric-block highlight-warn">
            <span class="m-val">{summary.acquisition.failedJobs}</span>
            <span class="m-lbl">Failed Jobs</span>
          </div>
          <div class="metric-block highlight-recov">
            <span class="m-val">{summary.acquisition.recoverableJobs}</span>
            <span class="m-lbl">Recoverable</span>
          </div>
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
          <button class="btn mini flat" onclick={() => go('/downloads')}>Manage Queue</button>
        </div>
      </section>

      <!-- 3. Release Intelligence -->
      <section class="dash-card release-card">
        <div class="card-head">
          <div class="card-head-title">
            <Icon name="bell" size={18} />
            <h2>Release Watchers</h2>
          </div>
          <button class="btn icon-link" onclick={() => go('/updates')} title="Open Updates">
            <Icon name="forward" size={16} />
          </button>
        </div>
        <div class="metrics-row">
          <div class="metric-block">
            <span class="m-val">{summary.releaseIntelligence.upcomingReleasesCount}</span>
            <span class="m-lbl">Upcoming Releases</span>
          </div>
          <div class="metric-block highlight-awaiting">
            <span class="m-val">{summary.releaseIntelligence.releasedAwaitingCount}</span>
            <span class="m-lbl">Awaiting Upload</span>
          </div>
          <div class="metric-block highlight-candidate">
            <span class="m-val">{summary.releaseIntelligence.candidateFoundCount}</span>
            <span class="m-lbl">Candidates Found</span>
          </div>
        </div>
        <div class="card-foot">
          <span class="status-indicator ok">
            <span class="dot-ok"></span> OpenTome Tracking Active
          </span>
          <button class="btn mini flat" onclick={() => go('/updates')}>View Watchers</button>
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
          <div class="metric-block">
            <span class="m-val">{formatBytes(summary.storage.stagingBytes)}</span>
            <span class="m-lbl">Staging Area</span>
          </div>
          <div class="metric-block highlight-reclaim">
            <span class="m-val">{formatBytes(summary.storage.reclaimableBytes)}</span>
            <span class="m-lbl">Reclaimable Space</span>
          </div>
          <div class="metric-block">
            <span class="m-val">{summary.storage.duplicateGroupsCount}</span>
            <span class="m-lbl">Duplicate Issues</span>
          </div>
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
              <span class="badge status-pill status-{prov.status.toLowerCase()}">{prov.status}</span>
            </div>
            <p class="prov-details">{prov.details}</p>
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
    max-width: 1280px;
    margin: 0 auto;
    width: 100%;
  }

  /* Editorial Hero Header */
  .home-hero {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1.5rem;
    border-bottom: 2px solid var(--border-color, rgba(255, 255, 255, 0.08));
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
    background: #22c55e;
    box-shadow: 0 0 8px #22c55e;
  }

  .hero-title {
    font-family: 'Dela Gothic One', sans-serif;
    font-size: 1.85rem;
    margin: 0 0 0.5rem;
    letter-spacing: -0.02em;
    color: var(--text-heading, #fff);
  }

  .hero-subtitle {
    margin: 0;
    font-size: 0.92rem;
    color: var(--muted, #8b949e);
    max-width: 720px;
    line-height: 1.5;
  }

  .hero-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .refresh-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-weight: 600;
    font-size: 0.85rem;
    padding: 0.5rem 0.85rem;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.15s ease;
  }

  .refresh-btn:hover {
    background: rgba(255, 255, 255, 0.1);
  }

  /* Quick Actions Bar */
  .quick-actions-bar {
    display: flex;
    align-items: center;
    gap: 1rem;
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 8px;
    padding: 0.65rem 1rem;
    overflow-x: auto;
  }

  .quick-actions-label {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.75rem;
    font-weight: 700;
    letter-spacing: 0.06em;
    color: var(--muted, #8b949e);
    white-space: nowrap;
    border-right: 1px solid rgba(255, 255, 255, 0.1);
    padding-right: 0.75rem;
  }

  .actions-group {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: nowrap;
  }

  .action-chip {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    background: rgba(255, 255, 255, 0.05);
    border: 1px solid rgba(255, 255, 255, 0.08);
    color: #e2e8f0;
    font-size: 0.8rem;
    font-weight: 500;
    padding: 0.35rem 0.65rem;
    border-radius: 6px;
    cursor: pointer;
    white-space: nowrap;
    transition: all 0.15s ease;
  }

  .action-chip:hover {
    background: var(--spot, #ff7a00);
    color: #111;
    border-color: var(--spot, #ff7a00);
  }

  /* Dashboard Cards Grid */
  .dashboard-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 1.25rem;
  }

  .dash-card {
    background: var(--bg-surface, #1e1e24);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 10px;
    padding: 1.25rem;
    display: flex;
    flex-direction: column;
    justify-content: space-between;
    gap: 1rem;
    box-shadow: 0 4px 12px rgba(0, 0, 0, 0.2);
    transition: transform 0.15s ease, border-color 0.15s ease;
  }

  .dash-card:hover {
    border-color: rgba(255, 255, 255, 0.2);
  }

  .card-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-bottom: 1px solid rgba(255, 255, 255, 0.06);
    padding-bottom: 0.65rem;
  }

  .card-head-title {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    color: var(--spot, #ff7a00);
  }

  .card-head-title h2 {
    font-size: 1.05rem;
    font-weight: 700;
    margin: 0;
    color: var(--text-heading, #fff);
  }

  .icon-link {
    background: transparent;
    border: none;
    color: var(--muted, #8b949e);
    cursor: pointer;
    padding: 4px;
    border-radius: 4px;
    display: flex;
    align-items: center;
  }

  .icon-link:hover {
    color: #fff;
    background: rgba(255, 255, 255, 0.08);
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
  }

  .m-val {
    font-size: 1.4rem;
    font-weight: 800;
    line-height: 1.1;
    color: #f8fafc;
  }

  .m-lbl {
    font-size: 0.74rem;
    color: var(--muted, #8b949e);
    line-height: 1.3;
  }

  .highlight-owned .m-val {
    color: #86efac;
  }

  .highlight-missing .m-val {
    color: #fca5a5;
  }

  .highlight-review .m-val {
    color: #fde68a;
  }

  .highlight-warn .m-val {
    color: #fca5a5;
  }

  .highlight-recov .m-val {
    color: #93c5fd;
  }

  .highlight-awaiting .m-val {
    color: #fed7aa;
  }

  .highlight-candidate .m-val {
    color: #86efac;
  }

  .highlight-reclaim .m-val {
    color: #c4b5fd;
  }

  .card-foot {
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-top: 1px solid rgba(255, 255, 255, 0.06);
    padding-top: 0.65rem;
    font-size: 0.78rem;
  }

  .status-indicator {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    color: var(--muted, #8b949e);
  }

  .dot-ok {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #22c55e;
  }

  .dot-warn {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #ef4444;
  }

  .dot-dim {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: #64748b;
  }

  /* Provider Telemetry Section */
  .provider-telemetry-section {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    background: rgba(0, 0, 0, 0.2);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 10px;
    padding: 1rem 1.25rem;
  }

  .sec-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .sec-title {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: var(--spot, #ff7a00);
  }

  .sec-title h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 700;
    color: #fff;
  }

  .provider-cards-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: 0.75rem;
  }

  .provider-pill-card {
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
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
    color: #f1f5f9;
  }

  .status-pill {
    font-size: 0.68rem;
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
    font-weight: 700;
  }

  .status-healthy,
  .status-available {
    background: rgba(34, 197, 94, 0.15);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .status-configured {
    background: rgba(59, 130, 246, 0.15);
    color: #93c5fd;
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .status-disabled {
    background: rgba(100, 116, 139, 0.15);
    color: #cbd5e1;
  }

  .prov-details {
    margin: 0;
    font-size: 0.74rem;
    color: var(--muted, #8b949e);
    line-height: 1.35;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 3rem;
    gap: 0.75rem;
    color: var(--muted, #8b949e);
  }

  @media (max-width: 640px) {
    .home-hero {
      flex-direction: column;
      align-items: flex-start;
    }
  }
</style>
