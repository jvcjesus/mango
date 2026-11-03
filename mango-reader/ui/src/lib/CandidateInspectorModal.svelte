<script lang="ts">
  import {
    inspectVolumeCandidates,
    message,
    type VolumeCandidateInspectionResult,
    type CandidateInspectionDto,
  } from './api';
  import Icon from './Icon.svelte';
  import Notice from './Notice.svelte';
  import { formatBytes } from './util';

  let {
    open = false,
    mangaId,
    volume,
    onClose,
  }: {
    open: boolean;
    mangaId: number;
    volume: number;
    onClose: () => void;
  } = $props();

  let loading = $state(false);
  let error = $state<string | null>(null);
  let data = $state<VolumeCandidateInspectionResult | null>(null);

  async function load() {
    if (!open || !mangaId || volume == null) return;
    loading = true;
    error = null;
    try {
      data = await inspectVolumeCandidates(mangaId, volume);
    } catch (e) {
      error = message(e);
    } finally {
      loading = false;
    }
  }

  $effect(() => {
    if (open) {
      load();
    } else {
      data = null;
      error = null;
    }
  });
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={onClose}>
    <div class="modal-card inspect-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-head">
        <div class="head-title">
          <Icon name="search" size={20} />
          <h2>Candidate Inspector — Volume {volume}</h2>
        </div>
        <button class="btn icon close-btn" onclick={onClose} aria-label="Close">
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body">
        {#if loading}
          <div class="loading-state">
            <span class="spin"><Icon name="spinner" size={24} stroke={3} /></span>
            <span>Inspecting candidates for Volume {volume}…</span>
          </div>
        {:else if error}
          <Notice error={error} retry={load} />
        {:else if data}
          <div class="target-summary-box">
            <div class="target-stat">
              <span class="label">Target</span>
              <span class="value">Volume {data.volumeNumber}</span>
            </div>
            <div class="target-stat">
              <span class="label">Release Status</span>
              <span class="value tag status-{data.targetStatus || 'unknown'}">
                {data.targetStatus || 'Unscheduled'}
              </span>
            </div>
            {#if data.targetReleaseDate}
              <div class="target-stat">
                <span class="label">Release Date</span>
                <span class="value">{data.targetReleaseDate}</span>
              </div>
            {/if}
            <div class="target-stat">
              <span class="label">Discovered Candidates</span>
              <span class="value badge-count">{data.candidatesCount}</span>
            </div>
          </div>

          {#if data.candidates.length === 0}
            <div class="empty-candidates">
              <Icon name="search" size={32} />
              <p>No candidates have been discovered for Volume {volume}.</p>
            </div>
          {:else}
            <div class="candidates-inspect-list">
              {#each data.candidates as cand (cand.candidateId)}
                <div class="candidate-inspect-card" class:top-card={cand.candidateId === data.topCandidate?.candidateId}>
                  <div class="cand-top-row">
                    <div class="cand-title-block">
                      <span class="cand-name" title={cand.rawTitle}>{cand.title || cand.rawTitle}</span>
                      {#if cand.rawTitle && cand.rawTitle !== cand.title}
                        <span class="cand-raw-name"><code>{cand.rawTitle}</code></span>
                      {/if}
                    </div>
                    <div class="cand-score-tag" title="Candidate quality score">
                      Score: {cand.score.toFixed(0)}
                    </div>
                  </div>

                  <div class="cand-badges-row">
                    <span class="badge transport-badge">
                      <Icon name="upload" size={12} /> {cand.transport}
                    </span>
                    <span class="badge match-badge match-{cand.matchType}">
                      {cand.matchType.replace(/_/g, ' ')}
                    </span>
                    {#if cand.isPre}
                      <span class="badge pre-badge">PRE-RELEASE</span>
                    {/if}
                    {#if cand.isDigital}
                      <span class="badge digital-badge">Digital</span>
                    {/if}
                    {#if cand.isTrusted}
                      <span class="badge trusted-badge">Trusted</span>
                    {/if}
                    {#if cand.sizeBytes}
                      <span class="badge meta-badge">{formatBytes(cand.sizeBytes)}</span>
                    {/if}
                    {#if cand.seeders != null}
                      <span class="badge meta-badge">{cand.seeders} seeds</span>
                    {/if}
                    {#if cand.uploader}
                      <span class="badge meta-badge">by {cand.uploader}</span>
                    {/if}
                  </div>

                  {#if cand.scoreExplanation}
                    <div class="cand-explanation">
                      <strong>Score Breakdown:</strong> {cand.scoreExplanation}
                    </div>
                  {/if}

                  <div class="cand-eligibility-box" class:eligible={cand.autoAcquireEligible} class:rejected={!cand.autoAcquireEligible}>
                    {#if cand.autoAcquireEligible}
                      <div class="eligibility-status success">
                        <Icon name="check" size={16} />
                        <strong>Auto-Acquisition Policy: ELIGIBLE</strong>
                      </div>
                      <p class="eligibility-desc">Passes hard eligibility gates (ExactVolume, supported provider, permitted format and quality thresholds).</p>
                    {:else}
                      <div class="eligibility-status blocked">
                        <Icon name="close" size={16} />
                        <strong>Auto-Acquisition Policy: BLOCKED ({cand.rejectionCode || 'REJECTED'})</strong>
                      </div>
                      <p class="eligibility-desc">{cand.rejectionReason || 'Failed hard candidate eligibility gate; manual review required.'}</p>
                    {/if}
                  </div>

                  {#if cand.sanitizedReference}
                    <div class="cand-sanitized-ref">
                      <span class="ref-label">Sanitized Reference (No Secrets):</span>
                      <code class="ref-code">{cand.sanitizedReference}</code>
                    </div>
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        {/if}
      </div>

      <div class="modal-foot">
        <button class="btn" onclick={onClose}>Close</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.65);
    backdrop-filter: blur(4px);
    z-index: 9999;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1rem;
  }

  .inspect-modal {
    background: var(--bg-surface, #1e1e24);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
    border-radius: 12px;
    width: 100%;
    max-width: 820px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    box-shadow: var(--sh-4, 0 16px 32px rgba(0, 0, 0, 0.5));
    overflow: hidden;
  }

  .modal-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem 1.25rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  }

  .head-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .head-title h2 {
    margin: 0;
    font-size: 1.15rem;
    font-weight: 600;
  }

  .modal-body {
    padding: 1.25rem;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .modal-foot {
    display: flex;
    justify-content: flex-end;
    padding: 0.75rem 1.25rem;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    background: var(--bg-surface-subtle, rgba(255, 255, 255, 0.02));
  }

  .target-summary-box {
    display: flex;
    flex-wrap: wrap;
    gap: 1.25rem;
    background: var(--bg-panel, rgba(255, 255, 255, 0.04));
    padding: 0.75rem 1rem;
    border-radius: 8px;
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  }

  .target-stat {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .target-stat .label {
    font-size: 0.75rem;
    color: var(--muted, #8b949e);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .target-stat .value {
    font-weight: 600;
    font-size: 0.95rem;
  }

  .candidates-inspect-list {
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
  }

  .candidate-inspect-card {
    background: var(--bg-card, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 8px;
    padding: 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }

  .candidate-inspect-card.top-card {
    border-color: var(--primary, #3b82f6);
    background: rgba(59, 130, 246, 0.04);
  }

  .cand-top-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 0.75rem;
  }

  .cand-title-block {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .cand-name {
    font-weight: 600;
    font-size: 0.95rem;
    line-height: 1.3;
  }

  .cand-raw-name {
    font-size: 0.75rem;
    color: var(--muted, #8b949e);
    word-break: break-all;
  }

  .cand-score-tag {
    font-size: 0.8rem;
    font-weight: 700;
    padding: 0.2rem 0.5rem;
    background: var(--bg-panel, rgba(255, 255, 255, 0.08));
    border-radius: 6px;
    white-space: nowrap;
  }

  .cand-badges-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .badge {
    font-size: 0.75rem;
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    background: var(--bg-panel, rgba(255, 255, 255, 0.06));
    color: var(--fg, #e6edf3);
  }

  .transport-badge {
    background: rgba(139, 92, 246, 0.15);
    color: #c4b5fd;
    border: 1px solid rgba(139, 92, 246, 0.3);
  }

  .pre-badge {
    background: rgba(245, 158, 11, 0.15);
    color: #fcd34d;
    font-weight: 700;
    border: 1px solid rgba(245, 158, 11, 0.3);
  }

  .digital-badge {
    background: rgba(16, 185, 129, 0.15);
    color: #6ee7b7;
  }

  .trusted-badge {
    background: rgba(59, 130, 246, 0.15);
    color: #93c5fd;
  }

  .match-exact_volume {
    background: rgba(16, 185, 129, 0.2);
    color: #a7f3d0;
  }

  .match-volume_range {
    background: rgba(245, 158, 11, 0.2);
    color: #fde68a;
  }

  .cand-explanation {
    font-size: 0.8rem;
    color: var(--muted, #8b949e);
    background: rgba(0, 0, 0, 0.15);
    padding: 0.4rem 0.6rem;
    border-radius: 4px;
  }

  .cand-eligibility-box {
    padding: 0.6rem 0.75rem;
    border-radius: 6px;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.82rem;
  }

  .cand-eligibility-box.eligible {
    background: rgba(16, 185, 129, 0.08);
    border: 1px solid rgba(16, 185, 129, 0.25);
  }

  .cand-eligibility-box.rejected {
    background: rgba(239, 68, 68, 0.08);
    border: 1px solid rgba(239, 68, 68, 0.25);
  }

  .eligibility-status {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
  }

  .eligibility-status.success {
    color: #34d399;
  }

  .eligibility-status.blocked {
    color: #f87171;
  }

  .eligibility-desc {
    margin: 0;
    color: var(--muted, #9ca3af);
  }

  .cand-sanitized-ref {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.75rem;
  }

  .ref-label {
    color: var(--muted, #8b949e);
  }

  .ref-code {
    background: rgba(0, 0, 0, 0.25);
    padding: 0.35rem 0.5rem;
    border-radius: 4px;
    color: #cbd5e1;
    word-break: break-all;
    font-family: monospace;
  }

  .loading-state,
  .empty-candidates {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 2.5rem;
    gap: 0.75rem;
    color: var(--muted, #8b949e);
  }
</style>
