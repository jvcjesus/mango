<script lang="ts">
  import Icon from './Icon.svelte';
  import type { SeriesSummary } from './volumeStatus';

  let {
    summary,
    onPlanMissing,
    onFindReleases,
    onRefreshKomga,
    refreshingKomga = false,
  }: {
    summary: SeriesSummary;
    onPlanMissing?: () => void;
    onFindReleases?: () => void;
    onRefreshKomga?: () => void;
    refreshingKomga?: boolean;
  } = $props();
</script>

<div class="series-summary-card">
  <div class="summary-top">
    <div class="summary-title-col">
      <div class="headline-row">
        <h2 class="display summary-heading">{summary.statusExplanation}</h2>
        {#if summary.isOngoing}
          <span class="tag ongoing-tag">Ongoing</span>
        {/if}
      </div>
      {#if summary.totalExpectedVolumes != null && summary.progressPercent != null}
        <span class="progress-pct-label">{summary.progressPercent}% of released volumes acquired</span>
      {/if}
    </div>

    <div class="summary-actions">
      {#if summary.missingCount > 0 && onPlanMissing}
        <button class="btn pill spot icon-text" onclick={onPlanMissing}>
          <Icon name="puzzle" size={14} /> Plan Missing
        </button>
      {/if}
      {#if onFindReleases}
        <button class="btn pill icon-text" onclick={onFindReleases}>
          <Icon name="search" size={14} /> Find Releases
        </button>
      {/if}
      {#if onRefreshKomga}
        <button class="btn pill icon-text" disabled={refreshingKomga} onclick={onRefreshKomga} title="Refresh ownership state directly from Komga">
          {#if refreshingKomga}
            <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
          {:else}
            <Icon name="refresh" size={14} />
          {/if}
          Komga
        </button>
      {/if}
    </div>
  </div>

  {#if summary.progressPercent !== null}
    <div class="progress-bar-container">
      <span class="meter summary-meter" aria-hidden="true">
        <span style:width="{summary.progressPercent}%"></span>
      </span>
    </div>
  {/if}

  <div class="metric-counters">
    <div class="counter-item owned">
      <span class="counter-num">{summary.ownedCount}</span>
      <span class="counter-lbl">Owned</span>
    </div>

    <div class="counter-item missing">
      <span class="counter-num">{summary.missingCount}</span>
      <span class="counter-lbl">Missing</span>
    </div>

    <div class="counter-item active" class:has-active={summary.activeCount > 0}>
      <span class="counter-num">{summary.activeCount}</span>
      <span class="counter-lbl">Active</span>
    </div>

    <div class="counter-item attention" class:has-attention={summary.attentionCount > 0}>
      <span class="counter-num">{summary.attentionCount}</span>
      <span class="counter-lbl">Needs Attention</span>
    </div>
  </div>
</div>

<style>
  .series-summary-card {
    background: var(--surface);
    border: 3px solid var(--ink);
    border-radius: var(--radius, 8px);
    padding: 18px 20px;
    box-shadow: 4px 4px 0 var(--ink);
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .summary-top {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
  }

  .summary-title-col {
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }

  .headline-row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
  }

  .summary-heading {
    font-size: 20px;
    margin: 0;
    line-height: 1.2;
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

  .summary-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
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
    grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
    gap: 12px;
    padding-top: 4px;
  }

  .counter-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 10px 12px;
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: 6px;
    text-align: center;
  }

  .counter-num {
    font-size: 20px;
    font-weight: 900;
    font-family: 'Dela Gothic One', sans-serif;
    line-height: 1.1;
  }

  .counter-lbl {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.06em;
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
</style>
