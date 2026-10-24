<script lang="ts">
  import { get, message, send, type HistoryEntry } from '../lib/api';
  import { confirm } from '../lib/confirm.svelte';
  import Icon from '../lib/Icon.svelte';
  import { keep, recall } from '../lib/keep';
  import Notice from '../lib/Notice.svelte';
  import { toast } from '../lib/toast.svelte';
  import { dayKey, dayLabel, whenVisible } from '../lib/util';

  const PAGE = 100;
  // The most the server gives in one answer.
  const MOST = 500;

  // What was here last time shows at once; the list is then asked for again.
  const kept = recall<{ entries: HistoryEntry[]; more: boolean }>('history');
  let entries = $state<HistoryEntry[]>(kept?.entries ?? []);
  let more = $state(kept?.more ?? false);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let clearing = $state(false);
  let busyEntry = $state<Record<number, boolean>>({});

  // Entries arrive newest first, so each new day starts a new group.
  const days = $derived.by(() => {
    const groups: { day: number; entries: HistoryEntry[] }[] = [];
    for (const entry of entries) {
      const day = dayKey(entry.readAt);
      if (groups.at(-1)?.day === day) groups.at(-1)!.entries.push(entry);
      else groups.push({ day, entries: [entry] });
    }
    return groups;
  });

  async function load(fresh: boolean) {
    loading = true;
    error = null;
    try {
      // Asked for afresh, the list comes back as long as it was, so the place in it holds.
      const limit = fresh ? Math.min(MOST, Math.max(PAGE, entries.length)) : PAGE;
      const page = (await get<{ entries: HistoryEntry[] }>(`/history?limit=${limit}&offset=${fresh ? 0 : entries.length}`)).entries;
      entries = fresh ? page : [...entries, ...page];
      more = page.length === limit;
      keep('history', { entries: $state.snapshot(entries), more });
    } catch (e) {
      error = message(e);
    }
    loading = false;
  }
  load(true);

  async function forget(entry: HistoryEntry) {
    const ok = await confirm({
      title: 'Remove from History?',
      message: `Remove "${entry.chapterName}" of "${entry.title}" from your reading history?`,
      confirmText: 'Remove',
      danger: true,
    });
    if (!ok) return;
    busyEntry[entry.chapterId] = true;
    entries = entries.filter((e) => e.chapterId !== entry.chapterId);
    try {
      await send('DELETE', `/history/${entry.chapterId}`);
      toast.info(`Removed ${entry.chapterName} from history`);
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
      load(true);
    } finally {
      busyEntry[entry.chapterId] = false;
    }
  }

  async function clear() {
    const ok = await confirm({
      title: 'Clear Reading History?',
      message: 'All your reading history will be cleared. This cannot be undone.',
      confirmText: 'Clear history',
      danger: true,
    });
    if (!ok) return;
    clearing = true;
    try {
      await send('DELETE', '/history');
      entries = [];
      more = false;
      keep('history', { entries, more });
      toast.info('Reading history cleared');
    } catch (e) {
      const msg = message(e);
      error = msg;
      toast.error(msg);
    } finally {
      clearing = false;
    }
  }

  const time = (seconds: number) => new Date(seconds * 1000).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' });

  function place(entry: HistoryEntry): string {
    if (entry.read) return 'finished';
    return `page ${entry.page + 1}${entry.pageCount ? ` of ${entry.pageCount}` : ''}`;
  }
</script>

<div class="page">
  <div class="page-head">
    <h1 class="display">HISTORY</h1>
    {#if entries.length}
      <button class="btn" disabled={clearing} onclick={clear}>
        {#if clearing}
          <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
        {:else}
          <Icon name="trash" size={18} />
        {/if}
        Clear history
      </button>
    {/if}
  </div>

  {#each days as group (group.day)}
    <section>
      <div class="section-head"><h2 class="display">{dayLabel(group.day)}</h2></div>
      <ul class="rows">
        {#each group.entries as entry (entry.chapterId)}
          <li>
            <a class="main" href="#/title/{entry.mangaId}">
              <strong>{entry.title}</strong>
              <span>{entry.chapterName} · {place(entry)} · {time(entry.readAt)}</span>
            </a>
            <span class="tools">
              <a class="btn" href="#/read/{entry.chapterId}">{entry.read ? 'Read again' : 'Resume'}<Icon name="arrow" size={18} stroke={3} /></a>
              <button
                class="btn icon"
                aria-label="Remove {entry.chapterName} of {entry.title} from the history"
                disabled={busyEntry[entry.chapterId]}
                onclick={() => forget(entry)}
              >
                {#if busyEntry[entry.chapterId]}
                  <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span>
                {:else}
                  <Icon name="close" />
                {/if}
              </button>
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {/each}

  <Notice {loading} {error} retry={() => load(true)} />
  {#if !loading && !error && entries.length === 0}
    <Notice>Nothing read yet. Chapters show up here once you open them.</Notice>
  {/if}
  {#if more && !loading}
    <div class="more" use:whenVisible={() => more && !loading && load(false)}>
      <button class="btn" onclick={() => load(false)}>Load more</button>
    </div>
  {/if}
</div>

<style>
  section {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .section-head h2 {
    font-size: 20px;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    flex-wrap: wrap;
  }

  .more {
    display: flex;
    justify-content: center;
  }
</style>
