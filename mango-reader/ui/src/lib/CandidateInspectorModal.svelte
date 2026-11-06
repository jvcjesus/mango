<script lang="ts">
  import {
    inspectVolumeCandidates,
    compareVolumeCandidates,
    acquireMangaVolumeCandidate,
    message,
    type VolumeCandidateInspectionResult,
    type CandidateComparisonMatrix,
    type CandidateInspectionDto,
    type CandidateComparisonItem,
    type ApproveCandidateResponse,
  } from './api';
  import Icon from './Icon.svelte';
  import Notice from './Notice.svelte';
  import { formatBytes } from './util';

  let {
    open = false,
    mangaId,
    volume,
    title = '',
    onClose,
  }: {
    open: boolean;
    mangaId: number;
    volume: number;
    title?: string;
    onClose: () => void;
  } = $props();

  let loading = $state(false);
  let error = $state<string | null>(null);
  let data = $state<VolumeCandidateInspectionResult | null>(null);

  let viewMode = $state<'list' | 'compare'>('list');
  let loadingCompare = $state(false);
  let compareError = $state<string | null>(null);
  let comparisonData = $state<CandidateComparisonMatrix | null>(null);

  let reviewingCandidate = $state<CandidateInspectionDto | CandidateComparisonItem | null>(null);
  let previewingPlan = $state(false);
  let planPreviewResult = $state<ApproveCandidateResponse | null>(null);
  let planPreviewError = $state<string | null>(null);
  let blockReasonModalCandidate = $state<CandidateInspectionDto | CandidateComparisonItem | null>(null);

  function getCacheBadge(cand: CandidateInspectionDto | CandidateComparisonItem): { label: string; cls: string; icon: 'check' | 'clock' | 'info' | 'download' } | null {
    if (cand.cacheStatus === 'cached') {
      return { label: 'TorBox Cached', cls: 'cache-cached', icon: 'check' };
    } else if (cand.cacheStatus === 'uncached') {
      return { label: 'TorBox Uncached', cls: 'cache-uncached', icon: 'clock' };
    } else if (cand.cacheStatus === 'unavailable') {
      return { label: 'TorBox Unavailable', cls: 'cache-unavailable', icon: 'info' };
    } else if (cand.cacheStatus === 'unknown') {
      return { label: 'TorBox Cache Unknown', cls: 'cache-unknown', icon: 'info' };
    } else if (cand.provider.toLowerCase().includes('madokami') || cand.transport.toLowerCase().includes('http')) {
      return { label: 'Direct HTTP Available', cls: 'cache-direct', icon: 'download' };
    }
    return null;
  }

  function isCandidateEligible(cand: CandidateInspectionDto | CandidateComparisonItem): boolean {
    if ('autoAcquireEligible' in cand) {
      return cand.autoAcquireEligible;
    }
    if (cand.hardEligibility) {
      return cand.hardEligibility.isEligible;
    }
    return cand.selectionVerdict !== 'BLOCKED' && cand.selectionVerdict !== 'INELIGIBLE';
  }

  function getRejectionDetails(cand: CandidateInspectionDto | CandidateComparisonItem) {
    const code = cand.hardEligibility?.rejectionCode || ('rejectionCode' in cand ? cand.rejectionCode : null) || 'BLOCKED';
    const reason = cand.hardEligibility?.rejectionReason || cand.rejectionReason || 'Requires manual confirmation or review before acquisition.';
    return { code, reason };
  }

  async function previewPlanAction(cand: CandidateInspectionDto | CandidateComparisonItem) {
    previewingPlan = true;
    planPreviewError = null;
    planPreviewResult = null;
    try {
      const res = await acquireMangaVolumeCandidate(mangaId, volume, cand.candidateId, { dryRun: true });
      planPreviewResult = res;
    } catch (e) {
      planPreviewError = message(e);
    } finally {
      previewingPlan = false;
    }
  }

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
      reviewingCandidate = null;
      planPreviewResult = null;
      planPreviewError = null;
      blockReasonModalCandidate = null;
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
          <h2>
            {#if reviewingCandidate}
              Manual Acquisition Review — Volume {volume}
            {:else}
              Candidate Inspector — Volume {volume}
            {/if}
          </h2>
        </div>
        <div class="head-actions">
          {#if !reviewingCandidate}
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
          {/if}
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
          {#if reviewingCandidate}
            <div class="acquisition-review-pane">
              <div class="review-header-bar">
                <button
                  class="btn ghost btn-sm back-nav-btn"
                  onclick={() => {
                    reviewingCandidate = null;
                    planPreviewResult = null;
                    planPreviewError = null;
                  }}
                >
                  <Icon name="back" size={16} />
                  <span>Back to Candidates</span>
                </button>
                <span class="badge review-step-badge">Manual Review & Plan Preview</span>
              </div>

              <div class="dry-run-banner">
                <Icon name="info" size={22} />
                <div class="dry-run-content">
                  <span class="dry-run-title">DRY-RUN SAFETY ACTIVE</span>
                  <span class="dry-run-desc">
                    Real acquisitions and remote jobs are simulated only. No torrents will be submitted to TorBox and no files will be downloaded.
                  </span>
                </div>
              </div>

              <div class="review-details-card">
                <h3 class="review-card-title">Acquisition Target Specifications</h3>

                <div class="review-specs-grid">
                  <div class="spec-row">
                    <span class="spec-label">Target Volume</span>
                    <span class="spec-val">Volume {volume}</span>
                  </div>
                  <div class="spec-row">
                    <span class="spec-label">Selected Release</span>
                    <span class="spec-val highlight-val">{reviewingCandidate.title || reviewingCandidate.rawTitle}</span>
                  </div>
                  {#if reviewingCandidate.rawTitle && reviewingCandidate.rawTitle !== reviewingCandidate.title}
                    <div class="spec-row">
                      <span class="spec-label">Raw Release Title</span>
                      <span class="spec-val code-val"><code>{reviewingCandidate.rawTitle}</code></span>
                    </div>
                  {/if}
                  <div class="spec-row">
                    <span class="spec-label">Provider & Backend</span>
                    <span class="spec-val">
                      <span class="badge provider-badge">{reviewingCandidate.provider}</span>
                      <span class="badge transport-badge">{reviewingCandidate.transport}</span>
                    </span>
                  </div>
                  <div class="spec-row">
                    <span class="spec-label">Source Availability</span>
                    <span class="spec-val">
                      {#if getCacheBadge(reviewingCandidate)}
                        {@const badge = getCacheBadge(reviewingCandidate)!}
                        <span class="badge {badge.cls}">
                          <Icon name={badge.icon} size={12} />
                          {badge.label}
                        </span>
                      {:else}
                        <span class="badge meta-badge">Standard Remote</span>
                      {/if}
                    </span>
                  </div>
                  {#if reviewingCandidate.sizeBytes}
                    <div class="spec-row">
                      <span class="spec-label">Archive Size</span>
                      <span class="spec-val">{formatBytes(reviewingCandidate.sizeBytes)}</span>
                    </div>
                  {/if}
                  <div class="spec-row">
                    <span class="spec-label">Current Komga State</span>
                    <span class="spec-val">
                      {#if data.isCurrentlyOwned}
                        <span class="badge owned-tag">Owned in Komga</span>
                      {:else}
                        <span class="badge missing-tag">Not Owned in Komga</span>
                      {/if}
                    </span>
                  </div>
                  <div class="spec-row">
                    <span class="spec-label">Expected Staging Destination</span>
                    <span class="spec-val code-val"><code>[library_dir]/_staging/vol_{volume}_{reviewingCandidate.candidateId.replace(/[^a-zA-Z0-9_-]/g, '_')}/</code></span>
                  </div>
                  <div class="spec-row">
                    <span class="spec-label">Expected Canonical Publication</span>
                    <span class="spec-val code-val"><code>[library_dir]/{data.seriesId.replace(/^title:/, '')} - v{String(volume).padStart(2, '0')}.cbz</code></span>
                  </div>
                  {#if reviewingCandidate.rankingBreakdown}
                    <div class="spec-row">
                      <span class="spec-label">Normalized Ranking Score</span>
                      <span class="spec-val">
                        <strong>{reviewingCandidate.rankingBreakdown.finalScore.toFixed(1)} / 100</strong>
                        <span class="confidence-tag">({reviewingCandidate.rankingBreakdown.confidencePercent}% confidence)</span>
                      </span>
                    </div>
                  {/if}
                </div>
              </div>

              {#if planPreviewError}
                <div class="plan-preview-error">
                  <Notice error={planPreviewError} />
                </div>
              {/if}

              {#if planPreviewResult}
                <div class="plan-preview-box">
                  <div class="preview-box-head">
                    <div class="status-indicator">
                      <Icon name="check" size={18} />
                      <strong>Plan Preview Generated ({planPreviewResult.status})</strong>
                    </div>
                    <span class="badge dry-run-pill">Dry-Run Validated</span>
                  </div>
                  <p class="preview-msg">{planPreviewResult.message}</p>
                  <div class="preview-meta-rows">
                    <div class="p-meta">
                      <span class="p-k">Already Owned:</span>
                      <span class="p-v">{planPreviewResult.alreadyOwned ? 'Yes' : 'No'}</span>
                    </div>
                    <div class="p-meta">
                      <span class="p-k">Already Acquiring:</span>
                      <span class="p-v">{planPreviewResult.alreadyAcquiring ? 'Yes' : 'No'}</span>
                    </div>
                    <div class="p-meta">
                      <span class="p-k">Execution ID:</span>
                      <span class="p-v">{planPreviewResult.executionId || 'Inhibited (dry-run preview only)'}</span>
                    </div>
                  </div>
                  <p class="preview-safety-footer">
                    <Icon name="clock" size={13} />
                    <span>Execution was inhibited. No remote download task or TorBox cache job was dispatched.</span>
                  </p>
                </div>
              {/if}

              <div class="review-actions-bar">
                <button
                  class="btn secondary preview-btn"
                  onclick={() => previewPlanAction(reviewingCandidate!)}
                  disabled={previewingPlan}
                >
                  {#if previewingPlan}
                    <span class="spin"><Icon name="spinner" size={15} stroke={3} /></span>
                    <span>Generating Preview Plan…</span>
                  {:else}
                    <Icon name="book" size={16} />
                    <span>Preview Plan</span>
                  {/if}
                </button>

                <button
                  class="btn primary disabled-action-btn"
                  disabled
                  title="Disabled: Dry-run safety mode is active. Real acquisitions and remote jobs are disabled."
                >
                  <Icon name="download" size={16} />
                  <span>Submit Acquisition</span>
                </button>

                <button
                  class="btn ghost cancel-review-btn"
                  onclick={() => {
                    reviewingCandidate = null;
                    planPreviewResult = null;
                    planPreviewError = null;
                  }}
                >
                  Back to Candidates
                </button>
              </div>
            </div>
          {:else}
            <div class="target-summary-box">
              <div class="target-stat">
                <span class="label">Target</span>
                <span class="value">Volume {data.volumeNumber}</span>
              </div>
              <div class="target-stat">
                <span class="label">Release Status</span>
                <span class="value tag status-{data.targetStatus || 'unknown'}">
                  {data.targetStatus === 'released' ? 'Officially Released' : (data.targetStatus || 'Unscheduled')}
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
                            {#if getCacheBadge(cand)}
                              {@const badge = getCacheBadge(cand)!}
                              <span class="badge {badge.cls}">
                                <Icon name={badge.icon} size={11} />
                                {badge.label}
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
                        <div class="cand-score-tag" title="Normalized multi-factor score">
                          Score: {cand.rankingBreakdown ? cand.rankingBreakdown.finalScore.toFixed(1) : cand.score.toFixed(0)} / 100
                          {#if cand.rankingBreakdown?.confidencePercent}
                            <span class="cand-confidence-pill">{cand.rankingBreakdown.confidencePercent}% conf</span>
                          {/if}
                        </div>
                      </div>

                      {#if cand.rankingBreakdown}
                        <div class="ranking-breakdown-bar">
                          <div class="breakdown-header">
                            <span class="breakdown-title">Two-Stage Scoring Breakdown:</span>
                            {#if cand.rankingBreakdown.isProvisional}
                              <span class="badge prov-badge">Provisional</span>
                            {/if}
                          </div>
                          <div class="breakdown-chips">
                            <span class="b-chip" title="Quality Signals (bonus/penalty): max 35 pts">Quality: <strong>{cand.rankingBreakdown.qualityScore.toFixed(0)}</strong>/35</span>
                            <span class="b-chip" title="Acquisition Reliability (direct provider, seeds): max 25 pts">Reliability: <strong>{cand.rankingBreakdown.reliabilityScore.toFixed(0)}</strong>/25</span>
                            <span class="b-chip" title="User and library preferences: max 20 pts">Preference: <strong>{cand.rankingBreakdown.preferenceScore.toFixed(0)}</strong>/20</span>
                            <span class="b-chip" title="Acquisition Convenience (cached/direct): max 15 pts">Convenience: <strong>{cand.rankingBreakdown.convenienceScore.toFixed(0)}</strong>/15</span>
                            <span class="b-chip" title="Storage Efficiency: max 5 pts">Storage: <strong>{cand.rankingBreakdown.storageEfficiencyScore.toFixed(0)}</strong>/5</span>
                          </div>
                          {#if cand.rankingBreakdown.whyRanksAbove}
                            <div class="why-ranks-above-box">
                              <Icon name="check" size={13} />
                              <span>{cand.rankingBreakdown.whyRanksAbove}</span>
                            </div>
                          {/if}
                        </div>
                      {/if}

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
                            <strong>Auto-Acquisition Policy: BLOCKED ({cand.hardEligibility?.rejectionCode || cand.rejectionCode || 'REJECTED'})</strong>
                          </div>
                          <p class="eligibility-desc">{cand.hardEligibility?.rejectionReason || cand.rejectionReason || 'Failed hard candidate eligibility gate; manual review required.'}</p>
                        {/if}
                      </div>

                      {#if cand.sanitizedReference}
                        <div class="cand-sanitized-ref">
                          <span class="ref-label">Sanitized Reference (No Secrets):</span>
                          <code class="ref-code">{cand.sanitizedReference}</code>
                        </div>
                      {/if}

                      <div class="cand-actions-row">
                        {#if isCandidateEligible(cand)}
                          <button
                            class="btn primary btn-sm select-cand-btn"
                            onclick={() => {
                              reviewingCandidate = cand;
                              planPreviewResult = null;
                              planPreviewError = null;
                            }}
                          >
                            <Icon name="arrow" size={14} />
                            <span>Review Acquisition</span>
                          </button>
                        {:else}
                          <button
                            class="btn secondary btn-sm block-reason-btn"
                            onclick={() => {
                              blockReasonModalCandidate = cand;
                            }}
                          >
                            <Icon name="info" size={14} />
                            <span>View Block Reason</span>
                          </button>
                        {/if}
                      </div>
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
                            {#if getCacheBadge(cand)}
                              {@const badge = getCacheBadge(cand)!}
                              <span class="badge {badge.cls}">
                                <Icon name={badge.icon} size={11} />
                                {badge.label}
                              </span>
                            {/if}
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

                        <div class="col-actions-wrap">
                          {#if isCandidateEligible(cand)}
                            <button
                              class="btn primary btn-sm select-cand-btn"
                              onclick={() => {
                                reviewingCandidate = cand;
                                planPreviewResult = null;
                                planPreviewError = null;
                              }}
                            >
                              <Icon name="arrow" size={14} />
                              <span>Review Acquisition</span>
                            </button>
                          {:else}
                            <button
                              class="btn secondary btn-sm block-reason-btn"
                              onclick={() => {
                                blockReasonModalCandidate = cand;
                              }}
                            >
                              <Icon name="info" size={14} />
                              <span>View Block Reason</span>
                            </button>
                          {/if}
                        </div>
                      </div>
                    {/each}
                  </div>
                {/if}
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

{#if blockReasonModalCandidate}
  {@const details = getRejectionDetails(blockReasonModalCandidate)}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="block-modal-backdrop" onclick={() => (blockReasonModalCandidate = null)}>
    <div class="modal-card block-details-card" onclick={(e) => e.stopPropagation()}>
      <div class="block-details-head">
        <div class="b-title">
          <Icon name="info" size={18} />
          <h3>Candidate Eligibility Details</h3>
        </div>
        <button class="btn icon close-btn" onclick={() => (blockReasonModalCandidate = null)}>
          <Icon name="close" size={16} />
        </button>
      </div>

      <div class="block-details-body">
        <div class="b-release-name">
          <span class="label">Release</span>
          <strong>{blockReasonModalCandidate.title || blockReasonModalCandidate.rawTitle}</strong>
        </div>

        <div class="b-code-box">
          <span class="b-gate-label">Gate Rejection Code</span>
          <span class="badge verdict-blocked">{details.code}</span>
        </div>

        <div class="b-reason-box">
          <span class="b-gate-label">Disqualification Reason</span>
          <p class="b-reason-text">{details.reason}</p>
        </div>

        <div class="b-policy-note">
          <Icon name="info" size={14} />
          <p>
            Mango enforces strict single-volume integrity. Multi-volume packs, chapter-only releases, and unconfirmed ranges require manual selection and cannot be automatically or silently acquired.
          </p>
        </div>
      </div>

      <div class="modal-foot">
        <button class="btn" onclick={() => (blockReasonModalCandidate = null)}>Close</button>
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

  .cand-confidence-pill {
    font-size: 0.68rem;
    font-weight: 600;
    padding: 0.1rem 0.35rem;
    border-radius: 4px;
    background: rgba(59, 130, 246, 0.2);
    color: #93c5fd;
    margin-left: 0.4rem;
    vertical-align: middle;
  }

  /* Review Pane */
  .acquisition-review-pane {
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
  }

  .review-header-bar {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .back-nav-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.82rem;
  }

  .review-step-badge {
    background: rgba(59, 130, 246, 0.15);
    color: #93c5fd;
    border: 1px solid rgba(59, 130, 246, 0.35);
    font-weight: 600;
  }

  .dry-run-banner {
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.9rem 1rem;
    background: rgba(245, 158, 11, 0.12);
    border: 1px solid rgba(245, 158, 11, 0.35);
    border-radius: 8px;
    color: #fef3c7;
  }

  .dry-run-content {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
  }

  .dry-run-title {
    font-weight: 700;
    font-size: 0.88rem;
    letter-spacing: 0.03em;
    color: #fde68a;
  }

  .dry-run-desc {
    font-size: 0.8rem;
    color: #fef08a;
    line-height: 1.4;
  }

  .review-details-card {
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .review-card-title {
    margin: 0;
    font-size: 0.92rem;
    font-weight: 700;
    color: #e2e8f0;
  }

  .review-specs-grid {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .spec-row {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    gap: 1rem;
    font-size: 0.82rem;
    padding-bottom: 0.35rem;
    border-bottom: 1px solid rgba(255, 255, 255, 0.05);
  }

  .spec-row:last-child {
    border-bottom: none;
    padding-bottom: 0;
  }

  .spec-label {
    color: var(--muted, #8b949e);
    flex-shrink: 0;
  }

  .spec-val {
    color: #f1f5f9;
    text-align: right;
    word-break: break-word;
  }

  .highlight-val {
    font-weight: 600;
    color: #60a5fa;
  }

  .code-val code {
    font-size: 0.76rem;
    background: rgba(0, 0, 0, 0.3);
    padding: 0.15rem 0.35rem;
    border-radius: 4px;
    color: #cbd5e1;
  }

  .confidence-tag {
    font-size: 0.76rem;
    color: #94a3b8;
    margin-left: 0.35rem;
  }

  .plan-preview-box {
    background: rgba(34, 197, 94, 0.08);
    border: 1px solid rgba(34, 197, 94, 0.3);
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }

  .preview-box-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .status-indicator {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    color: #86efac;
    font-size: 0.9rem;
  }

  .dry-run-pill {
    background: rgba(34, 197, 94, 0.2);
    color: #86efac;
    font-weight: 600;
  }

  .preview-msg {
    margin: 0;
    font-size: 0.82rem;
    color: #bbf7d0;
  }

  .preview-meta-rows {
    display: flex;
    gap: 1.5rem;
    flex-wrap: wrap;
    font-size: 0.8rem;
    background: rgba(0, 0, 0, 0.2);
    padding: 0.5rem 0.75rem;
    border-radius: 6px;
  }

  .p-meta {
    display: flex;
    gap: 0.35rem;
  }

  .p-k {
    color: #94a3b8;
  }

  .p-v {
    color: #f1f5f9;
    font-weight: 500;
  }

  .preview-safety-footer {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0;
    font-size: 0.76rem;
    color: #86efac;
  }

  .review-actions-bar {
    display: flex;
    gap: 0.75rem;
    align-items: center;
    padding-top: 0.5rem;
  }

  .disabled-action-btn {
    opacity: 0.5;
    cursor: not-allowed;
  }

  /* Cache Badges */
  .cache-cached {
    background: rgba(34, 197, 94, 0.15);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.35);
  }

  .cache-uncached {
    background: rgba(245, 158, 11, 0.15);
    color: #fde68a;
    border: 1px solid rgba(245, 158, 11, 0.35);
  }

  .cache-unavailable {
    background: rgba(239, 68, 68, 0.15);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.35);
  }

  .cache-unknown {
    background: rgba(148, 163, 184, 0.15);
    color: #cbd5e1;
    border: 1px solid rgba(148, 163, 184, 0.35);
  }

  .cache-direct {
    background: rgba(6, 182, 212, 0.15);
    color: #67e8f9;
    border: 1px solid rgba(6, 182, 212, 0.35);
  }

  /* Scoring Breakdown Bar */
  .ranking-breakdown-bar {
    background: rgba(0, 0, 0, 0.25);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 6px;
    padding: 0.55rem 0.75rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    margin-top: 0.4rem;
  }

  .breakdown-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .breakdown-title {
    font-size: 0.74rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: #94a3b8;
  }

  .prov-badge {
    background: rgba(245, 158, 11, 0.2);
    color: #fde68a;
    font-size: 0.68rem;
  }

  .breakdown-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .b-chip {
    font-size: 0.74rem;
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.06);
    color: #cbd5e1;
  }

  .b-chip strong {
    color: #f1f5f9;
  }

  .why-ranks-above-box {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.75rem;
    color: #86efac;
    padding-top: 0.2rem;
  }

  .cand-actions-row {
    display: flex;
    justify-content: flex-end;
    padding-top: 0.4rem;
  }

  .select-cand-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-weight: 600;
  }

  .block-reason-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.78rem;
  }

  .col-actions-wrap {
    margin-top: auto;
    padding-top: 0.75rem;
    display: flex;
    justify-content: center;
  }

  /* Block Reason Modal */
  .block-modal-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.7);
    z-index: 10005;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1rem;
  }

  .block-details-card {
    background: #1e1e24;
    border: 1px solid rgba(239, 68, 68, 0.4);
    border-radius: 10px;
    max-width: 480px;
    width: 100%;
    padding: 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .block-details-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .b-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    color: #fca5a5;
  }

  .b-title h3 {
    margin: 0;
    font-size: 1rem;
    color: #f87171;
  }

  .block-details-body {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .b-gate-label {
    display: block;
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: #94a3b8;
    margin-bottom: 0.25rem;
  }

  .b-release-name {
    font-size: 0.84rem;
    color: #cbd5e1;
  }

  .b-release-name .label {
    display: block;
    font-size: 0.72rem;
    color: #94a3b8;
  }

  .b-code-box {
    margin-bottom: 0.25rem;
  }

  .b-reason-box {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.25);
    border-radius: 6px;
    padding: 0.65rem 0.75rem;
  }

  .b-reason-text {
    margin: 0;
    font-size: 0.82rem;
    color: #fca5a5;
    line-height: 1.4;
  }

  .b-policy-note {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    font-size: 0.76rem;
    color: #94a3b8;
    line-height: 1.35;
    background: rgba(255, 255, 255, 0.04);
    padding: 0.5rem 0.65rem;
    border-radius: 6px;
  }

  .b-policy-note p {
    margin: 0;
  }
</style>
