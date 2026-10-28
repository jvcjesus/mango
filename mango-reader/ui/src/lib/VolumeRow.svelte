<script lang="ts">
  import Icon from './Icon.svelte';
  import { ago, formatBytes } from './util';
  import type { VolumePresentation } from './volumeStatus';

  let {
    volume,
    onRetryMetadata,
    onRetryKomga,
    onRetryDownload,
    onReview,
    onAcquire,
    children,
  }: {
    volume: VolumePresentation;
    onRetryMetadata?: (vol: VolumePresentation) => void;
    onRetryKomga?: (vol: VolumePresentation) => void;
    onRetryDownload?: (vol: VolumePresentation) => void;
    onReview?: (vol: VolumePresentation) => void;
    onAcquire?: (vol: VolumePresentation) => void;
    children?: import('svelte').Snippet;
  } = $props();

  let showDetails = $state(false);
</script>

<div class="volume-card status-{volume.badgeVariant}" class:has-warning={Boolean(volume.contradictionWarning)}>
  <div class="card-main">
    <div class="card-header">
      <div class="title-group">
        <strong class="volume-title">{volume.label}</strong>
        <span class="status-badge {volume.badgeVariant}" role="status">
          {#if volume.status === 'complete' || volume.status === 'owned'}
            <Icon name="check" size={13} stroke={2.5} />
          {:else if volume.status === 'acquiring' || volume.status === 'processing' || volume.status === 'metadata'}
            <span class="spin"><Icon name="spinner" size={13} stroke={2.5} /></span>
          {:else if volume.status === 'attention'}
            <Icon name="info" size={13} stroke={2.5} />
          {/if}
          {volume.statusLabel}
        </span>
      </div>

      <div class="actions-group">
        {#if volume.canRetryMetadata && onRetryMetadata}
          <button class="btn pill action-btn spot" onclick={() => onRetryMetadata(volume)}>
            <Icon name="refresh" size={14} /> Retry metadata
          </button>
        {/if}

        {#if volume.canRetryKomga && onRetryKomga}
          <button class="btn pill action-btn" onclick={() => onRetryKomga(volume)}>
            <Icon name="refresh" size={14} /> Retry Komga
          </button>
        {/if}

        {#if volume.canRetryDownload && onRetryDownload}
          <button class="btn pill action-btn spot" onclick={() => onRetryDownload(volume)}>
            <Icon name="refresh" size={14} /> Try again
          </button>
        {/if}

        {#if volume.canReview && onReview}
          <button class="btn pill action-btn spot" onclick={() => onReview(volume)}>
            Review assignment
          </button>
        {/if}

        {#if volume.canAcquire && onAcquire}
          <button class="btn pill action-btn" onclick={() => onAcquire(volume)}>
            <Icon name="download" size={14} /> Acquire
          </button>
        {/if}

        {@render children?.()}

        {#if volume.bookName || volume.publishedPath || volume.releaseName || volume.error || volume.contradictionWarning}
          <button
            class="btn icon detail-toggle"
            aria-label="Toggle details for {volume.label}"
            aria-expanded={showDetails}
            onclick={() => (showDetails = !showDetails)}
            title="Inspect technical details"
          >
            <Icon name={showDetails ? 'up' : 'down'} size={14} />
          </button>
        {/if}
      </div>
    </div>

    <!-- Clear distinction between Physical Media State and Metadata State -->
    <div class="states-row">
      <span class="state-pill physical">
        <span class="state-key">Physical:</span>
        <span class="state-val">{volume.physicalLabel}</span>
      </span>

      {#if volume.metadataState !== 'not_applicable' && volume.physicalState === 'in_library'}
        <span class="state-divider">·</span>
        <span class="state-pill metadata" class:error-text={volume.metadataState === 'failed'}>
          <span class="state-key">Metadata:</span>
          <span class="state-val">{volume.metadataLabel}</span>
        </span>
      {/if}
    </div>

    {#if volume.progressPercent !== null && (volume.status === 'acquiring' || volume.physicalState === 'downloading')}
      <div class="meter-wrapper" aria-label="Download progress: {volume.progressPercent}%">
        <span class="meter"><span style:width="{volume.progressPercent}%"></span></span>
        {#if volume.bytesDownloaded && volume.totalBytes}
          <span class="bytes-counter">{formatBytes(volume.bytesDownloaded)} / {formatBytes(volume.totalBytes)}</span>
        {/if}
      </div>
    {/if}

    {#if volume.contradictionWarning}
      <div class="warning-banner" role="alert">
        <Icon name="info" size={14} />
        <span>{volume.contradictionWarning}</span>
      </div>
    {/if}
  </div>

  {#if showDetails}
    <div class="technical-details">
      <div class="details-grid">
        {#if volume.bookName}
          <div class="detail-item">
            <span class="d-key">Komga Book:</span>
            <span class="d-val">{volume.bookName}</span>
          </div>
        {/if}
        {#if volume.publishedPath}
          <div class="detail-item full-width">
            <span class="d-key">Canonical CBZ:</span>
            <span class="d-val code-val" title={volume.publishedPath}>{volume.publishedPath}</span>
          </div>
        {/if}
        {#if volume.releaseName}
          <div class="detail-item full-width">
            <span class="d-key">Release:</span>
            <span class="d-val">{volume.releaseName}</span>
          </div>
        {/if}
        {#if volume.sourceType}
          <div class="detail-item">
            <span class="d-key">Source:</span>
            <span class="d-val">{volume.sourceType}</span>
          </div>
        {/if}
        {#if volume.uploader}
          <div class="detail-item">
            <span class="d-key">Uploader:</span>
            <span class="d-val">{volume.uploader}</span>
          </div>
        {/if}
        {#if volume.chapterId}
          <div class="detail-item">
            <span class="d-key">Internal ID:</span>
            <span class="d-val code-val">#{volume.chapterId}</span>
          </div>
        {/if}
        {#if volume.jobId}
          <div class="detail-item">
            <span class="d-key">Metadata Job:</span>
            <span class="d-val code-val">{volume.jobId}</span>
          </div>
        {/if}
        {#if volume.lastUpdated}
          <div class="detail-item">
            <span class="d-key">Published:</span>
            <span class="d-val">{ago(volume.lastUpdated * 1000)}</span>
          </div>
        {/if}
        {#if volume.error}
          <div class="detail-item full-width error-box">
            <span class="d-key">Diagnostic Error:</span>
            <span class="d-val error-text">{volume.error}</span>
          </div>
        {/if}
      </div>
    </div>
  {/if}
</div>

<style>
  .volume-card {
    background: var(--surface);
    border: 2px solid var(--ink);
    border-radius: var(--radius, 6px);
    overflow: hidden;
    box-shadow: 2px 2px 0 var(--ink);
    transition: transform 0.1s ease;
  }

  .volume-card.has-warning {
    border-color: #d97706;
  }

  .card-main {
    padding: 12px 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
  }

  .title-group {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .volume-title {
    font-size: 16px;
    font-weight: 700;
  }

  .status-badge {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 2px 10px;
    border: 1.5px solid var(--ink);
    border-radius: 999px;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }

  /* Color Semantics */
  .status-badge.complete {
    background: #10b981;
    color: #ffffff;
    border-color: #065f46;
  }

  .status-badge.active {
    background: #0284c7;
    color: #ffffff;
    border-color: #0369a1;
  }

  .status-badge.waiting {
    background: var(--spot, #ffc400);
    color: var(--on-spot, #111111);
  }

  .status-badge.attention {
    background: var(--error, #e53935);
    color: #ffffff;
    border-color: #b91c1c;
  }

  .status-badge.neutral {
    background: var(--paper, #f1f1ec);
    color: var(--muted, #555555);
    border-color: var(--ink);
  }

  .actions-group {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .action-btn {
    font-size: 12px;
    padding: 2px 12px;
    height: 28px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
  }

  .detail-toggle {
    width: 28px;
    height: 28px;
    padding: 0;
    display: grid;
    place-items: center;
  }

  .states-row {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 13px;
    color: var(--muted);
  }

  .state-pill {
    display: inline-flex;
    align-items: baseline;
    gap: 4px;
  }

  .state-key {
    font-weight: 700;
    color: var(--ink);
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .state-val {
    color: var(--ink);
  }

  .state-divider {
    color: var(--muted);
  }

  .meter-wrapper {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-top: 2px;
  }

  .meter {
    flex: 1;
    max-width: 320px;
    height: 8px;
  }

  .meter > span {
    background: #0284c7;
  }

  .bytes-counter {
    font-size: 12px;
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .warning-banner {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    background: #fef3c7;
    color: #92400e;
    border: 1px solid #f59e0b;
    border-radius: 4px;
    font-size: 12px;
    font-weight: 600;
  }

  .technical-details {
    padding: 12px 16px;
    background: var(--paper);
    border-top: 1px solid var(--ink);
    font-size: 12px;
  }

  .details-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
    gap: 8px 16px;
  }

  .detail-item {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .detail-item.full-width {
    grid-column: 1 / -1;
  }

  .d-key {
    font-weight: 700;
    font-size: 11px;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .d-val {
    word-break: break-all;
  }

  .code-val {
    font-family: monospace;
    font-size: 11px;
  }

  .error-box {
    background: #fee2e2;
    padding: 6px 10px;
    border-radius: 4px;
    border: 1px solid #f87171;
  }

  .error-text {
    color: var(--error, #e53935);
    font-weight: 600;
  }
</style>
