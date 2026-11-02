<script lang="ts">
  import { onDestroy } from 'svelte';
  import Icon from './Icon.svelte';
  import Cover from './Cover.svelte';
  import {
    searchMangaBakaCatalog,
    addMangaBakaCatalog,
    type MangaBakaSearchResult,
    message,
  } from './api';
  import { toast } from './toast.svelte';
  import { go } from './router.svelte';

  let {
    open = false,
    onclose,
    onadded,
  }: {
    open: boolean;
    onclose: () => void;
    onadded?: (mangaId: number) => void;
  } = $props();

  let query = $state('');
  let searching = $state(false);
  let searchedQuery = $state('');
  let results = $state<MangaBakaSearchResult[]>([]);
  let searchError = $state<string | null>(null);
  let addingId = $state<number | null>(null);
  let searchTimeout: ReturnType<typeof setTimeout> | null = null;
  let abortController: AbortController | null = null;

  onDestroy(() => {
    if (searchTimeout) clearTimeout(searchTimeout);
    if (abortController) abortController.abort();
  });

  function handleQueryInput(val: string) {
    query = val;
    if (searchTimeout) clearTimeout(searchTimeout);
    if (!val.trim()) {
      results = [];
      searchedQuery = '';
      searchError = null;
      return;
    }
    searchTimeout = setTimeout(() => {
      executeSearch();
    }, 400);
  }

  async function executeSearch() {
    const trimmed = query.trim();
    if (!trimmed) return;
    if (searchTimeout) clearTimeout(searchTimeout);

    if (abortController) abortController.abort();
    abortController = new AbortController();

    searching = true;
    searchError = null;
    searchedQuery = trimmed;

    try {
      const res = await searchMangaBakaCatalog(trimmed, abortController.signal);
      results = res;
    } catch (e: any) {
      if (e?.name !== 'AbortError') {
        searchError = message(e);
        results = [];
      }
    } finally {
      searching = false;
    }
  }

  async function handleAdd(item: MangaBakaSearchResult, convertExisting = false) {
    addingId = item.mangabakaId;
    try {
      const resp = await addMangaBakaCatalog(item.mangabakaId, convertExisting);
      toast.success(resp.message || `Added ${resp.title} to Mango`);
      onadded?.(resp.mangaId);
      onclose();
      go('/manga/' + resp.mangaId);
    } catch (e) {
      toast.error(message(e));
    } finally {
      addingId = null;
    }
  }

  function handleOpen(mangaId: number) {
    onclose();
    go('/manga/' + mangaId);
  }

  function handleKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') {
      event.preventDefault();
      onclose();
    }
  }

  function backdropClick(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      onclose();
    }
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
  <div class="backdrop" onclick={backdropClick} role="presentation">
    <div
      class="panel dialog add-manga-dialog"
      role="dialog"
      aria-modal="true"
      aria-labelledby="add-manga-title"
    >
      <div class="head">
        <div>
          <h2 id="add-manga-title" class="display">ADD TO MANGO</h2>
          <p class="subtitle">Search MangaBaka to add manga directly as a catalog title without web scraping.</p>
        </div>
        <button class="btn icon close" aria-label="Close dialog" onclick={onclose}>
          <Icon name="close" size={18} stroke={3} />
        </button>
      </div>

      <form class="search-form" onsubmit={(e) => { e.preventDefault(); executeSearch(); }}>
        <div class="input-wrap">
          <Icon name="search" size={18} />
          <input
            type="search"
            class="search-input"
            placeholder="Search by title, MangaBaka ID, or URL..."
            value={query}
            oninput={(e) => handleQueryInput((e.target as HTMLInputElement).value)}
          />
          {#if query}
            <button
              type="button"
              class="clear-btn"
              onclick={() => { query = ''; results = []; searchedQuery = ''; searchError = null; }}
              aria-label="Clear search"
            >
              <Icon name="close" size={16} />
            </button>
          {/if}
        </div>
        <button type="submit" class="btn primary search-btn" disabled={searching || !query.trim()}>
          {#if searching}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
            <span>Searching...</span>
          {:else}
            <span>Search</span>
          {/if}
        </button>
      </form>

      {#if searchError}
        <div class="notice error-notice">
          <Icon name="info" size={16} />
          <span>{searchError}</span>
          <button class="btn subtle" onclick={executeSearch}>Retry</button>
        </div>
      {/if}

      <div class="results-container">
        {#if searching && results.length === 0}
          <div class="empty-state">
            <span class="spin large-spin"><Icon name="spinner" size={32} stroke={3} /></span>
            <p>Searching MangaBaka catalog...</p>
          </div>
        {:else if searchedQuery && results.length === 0 && !searching}
          <div class="empty-state">
            <p class="no-results-title">No manga found for "{searchedQuery}"</p>
            <p class="subtext">Make sure the title is spelled correctly, or search directly by MangaBaka ID (e.g. 83 for Blue Lock).</p>
          </div>
        {:else if results.length > 0}
          <div class="results-list">
            {#each results as item (item.mangabakaId)}
              <div class="result-card">
                <div class="cover-thumb">
                  <Cover src={item.coverUrl ?? null} title={item.title} />
                </div>
                <div class="item-info">
                  <div class="title-row">
                    <h3 class="item-title">{item.title}</h3>
                    <a
                      href={`https://mangabaka.com/series/${item.mangabakaId}`}
                      target="_blank"
                      rel="noopener noreferrer"
                      class="mb-id-badge"
                      title="View on MangaBaka"
                    >
                      MB #{item.mangabakaId}
                    </a>
                  </div>

                  {#if item.englishTitle && item.englishTitle !== item.title}
                    <div class="alt-title">{item.englishTitle}</div>
                  {:else if item.romajiTitle && item.romajiTitle !== item.title}
                    <div class="alt-title">{item.romajiTitle}</div>
                  {/if}

                  <div class="meta-row">
                    {#if item.status}
                      <span class="meta-tag status-tag">{item.status}</span>
                    {/if}
                    {#if item.totalVolumes}
                      <span class="meta-tag">{item.totalVolumes} {item.totalVolumes === 1 ? 'volume' : 'volumes'}</span>
                    {/if}
                    {#if item.authors && item.authors.length > 0}
                      <span class="authors-tag">{item.authors.join(', ')}</span>
                    {/if}
                  </div>

                  {#if item.genres && item.genres.length > 0}
                    <div class="genres-row">
                      {#each item.genres.slice(0, 4) as genre}
                        <span class="genre-pill">{genre}</span>
                      {/each}
                    </div>
                  {/if}

                  {#if item.description}
                    <p class="item-desc">{item.description}</p>
                  {/if}

                  <div class="card-actions">
                    {#if item.alreadyInMango}
                      {#if item.existingIsCatalog}
                        <span class="status-indicator">Already in Mango (Catalog)</span>
                        <button
                          class="btn primary action-btn"
                          onclick={() => handleOpen(item.existingMangaId!)}
                        >
                          Open Title
                        </button>
                      {:else}
                        <span class="status-indicator">Source-backed in Mango</span>
                        <button
                          class="btn action-btn"
                          onclick={() => handleOpen(item.existingMangaId!)}
                        >
                          Open Source Title
                        </button>
                        <button
                          class="btn primary action-btn"
                          disabled={addingId === item.mangabakaId}
                          onclick={() => handleAdd(item, true)}
                        >
                          {#if addingId === item.mangabakaId}
                            <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                            <span>Converting...</span>
                          {:else}
                            <span>Convert to Catalog</span>
                          {/if}
                        </button>
                      {/if}
                    {:else}
                      <button
                        class="btn primary action-btn"
                        disabled={addingId === item.mangabakaId}
                        onclick={() => handleAdd(item, false)}
                      >
                        {#if addingId === item.mangabakaId}
                          <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                          <span>Adding...</span>
                        {:else}
                          <Icon name="plus" size={16} stroke={3} />
                          <span>Add to Mango</span>
                        {/if}
                      </button>
                    {/if}
                  </div>
                </div>
              </div>
            {/each}
          </div>
        {:else}
          <div class="hint-state">
            <Icon name="search" size={40} />
            <p>Type a manga title or MangaBaka ID to find works</p>
            <span class="subtext">Titles added from MangaBaka have first-class Komga ownership, OpenTome release intelligence, and Nyaa acquisition discovery.</span>
          </div>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--gutter);
    background-color: rgba(0, 0, 0, 0.6);
    backdrop-filter: blur(2px);
  }

  .dialog {
    width: 100%;
    max-width: 760px;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border: 3px solid var(--ink);
    box-shadow: var(--sh-8);
    border-radius: 4px;
    padding: 24px;
    box-sizing: border-box;
    overflow: hidden;
  }

  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 16px;
  }

  .head h2 {
    font-size: 24px;
    line-height: 1.1;
    margin: 0;
  }

  .subtitle {
    margin: 4px 0 0;
    font-size: 13px;
    color: var(--muted);
  }

  .close {
    flex: none;
    width: 36px;
    height: 36px;
  }

  .search-form {
    display: flex;
    gap: 8px;
    margin-bottom: 16px;
  }

  .input-wrap {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: 3px;
  }

  .search-input {
    flex: 1;
    border: none;
    background: transparent;
    font-size: 15px;
    font-family: inherit;
    color: var(--ink);
    outline: none;
  }

  .clear-btn {
    border: none;
    background: none;
    cursor: pointer;
    color: var(--muted);
    padding: 2px;
    display: flex;
    align-items: center;
  }

  .clear-btn:hover {
    color: var(--ink);
  }

  .search-btn {
    flex: none;
    min-width: 100px;
  }

  .notice {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 10px 12px;
    margin-bottom: 12px;
    border: 2px solid var(--ink);
    border-radius: 3px;
    font-size: 13px;
  }

  .error-notice {
    background: #ffebe8;
    color: #c92a2a;
  }

  .results-container {
    flex: 1;
    overflow-y: auto;
    min-height: 250px;
    max-height: calc(85vh - 200px);
    border-top: 1px solid color-mix(in srgb, var(--ink) 15%, transparent);
    padding-top: 16px;
  }

  .results-list {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .result-card {
    display: flex;
    gap: 16px;
    padding: 12px;
    background: var(--paper);
    border: 2px solid var(--ink);
    border-radius: 3px;
    box-shadow: var(--sh-3);
  }

  .cover-thumb {
    width: 80px;
    height: 120px;
    flex: none;
    border: 1px solid var(--ink);
    border-radius: 2px;
    overflow: hidden;
  }

  .item-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .title-row {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .item-title {
    font-size: 16px;
    font-weight: 800;
    margin: 0;
    overflow-wrap: anywhere;
  }

  .mb-id-badge {
    font-size: 11px;
    font-weight: 700;
    font-family: monospace;
    background: color-mix(in srgb, var(--spot) 35%, white);
    border: 1px solid var(--ink);
    padding: 2px 6px;
    border-radius: 2px;
    text-decoration: none;
    color: var(--ink);
    flex: none;
  }

  .mb-id-badge:hover {
    background: var(--spot);
  }

  .alt-title {
    font-size: 12px;
    color: var(--muted);
    margin-top: 2px;
  }

  .meta-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 6px;
    font-size: 12px;
  }

  .meta-tag {
    background: color-mix(in srgb, var(--ink) 8%, transparent);
    padding: 1px 6px;
    border-radius: 2px;
    font-weight: 600;
  }

  .status-tag {
    background: color-mix(in srgb, var(--spot) 20%, transparent);
    font-weight: 700;
  }

  .authors-tag {
    color: var(--muted);
  }

  .genres-row {
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
    margin-top: 6px;
  }

  .genre-pill {
    font-size: 11px;
    background: color-mix(in srgb, var(--ink) 6%, transparent);
    padding: 1px 6px;
    border-radius: 10px;
  }

  .item-desc {
    font-size: 12px;
    color: var(--ink);
    margin: 8px 0 0;
    line-height: 1.4;
    display: -webkit-box;
    line-clamp: 2;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }

  .card-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: auto;
    padding-top: 10px;
  }

  .status-indicator {
    font-size: 12px;
    font-weight: 700;
    color: var(--muted);
    margin-right: auto;
  }

  .action-btn {
    padding: 6px 14px;
    font-size: 13px;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .empty-state, .hint-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    text-align: center;
    padding: 40px 20px;
    color: var(--muted);
  }

  .hint-state p, .no-results-title {
    font-size: 16px;
    font-weight: 700;
    color: var(--ink);
    margin: 12px 0 4px;
  }

  .subtext {
    font-size: 13px;
    max-width: 440px;
    line-height: 1.4;
  }

  .spin {
    display: inline-flex;
    animation: spin 1s linear infinite;
  }

  .large-spin {
    margin-bottom: 12px;
  }

  @keyframes spin {
    from { transform: rotate(0deg); }
    to { transform: rotate(360deg); }
  }
</style>
