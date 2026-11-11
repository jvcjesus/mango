<script lang="ts">
  import {
    inspectVolumeCandidates,
    compareVolumeCandidates,
    acquireMangaVolumeCandidate,
    getManualAcquisitionReview,
    getAcquisitionSettings,
    message,
    type VolumeCandidateInspectionResult,
    type CandidateComparisonMatrix,
    type CandidateInspectionDto,
    type CandidateComparisonItem,
    type ApproveCandidateResponse,
    type ManualAcquisitionReviewDto,
  } from './api';
  import Icon from './Icon.svelte';
  import Notice from './Notice.svelte';
  import { toast } from './toast.svelte';
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

  let viewMode = $state<'dialog' | 'list' | 'compare'>('dialog');
  let isSimulationMode = $state(true);
  let loadingCompare = $state(false);
  let compareError = $state<string | null>(null);
  let comparisonData = $state<CandidateComparisonMatrix | null>(null);

  let selectedCandidate = $state<CandidateInspectionDto | CandidateComparisonItem | null>(null);
  let reviewingCandidate = $state<CandidateInspectionDto | CandidateComparisonItem | null>(null);
  let reviewDto = $state<ManualAcquisitionReviewDto | null>(null);
  let reviewLoading = $state(false);
  let reviewError = $state<string | null>(null);

  let previewingPlan = $state(false);
  let planPreviewResult = $state<ApproveCandidateResponse | null>(null);
  let planPreviewError = $state<string | null>(null);

  let submittingExecution = $state(false);
  let executionResult = $state<ApproveCandidateResponse | null>(null);
  let executionError = $state<string | null>(null);

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

  function selectCandidateForReview(cand: CandidateInspectionDto | CandidateComparisonItem) {
    selectedCandidate = cand;
    reviewingCandidate = cand;
    viewMode = 'dialog';
    planPreviewResult = null;
    planPreviewError = null;
    executionResult = null;
    executionError = null;
  }

  async function requestReview(cand: CandidateInspectionDto | CandidateComparisonItem) {
    reviewLoading = true;
    reviewError = null;
    try {
      reviewDto = await getManualAcquisitionReview(mangaId, volume, cand.candidateId);
    } catch (e) {
      reviewError = message(e);
    } finally {
      reviewLoading = false;
    }
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

  async function confirmAcquisitionAction(cand: CandidateInspectionDto | CandidateComparisonItem) {
    submittingExecution = true;
    executionError = null;
    executionResult = null;
    try {
      const res = await acquireMangaVolumeCandidate(mangaId, volume, cand.candidateId, {
        dryRun: isSimulationMode,
      });
      executionResult = res;
      if (res.status === 'queued') {
        toast.success(`Download queued for Volume ${volume}`);
      } else if (res.status === 'simulated' || res.status === 'dry_run_preview') {
        toast.info(res.message);
      } else if (res.alreadyOwned) {
        toast.info(`Volume ${volume} is already owned in Komga library`);
      } else if (res.alreadyAcquiring) {
        toast.info(`Volume ${volume} is already being acquired`);
      }
    } catch (e) {
      executionError = message(e);
      toast.error(executionError);
    } finally {
      submittingExecution = false;
    }
  }

  function clearReviewState() {
    reviewingCandidate = null;
    reviewDto = null;
    reviewError = null;
    planPreviewResult = null;
    planPreviewError = null;
    executionResult = null;
    executionError = null;
  }

  async function load() {
    if (!open || !mangaId || volume == null) return;
    loading = true;
    error = null;
    try {
      const [candData, acqCfg] = await Promise.all([
        inspectVolumeCandidates(mangaId, volume),
        getAcquisitionSettings().catch(() => null),
      ]);
      data = candData;
      if (acqCfg) {
        isSimulationMode = acqCfg.operatingMode !== 'normal' || !!acqCfg.autoAcquireDryRun;
      }
      if (candData?.topCandidate) {
        selectedCandidate = candData.topCandidate;
      } else if (candData?.candidates?.length) {
        selectedCandidate = candData.candidates[0];
      }
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
      viewMode = 'dialog';
      selectedCandidate = null;
      clearReviewState();
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
            {#if viewMode === 'dialog'}
              Acquire Volume {volume}
            {:else if viewMode === 'list'}
              Select Release — Volume {volume}
            {:else}
              Compare Releases — Volume {volume}
            {/if}
          </h2>
        </div>
        <div class="head-actions">
          <div class="view-toggle">
            <button
              class="toggle-btn"
              class:active={viewMode === 'dialog'}
              onclick={() => (viewMode = 'dialog')}
              disabled={!selectedCandidate}
            >
              <Icon name="book" size={14} />
              <span>Recommended</span>
            </button>
            <button
              class="toggle-btn"
              class:active={viewMode === 'list'}
              onclick={() => (viewMode = 'list')}
            >
              <Icon name="list" size={14} />
              <span>All Releases</span>
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
          {#if viewMode === 'dialog'}
            <div class="compact-acquisition-dialog">
              {#if isSimulationMode}
                <div class="simulation-banner">
                  <Icon name="info" size={18} />
                  <div class="banner-text">
                    <strong>Simulation Mode Active</strong>
                    <span>Downloads are simulated previews. No files will be retrieved from the network or published to library storage.</span>
                  </div>
                </div>
              {/if}

              {#if executionResult}
                <div class="result-banner" class:success={executionResult.status === 'queued'} class:sim={executionResult.status === 'simulated' || executionResult.status === 'dry_run_preview'}>
                  <Icon name="check" size={20} />
                  <div>
                    <strong>{executionResult.status === 'queued' ? 'Acquisition Queued' : 'Download Simulated'}</strong>
                    <p>{executionResult.message}</p>
                  </div>
                </div>
              {/if}

              {#if executionError}
                <div class="error-banner">
                  <Notice error={executionError} />
                </div>
              {/if}

              {#if !selectedCandidate}
                <div class="empty-candidates">
                  <Icon name="search" size={32} />
                  <p>No candidates have been discovered for Volume {volume}.</p>
                </div>
              {:else}
                <div class="release-card">
                  <div class="release-header">
                    <div class="release-title-col">
                      <span class="release-target-label">Volume {volume} · {title || data.seriesId.replace(/^title:/, '')}</span>
                      <h3 class="release-name">{selectedCandidate.title || selectedCandidate.rawTitle}</h3>
                    </div>
                    <div class="score-pill-wrap">
                      <span class="score-pill">
                        {selectedCandidate.rankingBreakdown ? selectedCandidate.rankingBreakdown.finalScore.toFixed(0) : ('totalScore' in selectedCandidate ? selectedCandidate.totalScore.toFixed(0) : ('score' in selectedCandidate ? selectedCandidate.score.toFixed(0) : '0'))}% Match
                      </span>
                    </div>
                  </div>

                  <div class="release-specs-grid">
                    <div class="spec-item">
                      <span class="spec-k">Provider</span>
                      <span class="spec-v">
                        <span class="badge provider-badge {selectedCandidate.provider}">{selectedCandidate.provider === 'nyaa' ? 'TorBox Debrid' : 'Madokami HTTP'}</span>
                      </span>
                    </div>
                    <div class="spec-item">
                      <span class="spec-k">Format</span>
                      <span class="spec-v">{('isDigital' in selectedCandidate && selectedCandidate.isDigital) || ('format' in selectedCandidate && selectedCandidate.format?.toLowerCase().includes('digital')) ? 'Digital CBZ' : 'CBZ Archive'}</span>
                    </div>
                    <div class="spec-item">
                      <span class="spec-k">Language</span>
                      <span class="spec-v">English</span>
                    </div>
                    <div class="spec-item">
                      <span class="spec-k">Size</span>
                      <span class="spec-v">{selectedCandidate.sizeBytes ? formatBytes(selectedCandidate.sizeBytes) : '—'}</span>
                    </div>
                    <div class="spec-item">
                      <span class="spec-k">Availability</span>
                      <span class="spec-v">
                        {#if getCacheBadge(selectedCandidate)}
                          {@const badge = getCacheBadge(selectedCandidate)!}
                          <span class="badge {badge.cls}"><Icon name={badge.icon} size={11} /> {badge.label}</span>
                        {:else}
                          <span class="badge meta-badge">Available</span>
                        {/if}
                      </span>
                    </div>
                    <div class="spec-item">
                      <span class="spec-k">Library Ownership</span>
                      <span class="spec-v">
                        {#if data.isCurrentlyOwned}
                          <span class="badge owned-tag"><Icon name="check" size={11} /> Owned in Komga</span>
                        {:else}
                          <span class="badge missing-tag">Missing (Ready to Acquire)</span>
                        {/if}
                      </span>
                    </div>
                  </div>

                  <!-- Advanced Details Collapsible -->
                  <details class="advanced-collapsible">
                    <summary class="advanced-summary">
                      <Icon name="sliders" size={13} />
                      <span>Advanced Details</span>
                    </summary>
                    <div class="advanced-content">
                      {#if selectedCandidate.rawTitle}
                        <div class="adv-row">
                          <span class="adv-label">Raw Release Title:</span>
                          <code class="adv-code">{selectedCandidate.rawTitle}</code>
                        </div>
                      {/if}
                      {#if selectedCandidate.rankingBreakdown}
                        <div class="adv-row">
                          <span class="adv-label">Scoring Breakdown:</span>
                          <div class="breakdown-chips">
                            <span class="b-chip">Quality: {selectedCandidate.rankingBreakdown.qualityScore.toFixed(0)}/35</span>
                            <span class="b-chip">Reliability: {selectedCandidate.rankingBreakdown.reliabilityScore.toFixed(0)}/25</span>
                            <span class="b-chip">Preference: {selectedCandidate.rankingBreakdown.preferenceScore.toFixed(0)}/20</span>
                            <span class="b-chip">Convenience: {selectedCandidate.rankingBreakdown.convenienceScore.toFixed(0)}/15</span>
                            <span class="b-chip">Storage: {selectedCandidate.rankingBreakdown.storageEfficiencyScore.toFixed(0)}/5</span>
                          </div>
                        </div>
                        {#if selectedCandidate.rankingBreakdown.verifiedEvidence && selectedCandidate.rankingBreakdown.verifiedEvidence.length > 0}
                          <div class="adv-row">
                            <span class="adv-label">Verified Signals:</span>
                            <div class="evidence-pills">
                              {#each selectedCandidate.rankingBreakdown.verifiedEvidence as item}
                                <span class="badge evidence-tag">{item}</span>
                              {/each}
                            </div>
                          </div>
                        {/if}
                      {/if}
                      <div class="adv-row">
                        <span class="adv-label">Canonical Publication Target:</span>
                        <code class="adv-code">[library_dir]/{data.seriesId.replace(/^title:/, '')}/{data.seriesId.replace(/^title:/, '')} - v{String(volume).padStart(2, '0')}.cbz</code>
                      </div>
                    </div>
                  </details>
                </div>

                <!-- Dialog Actions -->
                <div class="dialog-actions-bar">
                  {#if data.isCurrentlyOwned}
                    <button class="btn disabled" disabled title="This volume is already verified and owned in your Komga permanent library">
                      <Icon name="check" size={16} />
                      <span>Already Owned in Komga</span>
                    </button>
                  {:else}
                    <button
                      class="btn primary download-main-btn"
                      disabled={submittingExecution}
                      onclick={() => confirmAcquisitionAction(selectedCandidate!)}
                    >
                      {#if submittingExecution}
                        <span class="spin"><Icon name="spinner" size={15} stroke={3} /></span>
                        <span>Queuing Download…</span>
                      {:else}
                        <Icon name={isSimulationMode ? 'eye' : 'download'} size={16} />
                        <span>{isSimulationMode ? 'Simulate Download' : 'Download'} Volume {volume}</span>
                      {/if}
                    </button>
                  {/if}

                  <button
                    class="btn secondary change-release-btn"
                    onclick={() => (viewMode = 'list')}
                    disabled={submittingExecution}
                  >
                    <Icon name="sliders" size={15} />
                    <span>Change Release ({data.candidates.length})</span>
                  </button>

                  <button class="btn ghost cancel-btn" onclick={onClose} disabled={submittingExecution}>
                    Cancel
                  </button>
                </div>
              {/if}
            </div>
          {:else}
            <div class="releases-browse-header">
              <button class="btn ghost btn-sm" onclick={() => (viewMode = 'dialog')}>
                <Icon name="back" size={14} />
                <span>Back to Recommended Release</span>
              </button>
              <span class="subtext">Showing {data.candidates.length} discovered release(s) for Volume {volume}</span>
            </div>
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
                            <div class="breakdown-header-badges">
                              {#if cand.rankingBreakdown.isProvisional}
                                <span class="badge prov-badge" title="Pre-download confidence capped at 80% until archive inspection">Provisional (80% Max)</span>
                              {/if}
                              {#if cand.rankingBreakdown.evidenceCoveragePercent != null}
                                <span class="badge coverage-badge" class:coverage-high={cand.rankingBreakdown.evidenceCoveragePercent >= 70} class:coverage-partial={cand.rankingBreakdown.evidenceCoveragePercent < 70}>
                                  {cand.rankingBreakdown.evidenceCoveragePercent}% Evidence Coverage
                                </span>
                              {/if}
                            </div>
                          </div>
                          <div class="breakdown-chips">
                            <span class="b-chip" title="Quality Signals (bonus/penalty): max 35 pts">Quality: <strong>{cand.rankingBreakdown.qualityScore.toFixed(0)}</strong>/35</span>
                            <span class="b-chip" title="Acquisition Reliability (direct provider, seeds): max 25 pts">Reliability: <strong>{cand.rankingBreakdown.reliabilityScore.toFixed(0)}</strong>/25</span>
                            <span class="b-chip" title="User and library preferences: max 20 pts">Preference: <strong>{cand.rankingBreakdown.preferenceScore.toFixed(0)}</strong>/20</span>
                            <span class="b-chip" title="Acquisition Convenience (cached/direct): max 15 pts">Convenience: <strong>{cand.rankingBreakdown.convenienceScore.toFixed(0)}</strong>/15</span>
                            <span class="b-chip" title="Storage Efficiency: max 5 pts">Storage: <strong>{cand.rankingBreakdown.storageEfficiencyScore.toFixed(0)}</strong>/5</span>
                          </div>
                          {#if (cand.rankingBreakdown.verifiedEvidence && cand.rankingBreakdown.verifiedEvidence.length > 0) || (cand.rankingBreakdown.heuristicEstimates && cand.rankingBreakdown.heuristicEstimates.length > 0)}
                            <div class="evidence-honesty-row">
                              {#if cand.rankingBreakdown.verifiedEvidence && cand.rankingBreakdown.verifiedEvidence.length > 0}
                                <div class="evidence-group">
                                  <span class="evidence-label verified"><Icon name="check" size={11} /> Verified:</span>
                                  {#each cand.rankingBreakdown.verifiedEvidence as item}
                                    <span class="badge evidence-verified-tag">{item}</span>
                                  {/each}
                                </div>
                              {/if}
                              {#if cand.rankingBreakdown.heuristicEstimates && cand.rankingBreakdown.heuristicEstimates.length > 0}
                                <div class="evidence-group">
                                  <span class="evidence-label heuristic"><Icon name="info" size={11} /> Heuristic:</span>
                                  {#each cand.rankingBreakdown.heuristicEstimates as item}
                                    <span class="badge evidence-heuristic-tag">{item}</span>
                                  {/each}
                                </div>
                              {/if}
                            </div>
                          {/if}
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
                            onclick={() => selectCandidateForReview(cand)}
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
                              onclick={() => selectCandidateForReview(cand)}
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
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(3px);
    z-index: 9999;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1.25rem;
  }

  .inspect-modal {
    background: var(--surface);
    color: var(--ink);
    border: 3px solid var(--ink);
    border-radius: 4px;
    box-shadow: var(--sh-8);
    width: 100%;
    max-width: 960px;
    max-height: 90vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 1rem 1.25rem;
    border-bottom: 2px solid var(--ink);
    background: var(--paper);
    color: var(--ink);
  }

  .head-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .head-title h2 {
    font-family: 'Dela Gothic One', sans-serif;
    font-size: 1.15rem;
    font-weight: 700;
    color: var(--ink);
    margin: 0;
    letter-spacing: -0.02em;
  }

  .head-actions {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .view-toggle {
    display: flex;
    background: var(--shade);
    border: 2px solid var(--ink);
    border-radius: 4px;
    padding: 2px;
    gap: 2px;
  }

  .toggle-btn {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    background: transparent;
    border: none;
    color: var(--muted);
    padding: 0.3rem 0.65rem;
    border-radius: 2px;
    font-size: 0.8rem;
    cursor: pointer;
    font-weight: 700;
    transition: all 0.15s ease;
  }

  .toggle-btn:hover {
    color: var(--ink);
    background: var(--paper);
  }

  .toggle-btn.active {
    background: var(--ink);
    color: var(--paper);
    font-weight: 700;
  }

  .modal-body {
    padding: 1.25rem;
    overflow-y: auto;
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 1.25rem;
    background: var(--surface);
    color: var(--ink);
  }

  .modal-foot {
    display: flex;
    justify-content: flex-end;
    padding: 0.75rem 1.25rem;
    border-top: 2px solid var(--ink);
    background: var(--paper);
  }

  .target-summary-box {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 0.75rem;
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: 4px;
    box-shadow: var(--sh-3);
    padding: 0.75rem 1rem;
  }

  .target-stat {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .target-stat .label {
    font-size: 0.72rem;
    color: var(--muted);
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }

  .target-stat .value {
    font-size: 0.95rem;
    font-weight: 700;
    color: var(--ink);
  }

  .badge-count {
    background: var(--spot);
    color: var(--on-spot);
    border: 1.5px solid var(--ink);
    font-weight: 700;
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
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: 4px;
    box-shadow: var(--sh-3);
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    color: var(--ink);
    transition: transform 0.15s ease, box-shadow 0.15s ease;
  }

  .candidate-inspect-card.top-card {
    background: color-mix(in srgb, var(--spot) 8%, var(--surface));
    border: 2px solid var(--ink);
    box-shadow: var(--sh-5);
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
    font-weight: 700;
    line-height: 1.35;
    color: var(--ink);
    word-break: break-word;
  }

  .cand-raw-name {
    font-size: 0.75rem;
    color: var(--muted);
    word-break: break-all;
    font-family: monospace;
  }

  .cand-score-tag {
    font-size: 0.82rem;
    font-weight: 700;
    padding: 0.2rem 0.5rem;
    background: var(--spot);
    color: var(--on-spot);
    border: 1.5px solid var(--ink);
    border-radius: 4px;
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
    font-weight: 600;
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    border: 1px solid var(--ink);
  }

  .format-badge {
    background: var(--status-warn-bg);
    color: var(--status-warn);
    border: 1px solid var(--status-warn);
  }

  .edition-badge {
    background: var(--status-info-bg);
    color: var(--status-info);
    border: 1px solid var(--status-info);
  }

  .warning-badges-row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .warning-badge-pill {
    background: var(--status-error-bg);
    color: var(--status-error);
    border: 1.5px solid var(--status-error);
    font-size: 0.72rem;
    font-weight: 700;
  }

  .alternate-mirrors-box {
    background: var(--shade);
    border: 1px dashed var(--line);
    border-radius: 4px;
    padding: 0.4rem 0.6rem;
    font-size: 0.78rem;
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .alt-label {
    color: var(--muted);
    font-weight: 700;
  }

  .alt-pills {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }

  .alt-pill {
    background: var(--surface);
    border: 1px solid var(--line);
    padding: 0.15rem 0.45rem;
    border-radius: 4px;
    color: var(--ink);
  }

  .transport-badge {
    background: var(--shade);
    color: var(--ink);
    border: 1px solid var(--line);
  }

  .meta-badge {
    background: var(--shade);
    color: var(--muted);
    border: 1px solid var(--line);
  }

  .pre-badge {
    background: var(--status-warn-bg);
    color: var(--status-warn);
    font-weight: 700;
    border: 1px solid var(--status-warn);
  }

  .digital-badge {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1px solid var(--status-ok);
  }

  .trusted-badge {
    background: var(--status-info-bg);
    color: var(--status-info);
    border: 1px solid var(--status-info);
  }

  .match-exact_volume {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1.5px solid var(--status-ok);
  }

  .cand-explanation {
    font-size: 0.8rem;
    color: var(--ink);
    background: var(--shade);
    border: 1px solid var(--line);
    padding: 0.4rem 0.6rem;
    border-radius: 4px;
  }

  .cand-eligibility-box {
    padding: 0.6rem 0.75rem;
    border-radius: 4px;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.82rem;
  }

  .cand-eligibility-box.eligible {
    background: var(--status-ok-bg);
    border: 1.5px solid var(--status-ok);
    color: var(--ink);
  }

  .cand-eligibility-box.rejected {
    background: var(--status-error-bg);
    border: 1.5px solid var(--status-error);
    color: var(--ink);
  }

  .eligibility-status {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    font-weight: 700;
  }

  .eligibility-status.success {
    color: var(--status-ok);
  }

  .eligibility-status.blocked {
    color: var(--status-error);
  }

  .eligibility-desc {
    margin: 0;
    color: var(--ink);
  }

  .cand-sanitized-ref {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.75rem;
  }

  .ref-label {
    color: var(--muted);
    font-weight: 700;
  }

  .ref-code {
    background: var(--shade);
    border: 1px solid var(--line);
    padding: 0.35rem 0.5rem;
    border-radius: 4px;
    color: var(--ink);
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
    color: var(--muted);
  }

  .owned-tag {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1.5px solid var(--status-ok);
    font-weight: 700;
  }

  .missing-tag {
    background: var(--status-error-bg);
    color: var(--status-error);
    border: 1.5px solid var(--status-error);
    font-weight: 700;
  }

  .upgrade-protection-banner {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    background: var(--status-info-bg);
    border: 1.5px solid var(--status-info);
    border-radius: 4px;
    padding: 10px 14px;
    font-size: 0.85rem;
    color: var(--ink);
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
    background: var(--ink);
    color: var(--paper);
    font-weight: 700;
    font-size: 0.75rem;
    padding: 0.1rem 0.4rem;
    border-radius: 2px;
  }

  .verdict-badge {
    font-size: 0.75rem;
    font-weight: 700;
    text-transform: capitalize;
  }

  .verdict-preferred {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1.5px solid var(--status-ok);
  }

  .verdict-alternative {
    background: var(--status-info-bg);
    color: var(--status-info);
    border: 1.5px solid var(--status-info);
  }

  .verdict-manual_upgrade_only {
    background: var(--status-warn-bg);
    color: var(--status-warn);
    border: 1.5px solid var(--status-warn);
  }

  .verdict-already_owned {
    background: var(--shade);
    color: var(--muted);
    border: 1px solid var(--line);
  }

  .verdict-ineligible {
    background: var(--status-error-bg);
    color: var(--status-error);
  }

  .preferred-provider-badge {
    background: color-mix(in srgb, #6b21a8 12%, var(--surface));
    color: #6b21a8;
    border: 1.5px solid #6b21a8;
    font-size: 0.75rem;
    font-weight: 700;
  }

  .res-badge {
    background: var(--status-info-bg);
    color: var(--status-info);
    border: 1px solid var(--status-info);
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
    color: var(--muted);
    font-weight: 700;
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
    font-weight: 600;
  }

  .factor-bonus {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1px solid var(--status-ok);
  }

  .factor-penalty {
    background: var(--status-error-bg);
    color: var(--status-error);
    border: 1px solid var(--status-error);
  }

  /* Side-by-Side Comparison Styles */
  .compare-recommendation-banner {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    background: color-mix(in srgb, var(--spot) 15%, var(--surface));
    border: 2px solid var(--ink);
    box-shadow: var(--sh-3);
    border-radius: 4px;
    padding: 12px 14px;
    font-size: 0.88rem;
    color: var(--ink);
    line-height: 1.45;
  }

  .rec-text {
    margin: 4px 0 2px;
    color: var(--ink);
    font-weight: 700;
  }

  .dup-count-note {
    font-size: 0.78rem;
    color: var(--muted);
    font-style: italic;
  }

  .comparison-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(280px, 1fr));
    gap: 1rem;
    align-items: stretch;
  }

  .comparison-column {
    background: var(--paper);
    border: 2px solid var(--ink);
    box-shadow: var(--sh-3);
    border-radius: 4px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.85rem;
    color: var(--ink);
  }

  .comparison-column.rank-1 {
    background: color-mix(in srgb, var(--status-ok-bg) 60%, var(--surface));
    border: 2px solid var(--ink);
    box-shadow: var(--sh-5);
  }

  .col-head {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    border-bottom: 2px solid var(--ink);
    padding-bottom: 0.65rem;
  }

  .col-rank-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .col-cand-title {
    font-size: 0.92rem;
    font-weight: 700;
    line-height: 1.35;
    color: var(--ink);
    word-break: break-word;
  }

  .col-cand-score {
    font-size: 0.82rem;
    font-weight: 700;
    color: var(--muted);
  }

  .col-card-section {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    font-size: 0.8rem;
    background: var(--surface);
    border: 1px solid var(--line);
    padding: 0.55rem 0.65rem;
    border-radius: 4px;
  }

  .sec-label {
    font-size: 0.72rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
    margin-bottom: 0.2rem;
  }

  .matrix-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 0.5rem;
  }

  .m-k {
    color: var(--muted);
  }

  .m-v {
    font-weight: 600;
    color: var(--ink);
  }

  .text-dim {
    font-size: 0.74rem;
    color: var(--dim);
  }

  .text-success {
    color: var(--status-ok);
    font-weight: 700;
  }

  .text-danger {
    color: var(--status-error);
    font-weight: 700;
  }

  .text-warning {
    color: var(--status-warn);
    font-weight: 700;
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
    color: var(--status-ok);
  }

  .dis-item {
    color: var(--status-error);
  }

  .rejection-section {
    background: var(--status-error-bg);
    border: 1.5px solid var(--status-error);
    color: var(--ink);
  }

  .rejection-desc {
    margin: 0;
    font-size: 0.78rem;
    color: var(--status-error);
    font-weight: 600;
  }

  /* Authorization & Token Security (M8) */
  .review-auth-card {
    background: var(--paper);
    border: 2px solid var(--ink);
    box-shadow: var(--sh-3);
    border-radius: 4px;
    padding: 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
    color: var(--ink);
  }

  .dry-run-banner {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    padding: 12px 14px;
    border-radius: 4px;
    font-size: 0.85rem;
    line-height: 1.45;
    border: 2px solid var(--ink);
    box-shadow: var(--sh-3);
  }

  .dry-run-banner.banner-active {
    background: color-mix(in srgb, var(--spot) 20%, var(--surface));
    color: var(--ink);
  }

  .dry-run-banner.banner-inactive {
    background: var(--status-ok-bg);
    border-color: var(--status-ok);
    color: var(--ink);
  }

  .banner-icon {
    flex-shrink: 0;
    margin-top: 1px;
  }

  .dry-run-title {
    font-weight: 700;
    margin-bottom: 2px;
    color: var(--ink);
  }

  .dry-run-desc {
    margin: 0;
    color: var(--muted);
  }

  .review-auth-loading {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    font-size: 0.85rem;
    color: var(--status-info);
    padding: 0.5rem 0.75rem;
    background: var(--status-info-bg);
    border: 1.5px solid var(--status-info);
    border-radius: 4px;
  }

  .review-error-block {
    margin-bottom: 0.5rem;
  }

  .token-authorized-badge {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1.5px solid var(--status-ok);
    font-weight: 700;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }

  .token-pending-badge {
    background: var(--status-warn-bg);
    color: var(--status-warn);
    border: 1.5px solid var(--status-warn);
    font-weight: 700;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }

  .token-none-badge {
    background: var(--status-error-bg);
    color: var(--status-error);
    border: 1.5px solid var(--status-error);
    font-weight: 700;
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
  }

  .token-expiry-hint {
    display: block;
    font-size: 0.72rem;
    color: var(--muted);
    margin-top: 0.2rem;
  }

  .review-ranking-bar {
    margin-top: 0;
  }

  .review-details-card {
    background: var(--surface);
    border: 2px solid var(--ink);
    box-shadow: var(--sh-3);
    border-radius: 4px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    color: var(--ink);
  }

  .review-card-title {
    margin: 0;
    font-size: 0.92rem;
    font-weight: 700;
    color: var(--ink);
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
    border-bottom: 1px solid var(--line);
  }

  .spec-row:last-child {
    border-bottom: none;
    padding-bottom: 0;
  }

  .spec-label {
    color: var(--muted);
    font-weight: 600;
    flex-shrink: 0;
  }

  .spec-val {
    color: var(--ink);
    text-align: right;
    word-break: break-word;
  }

  .highlight-val {
    font-weight: 700;
    color: var(--status-info);
  }

  .confidence-tag {
    font-size: 0.76rem;
    color: var(--muted);
    margin-left: 0.35rem;
  }

  .plan-preview-box {
    background: var(--status-ok-bg);
    border: 1.5px solid var(--status-ok);
    border-radius: 4px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    color: var(--ink);
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
    color: var(--status-ok);
    font-size: 0.9rem;
    font-weight: 700;
  }

  .dry-run-pill {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1px solid var(--status-ok);
    font-weight: 700;
  }

  .preview-msg {
    margin: 0;
    font-size: 0.82rem;
    color: var(--ink);
  }

  .preview-meta-rows {
    display: flex;
    gap: 1.5rem;
    flex-wrap: wrap;
    font-size: 0.8rem;
    background: var(--surface);
    border: 1px solid var(--line);
    padding: 0.5rem 0.75rem;
    border-radius: 4px;
  }

  .p-meta {
    display: flex;
    gap: 0.35rem;
  }

  .p-k {
    color: var(--muted);
    font-weight: 600;
  }

  .p-v {
    color: var(--ink);
    font-weight: 600;
  }

  .preview-safety-footer {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin: 0;
    font-size: 0.76rem;
    color: var(--status-ok);
    font-weight: 600;
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
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1.5px solid var(--status-ok);
  }

  .cache-uncached {
    background: var(--status-warn-bg);
    color: var(--status-warn);
    border: 1.5px solid var(--status-warn);
  }

  .cache-unavailable {
    background: var(--status-error-bg);
    color: var(--status-error);
    border: 1.5px solid var(--status-error);
  }

  .cache-unknown {
    background: var(--shade);
    color: var(--muted);
    border: 1px solid var(--line);
  }

  .cache-direct {
    background: var(--status-info-bg);
    color: var(--status-info);
    border: 1.5px solid var(--status-info);
  }

  /* Scoring Breakdown Bar */
  .ranking-breakdown-bar {
    background: var(--paper);
    border: 1.5px solid var(--ink);
    border-radius: 4px;
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

  .breakdown-header-badges {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }

  .coverage-badge {
    font-size: 0.68rem;
    font-weight: 700;
  }

  .coverage-high {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1.5px solid var(--status-ok);
  }

  .coverage-partial {
    background: var(--status-warn-bg);
    color: var(--status-warn);
    border: 1.5px solid var(--status-warn);
  }

  .evidence-honesty-row {
    display: flex;
    flex-direction: column;
    gap: 0.35rem;
    padding-top: 0.25rem;
    border-top: 1px solid var(--line);
  }

  .evidence-group {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    flex-wrap: wrap;
    font-size: 0.75rem;
  }

  .evidence-label {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    font-weight: 700;
    font-size: 0.72rem;
  }

  .evidence-label.verified {
    color: var(--status-ok);
  }

  .evidence-label.heuristic {
    color: var(--status-info);
  }

  .evidence-verified-tag {
    background: var(--status-ok-bg);
    color: var(--status-ok);
    border: 1px solid var(--status-ok);
    font-size: 0.7rem;
    padding: 0.1rem 0.35rem;
  }

  .evidence-heuristic-tag {
    background: var(--status-info-bg);
    color: var(--status-info);
    border: 1px solid var(--status-info);
    font-size: 0.7rem;
    padding: 0.1rem 0.35rem;
  }

  .execution-success-box {
    background: var(--status-ok-bg);
    border: 2px solid var(--status-ok);
    border-radius: 4px;
    padding: 0.85rem 1rem;
    color: var(--ink);
  }

  .exec-success-ind {
    color: var(--status-ok);
    font-weight: 700;
  }

  .exec-success-pill {
    background: var(--status-ok);
    color: #ffffff;
    font-weight: 700;
  }

  .exec-safety-footer {
    color: var(--status-ok);
    font-weight: 600;
  }

  .execution-error-box,
  .plan-preview-error {
    margin-top: 0.25rem;
  }

  .auth-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }

  .confirm-acquire-btn {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
  }

  .breakdown-title {
    font-size: 0.74rem;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
  }

  .prov-badge {
    background: color-mix(in srgb, #6b21a8 12%, var(--surface));
    color: #6b21a8;
    border: 1px solid #6b21a8;
    font-size: 0.68rem;
    font-weight: 700;
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
    background: var(--shade);
    border: 1px solid var(--line);
    color: var(--ink);
  }

  .b-chip strong {
    color: var(--ink);
    font-weight: 700;
  }

  .why-ranks-above-box {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.75rem;
    color: var(--status-ok);
    font-weight: 600;
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
    font-weight: 700;
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
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(3px);
    z-index: 10005;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 1.25rem;
  }

  .block-details-card {
    background: var(--surface);
    color: var(--ink);
    border: 3px solid var(--ink);
    border-radius: 4px;
    box-shadow: var(--sh-8);
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
    border-bottom: 2px solid var(--ink);
    padding-bottom: 0.5rem;
  }

  .b-title {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    color: var(--status-error);
  }

  .b-title h3 {
    margin: 0;
    font-family: 'Dela Gothic One', sans-serif;
    font-size: 1rem;
    color: var(--status-error);
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
    color: var(--muted);
    margin-bottom: 0.25rem;
  }

  .b-release-name {
    font-size: 0.84rem;
    color: var(--ink);
    word-break: break-word;
  }

  .b-release-name .label {
    display: block;
    font-size: 0.72rem;
    color: var(--muted);
    font-weight: 700;
  }

  .b-code-box {
    margin-bottom: 0.25rem;
  }

  .b-reason-box {
    background: var(--status-error-bg);
    border: 1.5px solid var(--status-error);
    border-radius: 4px;
    padding: 0.65rem 0.75rem;
  }

  .b-reason-text {
    margin: 0;
    font-size: 0.82rem;
    color: var(--status-error);
    font-weight: 600;
    line-height: 1.4;
  }

  .b-policy-note {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
    font-size: 0.76rem;
    color: var(--ink);
    line-height: 1.35;
    background: var(--paper);
    border: 1px solid var(--line);
    padding: 0.5rem 0.65rem;
    border-radius: 4px;
  }

  .b-policy-note p {
    margin: 0;
  }
</style>