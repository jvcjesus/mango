<script lang="ts">
  import {
    inspectVolumeCandidates,
    compareVolumeCandidates,
    message,
    type VolumeCandidateInspectionResult,
    type CandidateComparisonMatrix,
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

  let viewMode = $state<'list' | 'compare'>('list');
  let loadingCompare = $state(false);
  let compareError = $state<string | null>(null);
  let comparisonData = $state<CandidateComparisonMatrix | null>(null);

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

  async function loadComparison() {
    if (!open || !mangaId || volume == null) return;
    loadingCompare = true;
    compareError = null;
    try {
      comparisonData = await compareVolumeCandidates(mangaId, volume);
    } catch (e) {
      compareError = message(e);
    } finally {
      loadingCompare = false;
    }
  }

  $effect(() => {
    if (open) {
      load();
      if (viewMode === 'compare') {
        loadComparison();
      }
    } else {
      data = null;
      comparisonData = null;
      error = null;
      compareError = null;
      viewMode = 'list';
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
        <div class="head-actions">
          <div class="view-toggle">
            <button
              class="toggle-btn"
              class:active={viewMode === 'list'}
              onclick={() => (viewMode = 'list')}
            >
              <Icon name="list" size={14} />
              <span>List</span>
            </button>
            <button
              class="toggle-btn"
              class:active={viewMode === 'compare'}
              onclick={() => {
                viewMode = 'compare';
                if (!comparisonData) loadComparison();
              }}
            >
              <Icon name="sliders" size={14} />
              <span>Compare</span>
            </button>
          </div>
          <button class="btn icon close-btn" onclick={onClose} aria-label="Close">
            <Icon name="close" size={18} />
          </button>
        </div>
      </div>

      <div class="modal-body">
        {#if loading && !data}
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
              <span class="label">Ownership</span>
              {#if data.isCurrentlyOwned}
                <span class="value tag owned-tag">Owned in Komga</span>
              {:else}
                <span class="value tag missing-tag">Not Owned</span>
              {/if}
            </div>
            <div class="target-stat">
              <span class="label">Discovered Candidates</span>
              <span class="value badge-count">{data.candidatesCount}</span>
            </div>
          </div>

          {#if data.isCurrentlyOwned}
            <div class="upgrade-protection-banner">
              <Icon name="check" size={16} />
              <div>
                <strong>Strict No-Silent-Upgrade Protection Active:</strong>
                <span> This volume is already owned. Automatic background acquisition will never overwrite or duplicate it. Upgrades must be manually initiated.</span>
              </div>
            </div>
          {/if}

          {#if viewMode === 'list'}
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
                        <div class="cand-rank-row">
                          {#if cand.qualityEvaluation?.rank}
                            <span class="badge rank-badge">#{cand.qualityEvaluation.rank}</span>
                          {/if}
                          {#if cand.qualityEvaluation?.selectionVerdict}
                            <span class="badge verdict-badge verdict-{cand.qualityEvaluation.selectionVerdict.toLowerCase()}">
                              {cand.qualityEvaluation.selectionVerdict.replace(/_/g, ' ')}
                            </span>
                          {/if}
                          {#if cand.qualityEvaluation?.isPreferredProvider}
                            <span class="badge preferred-provider-badge">Preferred Direct Provider</span>
                          {/if}
                          {#if cand.qualityEvaluation?.resolutionLabel}
                            <span class="badge res-badge">{cand.qualityEvaluation.resolutionLabel}</span>
                          {/if}
                          {#if cand.qualityEvaluation?.format}
                            <span class="badge format-badge">{cand.qualityEvaluation.format}</span>
                          {/if}
                          {#if cand.qualityEvaluation?.edition}
                            <span class="badge edition-badge">{cand.qualityEvaluation.edition}</span>
                          {/if}
                        </div>
                        <span class="cand-name" title={cand.rawTitle}>{cand.title || cand.rawTitle}</span>
                        {#if cand.rawTitle && cand.rawTitle !== cand.title}
                          <span class="cand-raw-name"><code>{cand.rawTitle}</code></span>
                        {/if}
                      </div>
                      <div class="cand-score-tag" title="Candidate quality score">
                        Score: {cand.score.toFixed(0)}
                      </div>
                    </div>

                    {#if cand.qualityEvaluation?.warningBadges && cand.qualityEvaluation.warningBadges.length > 0}
                      <div class="warning-badges-row">
                        {#each cand.qualityEvaluation.warningBadges as w}
                          <span class="badge warning-badge-pill">{w}</span>
                        {/each}
                      </div>
                    {/if}

                    {#if cand.qualityEvaluation?.factors && cand.qualityEvaluation.factors.length > 0}
                      <div class="quality-factors-wrap">
                        <span class="factors-label">Quality Signals:</span>
                        <div class="factors-pills">
                          {#each cand.qualityEvaluation.factors as f}
                            <span class="factor-pill" class:factor-bonus={f.isBonus} class:factor-penalty={!f.isBonus} title={f.description}>
                              {f.isBonus ? '+' : ''}{f.points.toFixed(0)} {f.name}
                            </span>
                          {/each}
                        </div>
                      </div>
                    {/if}

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
                      {#if cand.qualityEvaluation?.sizePerPageKb}
                        <span class="badge meta-badge">~{cand.qualityEvaluation.sizePerPageKb} KB/p</span>
                      {/if}
                      {#if cand.seeders != null}
                        <span class="badge meta-badge">{cand.seeders} seeds</span>
                      {/if}
                      {#if cand.uploader}
                        <span class="badge meta-badge">by {cand.uploader}</span>
                      {/if}
                    </div>

                    {#if cand.qualityEvaluation?.alternateSources && cand.qualityEvaluation.alternateSources.length > 0}
                      <div class="alternate-mirrors-box">
                        <span class="alt-label">Duplicate / Alternate Mirrors ({cand.qualityEvaluation.alternateSources.length}):</span>
                        <div class="alt-pills">
                          {#each cand.qualityEvaluation.alternateSources as alt}
                            <span class="alt-pill">
                              <strong>{alt.provider}</strong> ({alt.transport}){#if alt.seeders != null} · {alt.seeders}s{/if}{#if alt.sizeBytes} · {formatBytes(alt.sizeBytes)}{/if}
                            </span>
                          {/each}
                        </div>
                      </div>
                    {/if}

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
          {:else if viewMode === 'compare'}
            {#if loadingCompare && !comparisonData}
              <div class="loading-state">
                <span class="spin"><Icon name="spinner" size={24} stroke={3} /></span>
                <span>Generating side-by-side candidate comparison…</span>
              </div>
            {:else if compareError}
              <Notice error={compareError} retry={loadComparison} />
            {:else if comparisonData}
              {#if comparisonData.preferenceExplanation}
                <div class="compare-recommendation-banner">
                  <Icon name="info" size={20} />
                  <div>
                    <strong>Selection Analysis:</strong>
                    <p class="rec-text">{comparisonData.preferenceExplanation}</p>
                    {#if comparisonData.duplicateGroupsCount > 0}
                      <span class="dup-count-note">
                        ({comparisonData.duplicateGroupsCount} mirror release{comparisonData.duplicateGroupsCount > 1 ? 's' : ''} deduplicated across sources)
                      </span>
                    {/if}
                  </div>
                </div>
              {/if}

              {#if comparisonData.candidates.length === 0}
                <div class="empty-candidates">
                  <Icon name="search" size={32} />
                  <p>No candidates available to compare for Volume {volume}.</p>
                </div>
              {:else}
                <div class="comparison-grid">
                  {#each comparisonData.candidates as cand (cand.candidateId)}
                    <div class="comparison-column" class:rank-1={cand.rank === 1}>
                      <div class="col-head">
                        <div class="col-rank-row">
                          <span class="badge rank-badge">#{cand.rank}</span>
                          <span class="badge verdict-badge verdict-{cand.selectionVerdict.toLowerCase()}">
                            {cand.selectionVerdict.replace(/_/g, ' ')}
                          </span>
                        </div>
                        <div class="col-cand-title" title={cand.rawTitle}>{cand.title || cand.rawTitle}</div>
                        <div class="col-cand-score">Score: <strong>{cand.totalScore.toFixed(0)}</strong></div>
                      </div>

                      <div class="col-card-section">
                        <span class="sec-label">Release Specifications</span>
                        <div class="matrix-row"><span class="m-k">Provider</span><span class="m-v">{cand.provider}</span></div>
                        <div class="matrix-row"><span class="m-k">Transport</span><span class="m-v text-dim">{cand.transport}</span></div>
                        <div class="matrix-row"><span class="m-k">Format</span><span class="m-v badge format-badge">{cand.format}</span></div>
                        <div class="matrix-row"><span class="m-k">Edition</span><span class="m-v">{cand.edition}</span></div>
                        <div class="matrix-row"><span class="m-k">Size</span><span class="m-v">{formatBytes(cand.sizeBytes || 0)}</span></div>
                        {#if cand.sizePerPageKb}
                          <div class="matrix-row"><span class="m-k">Est. Page</span><span class="m-v">~{cand.sizePerPageKb} KB/p</span></div>
                        {/if}
                        {#if cand.seeders != null}
                          <div class="matrix-row"><span class="m-k">Swarm</span><span class="m-v">{cand.seeders} seeds ({cand.healthRating})</span></div>
                        {/if}
                        {#if cand.uploader}
                          <div class="matrix-row"><span class="m-k">Uploader</span><span class="m-v">{cand.uploader}</span></div>
                        {/if}
                      </div>

                      {#if cand.warningBadges && cand.warningBadges.length > 0}
                        <div class="col-card-section warning-section">
                          <span class="sec-label text-warning">Warning Flags</span>
                          <div class="warning-badges-list">
                            {#each cand.warningBadges as w}
                              <span class="badge warning-badge-pill">{w}</span>
                            {/each}
                          </div>
                        </div>
                      {/if}

                      {#if cand.advantages && cand.advantages.length > 0}
                        <div class="col-card-section">
                          <span class="sec-label text-success">Key Advantages</span>
                          <ul class="signal-list">
                            {#each cand.advantages as adv}
                              <li class="signal-item adv-item"><Icon name="check" size={13} /> <span>{adv}</span></li>
                            {/each}
                          </ul>
                        </div>
                      {/if}

                      {#if cand.disadvantages && cand.disadvantages.length > 0}
                        <div class="col-card-section">
                          <span class="sec-label text-danger">Disadvantages & Flags</span>
                          <ul class="signal-list">
                            {#each cand.disadvantages as dis}
                              <li class="signal-item dis-item"><Icon name="close" size={13} /> <span>{dis}</span></li>
                            {/each}
                          </ul>
                        </div>
                      {/if}

                      {#if cand.rejectionReason}
                        <div class="col-card-section rejection-section">
                          <span class="sec-label text-danger">Disqualification Reason</span>
                          <p class="rejection-desc">{cand.rejectionReason}</p>
                        </div>
                      {/if}

                      {#if cand.alternateSources && cand.alternateSources.length > 0}
                        <div class="col-card-section mirror-section">
                          <span class="sec-label">Alternate Mirrors ({cand.alternateSources.length})</span>
                          {#each cand.alternateSources as alt}
                            <div class="mirror-mini-card">
                              <span class="m-prov">{alt.provider}</span>
                              <span class="m-sub">{alt.transport}{#if alt.seeders != null} · {alt.seeders} seeds{/if}</span>
                            </div>
                          {/each}
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
    max-width: 960px;
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
    background: rgba(0, 0, 0, 0.15);
  }

  .head-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .head-title h2 {
    font-size: 1.1rem;
    font-weight: 700;
    margin: 0;
  }

  .head-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .view-toggle {
    display: flex;
    background: rgba(0, 0, 0, 0.3);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 6px;
    padding: 2px;
    gap: 2px;
  }

  .toggle-btn {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    background: transparent;
    border: none;
    color: var(--muted, #8b949e);
    padding: 0.3rem 0.6rem;
    border-radius: 4px;
    font-size: 0.8rem;
    cursor: pointer;
    font-weight: 500;
    transition: all 0.15s ease;
  }

  .toggle-btn:hover {
    color: #fff;
  }

  .toggle-btn.active {
    background: var(--accent, #6366f1);
    color: #fff;
    font-weight: 600;
  }

  .modal-body {
    padding: 1.25rem;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
  }

  .modal-foot {
    display: flex;
    justify-content: flex-end;
    padding: 0.75rem 1.25rem;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    background: rgba(0, 0, 0, 0.1);
  }

  .target-summary-box {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 0.75rem;
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 8px;
    padding: 0.75rem 1rem;
  }

  .target-stat {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .target-stat .label {
    font-size: 0.75rem;
    color: var(--muted, #8b949e);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .target-stat .value {
    font-size: 0.95rem;
    font-weight: 600;
  }

  .badge-count {
    background: rgba(99, 102, 241, 0.15);
    color: #818cf8;
    padding: 0.1rem 0.5rem;
    border-radius: 4px;
    width: fit-content;
  }

  .candidates-inspect-list {
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .candidate-inspect-card {
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    transition: border-color 0.15s ease;
  }

  .candidate-inspect-card.top-card {
    border-color: rgba(99, 102, 241, 0.5);
    background: rgba(99, 102, 241, 0.03);
  }

  .cand-top-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
  }

  .cand-title-block {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    flex: 1;
  }

  .cand-name {
    font-size: 0.95rem;
    font-weight: 600;
    line-height: 1.35;
    word-break: break-word;
  }

  .cand-raw-name {
    font-size: 0.75rem;
    color: var(--muted, #8b949e);
    word-break: break-all;
  }

  .cand-score-tag {
    font-size: 0.82rem;
    font-weight: 700;
    padding: 0.2rem 0.5rem;
    background: rgba(99, 102, 241, 0.15);
    color: #a5b4fc;
    border-radius: 6px;
    white-space: nowrap;
  }

  .cand-badges-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
  }

  .badge {
    font-size: 0.75rem;
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    font-weight: 500;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
  }

  .format-badge {
    background: rgba(234, 179, 8, 0.15);
    color: #fef08a;
    border: 1px solid rgba(234, 179, 8, 0.3);
  }

  .edition-badge {
    background: rgba(59, 130, 246, 0.15);
    color: #93c5fd;
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .warning-badges-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .warning-badge-pill {
    background: rgba(239, 68, 68, 0.15);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.3);
    font-size: 0.72rem;
    font-weight: 600;
  }

  .alternate-mirrors-box {
    background: rgba(0, 0, 0, 0.2);
    border: 1px dashed rgba(255, 255, 255, 0.12);
    border-radius: 6px;
    padding: 0.4rem 0.6rem;
    font-size: 0.78rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .alt-label {
    color: var(--muted, #8b949e);
    font-weight: 600;
  }

  .alt-pills {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .alt-pill {
    background: rgba(255, 255, 255, 0.06);
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    color: #cbd5e1;
  }

  .transport-badge {
    background: rgba(255, 255, 255, 0.08);
    color: #cbd5e1;
  }

  .meta-badge {
    background: rgba(255, 255, 255, 0.05);
    color: var(--muted, #8b949e);
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

  .owned-tag {
    background: rgba(34, 197, 94, 0.2);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .missing-tag {
    background: rgba(239, 68, 68, 0.2);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .upgrade-protection-banner {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    background: rgba(59, 130, 246, 0.08);
    border: 1px solid rgba(59, 130, 246, 0.25);
    border-radius: 8px;
    padding: 10px 14px;
    font-size: 0.85rem;
    color: #93c5fd;
    line-height: 1.4;
  }

  .cand-rank-row {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    margin-bottom: 2px;
  }

  .rank-badge {
    background: var(--accent, #6366f1);
    color: #fff;
    font-weight: 700;
    font-size: 0.75rem;
    padding: 0.1rem 0.4rem;
  }

  .verdict-badge {
    font-size: 0.75rem;
    font-weight: 600;
    text-transform: capitalize;
  }

  .verdict-preferred {
    background: rgba(34, 197, 94, 0.2);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .verdict-alternative {
    background: rgba(59, 130, 246, 0.2);
    color: #93c5fd;
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .verdict-manual_upgrade_only {
    background: rgba(245, 158, 11, 0.2);
    color: #fde68a;
    border: 1px solid rgba(245, 158, 11, 0.3);
  }

  .verdict-already_owned {
    background: rgba(107, 114, 128, 0.2);
    color: #d1d5db;
    border: 1px solid rgba(107, 114, 128, 0.3);
  }

  .verdict-ineligible {
    background: rgba(239, 68, 68, 0.2);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .preferred-provider-badge {
    background: rgba(168, 85, 247, 0.2);
    color: #d8b4fe;
    border: 1px solid rgba(168, 85, 247, 0.3);
    font-size: 0.75rem;
  }

  .res-badge {
    background: rgba(14, 165, 233, 0.2);
    color: #7dd3fc;
    border: 1px solid rgba(14, 165, 233, 0.3);
    font-size: 0.75rem;
  }

  .quality-factors-wrap {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    font-size: 0.78rem;
    margin: 4px 0 6px;
  }

  .factors-label {
    color: var(--muted, #8b949e);
    font-weight: 600;
  }

  .factors-pills {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .factor-pill {
    padding: 0.1rem 0.35rem;
    border-radius: 4px;
    font-size: 0.72rem;
    font-weight: 500;
  }

  .factor-bonus {
    background: rgba(34, 197, 94, 0.15);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.25);
  }

  .factor-penalty {
    background: rgba(239, 68, 68, 0.15);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.25);
  }

  /* Side-by-Side Comparison Styles */
  .compare-recommendation-banner {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    background: rgba(99, 102, 241, 0.1);
    border: 1px solid rgba(99, 102, 241, 0.3);
    border-radius: 8px;
    padding: 12px 14px;
    font-size: 0.88rem;
    color: #c7d2fe;
    line-height: 1.45;
  }

  .rec-text {
    margin: 4px 0 2px;
    color: #e0e7ff;
    font-weight: 500;
  }

  .dup-count-note {
    font-size: 0.78rem;
    color: #a5b4fc;
    font-style: italic;
  }

  .comparison-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 1rem;
    align-items: stretch;
  }

  .comparison-column {
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
  }

  .comparison-column.rank-1 {
    border-color: rgba(34, 197, 94, 0.5);
    background: rgba(34, 197, 94, 0.03);
    box-shadow: 0 0 12px rgba(34, 197, 94, 0.1);
  }

  .col-head {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
    padding-bottom: 0.65rem;
  }

  .col-rank-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .col-cand-title {
    font-size: 0.92rem;
    font-weight: 600;
    line-height: 1.35;
    word-break: break-word;
  }

  .col-cand-score {
    font-size: 0.82rem;
    color: var(--muted, #8b949e);
  }

  .col-card-section {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    font-size: 0.8rem;
    background: rgba(0, 0, 0, 0.15);
    padding: 0.55rem 0.65rem;
    border-radius: 6px;
  }

  .sec-label {
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted, #8b949e);
    margin-bottom: 0.2rem;
  }

  .matrix-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
  }

  .m-k {
    color: var(--muted, #8b949e);
  }

  .m-v {
    font-weight: 500;
  }

  .text-dim {
    font-size: 0.74rem;
    color: #94a3b8;
  }

  .text-success {
    color: #86efac;
  }

  .text-danger {
    color: #fca5a5;
  }

  .text-warning {
    color: #fde68a;
  }

  .signal-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .signal-item {
    display: flex;
    align-items: flex-start;
    gap: 0.35rem;
    font-size: 0.78rem;
    line-height: 1.35;
  }

  .adv-item {
    color: #86efac;
  }

  .dis-item {
    color: #fca5a5;
  }

  .rejection-section {
    background: rgba(239, 68, 68, 0.08);
    border: 1px solid rgba(239, 68, 68, 0.25);
  }

  .rejection-desc {
    margin: 0;
    font-size: 0.78rem;
    color: #fca5a5;
    line-height: 1.4;
  }

  .mirror-mini-card {
    display: flex;
    flex-direction: column;
    font-size: 0.74rem;
    padding: 0.2rem 0;
    border-bottom: 1px dashed rgba(255, 255, 255, 0.08);
  }

  .mirror-mini-card:last-child {
    border-bottom: none;
  }

  .m-prov {
    font-weight: 600;
    color: #cbd5e1;
    text-transform: capitalize;
  }

  .m-sub {
    color: var(--muted, #8b949e);
  }
</style>
