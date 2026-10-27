<script lang="ts">
import {onDestroy, onMount} from 'svelte';
import {getAcquisitionSources,planByTitle,createExecution,startExecution,message,type AcquisitionSourceOption,type InteractivePlan} from './api';
import {PlanningSession,parseRequestedVolumes,discoveredVolumes,SOURCE_OPTIONS_TIMEOUT_MS} from './planning';
import {confirm} from './confirm.svelte';
import {toast} from './toast.svelte';
import Icon from './Icon.svelte';
let {title,komgaSeriesId = null,initialReleaseId = '',initialMode = 'complete',onclose,onacquired}: {title:string;komgaSeriesId?:string|null;initialReleaseId?:string;initialMode?:'missing'|'complete'|'specific';onclose:()=>void;onacquired?:()=>void} = $props();
  // Title-level Acquisition Planning Modal state
  let planModalOpen = $state(true);
  const planModalItem = {get title(){return title}, get komgaSeriesId(){return komgaSeriesId}, acquisitionStatus:''};
  let sources = $state<AcquisitionSourceOption[]>([]);
  let selectedSourceId = $state<string>('auto');
  let contentMode = $state<'missing' | 'complete' | 'specific'>('missing');
  let specificVolumesInput = $state<string>('');
  let titlePlan = $state<InteractivePlan | null>(null);
  const planningSession = new PlanningSession();
  onDestroy(() => planningSession.cancel());
  let requestedEdition = $state('');
  let selectedReleaseId = $state('');
  let overrideConfirmed = $state(false);
  let selectedArchives = $state<{path:string;size:number}[]>([]);
  let selectedArchive = $state<{ path: string; size: number } | null>(null);
  function closePlanningModal() { planningSession.cancel(); generatingPlan = false; planModalOpen = false; onclose(); }
  $effect(() => { if (!planModalOpen) { planningSession.cancel(); generatingPlan = false; } });
  let generatingPlan = $state<boolean>(false);
  let approvingPlan = $state<boolean>(false);
  let planError = $state<string | null>(null);

  // Keep keyboard focus in the top overlay and return to its opener on close.
  function planningFocus(node: HTMLDivElement) {
    const opener = document.activeElement;
    const focusable = () => Array.from(node.querySelectorAll<HTMLElement>(
      'button:not(:disabled), select:not(:disabled), input:not(:disabled), [tabindex="0"]',
    ));
    node.focus();
    function keydown(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        closePlanningModal();
      } else if (event.key === 'Tab') {
        const controls = focusable();
        const first = controls[0];
        const last = controls[controls.length - 1];
        if (!first) {
          event.preventDefault();
          node.focus();
        } else if (event.shiftKey && (document.activeElement === first || document.activeElement === node)) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && (document.activeElement === last || document.activeElement === node)) {
          event.preventDefault();
          first.focus();
        }
      }
    }
    node.addEventListener('keydown', keydown);
    return { destroy() {
      node.removeEventListener('keydown', keydown);
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    } };
  }

  async function generateTitlePlan() {
    if (!planModalItem) return;
    planningSession.cancel(); generatingPlan = true; planError = null;
    const request = { title: planModalItem.title, komgaSeriesId: planModalItem.komgaSeriesId,
      sourceId: selectedSourceId, contentMode, selectedArchive, selectedArchives, requestedVolumes: null as number[] | null,
      requestedEdition: requestedEdition || null, selectedReleaseId: selectedReleaseId || null, useReleaseAnyway: overrideConfirmed };
    titlePlan = null;
    try { if (contentMode === 'specific') request.requestedVolumes = specificVolumesInput.trim() ? parseRequestedVolumes(specificVolumesInput) : []; }
    catch (error) { planError = message(error); generatingPlan = false; return; }
    const result = await planningSession.run(signal => planByTitle(request, signal));
    if (result.status === 'stale') return;
    generatingPlan = false;
    if (result.status === 'ready') titlePlan = result.value; else planError = result.error.message;
  }
  async function useReleaseAnyway() {
    const candidate = titlePlan?.candidates.find(c => c.providerReleaseId === selectedReleaseId);
    if (!candidate || !planModalItem) return;
    const selectedAtConfirmation = selectedReleaseId;
    const titleAtConfirmation = planModalItem;
    const approved = await confirm({ title: 'Use Release Anyway', confirmText: 'Use Release Anyway',
      message: `Mango cannot verify this release's exact series/edition identity against the catalog. Continue using the release's own edition/content and series-level metadata? Series: ${planModalItem.title}. Release: ${candidate.title}. Detected edition: ${candidate.edition ?? 'Unknown'}. Catalog edition: Unverified. Metadata: Series-level fallback. Existing files will not be replaced.` });
    if (!approved || !planModalOpen || selectedReleaseId !== selectedAtConfirmation || planModalItem !== titleAtConfirmation) return;
    overrideConfirmed = true; generateTitlePlan();
  }

  async function approveTitlePlan() {
    if (!titlePlan || !planModalItem) return;
    const approvedPlan = titlePlan;
    const approvedItem = planModalItem;
    approvingPlan = true;
    try {
      const execution = await createExecution(approvedPlan.mangaId, {
        planId: approvedPlan.id,
        approved: true,
        overrideConfirmed,
      });
      await startExecution(execution.id);
      toast.success(`Acquisition approved and started for ${approvedPlan.seriesTitle}!`);
      approvedItem.acquisitionStatus = 'Acquiring';
      onacquired?.();
      if (planModalItem === approvedItem && titlePlan?.id === approvedPlan.id) closePlanningModal();
    } catch (e) {
      toast.error(`Failed to approve acquisition: ${message(e)}`);
    } finally {
      approvingPlan = false;
    }
  }
onMount(() => { contentMode = initialMode; selectedReleaseId = initialReleaseId; generateTitlePlan(); getAcquisitionSources(AbortSignal.timeout(SOURCE_OPTIONS_TIMEOUT_MS)).then(value => sources = value).catch(() => {}); });
</script>
{#if planModalOpen && planModalItem}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={closePlanningModal}>
    <div
      class="modal planning-modal"
      role="dialog"
      aria-modal="true"
      aria-labelledby="plan-modal-title"
      tabindex="-1"
      use:planningFocus
      onclick={(e) => e.stopPropagation()}
    >
      <div class="modal-header">
        <div>
          <h2 id="plan-modal-title" class="modal-title">Acquire — {planModalItem.title}</h2>
          <span class="modal-subtitle">Review release candidates, target missing content, and approve safe execution</span>
        </div>
        <button class="btn icon close-btn" onclick={closePlanningModal} aria-label="Close dialog">
          <Icon name="close" size={20} />
        </button>
      </div>

      <div class="modal-body-scroll">
        <!-- Configuration Controls -->
        <div class="plan-controls-grid">
          <!-- Content Selection -->
          <div class="control-group">
            <span class="control-label"><strong>Content to Acquire:</strong></span>
            <div class="segmented-control">
              <button
                class="segment-btn"
                class:active={contentMode === 'missing'}
                onclick={() => {
                  selectedArchives = []; selectedArchive = null; contentMode = 'missing';
                  generateTitlePlan();
                }}
              >
                Missing Volumes
              </button>
              <button
                class="segment-btn"
                class:active={contentMode === 'complete'}
                onclick={() => {
                  selectedArchives = []; selectedArchive = null; contentMode = 'complete';
                  generateTitlePlan();
                }}
              >
                Complete Series
              </button>
              <button
                class="segment-btn"
                class:active={contentMode === 'specific'}
                onclick={() => {
                  selectedArchives = []; selectedArchive = null; contentMode = 'specific'; generateTitlePlan();
                }}
              >
                Specific Volumes
              </button>
            </div>
            {#if contentMode === 'specific'}
              <div class="specific-input-wrap">
                <input
                  type="text"
                  class="specific-input"
                  bind:value={specificVolumesInput}
                  aria-label="Edition-local volumes"
                  oninput={() => { selectedArchives = []; selectedArchive = null; planningSession.cancel(); generatingPlan = false; titlePlan = null; }}
                  placeholder="e.g. 1, 2, 5-8, 39"
                />
                <button class="btn small" onclick={generateTitlePlan} disabled={generatingPlan}>
                  Apply
                </button>
              </div>
            {/if}
          </div>

          <!-- Source Selection -->
          <div class="control-group">
            <label class="control-label" for="plan-source-select"><strong>Acquisition Source:</strong></label>
            <select
              id="plan-source-select"
              class="source-select"
              bind:value={selectedSourceId}
              onchange={() => { overrideConfirmed = false; selectedArchives = []; selectedArchive = null; selectedReleaseId = ''; generateTitlePlan(); }}
            >
              {#if sources.length === 0}
                <option value="auto">Auto (Recommended)</option>
                <option value="nyaa">Nyaa (BitTorrent / TorBox)</option>
              {:else}
                {#each sources as src}
                  <option value={src.id} disabled={!src.available}>
                    {src.name} {!src.available ? '(Unavailable)' : src.isDefault ? '(Default)' : ''}
                  </option>
                {/each}
              {/if}
            </select>
          </div>
        </div>

        <!-- Plan Status / Feedback -->
        {#if generatingPlan}
          <div class="loading-state">
            <Icon name="spinner" size={28} />
            <p>{selectedSourceId === 'nyaa' ? 'Searching Nyaa…' : 'Checking available sources…'}</p>
          </div>
        {:else if planError}
          <div class="plan-error-box">
            <Icon name="info" size={20} />
            <div>
              <strong>Plan Generation Warning</strong>
              <p>{planError}</p><button class="btn small" onclick={generateTitlePlan}>Try Again</button>
            </div>
          </div>
        {:else if titlePlan}
          {#if titlePlan.editions.some(ed => ed !== 'standard') || titlePlan.editions.length > 1}
            <label for="plan-edition">Edition (numbering is edition-local)</label>
            <select class="source-select" id="plan-edition" bind:value={requestedEdition} onchange={() => { overrideConfirmed = false; selectedArchives = []; selectedArchive = null; selectedReleaseId = ''; generateTitlePlan(); }}>
              <option value="">Choose a discovered edition</option>
              {#each titlePlan.editions as edition}<option value={edition}>{edition} Edition</option>{/each}
            </select>
          {/if}
          <details open={titlePlan.selectedItems.length === 0 || !!selectedReleaseId}>
            <summary>Choose source release or review identity</summary>
          {#if titlePlan.candidates.length > 0}
            <label for="plan-release">Source release</label>
            <select class="source-select" id="plan-release" bind:value={selectedReleaseId} onchange={() => {
              overrideConfirmed = false; selectedArchives = []; selectedArchive = null;
              const release = titlePlan?.candidates.find(candidate => candidate.providerReleaseId === selectedReleaseId);
              requestedEdition = release?.edition ?? '';
              generateTitlePlan();
            }}>
              <option value="">Recommended selection</option>
              {#each titlePlan.candidates as candidate}<option value={candidate.providerReleaseId}>{candidate.title}</option>{/each}
            </select>
          {/if}
          {#if selectedReleaseId}
            <p>Catalog edition: Unverified until metadata resolution. {overrideConfirmed ? 'Using source release identity · Series metadata fallback' : 'Metadata will be as complete as available.'}</p>
            <button class="btn small" onclick={useReleaseAnyway} disabled={overrideConfirmed}>{overrideConfirmed ? 'Source identity confirmed' : 'Use Release Anyway'}</button>
          {/if}
          </details>
          {#if selectedReleaseId}
            <div class="control-group">
              <strong>Source-local {requestedEdition ? `${requestedEdition} Edition` : 'release'} units</strong>
              <p role="status">{discoveredVolumes(titlePlan.sourceUnits).length} numbered source-local volumes discovered. These numbers belong to this release's edition.</p>
              {#if titlePlan.sourceUnits.length > 0}
                <button class="btn small" onclick={() => { selectedArchives = titlePlan?.sourceUnits.map(u=>u.archive) ?? []; generateTitlePlan(); }}>Select all source files</button>
                <p>Exact edition metadata may be unavailable. Selected files can still be downloaded. Unnumbered files require Use Release Anyway and receive the displayed selection order as their library number.</p>
                {#each titlePlan.sourceUnits as file, index}
                  <label class="source-file"><input type="checkbox" checked={selectedArchives.some(a=>a.path===file.archive.path)} onchange={(event) => { selectedArchives = event.currentTarget.checked ? [...selectedArchives,file.archive] : selectedArchives.filter(a=>a.path!==file.archive.path); generateTitlePlan(); }} />
                    <span>{file.archive.path}<small>{file.archive.size} bytes · {file.unit.type === 'unknown' ? `Selection order assigns the library number when confirmed` : file.unit.type}</small></span>
                  </label>
                {/each}

                <div class="segmented-control" role="group" aria-label="Discovered edition-local volumes">
                  {#each discoveredVolumes(titlePlan.sourceUnits) as volume}
                    <button class="segment-btn" onclick={() => { contentMode = 'specific'; specificVolumesInput = String(volume); selectedArchive = null; generateTitlePlan(); }}>Volume {volume}</button>
                  {/each}
                </div>
                <label for="source-archive">Exact source archive (resolves duplicate numbered alternatives)</label>
                <select id="source-archive" class="source-select" value={selectedArchive?.path ?? ''} onchange={(event) => {
                  selectedArchives = [];
                  const choice = titlePlan?.sourceUnits.find(u => u.archive.path === event.currentTarget.value);
                  if (choice?.eligible && (choice.unit.type === 'volume' || choice.unit.type === 'chapter')) { selectedArchive = choice.archive; specificVolumesInput = String(choice.unit.value); contentMode = 'specific'; }
                  else selectedArchive = null;
                  generateTitlePlan();
                }}>
                  <option value="">Automatic unambiguous file selection</option>
                  {#each titlePlan.sourceUnits as unit}
                    <option value={unit.archive.path} disabled={!unit.eligible}>{unit.archive.path} ({unit.archive.size} bytes){unit.eligible ? '' : ' — unverified unit/edition'}</option>
                  {/each}
                </select>
              {:else}
                <p>No numbered source archives are available. Choose Specific Volumes above to enter edition-local intent, or try another release. Mango will still require archive evidence before retrieval.</p>
              {/if}
              {#if contentMode === 'missing' && requestedEdition && requestedEdition !== 'standard' && !titlePlan.editionOwnershipKnown}
                <p>Edition-qualified ownership is unavailable. Standard-edition ownership is not used; choose Complete Series or Specific Volumes.</p>
              {/if}
            </div>
          {/if}
          {#if titlePlan.staleReason}<p role="status">{titlePlan.staleReason}</p>{/if}
          <!-- Plan Summary Cards -->
          <div class="plan-summary-metrics">
            <div class="metric-card">
              <span class="metric-val">{titlePlan.missingUnits.length}</span>
              <span class="metric-lbl">Missing Units</span>
            </div>
            <div class="metric-card">
              <span class="metric-val">{titlePlan.missingUnitsCoveredCount}</span>
              <span class="metric-lbl">Covered Units</span>
            </div>
            <div class="metric-card">
              <span class="metric-val">{titlePlan.selectedItems.length}</span>
              <span class="metric-lbl">Releases to Acquire</span>
            </div>
            <div class="metric-card">
              <span class="metric-val">{Math.max(0, titlePlan.missingUnits.length - titlePlan.missingUnitsCoveredCount)}</span>
              <span class="metric-lbl">Unresolved Units</span>
            </div>
          </div>

          <!-- Candidates Preview -->
          {#if titlePlan.selectedItems.length === 0}
            <div class="plan-empty-notice">
              <Icon name="info" size={24} />
              <div>
                <strong>No Candidate Releases Selected</strong>
                <p>
                  {#if titlePlan.missingUnits.length === 0}
                    No eligible units are selected. Choose a release and specify edition-local volumes when coverage is unknown.
                  {:else}
                    No releases matched the quality and profile criteria for the missing content.
                  {/if}
                </p>
              </div>
            </div>
          {:else}
            <div class="planned-items-list">
              <h4>Matched Candidate Releases ({titlePlan.selectedItems.length})</h4>
              {#each titlePlan.selectedItems as item, idx}
                <div class="planned-candidate-row">
                  <div class="cand-header">
                    <span class="cand-idx">#{idx + 1}</span>
                    <strong class="cand-title">{item.candidate.title}</strong>
                    <span class="cand-score-badge">
                      Score: {item.candidate.score.toFixed(0)}/100
                    </span>
                  </div>
                  <div class="cand-meta">
                    <span>Source: <strong>{item.candidate.provider}</strong></span>
                    {#if item.candidate.uploader}
                      <span>Group: <strong>{item.candidate.uploader}</strong></span>
                    {/if}
                    <span>Seeders: <strong>{item.candidate.health.seeders ?? 0}</strong></span>
                    <span>Size: <strong>{((item.candidate.size ?? 0) / 1024 / 1024).toFixed(1)} MB</strong></span>
                    <span>Units Covered: <strong>v{item.unitsCovered.join(', v')}</strong></span>
                  </div>
                </div>
              {/each}
            </div>
          {/if}

          <!-- Warnings -->
          {#if titlePlan.diagnostics.length > 0}
            <div class="plan-warnings-box">
              <div class="warning-title">
                <Icon name="info" size={16} />
                <strong>Pre-flight Warnings:</strong>
              </div>
              <ul>
                {#each titlePlan.diagnostics as w}
                  <li>{w}</li>
                {/each}
              </ul>
            </div>
          {/if}
        {/if}
      </div>

      <div class="modal-footer">
        <button class="btn" onclick={closePlanningModal}>Cancel</button>
        <button
          class="btn primary"
          onclick={approveTitlePlan}
          disabled={generatingPlan || !titlePlan || titlePlan.selectedItems.length === 0 || approvingPlan}
        >
          <Icon name={approvingPlan ? 'spinner' : 'check'} size={18} />
          <span>{approvingPlan ? 'Approving…' : 'Approve Acquisition'}</span>
        </button>
      </div>
    </div>
  </div>
{/if}


<style>
.modal-backdrop {position:fixed;inset:0;z-index:1000;background:rgba(0,0,0,.7);display:flex;align-items:center;justify-content:center;padding:16px;}
.planning-modal {background:var(--surface);color:var(--ink);border:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));border-radius:16px;width:min(900px,100%);max-height:calc(100dvh - 32px);display:flex;flex-direction:column;box-shadow:0 24px 80px #0008;}
.modal-header,.modal-footer {display:flex;align-items:center;justify-content:space-between;gap:16px;padding:20px;border-bottom:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));}
.modal-footer {border-top:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));border-bottom:0;justify-content:flex-end;}
.modal-title {font-size:1.2rem;overflow-wrap:anywhere;}.modal-subtitle {opacity:.7;font-size:.85rem;}
.modal-body-scroll {padding:20px;overflow-y:auto;min-height:0;}.plan-controls-grid {display:flex;gap:20px;flex-wrap:wrap;margin-bottom:20px;}
.control-group {display:flex;flex-direction:column;gap:10px;min-width:0;}.segmented-control {display:flex;gap:6px;flex-wrap:wrap;}
.segment-btn {padding:8px 12px;border:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));border-radius:8px;background:var(--surface);color:var(--ink);cursor:pointer;}.segment-btn.active {border-color:var(--spot);}
.source-select,.specific-input {width:100%;max-width:100%;padding:10px;border:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));border-radius:8px;background:var(--surface);color:var(--ink);}
.source-file {display:flex;gap:12px;align-items:flex-start;padding:8px 0;overflow-wrap:anywhere;}.source-file small {display:block;opacity:.7;}
.plan-summary-metrics {display:flex;flex-wrap:wrap;gap:16px;margin:20px 0;}.metric-card {display:flex;flex-direction:column;}.metric-val {font-size:1.4rem;font-weight:700;}.metric-lbl {font-size:.8rem;opacity:.7;}
.planned-candidate-row {padding:12px 0;border-bottom:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));overflow-wrap:anywhere;}
</style>