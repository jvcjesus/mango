<script lang="ts">
  import {
    clearManualTitleMapping,
    getTitleAvailability,
    getTitleDiagnostics,
    getTitleReceipts,
    listMangaBakaCollections,
    message,
    refreshTitleAvailability,
    rescanPublicationReceipt,
    retryReceiptMetadata,
    send,
    setManualTitleMapping,
    updateTitleAvailability,
    verifyPublicationReceipt,
    type MangaBakaCollectionInfo,
    type PublicationReceiptDto,
    type TitleAvailabilityDto,
    type TitleDiagnostics,
  } from './api';
  import Icon from './Icon.svelte';
  import Notice from './Notice.svelte';
  import { toast } from './toast.svelte';
  import { ago } from './util';

  let {
    open = false,
    mangaId,
    mangaTitle,
    onClose,
    onReconciled,
  }: {
    open: boolean;
    mangaId: number;
    mangaTitle: string;
    onClose: () => void;
    onReconciled?: () => void;
  } = $props();

  let loading = $state(false);
  let error = $state<string | null>(null);
  let diag = $state<TitleDiagnostics | null>(null);

  // Manual resolution state
  let showManualSection = $state(false);
  let manualMbIdInput = $state('');
  let manualEditionInput = $state('standard');
  let manualCollectionId = $state('');
  let availableCollections = $state<MangaBakaCollectionInfo[]>([]);
  let loadingCollections = $state(false);
  let savingManual = $state(false);
  let manualSearchQuery = $state('');
  let searchingMb = $state(false);
  let searchResults = $state<Array<{ id: number; title: string; year?: number | null }>>([]);

  // Release availability watcher state (M6.8.4)
  let availability = $state<TitleAvailabilityDto | null>(null);
  let refreshingAvailability = $state(false);
  let updatingAvailability = $state(false);

  async function handleToggleMonitoring() {
    if (!mangaId || !availability) return;
    updatingAvailability = true;
    try {
      availability = await updateTitleAvailability(mangaId, {
        monitoringEnabled: !availability.monitoringEnabled,
      });
      toast.success(`Release monitoring ${availability.monitoringEnabled ? 'enabled' : 'disabled'}`);
    } catch (e) {
      toast.error(`Failed to update monitoring: ${message(e)}`);
    } finally {
      updatingAvailability = false;
    }
  }

  async function handleToggleNotify() {
    if (!mangaId || !availability) return;
    updatingAvailability = true;
    try {
      availability = await updateTitleAvailability(mangaId, {
        notifyEnabled: !availability.notifyEnabled,
      });
      toast.success(`Release notifications ${availability.notifyEnabled ? 'enabled' : 'disabled'}`);
    } catch (e) {
      toast.error(`Failed to update notifications: ${message(e)}`);
    } finally {
      updatingAvailability = false;
    }
  }

  async function handleRefreshAvailability() {
    if (!mangaId) return;
    refreshingAvailability = true;
    try {
      const res = await refreshTitleAvailability(mangaId);
      toast.success(`Availability check triggered (${res.watchersChecked} watchers checked)`);
      availability = await getTitleAvailability(mangaId);
    } catch (e) {
      toast.error(`Availability check failed: ${message(e)}`);
    } finally {
      refreshingAvailability = false;
    }
  }

  // Publication receipts state (M6.9)
  let receipts = $state<PublicationReceiptDto[]>([]);
  let verifyingReceiptId = $state<string | null>(null);
  let rescanningReceiptId = $state<string | null>(null);
  let repairingReceiptId = $state<string | null>(null);

  async function handleVerifyReceipt(r: PublicationReceiptDto) {
    verifyingReceiptId = r.id;
    try {
      const res = await verifyPublicationReceipt(r.id);
      if (res.isValid) {
        toast.success(`Verified: ${r.seriesTitle} (${(r.fileSizeBytes / 1024 / 1024).toFixed(1)} MB, CBZ valid, SHA256 matched)`);
      } else {
        toast.error(`Verification issue: ${res.error ?? 'Invalid archive'}`);
      }
      receipts = await getTitleReceipts(mangaId).catch(() => receipts);
    } catch (e) {
      toast.error(`Verification failed: ${message(e)}`);
    } finally {
      verifyingReceiptId = null;
    }
  }

  async function handleRescanReceipt(r: PublicationReceiptDto) {
    rescanningReceiptId = r.id;
    try {
      await rescanPublicationReceipt(r.id);
      toast.success(`Triggered Komga library rescan for ${r.seriesTitle}`);
      receipts = await getTitleReceipts(mangaId).catch(() => receipts);
    } catch (e) {
      toast.error(`Rescan failed: ${message(e)}`);
    } finally {
      rescanningReceiptId = null;
    }
  }

  async function handleRepairReceiptMetadata(r: PublicationReceiptDto) {
    repairingReceiptId = r.id;
    try {
      await retryReceiptMetadata(r.id);
      toast.success(`Metadata repaired for ${r.seriesTitle}`);
      receipts = await getTitleReceipts(mangaId).catch(() => receipts);
    } catch (e) {
      toast.error(`Metadata repair failed: ${message(e)}`);
    } finally {
      repairingReceiptId = null;
    }
  }

  async function load() {
    if (!open || !mangaId) return;
    loading = true;
    error = null;
    try {
      diag = await getTitleDiagnostics(mangaId);
      availability = await getTitleAvailability(mangaId).catch(() => null);
      receipts = await getTitleReceipts(mangaId).catch(() => []);
      if (diag.identity.mangabakaId) {
        manualMbIdInput = String(diag.identity.mangabakaId);
        loadCollections(diag.identity.mangabakaId);
      }
      if (diag.collection.resolvedCollectionId) {
        manualCollectionId = diag.collection.resolvedCollectionId;
      }
      if (diag.collection.edition) {
        manualEditionInput = diag.collection.edition;
      }
    } catch (e) {
      error = message(e);
    } finally {
      loading = false;
    }
  }

  async function loadCollections(seriesId: number) {
    loadingCollections = true;
    try {
      availableCollections = await listMangaBakaCollections(seriesId);
      if (availableCollections.length > 0 && !manualCollectionId) {
        manualCollectionId = availableCollections[0].id;
        manualEditionInput = availableCollections[0].edition || 'standard';
      }
    } catch {
      availableCollections = [];
    } finally {
      loadingCollections = false;
    }
  }

  async function searchMangaBaka() {
    if (!manualSearchQuery.trim()) return;
    searchingMb = true;
    try {
      const res = await send<Array<{ id: number; title: string; year?: number | null }>>(
        'GET',
        `/catalog/mangabaka/search?q=${encodeURIComponent(manualSearchQuery.trim())}`
      );
      searchResults = res;
    } catch (e) {
      toast.error(`MangaBaka search failed: ${message(e)}`);
      searchResults = [];
    } finally {
      searchingMb = false;
    }
  }

  function selectSearchResult(item: { id: number; title: string }) {
    manualMbIdInput = String(item.id);
    searchResults = [];
    loadCollections(item.id);
  }

  async function handleApplyManualMapping() {
    const mbIdNum = manualMbIdInput.trim() ? parseInt(manualMbIdInput.trim(), 10) : undefined;
    if (manualMbIdInput.trim() && isNaN(mbIdNum!)) {
      toast.error('Invalid MangaBaka ID number');
      return;
    }

    savingManual = true;
    try {
      await setManualTitleMapping(mangaId, {
        mangabakaId: mbIdNum,
        collectionId: manualCollectionId.trim() || undefined,
        edition: manualEditionInput.trim() || 'standard',
      });
      toast.success('Manual mapping override applied successfully');
      showManualSection = false;
      await load();
      onReconciled?.();
    } catch (e) {
      toast.error(`Failed to apply manual mapping: ${message(e)}`);
    } finally {
      savingManual = false;
    }
  }

  async function handleClearManualMapping() {
    savingManual = true;
    try {
      await clearManualTitleMapping(mangaId);
      toast.success('Manual mapping cleared; reverted to automatic resolution');
      showManualSection = false;
      await load();
      onReconciled?.();
    } catch (e) {
      toast.error(`Failed to clear manual mapping: ${message(e)}`);
    } finally {
      savingManual = false;
    }
  }

  $effect(() => {
    if (open) {
      load();
    } else {
      diag = null;
      error = null;
      showManualSection = false;
    }
  });
</script>

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={onClose}>
    <div class="modal-card diag-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-head">
        <div class="head-title">
          <Icon name="info" size={20} />
          <h2>Diagnostics & Authority Resolution — {mangaTitle}</h2>
        </div>
        <button class="btn icon close-btn" onclick={onClose} aria-label="Close">
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body">
        {#if loading}
          <div class="loading-state">
            <span class="spin"><Icon name="spinner" size={24} stroke={3} /></span>
            <span>Gathering title diagnostics…</span>
          </div>
        {:else if error}
          <Notice error={error} retry={load} />
        {:else if diag}
          <!-- Auto-Acquisition Policy Gate Banner -->
          <div class="gate-eval-banner code-{diag.autoAcquireGates.finalCode}">
            <div class="gate-header">
              <span class="gate-badge">{diag.autoAcquireGates.finalCode}</span>
              <span class="gate-decision-text">{diag.autoAcquireGates.finalReason}</span>
            </div>
            <div class="gate-matrix">
              <div class="matrix-item" class:passed={diag.autoAcquireGates.globalEnabled}>
                <span class="matrix-lbl">Master Switch</span>
                <span class="matrix-val">{diag.autoAcquireGates.globalEnabled ? 'ON' : 'OFF'}</span>
              </div>
              <div class="matrix-item" class:passed={diag.autoAcquireGates.titleOptIn}>
                <span class="matrix-lbl">Title Opt-In</span>
                <span class="matrix-val">{diag.autoAcquireGates.titleOptIn ? 'Opted In' : 'Disabled'}</span>
              </div>
              <div class="matrix-item info">
                <span class="matrix-lbl">Dry-Run Mode</span>
                <span class="matrix-val">{diag.autoAcquireGates.dryRun ? 'ON (Simulate)' : 'OFF (Live)'}</span>
              </div>
              <div class="matrix-item" class:passed={diag.autoAcquireGates.releaseGatePassed}>
                <span class="matrix-lbl">Release Gate</span>
                <span class="matrix-val">{diag.autoAcquireGates.releaseGatePassed ? 'Passed' : 'Blocked'}</span>
              </div>
              <div class="matrix-item" class:passed={diag.autoAcquireGates.ownershipGatePassed}>
                <span class="matrix-lbl">Ownership Gate</span>
                <span class="matrix-val">{diag.autoAcquireGates.ownershipGatePassed ? 'Passed' : 'Owned'}</span>
              </div>
              <div class="matrix-item" class:passed={diag.autoAcquireGates.correspondenceGatePassed}>
                <span class="matrix-lbl">Correspondence</span>
                <span class="matrix-val">{diag.autoAcquireGates.correspondenceGatePassed ? 'Passed' : 'Review'}</span>
              </div>
            </div>
          </div>

          <!-- Identity Conflicts Warning -->
          {#if diag.identity.conflicts.length > 0}
            <div class="conflict-alert">
              <Icon name="close" size={18} />
              <div class="conflict-content">
                <strong>Identity Conflicts Detected:</strong>
                <ul>
                  {#each diag.identity.conflicts as conflict}
                    <li>{conflict}</li>
                  {/each}
                </ul>
              </div>
            </div>
          {/if}

          <!-- Grid: Identity & MangaBaka Collection -->
          <div class="diag-grid">
            <!-- 1. Identity & Provenance -->
            <div class="diag-card">
              <div class="card-head">
                <Icon name="link" size={16} />
                <h3>Series Identity & Provenance</h3>
              </div>
              <div class="card-content">
                <div class="diag-field">
                  <span class="field-k">Mango Title ID:</span>
                  <span class="field-v">#{diag.identity.mangaId} ({diag.identity.sourceId === 0 ? 'Catalog Native' : `Source #${diag.identity.sourceId}`})</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">MangaBaka ID:</span>
                  <span class="field-v">
                    {#if diag.identity.mangabakaId}
                      <strong>{diag.identity.mangabakaId}</strong> {diag.identity.mangabakaTitle ? `(${diag.identity.mangabakaTitle})` : ''}
                      {#if diag.identity.manualOverride}
                        <span class="badge manual-badge">MANUAL OVERRIDE</span>
                      {/if}
                    {:else}
                      <span class="muted">Unlinked</span>
                    {/if}
                  </span>
                </div>
                <div class="diag-field">
                  <span class="field-k">AniList Tracker ID:</span>
                  <span class="field-v">{diag.identity.anilistId ? `#${diag.identity.anilistId}` : 'Not Linked'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Komga Series ID:</span>
                  <span class="field-v">{diag.identity.komgaSeriesId ? `<code>${diag.identity.komgaSeriesId}</code>` : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Identity Source:</span>
                  <span class="field-v"><code>{diag.identity.identitySource}</code></span>
                </div>
              </div>
              <div class="card-actions">
                <button
                  class="btn mini outline icon-text"
                  onclick={() => (showManualSection = !showManualSection)}
                >
                  <Icon name="sliders" size={14} />
                  {showManualSection ? 'Hide Resolution Controls' : 'Manual Identity Resolution'}
                </button>
                {#if diag.identity.manualOverride}
                  <button
                    class="btn mini warn icon-text"
                    disabled={savingManual}
                    onclick={handleClearManualMapping}
                    title="Clear manual override and revert to automatic resolution"
                  >
                    Revert to Auto
                  </button>
                {/if}
              </div>
            </div>

            <!-- 2. MangaBaka Collection -->
            <div class="diag-card">
              <div class="card-head">
                <Icon name="book" size={16} />
                <h3>MangaBaka Collection & Edition</h3>
              </div>
              <div class="card-content">
                <div class="diag-field">
                  <span class="field-k">Collection Status:</span>
                  <span class="field-v tag status-{diag.collection.collectionStatus}">
                    {diag.collection.collectionStatus}
                  </span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Resolved Collection:</span>
                  <span class="field-v">{diag.collection.resolvedCollectionId ? `<code>${diag.collection.resolvedCollectionId}</code>` : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Market & Edition:</span>
                  <span class="field-v">{diag.collection.market.toUpperCase()} · {diag.collection.edition}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Total Official Volumes:</span>
                  <span class="field-v">{diag.collection.totalVolumes != null ? diag.collection.totalVolumes : 'Unknown'}</span>
                </div>
                {#if diag.collection.reason}
                  <div class="diag-field">
                    <span class="field-k">Review Reason:</span>
                    <span class="field-v warning-text">{diag.collection.reason}</span>
                  </div>
                {/if}
              </div>
            </div>
          </div>

          <!-- Manual Resolution Controls Panel (Expandable) -->
          {#if showManualSection}
            <div class="manual-resolution-panel">
              <div class="panel-head">
                <Icon name="sliders" size={16} />
                <strong>Manual Identity & Collection Resolution Controls</strong>
              </div>
              <p class="panel-desc">
                Sets a durable, auditable manual mapping override that will never be replaced by automatic resolution.
              </p>

              <!-- Search / Select MangaBaka Title -->
              <div class="search-mb-row">
                <label class="field grow">
                  <span class="lbl">Search MangaBaka Catalog:</span>
                  <div class="input-with-btn">
                    <input
                      type="text"
                      bind:value={manualSearchQuery}
                      placeholder="Search title by name on MangaBaka…"
                      onkeydown={(e) => e.key === 'Enter' && searchMangaBaka()}
                    />
                    <button class="btn small" disabled={searchingMb || !manualSearchQuery.trim()} onclick={searchMangaBaka}>
                      {searchingMb ? 'Searching…' : 'Search'}
                    </button>
                  </div>
                </label>
              </div>

              {#if searchResults.length > 0}
                <div class="search-results-dropdown">
                  {#each searchResults as item}
                    <!-- svelte-ignore a11y_click_events_have_key_events -->
                    <div class="search-item" onclick={() => selectSearchResult(item)}>
                      <strong>{item.title}</strong>
                      <span class="item-meta">MangaBaka #{item.id} {item.year ? `(${item.year})` : ''}</span>
                    </div>
                  {/each}
                </div>
              {/if}

              <div class="manual-inputs-grid">
                <label class="field">
                  <span class="lbl">MangaBaka Series ID:</span>
                  <input
                    type="text"
                    bind:value={manualMbIdInput}
                    placeholder="e.g. 708"
                    onchange={() => {
                      const idNum = parseInt(manualMbIdInput.trim(), 10);
                      if (!isNaN(idNum)) loadCollections(idNum);
                    }}
                  />
                </label>

                <label class="field">
                  <span class="lbl">Edition:</span>
                  <input type="text" bind:value={manualEditionInput} placeholder="e.g. standard" />
                </label>

                <label class="field">
                  <span class="lbl">Collection / Format:</span>
                  {#if availableCollections.length > 0}
                    <select bind:value={manualCollectionId}>
                      {#each availableCollections as col}
                        <option value={col.id}>{col.name} ({col.language || 'en'}) — {col.totalWorks ? `${col.totalWorks} vols` : 'unknown'}</option>
                      {/each}
                    </select>
                  {:else}
                    <input
                      type="text"
                      bind:value={manualCollectionId}
                      placeholder={loadingCollections ? 'Loading collections…' : 'Collection ID or UUID'}
                    />
                  {/if}
                </label>
              </div>

              <div class="panel-foot">
                <button
                  class="btn spot icon-text"
                  disabled={savingManual || !manualMbIdInput.trim()}
                  onclick={handleApplyManualMapping}
                >
                  <Icon name="check" size={14} />
                  {savingManual ? 'Saving Override…' : 'Apply Manual Override'}
                </button>
              </div>
            </div>
          {/if}

          <!-- Grid: Physical Ownership & OpenTome Intelligence -->
          <div class="diag-grid">
            <!-- 3. Physical Ownership Accounting -->
            <div class="diag-card">
              <div class="card-head">
                <Icon name="bookmark" size={16} />
                <h3>Physical Ownership Breakdown</h3>
              </div>
              <div class="card-content">
                <div class="diag-field">
                  <span class="field-k">Total Owned Count:</span>
                  <span class="field-v"><strong>{diag.ownership.totalOwnedCount} volumes</strong></span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Komga Owned Volumes:</span>
                  <span class="field-v">{diag.ownership.komgaOwnedVolumes.length ? diag.ownership.komgaOwnedVolumes.join(', ') : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Mango Local Downloads:</span>
                  <span class="field-v">{diag.ownership.mangoLocalOwnedVolumes.length ? diag.ownership.mangoLocalOwnedVolumes.join(', ') : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Local DB Query:</span>
                  <span class="field-v">{diag.ownership.localQuerySkipped ? 'Skipped (manga_id is NULL)' : 'Queried'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Missing Released:</span>
                  <span class="field-v {diag.ownership.missingVolumes.length ? 'warning-text' : 'success-text'}">
                    {diag.ownership.missingVolumes.length ? diag.ownership.missingVolumes.join(', ') : 'None (Up to Date)'}
                  </span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Future Scheduled:</span>
                  <span class="field-v">{diag.ownership.futureVolumes.length ? diag.ownership.futureVolumes.join(', ') : 'None'}</span>
                </div>
              </div>
            </div>

            <!-- 4. OpenTome Release Intelligence -->
            <div class="diag-card">
              <div class="card-head">
                <Icon name="clock" size={16} />
                <h3>OpenTome Release Intelligence</h3>
              </div>
              <div class="card-content">
                <div class="diag-field">
                  <span class="field-k">Tracked Schedule:</span>
                  <span class="field-v">{diag.releaseIntelligence.opentomeTracked ? 'Active' : 'Unmapped'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">OpenTome Work ID:</span>
                  <span class="field-v">{diag.releaseIntelligence.workId ? `<code>${diag.releaseIntelligence.workId}</code>` : 'None'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Mapping Confidence:</span>
                  <span class="field-v">{diag.releaseIntelligence.mappingConfidence || 'N/A'}</span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Next Expected Release:</span>
                  <span class="field-v">
                    {#if diag.releaseIntelligence.nextReleaseVolume}
                      <strong>Volume {diag.releaseIntelligence.nextReleaseVolume}</strong> ({diag.releaseIntelligence.nextReleaseDate || 'No date'})
                    {:else}
                      None scheduled
                    {/if}
                  </span>
                </div>
                <div class="diag-field">
                  <span class="field-k">Release Discovery Targets:</span>
                  <span class="field-v">{diag.releaseIntelligence.totalReleaseTargets} targets tracked</span>
                </div>
              </div>
            </div>
          </div>

          <!-- Active Reconciliation Event if present -->
          {#if diag.reconciliation.activeEvent}
            <div class="active-event-card">
              <div class="event-head">
                <Icon name="bell" size={16} />
                <strong>Active Attention Event: {diag.reconciliation.activeEvent.eventType}</strong>
                <span class="event-time">{ago(diag.reconciliation.activeEvent.createdAt * 1000)}</span>
              </div>
              <p class="event-msg">{diag.reconciliation.activeEvent.message}</p>
              {#if diag.reconciliation.activeEvent.outboxStatus}
                <div class="event-outbox-info">
                  Outbox status: <code>{diag.reconciliation.activeEvent.outboxStatus}</code>
                  {diag.reconciliation.activeEvent.outboxAttempts ? `(${diag.reconciliation.activeEvent.outboxAttempts} delivery attempts)` : ''}
                </div>
              {/if}
            </div>
          {/if}

          <!-- 5. Release Availability Watchers (M6.8.4) -->
          <div class="diag-card watcher-section">
            <div class="card-head">
              <div class="head-title">
                <Icon name="search" size={16} />
                <h3>Release Availability Watchers (M6.8.4)</h3>
              </div>
              <div class="card-actions">
                <button
                  class="btn mini outline icon-text"
                  disabled={refreshingAvailability}
                  onclick={handleRefreshAvailability}
                  title="Trigger immediate availability check for missing/upcoming volumes"
                >
                  <Icon name="refresh" size={14} />
                  {refreshingAvailability ? 'Checking…' : 'Check Availability'}
                </button>
              </div>
            </div>
            <div class="card-content">
              <div class="watcher-controls-bar">
                <div class="watcher-status-pill">
                  <span class="control-lbl">Monitoring:</span>
                  <button
                    class="btn mini pill"
                    class:spot={availability?.monitoringEnabled}
                    disabled={updatingAvailability}
                    onclick={handleToggleMonitoring}
                  >
                    {availability?.monitoringEnabled ? 'Enabled' : 'Disabled'}
                  </button>
                </div>
                <div class="watcher-status-pill">
                  <span class="control-lbl">Notifications:</span>
                  <button
                    class="btn mini pill"
                    class:spot={availability?.notifyEnabled}
                    disabled={updatingAvailability}
                    onclick={handleToggleNotify}
                  >
                    {availability?.notifyEnabled ? 'Enabled' : 'Disabled'}
                  </button>
                </div>
                <div class="watcher-status-pill">
                  <span class="control-lbl">Active Watchers:</span>
                  <strong>{availability?.activeWatchersCount ?? 0} volumes</strong>
                </div>
              </div>

              {#if availability && availability.watchers.length > 0}
                <div class="watchers-table-wrapper">
                  <table class="watchers-table">
                    <thead>
                      <tr>
                        <th>Volume</th>
                        <th>Release Date</th>
                        <th>State</th>
                        <th>Provider</th>
                        <th>Interval</th>
                        <th>Next Check</th>
                        <th>Result / Note</th>
                      </tr>
                    </thead>
                    <tbody>
                      {#each availability.watchers as w}
                        <tr class="watcher-row">
                          <td class="vol-cell"><strong>Vol {w.volumeNumber}</strong></td>
                          <td class="date-cell">{w.releaseDate || 'Unknown'}</td>
                          <td>
                            <span class="tag watcher-state watcher-state-{w.state.toLowerCase()}">
                              {w.state}
                            </span>
                          </td>
                          <td class="provider-cell"><code>{w.preferredProvider}</code></td>
                          <td class="interval-cell">{Math.round(w.checkIntervalSecs / 3600)}h</td>
                          <td class="next-cell">
                            {#if w.nextCheckAt > 0}
                              {new Date(w.nextCheckAt * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
                            {:else}
                              <span class="muted">—</span>
                            {/if}
                          </td>
                          <td class="result-cell">
                            {#if w.foundCandidateId}
                              <span class="found-text">Found ({w.foundProvider}): {w.foundReleaseTitle || w.foundCandidateId}</span>
                            {:else if w.stopReason}
                              <span class="stop-text">{w.stopReason}</span>
                            {:else if w.lastError}
                              <span class="error-text">{w.lastError}</span>
                            {:else}
                              <span class="muted">Awaiting release</span>
                            {/if}
                          </td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {:else}
                <div class="no-watchers-msg">
                  <p class="muted">No unowned volumes actively monitored. All released volumes are owned or reconciled.</p>
                </div>
              {/if}
            </div>
          </div>

          <!-- 6. Durable Publication Receipts (M6.9) -->
          <div class="diag-card receipts-section">
            <div class="card-head">
              <div class="head-title">
                <Icon name="bookmark" size={16} />
                <h3>Durable Publication Receipts (M6.9)</h3>
              </div>
              <div class="card-actions">
                <span class="receipt-count-pill">{receipts.length} verified publications</span>
              </div>
            </div>
            <div class="card-content">
              {#if receipts.length > 0}
                <div class="watchers-table-wrapper">
                  <table class="watchers-table">
                    <thead>
                      <tr>
                        <th>Unit</th>
                        <th>Canonical Path</th>
                        <th>Size</th>
                        <th>SHA-256</th>
                        <th>Metadata</th>
                        <th>Komga</th>
                        <th>Actions</th>
                      </tr>
                    </thead>
                    <tbody>
                      {#each receipts as r}
                        <tr class="receipt-row">
                          <td class="vol-cell">
                            <strong>{r.isVolume ? `Vol ${r.volumeNumber}` : `Ch ${r.chapterNumber}`}</strong>
                          </td>
                          <td class="path-cell" title={r.canonicalPath}>
                            <code>{r.canonicalPath.split('/').pop() || r.canonicalPath}</code>
                          </td>
                          <td class="size-cell">
                            {(r.fileSizeBytes / (1024 * 1024)).toFixed(1)} MB
                          </td>
                          <td class="sha-cell" title={r.fileSha256}>
                            <code>{r.fileSha256.slice(0, 8)}…</code>
                          </td>
                          <td>
                            <span class="tag metadata-state metadata-state-{r.metadataStatus.toLowerCase()}">
                              {r.metadataStatus}
                            </span>
                          </td>
                          <td>
                            <span class="tag komga-notif-state" class:spot={r.komgaNotified}>
                              {r.komgaNotified ? 'Notified' : 'Pending'}
                            </span>
                          </td>
                          <td class="receipt-actions-cell">
                            <button
                              class="btn mini outline"
                              disabled={verifyingReceiptId === r.id}
                              onclick={() => handleVerifyReceipt(r)}
                              title="Verify CBZ integrity, size, and SHA-256 on disk"
                            >
                              {verifyingReceiptId === r.id ? 'Verifying…' : 'Verify'}
                            </button>
                            <button
                              class="btn mini outline"
                              disabled={rescanningReceiptId === r.id}
                              onclick={() => handleRescanReceipt(r)}
                              title="Trigger Komga library rescan for this publication"
                            >
                              {rescanningReceiptId === r.id ? 'Scanning…' : 'Rescan'}
                            </button>
                            {#if r.metadataStatus === 'failed'}
                              <button
                                class="btn mini spot"
                                disabled={repairingReceiptId === r.id}
                                onclick={() => handleRepairReceiptMetadata(r)}
                                title="Repair ComicInfo.xml metadata without re-downloading archive"
                              >
                                {repairingReceiptId === r.id ? 'Repairing…' : 'Repair Meta'}
                              </button>
                            {/if}
                          </td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                </div>
              {:else}
                <div class="no-receipts-msg">
                  <p class="muted">No publication receipts recorded for this series yet.</p>
                </div>
              {/if}
            </div>
          </div>

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

  .diag-modal {
    background: var(--bg-surface, #1e1e24);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.12));
    border-radius: 12px;
    width: 100%;
    max-width: 900px;
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
    gap: 1.25rem;
  }

  .modal-foot {
    display: flex;
    justify-content: flex-end;
    padding: 0.75rem 1.25rem;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    background: var(--bg-surface-subtle, rgba(255, 255, 255, 0.02));
  }

  .gate-eval-banner {
    background: var(--bg-card, rgba(255, 255, 255, 0.04));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .gate-header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }

  .gate-badge {
    background: #3b82f6;
    color: #fff;
    font-size: 0.75rem;
    font-weight: 700;
    padding: 0.2rem 0.6rem;
    border-radius: 4px;
    letter-spacing: 0.04em;
  }

  .gate-decision-text {
    font-weight: 600;
    font-size: 0.95rem;
  }

  .gate-matrix {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
    gap: 0.5rem;
    background: rgba(0, 0, 0, 0.2);
    padding: 0.5rem;
    border-radius: 6px;
  }

  .matrix-item {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
    padding: 0.4rem;
    border-radius: 4px;
    background: rgba(255, 255, 255, 0.02);
  }

  .matrix-item.passed {
    border-left: 3px solid #10b981;
  }

  .matrix-item:not(.passed):not(.info) {
    border-left: 3px solid #f59e0b;
  }

  .matrix-item.info {
    border-left: 3px solid #8b5cf6;
  }

  .matrix-lbl {
    font-size: 0.7rem;
    color: var(--muted, #8b949e);
    text-transform: uppercase;
  }

  .matrix-val {
    font-size: 0.8rem;
    font-weight: 600;
  }

  .conflict-alert {
    background: rgba(239, 68, 68, 0.1);
    border: 1px solid rgba(239, 68, 68, 0.3);
    border-radius: 8px;
    padding: 0.85rem 1rem;
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    color: #f87171;
  }

  .conflict-content ul {
    margin: 0.25rem 0 0;
    padding-left: 1.25rem;
    font-size: 0.85rem;
  }

  .diag-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(360px, 1fr));
    gap: 1rem;
  }

  .diag-card {
    background: var(--bg-card, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .card-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.06));
    padding-bottom: 0.5rem;
  }

  .card-head h3 {
    margin: 0;
    font-size: 0.95rem;
    font-weight: 600;
  }

  .card-content {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }

  .diag-field {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 0.75rem;
    font-size: 0.85rem;
  }

  .field-k {
    color: var(--muted, #8b949e);
    flex-shrink: 0;
  }

  .field-v {
    text-align: right;
    word-break: break-all;
  }

  .manual-badge {
    background: rgba(139, 92, 246, 0.2);
    color: #c4b5fd;
    font-size: 0.7rem;
    padding: 0.1rem 0.4rem;
    border-radius: 4px;
    margin-left: 0.3rem;
  }

  .card-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: auto;
    padding-top: 0.5rem;
    border-top: 1px solid var(--border-color, rgba(255, 255, 255, 0.04));
  }

  .manual-resolution-panel {
    background: rgba(59, 130, 246, 0.05);
    border: 1px solid rgba(59, 130, 246, 0.2);
    border-radius: 8px;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .panel-head {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.95rem;
  }

  .panel-desc {
    margin: 0;
    font-size: 0.8rem;
    color: var(--muted, #8b949e);
  }

  .input-with-btn {
    display: flex;
    gap: 0.4rem;
  }

  .input-with-btn input {
    flex-grow: 1;
  }

  .search-results-dropdown {
    background: var(--bg-surface, #1e1e24);
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.15));
    border-radius: 6px;
    max-height: 180px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
  }

  .search-item {
    padding: 0.5rem 0.75rem;
    cursor: pointer;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.05));
    display: flex;
    justify-content: space-between;
    font-size: 0.85rem;
  }

  .search-item:hover {
    background: rgba(255, 255, 255, 0.05);
  }

  .search-item .item-meta {
    color: var(--muted, #8b949e);
  }

  .manual-inputs-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(200px, 1fr));
    gap: 0.75rem;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .field .lbl {
    font-size: 0.75rem;
    color: var(--muted, #8b949e);
  }

  .active-event-card {
    background: rgba(245, 158, 11, 0.08);
    border: 1px solid rgba(245, 158, 11, 0.25);
    border-radius: 8px;
    padding: 0.85rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }

  .event-head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
    color: #fde68a;
  }

  .event-time {
    margin-left: auto;
    color: var(--muted, #8b949e);
    font-size: 0.75rem;
  }

  .event-msg {
    margin: 0;
    font-size: 0.85rem;
  }

  .event-outbox-info {
    font-size: 0.75rem;
    color: var(--muted, #8b949e);
  }

  .warning-text {
    color: #fca5a5;
  }

  .success-text {
    color: #86efac;
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 3rem;
    gap: 0.75rem;
    color: var(--muted, #8b949e);
  }

  .watcher-section {
    grid-column: 1 / -1;
  }

  .watcher-controls-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 1.25rem;
    padding-bottom: 0.75rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
    margin-bottom: 0.75rem;
  }

  .watcher-status-pill {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
  }

  .control-lbl {
    color: var(--muted, #8b949e);
  }

  .watchers-table-wrapper {
    overflow-x: auto;
  }

  .watchers-table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.82rem;
    text-align: left;
  }

  .watchers-table th {
    padding: 0.4rem 0.6rem;
    font-weight: 600;
    color: var(--muted, #8b949e);
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.08));
  }

  .watchers-table td {
    padding: 0.5rem 0.6rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.04));
    vertical-align: middle;
  }

  .watcher-state {
    font-size: 0.7rem;
    font-weight: 600;
    padding: 2px 6px;
    border-radius: 4px;
    text-transform: uppercase;
  }

  .watcher-state-polling {
    background: rgba(59, 130, 246, 0.2);
    color: #93c5fd;
    border: 1px solid rgba(59, 130, 246, 0.3);
  }

  .watcher-state-candidate_found {
    background: rgba(34, 197, 94, 0.2);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .watcher-state-ownership_detected {
    background: rgba(168, 85, 247, 0.2);
    color: #d8b4fe;
    border: 1px solid rgba(168, 85, 247, 0.3);
  }

  .watcher-state-stopped {
    background: rgba(107, 114, 128, 0.2);
    color: #9ca3af;
    border: 1px solid rgba(107, 114, 128, 0.3);
  }

  .watcher-state-failed_permanent {
    background: rgba(239, 68, 68, 0.2);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .found-text {
    color: #86efac;
    font-size: 0.78rem;
  }

  .stop-text {
    color: var(--muted, #8b949e);
    font-size: 0.78rem;
  }

  .error-text {
    color: #fca5a5;
    font-size: 0.78rem;
  }

  .no-watchers-msg {
    padding: 0.75rem 0;
  }

  .receipts-section {
    grid-column: 1 / -1;
  }

  .receipt-count-pill {
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--muted, #8b949e);
  }

  .receipt-actions-cell {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
  }

  .metadata-state-complete {
    background: rgba(34, 197, 94, 0.2);
    color: #86efac;
    border: 1px solid rgba(34, 197, 94, 0.3);
  }

  .metadata-state-failed {
    background: rgba(239, 68, 68, 0.2);
    color: #fca5a5;
    border: 1px solid rgba(239, 68, 68, 0.3);
  }

  .metadata-state-pending {
    background: rgba(245, 158, 11, 0.2);
    color: #fde68a;
    border: 1px solid rgba(245, 158, 11, 0.3);
  }

  .no-receipts-msg {
    padding: 0.75rem 0;
  }
</style>
