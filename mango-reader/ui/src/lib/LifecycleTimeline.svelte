<script lang="ts">
  import Icon from './Icon.svelte';
  import {
    deriveLifecycleTimeline,
    type LifecycleStage,
  } from './volumeStatus';
  import { ago } from './util';

  interface Props {
    stages?: LifecycleStage[];
    timelineInputs?: Parameters<typeof deriveLifecycleTimeline>[0];
    compact?: boolean;
  }

  let { stages, timelineInputs, compact = false }: Props = $props();

  const resolvedStages = $derived.by<LifecycleStage[]>(() => {
    if (stages && stages.length > 0) return stages;
    if (timelineInputs) return deriveLifecycleTimeline(timelineInputs);
    return [];
  });
</script>

<div class="lifecycle-timeline" class:compact>
  <div class="timeline-track">
    {#each resolvedStages as stage, index (stage.id)}
      {@const isLast = index === resolvedStages.length - 1}
      <div
        class="timeline-node stage-{stage.status}"
        title={`${stage.label}: ${stage.status.toUpperCase()}${stage.detail ? ` (${stage.detail})` : ''}`}
      >
        <div class="node-marker" aria-label={`${stage.label} is ${stage.status}`}>
          {#if stage.status === 'completed'}
            <Icon name="check" size={compact ? 11 : 13} />
          {:else if stage.status === 'current'}
            <span class="active-pulse" aria-hidden="true"></span>
          {:else if stage.status === 'failed'}
            <Icon name="alert" size={compact ? 11 : 13} />
          {:else if stage.status === 'skipped'}
            <span class="skipped-line" aria-hidden="true"></span>
          {:else}
            <span class="pending-dot" aria-hidden="true"></span>
          {/if}
        </div>
        <div class="node-content">
          <span class="node-label">{stage.label}</span>
          {#if !compact}
            {#if stage.timestamp}
              <span class="node-time">{ago(stage.timestamp * 1000)}</span>
            {:else if stage.status === 'current'}
              <span class="node-active-tag">Active</span>
            {/if}
          {/if}
        </div>
      </div>
      {#if !isLast}
        <div class="timeline-connector" class:active={stage.status === 'completed'} aria-hidden="true"></div>
      {/if}
    {/each}
  </div>
</div>

<style>
  .lifecycle-timeline {
    width: 100%;
    overflow-x: auto;
    padding: 6px 2px;
  }

  .timeline-track {
    display: flex;
    align-items: center;
    gap: 0;
    min-width: max-content;
  }

  .timeline-node {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 4px;
    position: relative;
    z-index: 2;
  }

  .node-marker {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 50%;
    border: 2px solid var(--ink);
    background: var(--surface);
    color: var(--ink);
    font-size: 11px;
    transition: all 0.15s ease;
  }

  .compact .node-marker {
    width: 18px;
    height: 18px;
  }

  .stage-completed .node-marker {
    background: var(--status-ok, #2e7d32);
    border-color: var(--ink);
    color: #ffffff;
  }

  .stage-current .node-marker {
    background: var(--spot);
    border-color: var(--ink);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--spot) 35%, transparent);
  }

  .active-pulse {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--ink);
    animation: pulse 1.4s infinite ease-in-out;
  }

  @keyframes pulse {
    0%, 100% {
      transform: scale(0.8);
      opacity: 0.8;
    }
    50% {
      transform: scale(1.2);
      opacity: 1;
    }
  }

  .stage-failed .node-marker {
    background: var(--status-error, #c62828);
    border-color: var(--ink);
    color: #ffffff;
  }

  .stage-skipped .node-marker {
    background: var(--shade);
    border-color: var(--line);
    color: var(--muted);
  }

  .skipped-line {
    width: 8px;
    height: 2px;
    background: var(--muted);
  }

  .stage-pending .node-marker {
    background: var(--shade);
    border-color: var(--line);
    color: var(--dim);
  }

  .pending-dot {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--dim);
  }

  .node-content {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 2px;
    max-width: 90px;
  }

  .node-label {
    font-size: 11px;
    font-weight: 700;
    line-height: 1.2;
    color: var(--ink);
    text-transform: capitalize;
    white-space: nowrap;
  }

  .compact .node-label {
    font-size: 10px;
  }

  .node-time {
    font-size: 9.5px;
    font-weight: 600;
    color: var(--muted);
  }

  .node-active-tag {
    font-size: 9px;
    font-weight: 800;
    text-transform: uppercase;
    color: var(--ink);
    background: var(--spot);
    padding: 1px 4px;
    border: 1px solid var(--ink);
    border-radius: 2px;
  }

  .timeline-connector {
    flex: 1 1 28px;
    min-width: 20px;
    height: 2px;
    background: var(--line);
    margin: 0 4px;
    margin-bottom: 22px;
  }

  .compact .timeline-connector {
    margin-bottom: 14px;
  }

  .timeline-connector.active {
    background: var(--status-ok, #2e7d32);
    height: 3px;
  }
</style>
