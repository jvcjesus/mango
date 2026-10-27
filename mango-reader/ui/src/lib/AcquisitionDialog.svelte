<script lang="ts">
import {onDestroy, onMount} from 'svelte';
import {getAcquisitionSources,planByTitle,createExecution,startExecution,message,type AcquisitionSourceOption,type InteractivePlan} from './api';
import {PlanningSession,SOURCE_OPTIONS_TIMEOUT_MS} from './planning';
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

  let titlePlan = $state<InteractivePlan | null>(null);
  const planningSession = new PlanningSession();
  onDestroy(() => planningSession.cancel());
  let requestedEdition = $state('');
  let selectedReleaseId = $state('');
  let selectedArchives = $state<{path:string;size:number}[]>([]);

  function closePlanningModal() { planningSession.cancel(); generatingPlan = false; planModalOpen = false; onclose(); }
  $effect(() => { if (!planModalOpen) { planningSession.cancel(); generatingPlan = false; } });
  let generatingPlan = $state<boolean>(false);
  let approvingPlan = $state<boolean>(false);
  let planError = $state<string | null>(null);

  let dialogElement:HTMLDivElement|null=null;
  // Keep keyboard focus in the top overlay and return to its opener on close.
  function planningFocus(node: HTMLDivElement) {
    dialogElement=node;
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
      dialogElement=null;
      node.removeEventListener('keydown', keydown);
      if (opener instanceof HTMLElement && opener.isConnected) opener.focus();
    } };
  }

  async function generateTitlePlan() {
    if (!planModalItem) return;
    const active=document.activeElement;
    if (dialogElement?.contains(active)) dialogElement.focus();
    planningSession.cancel(); generatingPlan = true; planError = null;
    const request = { title: planModalItem.title, komgaSeriesId: planModalItem.komgaSeriesId,
      sourceId: selectedSourceId, inspectPayload: true, contentMode, selectedArchives, requestedVolumes: null as number[] | null,
      requestedEdition: requestedEdition || null, selectedReleaseId: selectedReleaseId || null, useReleaseAnyway: false };
    titlePlan = null;
    try { if (contentMode === 'specific') request.requestedVolumes = []; }
    catch (error) { planError = message(error); generatingPlan = false; return; }
    const result = await planningSession.run(signal => planByTitle(request, signal));
    if (result.status === 'stale') return;
    generatingPlan = false;
    if (dialogElement && document.activeElement === dialogElement && active instanceof HTMLElement && active.isConnected) active.focus();
    if (result.status === 'ready') titlePlan = result.value; else planError = result.error.message;
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
        overrideConfirmed:false,
      });
      await startExecution(execution.id);
      toast.success(`Payload retrieval started for ${approvedPlan.seriesTitle}. Review discovered books in Downloads.`);
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
          <span class="modal-subtitle">Select physical payloads to retrieve and inspect. Assign discovered books in Downloads.</span>
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
                  selectedArchives = [];  contentMode = 'missing';
                  generateTitlePlan();
                }}
              >
                Missing suggestions
              </button>
              <button
                class="segment-btn"
                class:active={contentMode === 'complete'}
                onclick={() => {
                  selectedArchives = [];  contentMode = 'complete';
                  generateTitlePlan();
                }}
              >
                Complete release
              </button>
              <button
                class="segment-btn"
                class:active={contentMode === 'specific'}
                onclick={() => {
                  selectedArchives = [];  contentMode = 'specific'; generateTitlePlan();
                }}
              >
                Specific files
              </button>
            </div>

          </div>

          <!-- Source Selection -->
          <div class="control-group">
            <label class="control-label" for="plan-source-select"><strong>Acquisition Source:</strong></label>
            <select
              id="plan-source-select"
              class="source-select"
              bind:value={selectedSourceId}
              onchange={() => { selectedArchives = [];  selectedReleaseId = ''; generateTitlePlan(); }}
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
          <details open={titlePlan.selectedItems.length === 0 || !!selectedReleaseId}>
            <summary>Choose source release or review identity</summary>
          {#if titlePlan.candidates.length > 0}
            <label for="plan-release">Source release</label>
            <select class="source-select" id="plan-release" bind:value={selectedReleaseId} onchange={() => {
              selectedArchives = [];
              const release = titlePlan?.candidates.find(candidate => candidate.providerReleaseId === selectedReleaseId);
              requestedEdition = release?.edition ?? '';
              generateTitlePlan();
            }}>
              <option value="">Recommended selection</option>
              {#each titlePlan.candidates as candidate}<option value={candidate.providerReleaseId}>{candidate.title}</option>{/each}
            </select>
          {/if}
          {#if selectedReleaseId}
            <p>Book identity is assigned after inspection. Metadata is enriched after you assign the discovered books.</p>

          {/if}
          </details>
          {#if selectedReleaseId}
            <div class="control-group">
              <strong>Physical source payloads</strong>
              <p role="status">{titlePlan.sourceUnits.length} payload files discovered. Contents require inspection after retrieval; filenames only suggest book assignments.</p>
              {#if titlePlan.sourceUnits.length > 0}
                <button class="btn small" onclick={() => { selectedArchives = titlePlan?.sourceUnits.map(u=>u.archive) ?? []; generateTitlePlan(); }}>Select all source files</button>
                <p>Acquire the selected physical payloads. Mango will inspect their contents; book assignments are reviewed in Downloads before publication.</p>
                {#each titlePlan.sourceUnits as file, index}
                  <label class="source-file"><input type="checkbox" checked={selectedArchives.some(a=>a.path===file.archive.path)} onchange={(event) => { selectedArchives = event.currentTarget.checked ? [...selectedArchives,file.archive] : selectedArchives.filter(a=>a.path!==file.archive.path); generateTitlePlan(); }} />
                    <span>{file.archive.path}<small>{file.archive.size} bytes · {file.unit.type === 'unknown' ? 'Assignment follows inspection' : file.unit.type}</small></span>
                  </label>
                {/each}

              {:else}
                <p>No inspectable ZIP/CBZ payloads were discovered. Choose another source release; RAR/CBR/7z/PDF/EPUB imports are unsupported.</p>
              {/if}
            </div>
          {/if}
          {#if titlePlan.staleReason}<p role="status">{titlePlan.staleReason}</p>{/if}
          <!-- Candidates Preview -->
          {#if titlePlan.selectedItems.length === 0}
            <div class="plan-empty-notice">
              <Icon name="info" size={24} />
              <div>
                <strong>Choose payloads to inspect</strong>
                <p>
                  Select a source release, then choose its physical files. If nothing is available, try another source or retry discovery.
                </p>
              </div>
            </div>
          {:else}
            <div class="planned-items-list">
              <h4>Payloads to acquire ({titlePlan.selectedItems.length})</h4>
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
                    <span>Size: <strong>{((selectedArchives.find(a=>a.path===item.candidate.title)?.size ?? 0) / 1024 / 1024).toFixed(1)} MB</strong></span>

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
          <span>{approvingPlan ? 'Approving…' : 'Acquire & Inspect'}</span>
        </button>
      </div>
    </div>
  </div>
{/if}


<style>
.source-select {width:100%;max-width:100%;padding:10px;border:1px solid var(--ink);border-radius:8px;background:var(--surface);color:var(--ink);}

.modal-backdrop {position:fixed;inset:0;z-index:1000;background:rgba(0,0,0,.7);display:flex;align-items:center;justify-content:center;padding:16px;}
.planning-modal {background:var(--surface);color:var(--ink);border:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));border-radius:16px;width:min(900px,100%);max-height:calc(100dvh - 32px);display:flex;flex-direction:column;box-shadow:0 24px 80px #0008;}
.modal-header,.modal-footer {display:flex;align-items:center;justify-content:space-between;gap:16px;padding:20px;border-bottom:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));}
.modal-footer {border-top:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));border-bottom:0;justify-content:flex-end;}
.modal-title {font-size:1.2rem;overflow-wrap:anywhere;}.modal-subtitle {opacity:.7;font-size:.85rem;}
.modal-body-scroll {padding:20px;overflow-y:auto;min-height:0;}.plan-controls-grid {display:flex;gap:20px;flex-wrap:wrap;margin-bottom:20px;}
.control-group {display:flex;flex-direction:column;gap:10px;min-width:0;}.segmented-control {display:flex;gap:6px;flex-wrap:wrap;}
.segment-btn {padding:8px 12px;border:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));border-radius:8px;background:var(--surface);color:var(--ink);cursor:pointer;}.segment-btn.active {border-color:var(--spot);}

.source-file {display:flex;gap:12px;align-items:flex-start;padding:8px 0;overflow-wrap:anywhere;}.source-file small {display:block;opacity:.7;}

.planned-candidate-row {padding:12px 0;border-bottom:1px solid color-mix(in srgb,var(--ink) 20%,var(--paper));overflow-wrap:anywhere;}
</style>
