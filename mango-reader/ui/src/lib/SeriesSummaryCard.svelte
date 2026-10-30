<script lang="ts">
  import Icon from './Icon.svelte';
  import type { SeriesSummary } from './volumeStatus';
  import type {
    ReconciliationSummary,
    OpenTomeSeriesSchedule,
    ReleaseDiscoveryTarget,
    CanonicalLibraryReleasesSummary,
    TitleOwnership,
  } from './api';
  import { ago } from './util';

  function formatVolumeRanges(numbers: number[]): string {
    if (!numbers.length) return 'None';
    const sorted = [...numbers].sort((a, b) => a - b);
    const ranges: string[] = [];
    let start = sorted[0];
    let end = start;

    for (let i = 1; i < sorted.length; i++) {
      if (sorted[i] === end + 1) {
        end = sorted[i];
      } else {
        ranges.push(start === end ? `v${start}` : `v${start}–${end}`);
        start = sorted[i];
        end = start;
      }
    }
    ranges.push(start === end ? `v${start}` : `v${start}–${end}`);
    return ranges.join(', ');
  }

  let {
    summary,
    libraryState = null,
    reconciliation = null,
    ownership = null,
    ownedVolumeNumbers = [],
    missingVolumeNumbers = [],
    openTomeSchedule = null,
    releaseDiscoveryTargets = [],
    onResolveIdentity,
    onReviewRelease,
    onRetryMetadata,
    onDiscoverMissing,
    discoveringMissing = false,
    onPlanMissing,
    onRefreshKomga,
    refreshingKomga = false,
    onReconcile,
    reconciling = false,
    onOpenTomeRemap,
    openTomeLoading = false,
    onSearchNyaa,
    onCheckReleaseDiscovery,
  }: {
    summary: SeriesSummary;
    libraryState?: CanonicalLibraryReleasesSummary | null;
    reconciliation?: ReconciliationSummary | null;
    ownership?: TitleOwnership | null;
    ownedVolumeNumbers?: number[];
    missingVolumeNumbers?: number[];
    openTomeSchedule?: OpenTomeSeriesSchedule | null;
    releaseDiscoveryTargets?: ReleaseDiscoveryTarget[];
    onResolveIdentity?: () => void;
    onReviewRelease?: () => void;
    onRetryMetadata?: () => void;
    onDiscoverMissing?: () => void;
    discoveringMissing?: boolean;
    onPlanMissing?: () => void;
    onRefreshKomga?: () => void;
    refreshingKomga?: boolean;
    onReconcile?: () => void;
    reconciling?: boolean;
    onOpenTomeRemap?: (action: 'confirm' | 'reject' | 'auto') => void;
    openTomeLoading?: boolean;
    onSearchNyaa?: () => void;
    onCheckReleaseDiscovery?: () => void;
  } = $props();

  let showAdvanced = $state(false);

  // Determine top release discovery target status
  const activeReleaseTarget = $derived.by<ReleaseDiscoveryTarget | null>(() => {
    if (!releaseDiscoveryTargets.length) return null;
    return (
      releaseDiscoveryTargets.find((t) => t.status === 'candidate_available') ??
      releaseDiscoveryTargets.find((t) => t.status === 'releases_today' || t.status === 'discovering') ??
      releaseDiscoveryTargets.find((t) => t.status === 'not_found_retry_wait') ??
      releaseDiscoveryTargets.find((t) => t.status === 'scheduled') ??
      null
    );
  });

  const isComplete = $derived(
    !summary.isIdentityAmbiguous &&
      (libraryState?.libraryStatus === 'library_complete' ||
        summary.libraryStatus === 'library_complete' ||
        (!summary.isOngoing &&
          summary.totalExpectedVolumes != null &&
          summary.totalExpectedVolumes > 0 &&
          summary.ownedCount >= summary.totalExpectedVolumes))
  );

  const isUpToDate = $derived(
    !summary.isIdentityAmbiguous &&
      !isComplete &&
      (libraryState?.libraryStatus === 'up_to_date' ||
        summary.libraryStatus === 'up_to_date' ||
        (summary.isOngoing && summary.missingCount === 0 && summary.ownedCount > 0))
  );
</script>

<div class="library-releases-card">
  <!-- Card Header -->
  <div class="card-top">
    <div class="section-title-col">
      <div class="section-headline">
        <h2 class="display section-heading">Library & Releases</h2>
        {#if isComplete}
          <span class="tag complete-tag">✓ Library Complete</span>
        {:else if isUpToDate}
          <span class="tag uptodate-tag">✓ Up to date</span>
        {:else if summary.isOngoing}
          <span class="tag ongoing-tag">Ongoing</span>
        {/if}
      </div>
      {#if !summary.isIdentityAmbiguous && summary.totalExpectedVolumes != null && summary.progressPercent != null}
        <span class="progress-pct-label">{summary.progressPercent}% of released volumes acquired</span>
      {/if}
    </div>

    <!-- Primary Action Button (Single Primary CTA) -->
    <div class="primary-action-slot">
      {#if summary.isIdentityAmbiguous}
        <button
          class="btn pill spot icon-text primary-cta"
          disabled={reconciling}
          onclick={onResolveIdentity ?? onReconcile}
          title="Resolve ambiguous MangaBaka series identity"
        >
          {#if reconciling}
            <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
          {:else}
            <Icon name="search" size={14} />
          {/if}
          Resolve Identity
        </button>
      {:else if summary.primaryActionType === 'retry_metadata' && onRetryMetadata}
        <button class="btn pill warn icon-text primary-cta" onclick={onRetryMetadata}>
          <Icon name="refresh" size={14} /> Retry Metadata
        </button>
      {:else if (summary.primaryActionType === 'review_release' || activeReleaseTarget?.status === 'candidate_available') && onReviewRelease}
        <button class="btn pill spot icon-text primary-cta pulse-cta" onclick={onReviewRelease}>
          <Icon name="check" size={14} /> Review Release
        </button>
      {:else if summary.missingCount > 0 && onDiscoverMissing}
        <button
          class="btn pill spot icon-text primary-cta"
          disabled={discoveringMissing}
          onclick={onDiscoverMissing}
          title="Discover release candidates for missing volumes"
        >
          {#if discoveringMissing}
            <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
          {:else}
            <Icon name="search" size={14} />
          {/if}
          Discover Releases
        </button>
      {/if}
    </div>
  </div>

  <!-- Primary State Banner -->
  {#if summary.isIdentityAmbiguous}
    <!-- Ambiguous Identity Banner -->
    <div class="state-banner alert-warning">
      <div class="banner-icon"><Icon name="info" size={20} /></div>
      <div class="banner-body">
        <strong>Series identity needs review</strong>
        <p class="banner-text">
          Canonical MangaBaka reconciliation is unresolved. Ownership metrics are paused until identity is confirmed.
        </p>
      </div>
    </div>
  {:else}
    <!-- Status Overview Row -->
    <div class="status-summary-row">
      <div class="status-headline">
        {#if isComplete}
          <span class="status-text complete">
            {summary.ownedCount} / {summary.totalExpectedVolumes} released volumes owned · Complete
          </span>
        {:else if isUpToDate}
          <span class="status-text uptodate">
            {summary.ownedCount} / {summary.totalExpectedVolumes ?? summary.ownedCount} released volumes owned · Up to date
          </span>
        {:else}
          <span class="status-text">
            {summary.statusExplanation}
            {#if summary.missingCount > 0}
              <span class="missing-accent"> · {summary.missingCount} missing</span>
            {/if}
          </span>
        {/if}
      </div>

      {#if activeReleaseTarget}
        <div class="release-trigger-notice" class:notice-found={activeReleaseTarget.status === 'candidate_available'}>
          {#if activeReleaseTarget.status === 'candidate_available'}
            <Icon name="check" size={14} />
            <span>Vol. {activeReleaseTarget.volumeNumber} — release found on Nyaa</span>
          {:else if activeReleaseTarget.status === 'releases_today' || activeReleaseTarget.status === 'discovering'}
            <Icon name="spinner" size={14} />
            <span>Vol. {activeReleaseTarget.volumeNumber} — released today · Searching for release…</span>
          {:else if activeReleaseTarget.status === 'not_found_retry_wait'}
            <Icon name="clock" size={14} />
            <span>Vol. {activeReleaseTarget.volumeNumber} — no release found yet · retry scheduled</span>
          {:else if activeReleaseTarget.status === 'scheduled'}
            <Icon name="clock" size={14} />
            <span>Next release: Vol. {activeReleaseTarget.volumeNumber} on {activeReleaseTarget.releaseDate}</span>
          {/if}
        </div>
      {/if}
    </div>

    <!-- Progress Meter -->
    {#if summary.progressPercent !== null}
      <div class="progress-bar-container">
        <span class="meter summary-meter" aria-hidden="true">
          <span style:width="{summary.progressPercent}%"></span>
        </span>
      </div>
    {/if}

    <!-- Metric Counters (Only when identity verified) -->
    <div class="metric-counters">
      <div class="counter-item owned">
        <span class="counter-num">{summary.ownedCount}</span>
        <span class="counter-lbl">Owned</span>
      </div>

      <div class="counter-item missing">
        <span class="counter-num">{summary.missingCount}</span>
        <span class="counter-lbl">Missing</span>
      </div>

      {#if summary.activeCount > 0}
        <div class="counter-item has-active">
          <span class="counter-num">{summary.activeCount}</span>
          <span class="counter-lbl">Active</span>
        </div>
      {/if}

      {#if summary.attentionCount > 0}
        <div class="counter-item has-attention">
          <span class="counter-num">{summary.attentionCount}</span>
          <span class="counter-lbl">Needs Attention</span>
        </div>
      {/if}
    </div>
  {/if}

  <!-- Integrated OpenTome Schedule -->
  {#if openTomeSchedule}
    <div class="opentome-integrated-section">
      <div class="opentome-section-header">
        <span class="ot-badge">OpenTome</span>
        <span class="ot-section-title">Physical Release Schedule</span>
        {#if openTomeSchedule.mappingConfidence}
          <span
            class="confidence-tag"
            class:conf-exact={openTomeSchedule.mappingConfidence === 'exact' || openTomeSchedule.mappingConfidence === 'user_confirmed'}
            class:conf-high={openTomeSchedule.mappingConfidence === 'high'}
            class:conf-ambiguous={openTomeSchedule.mappingConfidence === 'ambiguous'}
          >
            {openTomeSchedule.mappingConfidence.toUpperCase()}
          </span>
        {/if}
      </div>

      <div class="opentome-market-lines">
        <!-- English Line -->
        <div class="market-line">
          <span class="lang-tag en">EN</span>
          {#if openTomeSchedule.english}
            {@const en = openTomeSchedule.english}
            <span class="line-status">
              {#if en.nextVolume}
                Vol. {en.nextVolume.volumeNumber} · {en.nextVolume.releaseDate ?? en.nextVolume.releaseDateRaw ?? 'Upcoming'}
                {#if en.nextVolume.status === 'releases_today'}
                  <span class="today-tag">RELEASES TODAY</span>
                {/if}
              {:else if isComplete}
                {en.totalVolumes} released · Complete
              {:else}
                {en.totalVolumes} released
              {/if}
            </span>
          {:else}
            <span class="line-status muted">No English edition schedule</span>
          {/if}
        </div>

        <!-- Japanese Line -->
        <div class="market-line">
          <span class="lang-tag ja">JA</span>
          {#if openTomeSchedule.japanese}
            {@const ja = openTomeSchedule.japanese}
            <span class="line-status">
              {#if ja.nextVolume}
                Vol. {ja.nextVolume.volumeNumber} · {ja.nextVolume.releaseDate ?? ja.nextVolume.releaseDateRaw ?? 'Upcoming'}
                {#if ja.nextVolume.status === 'releases_today'}
                  <span class="today-tag">RELEASES TODAY</span>
                {/if}
              {:else if isComplete}
                {ja.totalVolumes} released · Complete
              {:else}
                {ja.totalVolumes} released
              {/if}
            </span>
          {:else}
            <span class="line-status muted">No Japanese edition schedule</span>
          {/if}
        </div>
      </div>
    </div>
  {/if}

  <!-- Collapsible Advanced Actions -->
  <div class="advanced-disclosure">
    <button
      class="btn advanced-toggle"
      onclick={() => (showAdvanced = !showAdvanced)}
      aria-expanded={showAdvanced}
    >
      <Icon name={showAdvanced ? 'up' : 'down'} size={14} />
      <span>Advanced actions</span>
    </button>

    {#if showAdvanced}
      <div class="advanced-panel">
        <div class="advanced-buttons-row">
          {#if onReconcile}
            <button
              class="btn pill sm icon-text"
              disabled={reconciling}
              onclick={onReconcile}
              title="Reconcile official volumes directly with MangaBaka"
            >
              {#if reconciling}
                <span class="spin"><Icon name="spinner" size={12} stroke={3} /></span>
              {:else}
                <Icon name="refresh" size={12} />
              {/if}
              MangaBaka Reconcile
            </button>
          {/if}

          {#if onRefreshKomga}
            <button
              class="btn pill sm icon-text"
              disabled={refreshingKomga}
              onclick={onRefreshKomga}
              title="Refresh ownership state directly from Komga"
            >
              {#if refreshingKomga}
                <span class="spin"><Icon name="spinner" size={12} stroke={3} /></span>
              {:else}
                <Icon name="refresh" size={12} />
              {/if}
              Refresh Komga
            </button>
          {/if}

          {#if onPlanMissing}
            <button class="btn pill sm icon-text" onclick={onPlanMissing} title="Open volume acquisition planner">
              <Icon name="puzzle" size={12} /> Plan Missing
            </button>
          {/if}

          {#if onDiscoverMissing}
            <button
              class="btn pill sm icon-text"
              disabled={discoveringMissing}
              onclick={onDiscoverMissing}
              title="Discover release candidates for all missing volumes"
            >
              {#if discoveringMissing}
                <span class="spin"><Icon name="spinner" size={12} stroke={3} /></span>
              {:else}
                <Icon name="search" size={12} />
              {/if}
              Discover All Missing
            </button>
          {/if}

          {#if onCheckReleaseDiscovery}
            <button
              class="btn pill sm icon-text"
              onclick={onCheckReleaseDiscovery}
              title="Evaluate release discovery targets immediately"
            >
              <Icon name="clock" size={12} />
              Check Release Triggers
            </button>
          {/if}

          {#if onSearchNyaa}
            <button
              class="btn pill sm icon-text"
              onclick={onSearchNyaa}
              title="Open manual Nyaa release search"
            >
              <Icon name="search" size={12} />
              Search Nyaa
            </button>
          {/if}
        </div>

        <!-- OpenTome Remap controls inside Advanced -->
        {#if openTomeSchedule && onOpenTomeRemap}
          <div class="opentome-remap-row">
            <span class="remap-lbl">OpenTome Mapping:</span>
            <div class="remap-buttons">
              {#if openTomeSchedule.mappingConfidence !== 'user_confirmed'}
                <button
                  class="btn pill xs"
                  disabled={openTomeLoading}
                  onclick={() => onOpenTomeRemap('confirm')}
                  title="Confirm this OpenTome match"
                >
                  <Icon name="check" size={11} /> Confirm
                </button>
              {/if}
              <button
                class="btn pill xs"
                disabled={openTomeLoading}
                onclick={() => onOpenTomeRemap('reject')}
                title="Reject this OpenTome match"
              >
                <Icon name="close" size={11} /> Reject
              </button>
              <button
                class="btn pill xs"
                disabled={openTomeLoading}
                onclick={() => onOpenTomeRemap('auto')}
                title="Reset to automatic detection"
              >
                <Icon name="refresh" size={11} /> Auto
              </button>
            </div>
          </div>
        {/if}

        <!-- Komga Ownership and Provenance Details -->
        <div class="provenance-details-box">
          <div class="provenance-header">
            <strong>Ownership & Provenance Details</strong>
          </div>
          {#if ownership?.seriesName}
            <div class="detail-row">
              <span class="detail-k">Matched Series:</span>
              <span class="detail-v"><strong>{ownership.seriesName}</strong> <code class="detail-id">{ownership.seriesId}</code></span>
            </div>
          {/if}
          {#if ownership?.libraryName || ownership?.libraryId}
            <div class="detail-row">
              <span class="detail-k">Komga Library:</span>
              <span class="detail-v">{ownership.libraryName ?? ownership.libraryId}</span>
            </div>
          {/if}
          {#if ownedVolumeNumbers.length > 0}
            <div class="detail-row">
              <span class="detail-k">Owned Volumes:</span>
              <span class="detail-v">{formatVolumeRanges(ownedVolumeNumbers)}</span>
            </div>
          {/if}
          {#if missingVolumeNumbers.length > 0}
            <div class="detail-row">
              <span class="detail-k">Missing Volumes:</span>
              <span class="detail-v">{formatVolumeRanges(missingVolumeNumbers)}</span>
            </div>
          {/if}
          {#if libraryState?.provenance}
            <div class="detail-row">
              <span class="detail-k">Ownership Source:</span>
              <span class="detail-v">{libraryState.provenance.ownedSource}</span>
            </div>
            <div class="detail-row">
              <span class="detail-k">Release Schedule:</span>
              <span class="detail-v">{libraryState.provenance.releasedSource}</span>
            </div>
            <div class="detail-row">
              <span class="detail-k">Missing Calculation:</span>
              <span class="detail-v">{libraryState.provenance.missingSource}</span>
            </div>
            {#if libraryState.provenance.upcomingSource}
              <div class="detail-row">
                <span class="detail-k">Upcoming Release:</span>
                <span class="detail-v">{libraryState.provenance.upcomingSource}</span>
              </div>
            {/if}
          {/if}
          {#if ownership?.lastRefreshed}
            <div class="detail-row">
              <span class="detail-k">Last Checked:</span>
              <span class="detail-v">{ago(ownership.lastRefreshed)}</span>
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .library-releases-card {
    background: var(--surface);
    border: 3px solid var(--ink);
    border-radius: var(--radius, 8px);
    padding: 18px 20px;
    box-shadow: 4px 4px 0 var(--ink);
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .card-top {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }

  .section-title-col {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .section-headline {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .section-heading {
    font-size: 20px;
    margin: 0;
    line-height: 1.2;
    font-weight: 900;
  }

  .complete-tag {
    background: #e6f9ed;
    color: #137333;
    border: 1.5px solid #137333;
    border-radius: 999px;
    font-size: 11px;
    padding: 2px 10px;
    font-weight: 700;
  }

  .uptodate-tag {
    background: #e6f9ed;
    color: #137333;
    border: 1.5px solid #137333;
    border-radius: 999px;
    font-size: 11px;
    padding: 2px 10px;
    font-weight: 700;
  }

  .ongoing-tag {
    background: var(--paper);
    border: 1.5px solid var(--ink);
    border-radius: 999px;
    font-size: 11px;
    padding: 2px 8px;
    font-weight: 700;
  }

  .progress-pct-label {
    font-size: 13px;
    color: var(--muted);
  }

  .primary-action-slot {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .primary-cta {
    font-weight: 700;
    font-size: 14px;
    padding: 8px 16px;
  }

  .pulse-cta {
    animation: gentle-pulse 2s infinite ease-in-out;
  }

  @keyframes gentle-pulse {
    0%, 100% {
      transform: scale(1);
    }
    50% {
      transform: scale(1.03);
    }
  }

  .state-banner {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 12px 16px;
    border-radius: 6px;
    border: 2px solid;
  }

  .state-banner.alert-warning {
    background: #fef7e0;
    border-color: #b06000;
    color: #b06000;
  }

  .banner-icon {
    flex-shrink: 0;
  }

  .banner-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .banner-body strong {
    font-size: 14px;
    color: #7a3e00;
  }

  .banner-text {
    font-size: 12px;
    margin: 0;
    color: #7a3e00;
  }

  .status-summary-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }

  .status-headline {
    font-size: 14px;
    font-weight: 600;
  }

  .status-text.complete {
    color: #065f46;
  }

  .status-text.uptodate {
    color: #065f46;
    font-weight: 700;
  }

  .missing-accent {
    color: #b06000;
    font-weight: 700;
  }

  .release-trigger-notice {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    padding: 3px 8px;
    border-radius: 4px;
    border: 1px solid var(--ink);
    background: var(--paper);
  }

  .release-trigger-notice.notice-found {
    background: #e6f9ed;
    border-color: #137333;
    color: #137333;
    font-weight: 700;
  }

  .progress-bar-container {
    width: 100%;
  }

  .summary-meter {
    width: 100%;
    height: 10px;
    border-radius: 999px;
    overflow: hidden;
  }

  .summary-meter > span {
    background: #10b981;
    border-radius: 999px;
  }

  .metric-counters {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(100px, 1fr));
    gap: 10px;
    padding-top: 2px;
  }

  .counter-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 8px 10px;
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: 6px;
    text-align: center;
  }

  .counter-num {
    font-size: 18px;
    font-weight: 900;
    font-family: 'Dela Gothic One', sans-serif;
    line-height: 1.1;
  }

  .counter-lbl {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    margin-top: 2px;
  }

  .counter-item.owned .counter-num {
    color: #065f46;
  }

  .counter-item.missing .counter-num {
    color: var(--muted);
  }

  .counter-item.has-active {
    background: #e0f2fe;
    border-color: #0284c7;
  }

  .counter-item.has-active .counter-num {
    color: #0369a1;
  }

  .counter-item.has-attention {
    background: #fee2e2;
    border-color: #dc2626;
  }

  .counter-item.has-attention .counter-num {
    color: #b91c1c;
  }

  /* Integrated OpenTome Section */
  .opentome-integrated-section {
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: 6px;
    padding: 10px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .opentome-section-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .ot-badge {
    background: #4f46e5;
    color: white;
    font-size: 10px;
    font-weight: 800;
    padding: 2px 6px;
    border-radius: 4px;
    letter-spacing: 0.05em;
  }

  .ot-section-title {
    font-size: 12px;
    font-weight: 700;
    color: var(--ink);
  }

  .confidence-tag {
    font-size: 10px;
    font-weight: 700;
    padding: 1px 6px;
    border-radius: 4px;
    margin-left: auto;
  }

  .confidence-tag.conf-exact {
    background: #e6f9ed;
    color: #137333;
  }

  .confidence-tag.conf-high {
    background: #e0f2fe;
    color: #0284c7;
  }

  .confidence-tag.conf-ambiguous {
    background: #fef7e0;
    color: #b06000;
  }

  .opentome-market-lines {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 8px;
  }

  .market-line {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }

  .lang-tag {
    font-size: 10px;
    font-weight: 800;
    padding: 1px 5px;
    border-radius: 3px;
    border: 1px solid var(--ink);
  }

  .lang-tag.en {
    background: #e0e7ff;
    color: #3730a3;
  }

  .lang-tag.ja {
    background: #fee2e2;
    color: #991b1b;
  }

  .line-status {
    font-weight: 500;
  }

  .line-status.muted {
    color: var(--muted);
  }

  .today-tag {
    background: #fef08a;
    color: #854d0e;
    font-size: 9px;
    font-weight: 800;
    padding: 1px 4px;
    border-radius: 3px;
    margin-left: 4px;
  }

  /* Advanced disclosure */
  .advanced-disclosure {
    border-top: 1px solid var(--outline, #e5e7eb);
    padding-top: 8px;
    margin-top: 2px;
  }

  .advanced-toggle {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
    background: none;
    border: none;
    padding: 4px 0;
    cursor: pointer;
    font-weight: 600;
  }

  .advanced-toggle:hover {
    color: var(--ink);
  }

  .advanced-panel {
    display: flex;
    flex-direction: column;
    gap: 10px;
    padding-top: 8px;
  }

  .advanced-buttons-row {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .opentome-remap-row {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 11px;
    color: var(--muted);
    padding-top: 4px;
  }

  .remap-lbl {
    font-weight: 600;
  }

  .remap-buttons {
    display: flex;
    gap: 6px;
  }

  .provenance-details-box {
    margin-top: 14px;
    padding-top: 14px;
    border-top: 1px dashed var(--line);
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
  }

  .provenance-header {
    font-size: 12px;
    font-weight: 800;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    margin-bottom: 4px;
  }

  .detail-row {
    display: flex;
    gap: 8px;
    line-height: 1.4;
  }

  .detail-k {
    color: var(--muted);
    min-width: 140px;
    flex-shrink: 0;
  }

  .detail-v {
    color: var(--ink);
  }

  .detail-id {
    font-family: var(--font-mono, monospace);
    font-size: 11px;
    background: var(--paper);
    padding: 1px 4px;
    border-radius: 4px;
  }
</style>
