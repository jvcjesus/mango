<script lang="ts">
  import {
    get,
    message,
    send,
    getTitleAcquisitionReleases,
    acquireRelease,
    acquireCollection,
    inspectCollection,
    getTitleProfile,
    setTitleProfile,
    getAcquisitionProfiles,
    getTitleMissingContent,
    planTitleMissingContent,
    getTitlePlan,
    type AcquisitionProfile,
    type MissingContentSummary,
    type AcquisitionPlan,
    type AcquisitionPlanItem,
    type AcquisitionCandidate,
    type AcquisitionRelease,
    type CollectionInspection,
    type Category,
    type Chapter,
    type OwnershipStatus,
    type Title,
    type TitleOwnership,
  } from '../lib/api';
  import { confirm } from '../lib/confirm.svelte';
  import Cover from '../lib/Cover.svelte';
  import Icon from '../lib/Icon.svelte';
  import { keep, recall } from '../lib/keep';
  import Notice from '../lib/Notice.svelte';
  import { pref, setPref } from '../lib/prefs.svelte';
  import { back } from '../lib/router.svelte';
  import { toast } from '../lib/toast.svelte';
  import Tracking from '../lib/Tracking.svelte';
  import { ago, chapterShort, langName, parseVolume, volumeHeading, STATUS } from '../lib/util';

  const WEEK = 7 * 86400;

  type Sort = 'source' | 'number' | 'date';
  type View = { sort: Sort; down: boolean; volumes: boolean };

  const SORTS: [Sort, string][] = [
    ['source', 'Source order'],
    ['number', 'Number'],
    ['date', 'Date'],
  ];
  const DEFAULT: View = { sort: 'source', down: true, volumes: false };

  type Show = 'all' | 'unread' | 'downloaded' | 'bookmarked';

  const SHOWS: [Show, string][] = [
    ['all', 'All chapters'],
    ['unread', 'Unread'],
    ['downloaded', 'Downloaded'],
    ['bookmarked', 'Bookmarked'],
  ];

  let { id }: { id: number } = $props();

  /** The title last open, if it is this one: come back to from the reader, it shows at once. */
  function before(): Title | null {
    const was = recall<Title>('title');
    return was?.id === id ? was : null;
  }

  let title = $state<Title | null>(before());
  let loading = $state(true);
  let error = $state<string | null>(null);
  // A failed refresh keeps what is on screen and says so, instead of replacing it.
  let refreshing = $state(false);
  let refreshError = $state<string | null>(null);
  let menu = $state(false);
  let wholeText = $state(false);
  let filters = $state(false);
  // The chapter whose own menu is open.
  let opened = $state<number | null>(null);
  // What the list is narrowed to lasts as long as the page: the next title opened shows whole.
  let show = $state<Show>('all');
  // Selected uploader filter: '' means All, '__unknown__' means chapters with no uploader/scanlator,
  // or a specific scanlator/uploader string.
  function initialUploader(): string {
    return recall<string>(`title.${id}.uploader`) ?? '';
  }
  let uploader = $state(initialUploader());

  $effect(() => {
    // Whenever the title id changes, recall that title's uploader preference
    uploader = recall<string>(`title.${id}.uploader`) ?? '';
  });

  function setUploader(val: string) {
    uploader = val;
    keep(`title.${id}.uploader`, val);
  }

  const chapters = $derived(title?.chapters ?? []);
  const unread = $derived(chapters.filter((c) => !c.read).length);
  const saved = $derived(chapters.filter((c) => c.downloaded).length);
  const hasVolumes = $derived(
    chapters.some((c) => (c.volume !== undefined && c.volume !== null) || parseVolume(c.name) !== null)
  );
  // The order is kept for every title, on the server like the library's.
  // When trustworthy volumes exist, default to volume view unless user explicitly chose otherwise.
  // For chapter-only/manhwa titles without volume metadata, never force volume grouping.
  const savedView = $derived(pref<Partial<View>>('title.view', {}));
  const view = $derived<View>({
    sort: savedView.sort ?? 'source',
    down: savedView.down ?? true,
    volumes: hasVolumes && (savedView.volumes !== undefined ? savedView.volumes : true),
  });

  const hasUnknown = $derived(chapters.some((c) => !c.scanlator?.trim()));
  const namedGroups = $derived(
    [...new Set(chapters.map((c) => c.scanlator?.trim() ?? '').filter(Boolean))].sort((a, b) => a.localeCompare(b))
  );
  // Show uploader filter when there are multiple uploaders or a named uploader mixed with unknown releases
  const hasMultipleUploaders = $derived(namedGroups.length > 1 || (namedGroups.length >= 1 && hasUnknown));

  // If a refresh took away the selected uploader, safely fall back to ''
  const currentUploader = $derived(
    uploader && (uploader === '__unknown__' ? hasUnknown : namedGroups.includes(uploader)) ? uploader : ''
  );

  type UploaderChoice = { value: string; label: string };
  const uploaderChoices = $derived.by<UploaderChoice[]>(() => {
    const list: UploaderChoice[] = [{ value: '', label: 'All' }];
    for (const g of namedGroups) {
      list.push({ value: g, label: g });
    }
    if (hasUnknown && namedGroups.length > 0) {
      list.push({ value: '__unknown__', label: 'Unknown' });
    }
    return list;
  });

  const filtering = $derived(show !== 'all' || currentUploader !== '');
  const listed = $derived.by(() => {
    const shown: Record<Show, (c: Chapter) => boolean> = { all: () => true, unread: (c) => !c.read, downloaded: (c) => c.downloaded, bookmarked: (c) => c.bookmark };
    const list = chapters.filter((c) => {
      if (!shown[show](c)) return false;
      if (!currentUploader) return true;
      if (currentUploader === '__unknown__') return !c.scanlator?.trim();
      return c.scanlator?.trim() === currentUploader;
    });
    // Sources list the newest first, and so does every order here until it is turned round.
    if (view.sort !== 'source') {
      const key = view.sort === 'number' ? (c: Chapter) => c.number : (c: Chapter) => c.dateUpload || c.fetchedAt * 1000;
      list.sort((a, b) => key(b) - key(a));
    }
    return view.down ? list : list.reverse();
  });
  const filteredUnread = $derived(listed.filter((c) => !c.read && !c.downloaded));
  let ownership = $state<TitleOwnership | null>(null);
  let refreshingOwnership = $state(false);
  let showOwnershipDetails = $state(false);

  async function loadOwnership() {
    try {
      ownership = await get<TitleOwnership>(`/manga/${id}/ownership`);
      loadMissingAndProfile();
    } catch {
      ownership = null;
    }
  }

  async function refreshOwnership() {
    refreshingOwnership = true;
    try {
      ownership = await send<TitleOwnership>('POST', `/manga/${id}/refresh-ownership`);
      loadMissingAndProfile();
      toast.success('Ownership refreshed from Komga');
    } catch (e) {
      toast.error(`Ownership refresh failed: ${message(e)}`);
    } finally {
      refreshingOwnership = false;
    }
  }

  // ---------- Missing Content & Acquisition Planning ----------
  let currentProfile = $state<AcquisitionProfile | null>(null);
  let availableProfiles = $state<AcquisitionProfile[]>([]);
  let missingContent = $state<MissingContentSummary | null>(null);
  let loadingMissing = $state(false);

  // Plan Modal state
  let planModalOpen = $state(false);
  let currentPlan = $state<AcquisitionPlan | null>(null);
  let planLoading = $state(false);
  let planError = $state<string | null>(null);
  let showAlternatives = $state(false);
  let selectedPlanItemIds = $state<Record<string, boolean>>({});
  let acquiringPlanLoading = $state(false);

  async function loadMissingAndProfile() {
    loadingMissing = true;
    try {
      const [miss, prof, allProfs] = await Promise.all([
        getTitleMissingContent(id).catch(() => null),
        getTitleProfile(id).catch(() => null),
        getAcquisitionProfiles().catch(() => []),
      ]);
      missingContent = miss;
      currentProfile = prof;
      availableProfiles = allProfs;
    } finally {
      loadingMissing = false;
    }
  }

  async function handleProfileChange(newProfileId: string) {
    try {
      const updated = await setTitleProfile(id, newProfileId || null);
      currentProfile = updated;
      toast.success(`Acquisition profile set to "${updated.name}"`);
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function openPlanModal(dryRun: boolean = true) {
    planModalOpen = true;
    planLoading = true;
    planError = null;
    showAlternatives = false;
    selectedPlanItemIds = {};
    try {
      const plan = await planTitleMissingContent(id, dryRun);
      currentPlan = plan;
      for (const item of plan.selectedItems) {
        selectedPlanItemIds[item.candidate.providerReleaseId] = true;
      }
    } catch (e) {
      planError = message(e);
    } finally {
      planLoading = false;
    }
  }

  async function handleSavePlan() {
    if (!currentPlan) return;
    try {
      const saved = await planTitleMissingContent(id, false);
      currentPlan = saved;
      toast.success('Acquisition plan saved');
    } catch (e) {
      toast.error(message(e));
    }
  }

  async function handleAcquirePlanSelected() {
    if (!currentPlan) return;
    const selected = currentPlan.selectedItems.filter((i) => selectedPlanItemIds[i.candidate.providerReleaseId]);
    if (selected.length === 0) {
      toast.error('No plan releases selected for acquisition');
      return;
    }

    acquiringPlanLoading = true;
    try {
      let count = 0;
      for (const item of selected) {
        const cand = item.candidate;
        if (cand.releaseUnit.type === 'collection') {
          await acquireCollection(id, {
            release: {
              source: cand.provider,
              sourceItemId: cand.providerReleaseId,
              title: cand.title,
              rawTitle: cand.title,
              uploader: cand.uploader ?? null,
              sourceUrl: cand.providerMetadata?.sourceUrl ?? '',
              torrentUrl: null,
              magnetUrl: null,
              infoHash: cand.providerMetadata?.torrentHash ?? null,
              categoryId: null,
              categoryName: null,
              sizeBytes: cand.size ?? 0,
              sizeFormatted: `${Math.round((cand.size ?? 0) / (1024 * 1024))} MiB`,
              seeders: cand.health.seeders ?? 0,
              leechers: cand.health.leechers ?? 0,
              publishedAt: cand.publishedAt ?? null,
              isTrusted: cand.qualitySignals.isTrusted,
              isRemake: false,
              isVariant: cand.qualitySignals.editionPenalty > 0,
              variantName: cand.edition ?? null,
              isMixed: false,
              releaseUnit: cand.releaseUnit,
              releaseNumber: cand.releaseNumber ?? null,
              releaseRange: cand.releaseRange ?? null,
              language: cand.language || 'en',
            } as any,
            selectedVolumes: item.unitsCovered,
            mode: 'missing_only',
          });
          count += item.unitsCovered.length;
        } else {
          await acquireRelease(id, {
            source: cand.provider,
            sourceItemId: cand.providerReleaseId,
            title: cand.title,
            rawTitle: cand.title,
            uploader: cand.uploader ?? null,
            sourceUrl: cand.providerMetadata?.sourceUrl ?? '',
            torrentUrl: null,
            magnetUrl: null,
            infoHash: cand.providerMetadata?.torrentHash ?? null,
            categoryId: null,
            categoryName: null,
            sizeBytes: cand.size ?? 0,
            sizeFormatted: `${Math.round((cand.size ?? 0) / (1024 * 1024))} MiB`,
            seeders: cand.health.seeders ?? 0,
            leechers: cand.health.leechers ?? 0,
            publishedAt: cand.publishedAt ?? null,
            isTrusted: cand.qualitySignals.isTrusted,
            isRemake: false,
            isVariant: cand.qualitySignals.editionPenalty > 0,
            variantName: cand.edition ?? null,
            isMixed: false,
            releaseUnit: cand.releaseUnit,
            releaseNumber: cand.releaseNumber ?? null,
            releaseRange: cand.releaseRange ?? null,
            language: cand.language || 'en',
          } as any);
          count += 1;
        }
      }
      toast.success(`Queued ${count} releases/units for acquisition!`);
      planModalOpen = false;
      refreshOwnership();
      loadMissingAndProfile();
    } catch (e) {
      toast.error(message(e));
    } finally {
      acquiringPlanLoading = false;
    }
  }

  const ownedVolumeNumbers = $derived.by<number[]>(() => {
    if (!ownership?.volumes) return [];
    return ownership.volumes.filter((v) => v.status === 'owned').map((v) => v.volume).sort((a, b) => a - b);
  });

  const missingVolumeNumbers = $derived.by<number[]>(() => {
    if (!ownership?.summary?.totalSourceVolumes || !ownership.volumes) return [];
    const ownedSet = new Set(ownership.volumes.filter((v) => v.status === 'owned').map((v) => Math.round(v.volume)));
    const missing: number[] = [];
    for (let i = 1; i <= ownership.summary.totalSourceVolumes; i++) {
      if (!ownedSet.has(i)) missing.push(i);
    }
    return missing;
  });

  // ---------- Nyaa Release Discovery & TorBox Acquisition ----------
  let nyaaReleases = $state<AcquisitionCandidate[]>([]);
  let loadingNyaa = $state(false);
  let nyaaError = $state<string | null>(null);
  let showNyaa = $state(false);
  let acquiringReleases = $state<Record<string, boolean>>({});

  // Filter & sort state
  let filterType = $state('all');
  let filterOwnership = $state('all');
  let filterUploader = $state('all');
  let filterSeeders = $state(0);
  let sortBy = $state('smart');
  let targetedUnit = $state<{ type: string; number: number } | null>(null);
  let expandedScoreBreakdown = $state<Record<string, boolean>>({});

  // Collection Inspector state
  let inspectingCollection = $state<CollectionInspection | null>(null);
  let inspectingLoading = $state(false);
  let collectionMode = $state<'missing_only' | 'entire' | 'manual'>('missing_only');
  let manualSelectedVolumes = $state<Record<number, boolean>>({});
  let acquiringCollectionLoading = $state(false);

  const discoveredUploaders = $derived.by(() => {
    const set = new Set<string>();
    for (const cand of nyaaReleases) {
      if (cand.release.uploader) {
        set.add(cand.release.uploader);
      }
    }
    return Array.from(set).sort();
  });

  async function searchNyaa(unitType?: string, unitNumber?: number) {
    showNyaa = true;
    loadingNyaa = true;
    nyaaError = null;
    if (unitType && unitNumber !== undefined) {
      targetedUnit = { type: unitType, number: unitNumber };
    }
    try {
      nyaaReleases = await getTitleAcquisitionReleases(id, {
        unitType: targetedUnit?.type,
        unitNumber: targetedUnit?.number,
        sort: sortBy,
        filterType: filterType !== 'all' ? filterType : undefined,
        filterOwnership: filterOwnership !== 'all' ? filterOwnership : undefined,
        filterUploader: filterUploader !== 'all' ? filterUploader : undefined,
        minSeeders: filterSeeders > 0 ? filterSeeders : undefined,
      });
      if (nyaaReleases.length === 0) {
        toast.info('No matching manga literature releases found on Nyaa');
      }
    } catch (e) {
      nyaaError = message(e);
      toast.error(nyaaError);
    } finally {
      loadingNyaa = false;
    }
  }

  function clearTargetedUnit() {
    targetedUnit = null;
    searchNyaa();
  }

  async function openCollectionInspector(release: AcquisitionRelease) {
    inspectingLoading = true;
    inspectingCollection = null;
    manualSelectedVolumes = {};
    collectionMode = 'missing_only';
    try {
      const inspection = await inspectCollection(id, release);
      inspectingCollection = inspection;
      collectionMode = inspection.recommendedMode;
      for (const u of inspection.units) {
        manualSelectedVolumes[u.volume] = !u.isOwned;
      }
    } catch (e) {
      toast.error(message(e));
    } finally {
      inspectingLoading = false;
    }
  }

  function closeCollectionInspector() {
    inspectingCollection = null;
  }

  async function executeAcquireCollection() {
    if (!inspectingCollection) return;
    acquiringCollectionLoading = true;
    try {
      let selectedVolumes: number[] = [];
      if (collectionMode === 'missing_only') {
        selectedVolumes = inspectingCollection.units.filter((u) => !u.isOwned).map((u) => u.volume);
      } else if (collectionMode === 'entire') {
        selectedVolumes = inspectingCollection.units.map((u) => u.volume);
      } else {
        selectedVolumes = inspectingCollection.units.filter((u) => manualSelectedVolumes[u.volume]).map((u) => u.volume);
      }

      if (selectedVolumes.length === 0) {
        toast.error('No volumes selected for acquisition');
        return;
      }

      const res = await acquireCollection(id, {
        release: inspectingCollection.release,
        selectedVolumes,
        mode: collectionMode,
      });

      toast.success(`Queued ${res.queuedCount} volumes for acquisition via TorBox!`);
      closeCollectionInspector();
      refreshOwnership();
    } catch (e) {
      toast.error(message(e));
    } finally {
      acquiringCollectionLoading = false;
    }
  }

  function toggleManualVolume(vol: number) {
    manualSelectedVolumes[vol] = !manualSelectedVolumes[vol];
  }

  function selectAllManual(select: boolean) {
    if (!inspectingCollection) return;
    for (const u of inspectingCollection.units) {
      manualSelectedVolumes[u.volume] = select;
    }
  }

  async function acquireCandidate(cand: AcquisitionCandidate) {
    const key = cand.release.sourceItemId;
    acquiringReleases[key] = true;
    try {
      await acquireRelease(id, cand.release, cand.release.releaseUnit);
      toast.success(`Queued "${cand.release.title}" for download via TorBox!`);
      cand.ownershipStatus = 'acquiring';
      refreshOwnership();
    } catch (e) {
      toast.error(message(e));
    } finally {
      acquiringReleases[key] = false;
    }
  }

  function formatVolumeRanges(nums: number[]): string {
    if (!nums.length) return 'None';
    const sorted = [...nums].sort((a, b) => a - b);
    const ranges: string[] = [];
    let start = sorted[0];
    let end = sorted[0];
    for (let i = 1; i < sorted.length; i++) {
      if (sorted[i] === end + 1) {
        end = sorted[i];
      } else {
        ranges.push(start === end ? `v${start}` : `v${start}–${end}`);
        start = end = sorted[i];
      }
    }
    ranges.push(start === end ? `v${start}` : `v${start}–${end}`);
    return ranges.join(', ');
  }

  type VolumeGroup = {
    volume: number | null;
    label: string;
    chapters: Chapter[];
    downloadedCount: number;
    missingCount: number;
    isFullyDownloaded: boolean;
    isPartiallyDownloaded: boolean;
    isBusy: boolean;
    ownershipStatus: OwnershipStatus | null;
    ownedBookName: string | null;
  };
  const volumeGroups = $derived.by<VolumeGroup[]>(() => {
    if (!view.volumes) {
      const downloadedCount = listed.filter((c) => c.downloaded).length;
      const missingCount = listed.filter((c) => !c.downloaded).length;
      return [{
        volume: null,
        label: '',
        chapters: listed,
        downloadedCount,
        missingCount,
        isFullyDownloaded: listed.length > 0 && missingCount === 0,
        isPartiallyDownloaded: downloadedCount > 0 && missingCount > 0,
        isBusy: listed.some((c) => busyChapter[c.id]),
        ownershipStatus: null,
        ownedBookName: null,
      }];
    }
    const map = new Map<number | null, Chapter[]>();
    for (const c of listed) {
      const vol = c.volume !== undefined && c.volume !== null ? c.volume : parseVolume(c.name);
      const list = map.get(vol) ?? [];
      list.push(c);
      map.set(vol, list);
    }
    const entries = Array.from(map.entries());
    entries.sort(([a], [b]) => {
      if (a === null) return 1;
      if (b === null) return -1;
      return view.down ? b - a : a - b;
    });
    return entries.map(([vol, chs]) => {
      const downloadedCount = chs.filter((c) => c.downloaded).length;
      const missingCount = chs.filter((c) => !c.downloaded).length;
      const volOwnership = vol !== null ? ownership?.volumes.find((v) => Math.abs(v.volume - vol) < 0.001) : null;
      return {
        volume: vol,
        label: volumeHeading(vol),
        chapters: chs,
        downloadedCount,
        missingCount,
        isFullyDownloaded: chs.length > 0 && missingCount === 0,
        isPartiallyDownloaded: downloadedCount > 0 && missingCount > 0,
        isBusy: chs.some((c) => busyChapter[c.id]),
        ownershipStatus: volOwnership?.status ?? null,
        ownedBookName: volOwnership?.bookName ?? null,
      };
    });
  });
  const nothing = $derived(
    !chapters.length
      ? 'No chapters yet.'
      : show === 'unread' && !currentUploader
        ? 'Every chapter is read.'
        : 'No chapter fits the filter.',
  );
  // Reading goes from the bottom of the list up: the next one to read is the last unread.
  const resume = $derived(chapters.findLast((c) => !c.read) ?? null);
  const started = $derived(chapters.some((c) => c.read || c.lastPageRead > 0));

  async function load() {
    loading = !title;
    error = null;
    try {
      const fresh = await get<Title>(`/manga/${id}`);
      keep('title', fresh);
      title = fresh;
      loadOwnership();
      // A title only seen in a list has no details or chapters yet.
      if (!title.initialized || title.chapters.length === 0) refresh();
    } catch (e) {
      error = message(e);
    }
    loading = false;
  }
  load();

  let savingLibrary = $state(false);
  let downloading = $state(false);
  let busyChapter = $state<Record<number, boolean>>({});

  async function refresh() {
    menu = false;
    refreshing = true;
    refreshError = null;
    try {
      title = await send<Title>('POST', `/manga/${id}/refresh`);
      await loadOwnership();
      toast.success('Updated title from source');
    } catch (e) {
      refreshError = message(e);
      toast.error(refreshError);
    }
    refreshing = false;
  }

  let allCategories = $state<Category[]>([]);
  get<{ categories: Category[] }>('/categories')
    .then((answer) => (allCategories = answer.categories))
    .catch(() => {});

  function toggleCategory(category: number) {
    if (!title) return;
    const t = title;
    t.categories = t.categories.includes(category) ? t.categories.filter((c) => c !== category) : [...t.categories, category];
    change(() => send('PUT', `/manga/${id}/categories`, { ids: t.categories }), true);
  }

  let note = $state('');

  async function download(list: Chapter[]) {
    if (!list.length || downloading) return;
    const ownedVols = new Set(ownership?.volumes.filter((v) => v.status === 'owned').map((v) => v.volume) ?? []);
    const downloadingOwned = list.some((c) => {
      const vol = c.volume !== undefined && c.volume !== null ? c.volume : parseVolume(c.name);
      return vol !== null && ownedVols.has(vol);
    });
    if (downloadingOwned && list.length === 1) {
      const ok = await confirm({
        title: 'Already Owned in Komga',
        message: `This release belongs to a volume already present in your canonical Komga library. Acquire anyway?`,
        confirmText: 'Acquire anyway',
        danger: false,
      });
      if (!ok) return;
    }
    downloading = true;
    for (const c of list) busyChapter[c.id] = true;
    try {
      const answer = await send<{ queued: number }>('POST', '/downloads', { chapters: list.map((c) => c.id) });
      const queuedCount = answer.queued;
      const text = queuedCount ? `${queuedCount} ${queuedCount === 1 ? 'chapter' : 'chapters'} queued for download.` : 'Already saved or queued.';
      note = text;
      toast.info(text, { action: { label: 'See downloads', run: () => (location.hash = '#/downloads') } });
    } catch (e) {
      refreshError = message(e);
      toast.error(refreshError);
    } finally {
      downloading = false;
      for (const c of list) busyChapter[c.id] = false;
    }
  }

  async function downloadVolume(grp: VolumeGroup) {
    if (downloading) return;
    if (grp.ownershipStatus === 'owned') {
      const ok = await confirm({
        title: 'Already Owned in Komga',
        message: `${grp.label} is already present in your canonical Komga library${grp.ownedBookName ? ` ("${grp.ownedBookName}")` : ''}. Are you sure you want to acquire it again?`,
        confirmText: 'Acquire anyway',
        danger: false,
      });
      if (!ok) return;
    }
    const missing = grp.chapters.filter((c) => !c.downloaded && !busyChapter[c.id]);
    if (!missing.length) return;
    for (const c of missing) busyChapter[c.id] = true;
    try {
      const answer = await send<{ queued: number }>('POST', '/downloads', { chapters: missing.map((c) => c.id) });
      const queuedCount = answer.queued;
      const volName = grp.volume !== null ? `Volume ${grp.volume}` : 'Chapters';
      if (queuedCount === missing.length) {
        const text = `Queued ${volName} (${queuedCount} ${queuedCount === 1 ? 'chapter' : 'chapters'}) for download.`;
        note = text;
        toast.info(text, { action: { label: 'See downloads', run: () => (location.hash = '#/downloads') } });
      } else if (queuedCount > 0) {
        const text = `Queued ${queuedCount} of ${missing.length} chapters for ${volName}.`;
        note = text;
        toast.info(text, { action: { label: 'See downloads', run: () => (location.hash = '#/downloads') } });
      } else {
        const text = `All chapters for ${volName} already saved or queued.`;
        note = text;
        toast.info(text);
      }
    } catch (e) {
      refreshError = message(e);
      toast.error(`Failed to queue ${grp.label}: ${refreshError}`);
    } finally {
      for (const c of missing) busyChapter[c.id] = false;
    }
  }

  async function deleteFile(chapter: Chapter) {
    const ok = await confirm({
      title: 'Delete Download?',
      message: `Delete the saved file for ${chapter.name}? You will need to download it again to read offline.`,
      confirmText: 'Delete',
      danger: true,
    });
    if (!ok) return;
    busyChapter[chapter.id] = true;
    chapter.downloaded = false;
    try {
      await change(() => send('DELETE', `/chapters/${chapter.id}/download`));
      toast.info(`Deleted saved file for ${chapter.name}`);
    } finally {
      busyChapter[chapter.id] = false;
    }
  }

  function toggleAuto() {
    if (!title) return;
    const t = title;
    t.autoDownload = !t.autoDownload;
    change(() => send('PUT', `/manga/${id}/auto-download`, { on: t.autoDownload }), true);
  }

  async function change(work: () => Promise<unknown>, keepMenu = false) {
    menu = keepMenu && menu;
    opened = null;
    refreshError = null;
    try {
      await work();
    } catch (e) {
      refreshError = message(e);
      toast.error(refreshError);
      title = await get<Title>(`/manga/${id}`).catch(() => title);
    }
  }

  async function toggleLibrary() {
    if (!title || savingLibrary) return;
    savingLibrary = true;
    const nextState = !title.inLibrary;
    title.inLibrary = nextState;
    try {
      await send('PUT', `/manga/${id}/library`, { inLibrary: nextState });
      toast.success(nextState ? `Added "${title.title}" to library` : `Removed "${title.title}" from library`);
    } catch (e) {
      title.inLibrary = !nextState;
      refreshError = message(e);
      toast.error(refreshError);
    } finally {
      savingLibrary = false;
    }
  }

  function mark(list: Chapter[], read: boolean) {
    for (const chapter of list) {
      chapter.read = read;
      chapter.lastPageRead = 0;
    }
    change(() => send('PUT', `/manga/${id}/chapters`, { ids: list.map((c) => c.id), read }));
  }

  function bookmark(chapter: Chapter) {
    chapter.bookmark = !chapter.bookmark;
    change(() => send('PUT', `/manga/${id}/chapters`, { ids: [chapter.id], bookmark: chapter.bookmark }));
  }

  function setView(patch: Partial<View>) {
    setPref('title.view', { ...view, ...patch }).catch((e) => (refreshError = message(e)));
  }

  /** "Weekly", from the days a title usually leaves between two chapters. */
  function pace(days: number): string {
    return days === 1 ? 'Daily' : days === 7 ? 'Weekly' : days >= 28 ? 'Monthly or slower' : `Every ${days} days`;
  }

  function about(chapter: Chapter): string {
    const when = chapter.dateUpload > 0 ? ago(chapter.dateUpload / 1000) : chapter.fetchedAt > 0 ? `found ${ago(chapter.fetchedAt)}` : '';
    const where = chapter.read
      ? 'read'
      : chapter.lastPageRead > 0
        ? `page ${chapter.lastPageRead + 1}${chapter.pageCount ? ` of ${chapter.pageCount}` : ''}`
        : chapter.pageCount
          ? `${chapter.pageCount} pages`
          : '';
    return [when, chapter.scanlator, where, chapter.downloaded ? 'saved' : ''].filter(Boolean).join(' · ');
  }

  function isNew(chapter: Chapter): boolean {
    return !chapter.read && chapter.lastPageRead === 0 && !!title?.addedAt && chapter.fetchedAt > title.addedAt && Date.now() / 1000 - chapter.fetchedAt < WEEK;
  }
</script>

<!-- Menus close when clicking outside or pressing Escape. -->
<svelte:window
  onclick={(e) => {
    const el = e.target as Element;
    if (opened !== null && !el.closest?.('.own')) opened = null;
    if (menu && !el.closest?.('.menu')) menu = false;
  }}
  onkeydown={(e) => {
    if (e.key === 'Escape') {
      opened = null;
      menu = false;
    }
  }}
/>

<div class="page">
  <Notice {loading} {error} retry={load} />

  {#if title}
    <div class="bar">
      <a class="btn icon lift" href="#/" aria-label="Back" onclick={(e) => (e.preventDefault(), back())}>
        <Icon name="back" stroke={3} />
      </a>
      <div class="tools">
        {#if title.webUrl}
          <a
            class="btn lift"
            href={title.webUrl}
            target="_blank"
            rel="noreferrer noopener"
            aria-label="Open on the source site"
            title="Open in WebView / Website"
          >
            <Icon name="globe" size={18} stroke={3} />
            <span class="site-label">Website</span>
          </a>
        {/if}
        <button
          class="btn lift spot-on"
          aria-pressed={title.inLibrary}
          disabled={savingLibrary}
          onclick={toggleLibrary}
        >
          {#if savingLibrary}
            <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
          {:else}
            <Icon name="bookmark" fill={title.inLibrary} />
          {/if}
          {title.inLibrary ? 'In library' : 'Add to library'}
        </button>
        <div class="menu">
          <button class="btn icon lift" aria-label="More actions" aria-expanded={menu} onclick={() => (menu = !menu)}><Icon name="more" fill /></button>
          {#if menu}
            <div class="panel sheet">
              {#if title.webUrl}
                <a class="btn" href={title.webUrl} target="_blank" rel="noreferrer noopener">
                  <Icon name="globe" size={18} stroke={3} />Open in WebView / Website
                </a>
              {/if}
              <button class="btn" onclick={refresh}><Icon name="refresh" size={18} />Refresh from the source</button>
              <button class="btn" onclick={() => mark(chapters, true)}><Icon name="check" size={18} />Mark all read</button>
              <button class="btn" onclick={() => mark(chapters, false)}>Mark all unread</button>
              <a class="btn" href="#/search?q={encodeURIComponent(title.title)}&for={id}"><Icon name="arrow" size={18} />Move to another source</a>
              {#if title.inLibrary}
                <button class="btn" aria-pressed={title.autoDownload} onclick={toggleAuto}>
                  {#if title.autoDownload}<Icon name="check" size={18} stroke={3} />{/if}Download new chapters
                </button>
              {/if}
              {#if title.inLibrary && allCategories.length}
                <div class="cats" role="group" aria-label="Categories">
                  <span>Categories</span>
                  {#each allCategories as category (category.id)}
                    {@const inIt = title.categories.includes(category.id)}
                    <button class="btn" aria-pressed={inIt} onclick={() => toggleCategory(category.id)}>
                      {#if inIt}<Icon name="check" size={18} stroke={3} />{/if}{category.name}
                    </button>
                  {/each}
                </div>
              {/if}
            </div>
          {/if}
        </div>
      </div>
    </div>

    <div class="columns">
      <div class="about">
        <div class="hero">
          <div class="art"><Cover src={title.cover} title={title.title} tilt /></div>
          <div class="facts">
            <div class="tools">
              <span class="tag ink">{STATUS[title.status] ?? 'Unknown'}</span>
              {#if title.sourceLang}<span class="tag">{langName(title.sourceLang)}</span>{/if}
              {#if title.releaseDays && title.status !== 2 && title.status !== 4}
                <span class="tag" title="How often it gets a chapter, going by the source's dates">{pace(title.releaseDays)}</span>
              {/if}
            </div>
            <h1 class="display">{title.title}</h1>
            <strong>{title.sourceName ?? 'Its extension is not installed'}</strong>
            {#if title.author || title.artist}
              <span>{[...new Set([title.author, title.artist].filter(Boolean))].join(' · ')}</span>
            {/if}
          </div>
        </div>

        <div class="stats">
          <div><span class="display">{chapters.length}</span><span>Chapters</span></div>
          <div class:hot={unread > 0}><span class="display">{unread}</span><span>Unread</span></div>
          <div><span class="display">{saved}</span><span>Saved</span></div>
        </div>

        {#if ownership}
          <div class="ownership-summary" class:unavailable={!ownership.komgaConnected} class:has-err={!!ownership.error}>
            <div class="ownership-top">
              <div class="ownership-badge-title">
                <Icon name="book" size={16} />
                <span class="ownership-heading">KOMGA OWNED</span>
              </div>
              <div class="ownership-actions">
                <button
                  class="btn icon mini"
                  title="Refresh ownership from Komga"
                  disabled={refreshingOwnership}
                  onclick={refreshOwnership}
                  aria-label="Refresh Komga ownership"
                >
                  {#if refreshingOwnership}
                    <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
                  {:else}
                    <Icon name="refresh" size={14} />
                  {/if}
                </button>
                {#if ownership.komgaConnected && ownership.seriesId}
                  <button class="btn mini" onclick={() => (showOwnershipDetails = !showOwnershipDetails)}>
                    {showOwnershipDetails ? 'Hide details' : 'Details'}
                  </button>
                {/if}
              </div>
            </div>

            <div class="ownership-body">
              {#if !ownership.komgaConnected}
                <div class="ownership-line">
                  <span class="tag warn">Unavailable</span>
                  <span class="ownership-expl">{ownership.error ?? 'Komga is offline or unreachable'}</span>
                </div>
              {:else if ownership.error}
                <div class="ownership-line">
                  <span class="tag error">Unknown</span>
                  <span class="ownership-expl">{ownership.error}</span>
                </div>
              {:else if ownership.summary.ownedVolumesCount === 0 && ownership.summary.ownedChaptersCount === 0}
                <div class="ownership-line">
                  <span class="ownership-metric">None owned in Komga</span>
                  {#if ownership.summary.explanation}
                    <span class="ownership-expl">{ownership.summary.explanation}</span>
                  {/if}
                </div>
              {:else}
                <div class="ownership-metrics-row">
                  {#if ownership.summary.ownedVolumesCount > 0}
                    <div class="ownership-metric-col">
                      <span class="ownership-metric">
                        VOLUMES <strong>{ownership.summary.ownedVolumesCount}{#if ownership.summary.totalSourceVolumes} / {ownership.summary.totalSourceVolumes}{:else if ownership.summary.authoritativeTotalVolumes} / {ownership.summary.authoritativeTotalVolumes}{/if}</strong>
                      </span>
                      {#if ownership.summary.explanation}
                        <span class="ownership-expl">{ownership.summary.explanation}</span>
                      {/if}
                    </div>
                  {/if}
                  {#if ownership.summary.ownedChaptersCount > 0}
                    <div class="ownership-metric-col">
                      <span class="ownership-metric">
                        CHAPTERS <strong>{ownership.summary.ownedChaptersCount}{#if ownership.summary.totalSourceChapters} / {ownership.summary.totalSourceChapters}{:else if ownership.summary.authoritativeTotalChapters} / {ownership.summary.authoritativeTotalChapters}{/if}</strong>
                      </span>
                    </div>
                  {/if}
                </div>
              {/if}
            </div>

            {#if showOwnershipDetails && ownership.komgaConnected}
              <div class="ownership-details-box">
                {#if ownership.seriesName}
                  <div class="detail-row">
                    <span class="detail-k">Matched Series:</span>
                    <span class="detail-v"><strong>{ownership.seriesName}</strong> <code class="detail-id">{ownership.seriesId}</code></span>
                  </div>
                {/if}
                {#if ownership.libraryName || ownership.libraryId}
                  <div class="detail-row">
                    <span class="detail-k">Komga Library:</span>
                    <span class="detail-v">{ownership.libraryName ?? ownership.libraryId}</span>
                  </div>
                {/if}
                {#if ownedVolumeNumbers.length > 0}
                  <div class="detail-row">
                    <span class="detail-k">Owned Volumes:</span>
                    <span class="detail-v">{formatVolumeRanges(ownedVolumeNumbers)}</span>
                  </div>
                {/if}
                {#if missingVolumeNumbers.length > 0}
                  <div class="detail-row">
                    <span class="detail-k">Missing Volumes:</span>
                    <span class="detail-v">{formatVolumeRanges(missingVolumeNumbers)}</span>
                  </div>
                {/if}
                <div class="detail-row">
                  <span class="detail-k">Source Releases:</span>
                  <span class="detail-v capitalize">{ownership.summary.sourceReleaseType} ({chapters.length} releases)</span>
                </div>
                {#if ownership.lastRefreshed}
                  <div class="detail-row">
                    <span class="detail-k">Last Checked:</span>
                    <span class="detail-v">{ago(ownership.lastRefreshed)}</span>
                  </div>
                {/if}
              </div>
            {/if}

            <!-- Missing Content & Acquisition Overview -->
            {#if missingContent}
              <div class="missing-content-overview-row">
                <div class="missing-status-line">
                  {#if missingContent.hasMissing}
                    <span class="tag warn" style="font-weight: 700;">
                      MISSING: {missingContent.missingVolumes.length === 1 ? `Volume ${missingContent.missingVolumes[0]}` : `v${formatVolumeRanges(missingContent.missingVolumes)}`}
                    </span>
                  {:else if missingContent.isFullyOwned}
                    <span class="tag spot">FULLY OWNED IN KOMGA</span>
                  {/if}
                  {#if missingContent.acquiringVolumes.length > 0}
                    <span class="tag">ACQUIRING: v{formatVolumeRanges(missingContent.acquiringVolumes)}</span>
                  {/if}
                </div>

                <div class="planner-actions-row">
                  {#if missingContent.hasMissing}
                    <button class="btn spot pill icon-text" onclick={() => openPlanModal(true)} disabled={planLoading}>
                      <Icon name="puzzle" size={14} /> Plan Missing Volumes
                    </button>
                  {/if}
                  <button class="btn pill icon-text" onclick={() => searchNyaa()} disabled={loadingNyaa}>
                    <Icon name="search" size={14} /> Find Best Release
                  </button>
                </div>
              </div>
            {/if}

            <!-- Profile selector dropdown -->
            <div class="title-profile-row">
              <span class="detail-k">Acquisition Profile:</span>
              <select
                class="input select-compact"
                value={currentProfile?.id ?? 'default'}
                onchange={(e) => handleProfileChange((e.target as HTMLSelectElement).value)}
              >
                {#each availableProfiles as prof (prof.id)}
                  <option value={prof.id}>{prof.name}{prof.isDefault ? ' (Default)' : ''}</option>
                {/each}
              </select>
            </div>
          </div>
        {/if}

        {#if title.description}
          <p class="text" class:clamp={!wholeText}>{title.description}</p>
          {#if title.description.length > 280}
            <button class="btn more" aria-expanded={wholeText} onclick={() => (wholeText = !wholeText)}>{wholeText ? 'Less' : 'More'}</button>
          {/if}
        {/if}
        {#if title.genres.length}
          <div class="tools genres">
            {#each title.genres as genre}<span class="tag">{genre}</span>{/each}
          </div>
        {/if}
        <Tracking {id} title={title.title} />

        <div class="nyaa-discovery-card">
          <div class="nyaa-header-row">
            <div>
              <h3 class="display nyaa-title">NYAA RELEASES</h3>
              <span class="nyaa-subtext">Discover manga releases on Nyaa and acquire via TorBox.</span>
            </div>
            <button class="btn pill" onclick={() => searchNyaa()} disabled={loadingNyaa}>
              {#if loadingNyaa}
                <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
              {:else}
                <Icon name="search" size={14} />
              {/if}
              {showNyaa ? 'Refresh Nyaa' : 'Find Releases on Nyaa'}
            </button>
          </div>

          {#if showNyaa}
            <div class="nyaa-controls">
              <div class="nyaa-filter-bar">
                <div class="filter-group">
                  <span class="filter-label">Type:</span>
                  <div class="pill-group">
                    <button class="filter-pill" class:active={filterType === 'all'} onclick={() => { filterType = 'all'; searchNyaa(); }}>All</button>
                    <button class="filter-pill" class:active={filterType === 'volumes'} onclick={() => { filterType = 'volumes'; searchNyaa(); }}>Volumes</button>
                    <button class="filter-pill" class:active={filterType === 'collections'} onclick={() => { filterType = 'collections'; searchNyaa(); }}>Collections</button>
                    <button class="filter-pill" class:active={filterType === 'chapters'} onclick={() => { filterType = 'chapters'; searchNyaa(); }}>Chapters</button>
                  </div>
                </div>

                <div class="filter-group">
                  <span class="filter-label">Coverage:</span>
                  <div class="pill-group">
                    <button class="filter-pill" class:active={filterOwnership === 'all'} onclick={() => { filterOwnership = 'all'; searchNyaa(); }}>All</button>
                    <button class="filter-pill" class:active={filterOwnership === 'missing_only'} onclick={() => { filterOwnership = 'missing_only'; searchNyaa(); }}>Missing</button>
                    <button class="filter-pill" class:active={filterOwnership === 'owned'} onclick={() => { filterOwnership = 'owned'; searchNyaa(); }}>Owned</button>
                  </div>
                </div>

                {#if discoveredUploaders.length > 0}
                  <div class="filter-group">
                    <span class="filter-label">Uploader:</span>
                    <select class="input select-compact" bind:value={filterUploader} onchange={() => searchNyaa()}>
                      <option value="all">All Uploaders</option>
                      {#each discoveredUploaders as up}
                        <option value={up}>{up}</option>
                      {/each}
                    </select>
                  </div>
                {/if}

                <div class="filter-group">
                  <span class="filter-label">Seeders:</span>
                  <select class="input select-compact" bind:value={filterSeeders} onchange={() => searchNyaa()}>
                    <option value={0}>Any</option>
                    <option value={1}>1+ seeders</option>
                    <option value={3}>3+ seeders</option>
                    <option value={5}>5+ seeders</option>
                    <option value={10}>10+ seeders</option>
                  </select>
                </div>

                <div class="filter-group sort-group">
                  <span class="filter-label">Sort:</span>
                  <select class="input select-compact" bind:value={sortBy} onchange={() => searchNyaa()}>
                    <option value="smart">Smart Score</option>
                    <option value="coverage">Missing Coverage</option>
                    <option value="quality">Quality</option>
                    <option value="newest">Newest First</option>
                    <option value="oldest">Oldest First</option>
                    <option value="seeders">Most Seeders</option>
                    <option value="size_asc">Smallest Size</option>
                    <option value="size_desc">Largest Size</option>
                    <option value="volume_asc">Volume Asc</option>
                    <option value="volume_desc">Volume Desc</option>
                  </select>
                </div>
              </div>

              {#if targetedUnit}
                <div class="targeted-search-banner">
                  <span>Targeted Search: <strong>Volume {targetedUnit.number}</strong></span>
                  <button class="btn pill icon-text clear-btn" onclick={clearTargetedUnit}>
                    <Icon name="close" size={12} /> Clear Filter
                  </button>
                </div>
              {/if}
            </div>

            {#if nyaaError}
              <Notice error={nyaaError} retry={() => searchNyaa()} />
            {:else if loadingNyaa}
              <div class="nyaa-loading-box">
                <span class="spin"><Icon name="spinner" size={20} stroke={3} /></span>
                Searching Nyaa manga literature...
              </div>
            {:else if nyaaReleases.length === 0}
              <p class="nyaa-muted">No manga literature releases found matching this title on Nyaa.</p>
            {:else}
              <ul class="candidate-list">
                {#each nyaaReleases as cand (cand.release.sourceItemId)}
                  {@const isCollection = cand.release.releaseUnit.type === 'collection' || cand.matchType === 'collection' || cand.matchType === 'mixed_collection'}
                  <li class="candidate-item" class:owned={cand.ownershipStatus === 'owned'} class:acquiring={cand.ownershipStatus === 'acquiring'}>
                    <div class="candidate-main">
                      <div class="candidate-badges">
                        {#if cand.smartScore}
                          <span
                            class="tag smart-badge"
                            class:spot={cand.smartScore.summaryTag === 'BEST MATCH'}
                            class:warn={cand.smartScore.summaryTag === 'GOOD ALTERNATIVE'}
                            class:error={cand.smartScore.summaryTag === 'VARIANT' || cand.smartScore.summaryTag === 'MIXED COLLECTION'}
                          >
                            ★ {cand.smartScore.total.toFixed(0)} · {cand.smartScore.summaryTag}
                          </span>
                        {/if}
                        <span class="tag" class:spot={cand.matchType.startsWith('exact')}>
                          {cand.matchType.replace('_', ' ').toUpperCase()}
                        </span>
                        {#if cand.coverageSummary}
                          <span class="tag coverage-pill">{cand.coverageSummary}</span>
                        {/if}
                        {#if cand.isVariant}
                          <span class="tag error">VARIANT: {cand.variantName}</span>
                        {/if}
                        {#if cand.release.isTrusted}
                          <span class="tag spot">TRUSTED</span>
                        {/if}
                        {#if cand.release.isRemake}
                          <span class="tag error">REMAKE</span>
                        {/if}
                        {#if cand.ownershipStatus === 'owned'}
                          <span class="tag spot">OWNED IN KOMGA</span>
                        {:else if cand.ownershipStatus === 'acquiring'}
                          <span class="tag">ACQUIRING</span>
                        {/if}
                      </div>
                      <div class="candidate-title" title={cand.release.rawTitle}>
                        <strong>{cand.release.rawTitle}</strong>
                      </div>

                      {#if cand.smartScore?.explanation}
                        <div class="candidate-explanation">
                          <span>{cand.smartScore.explanation}</span>
                          <button
                            class="btn-text-score"
                            onclick={() => {
                              expandedScoreBreakdown[cand.release.sourceItemId] = !expandedScoreBreakdown[cand.release.sourceItemId];
                            }}
                          >
                            {expandedScoreBreakdown[cand.release.sourceItemId] ? 'Hide breakdown ▴' : 'Score breakdown ▾'}
                          </button>
                        </div>
                      {/if}

                      {#if expandedScoreBreakdown[cand.release.sourceItemId] && cand.smartScore}
                        <div class="score-breakdown-panel">
                          <div class="score-col">Identity: <strong>{cand.smartScore.identity.toFixed(1)}/25</strong></div>
                          <div class="score-col">Coverage: <strong>{cand.smartScore.coverage.toFixed(1)}/25</strong></div>
                          <div class="score-col">Quality: <strong>{cand.smartScore.quality.toFixed(1)}/15</strong></div>
                          <div class="score-col">Health: <strong>{cand.smartScore.health.toFixed(1)}/15</strong></div>
                          <div class="score-col">Freshness: <strong>{cand.smartScore.freshness.toFixed(1)}/10</strong></div>
                          <div class="score-col">Efficiency: <strong>{cand.smartScore.efficiency.toFixed(1)}/10</strong></div>
                          {#if cand.smartScore.penalties < 0}
                            <div class="score-col penalty">Penalties: <strong>{cand.smartScore.penalties.toFixed(1)}</strong></div>
                          {/if}
                        </div>
                      {/if}

                      <div class="candidate-meta">
                        {#if cand.release.uploader}
                          <span>Uploader: <strong>{cand.release.uploader}</strong></span>
                          <span>·</span>
                        {/if}
                        <span>Size: <strong>{cand.release.sizeFormatted}</strong></span>
                        {#if cand.usefulEfficiencyBytes}
                          <span>·</span>
                          <span>~{Math.round(cand.usefulEfficiencyBytes / (1024 * 1024))} MB/vol</span>
                        {/if}
                        <span>·</span>
                        <span class="seeders">Seeders: <strong>{cand.release.seeders}</strong></span>
                        <span>·</span>
                        <span>Leechers: {cand.release.leechers}</span>
                      </div>
                    </div>
                    <div class="candidate-action">
                      {#if isCollection}
                        <button
                          class="btn spot pill"
                          onclick={() => openCollectionInspector(cand.release)}
                          disabled={inspectingLoading}
                        >
                          <Icon name="folder" size={14} /> View Collection ({cand.unitsIncluded.length} vols)
                        </button>
                      {:else if cand.ownershipStatus === 'owned'}
                        <button class="btn pill" disabled title="Already owned in canonical library">
                          <Icon name="check" size={14} /> Owned
                        </button>
                      {:else if cand.ownershipStatus === 'acquiring'}
                        <button class="btn pill" disabled title="Currently in download or verification queue">
                          <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span> Acquiring
                        </button>
                      {:else}
                        <button
                          class="btn spot pill"
                          disabled={acquiringReleases[cand.release.sourceItemId]}
                          onclick={() => acquireCandidate(cand)}
                        >
                          {#if acquiringReleases[cand.release.sourceItemId]}
                            <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span> Acquiring…
                          {:else}
                            <Icon name="download" size={14} /> Acquire with TorBox
                          {/if}
                        </button>
                      {/if}
                    </div>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        </div>
      </div>

      <div class="chapters">
        <div class="section-head">
          <h2 class="display">CHAPTERS</h2>
          <div class="tools">
            <button class="btn" aria-expanded={filters} onclick={() => (filters = !filters)}>
              <Icon name="filter" size={18} />Sort &amp; filter{filtering ? ' · on' : ''}
            </button>
            <button
              class="btn icon"
              aria-label="Download the unread chapters"
              disabled={downloading || !filteredUnread.length}
              onclick={() => download(filteredUnread)}
            >
              {#if downloading}
                <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
              {:else}
                <Icon name="download" />
              {/if}
            </button>
            <button
              class="btn icon"
              aria-label="Refresh from the source"
              disabled={refreshing}
              onclick={refresh}
            >
              {#if refreshing}
                <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
              {:else}
                <Icon name="refresh" />
              {/if}
            </button>
          </div>
        </div>

        {#if hasMultipleUploaders}
          <div class="uploader-filter" role="group" aria-label="Filter by uploader">
            <span class="uploader-label">Uploader</span>
            {#if uploaderChoices.length <= 4}
              <div class="pills">
                {#each uploaderChoices as choice (choice.value)}
                  <button
                    class="btn pill"
                    class:spot={currentUploader === choice.value}
                    aria-pressed={currentUploader === choice.value}
                    onclick={() => setUploader(choice.value)}
                  >
                    {choice.label}
                  </button>
                {/each}
              </div>
            {:else}
              <label class="sorter">
                <span class="sr">Filter by uploader</span>
                <select class="btn" value={currentUploader} onchange={(e) => setUploader(e.currentTarget.value)}>
                  {#each uploaderChoices as choice (choice.value)}
                    <option value={choice.value}>{choice.label}</option>
                  {/each}
                </select>
              </label>
            {/if}
          </div>
        {/if}

        {#if filters}
          <div class="tools" role="group" aria-label="Sort and filter">
            <label class="sorter">
              <span class="sr">Sort by</span>
              <select class="btn" value={view.sort} onchange={(e) => setView({ sort: e.currentTarget.value as Sort })}>
                {#each SORTS as [value, label] (value)}<option {value}>{label}</option>{/each}
              </select>
            </label>
            <button class="btn icon" aria-label={view.down ? 'Sorted downwards. Sort upwards' : 'Sorted upwards. Sort downwards'} onclick={() => setView({ down: !view.down })}>
              <Icon name={view.down ? 'down' : 'up'} />
            </button>
            <label class="sorter">
              <span class="sr">Show</span>
              <select class="btn" bind:value={show}>
                {#each SHOWS as [value, label] (value)}<option {value}>{label}</option>{/each}
              </select>
            </label>
            {#if hasVolumes}
              <button
                class="btn pill"
                class:spot={view.volumes}
                aria-pressed={view.volumes}
                onclick={() => setView({ volumes: !view.volumes })}
              >
                Volumes
              </button>
            {/if}
          </div>
        {/if}

        <Notice loading={refreshing} error={refreshError} retry={refresh} />
        {#if note}<p role="status"><strong>{note}</strong> <a class="link" href="#/downloads">See downloads</a></p>{/if}

        {#each volumeGroups as grp (grp.volume ?? 'unassigned')}
          {#if grp.label}
            <div class="volume-header">
              <div class="volume-info">
                <h3 class="display">{grp.label}</h3>
                <span class="tag">{grp.chapters.length} {grp.chapters.length === 1 ? 'chapter' : 'chapters'}</span>
                {#if grp.ownershipStatus === 'owned'}
                  <span class="tag spot" title="Present in canonical Komga library"><Icon name="check" size={14} /> Owned in Komga</span>
                {:else if grp.ownershipStatus === 'acquiring'}
                  <span class="tag" title="In acquisition pipeline"><span class="spin"><Icon name="spinner" size={14} stroke={3} /></span> Acquiring</span>
                {/if}
                {#if grp.isFullyDownloaded}
                  <span class="tag spot">Downloaded</span>
                {:else if grp.isPartiallyDownloaded}
                  <span class="tag">{grp.downloadedCount}/{grp.chapters.length} saved</span>
                {/if}
              </div>
              {#if grp.volume !== null}
                <div class="volume-actions">
                  {#if grp.ownershipStatus === 'owned'}
                    <button
                      class="btn pill"
                      aria-label="Re-acquire {grp.label}"
                      disabled={downloading || grp.isBusy || !grp.chapters.length}
                      onclick={() => downloadVolume(grp)}
                      title="Already in Komga library. Click to re-acquire"
                    >
                      {#if grp.isBusy}
                        <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
                      {:else}
                        <Icon name="refresh" size={14} />
                      {/if}
                      Re-acquire
                    </button>
                  {:else if grp.isFullyDownloaded}
                    <span class="tag spot"><Icon name="check" size={14} /> Complete</span>
                  {:else if grp.isPartiallyDownloaded}
                    <button
                      class="btn pill"
                      aria-label="Download missing chapters for {grp.label}"
                      disabled={downloading || grp.isBusy || grp.missingCount === 0}
                      onclick={() => downloadVolume(grp)}
                    >
                      {#if grp.isBusy}
                        <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
                      {:else}
                        <Icon name="download" size={14} />
                      {/if}
                      Missing ({grp.missingCount})
                    </button>
                  {:else}
                    <button
                      class="btn pill spot"
                      aria-label="Download {grp.label}"
                      disabled={downloading || grp.isBusy || !grp.chapters.length}
                      onclick={() => downloadVolume(grp)}
                    >
                      {#if grp.isBusy}
                        <span class="spin"><Icon name="spinner" size={14} stroke={3} /></span>
                      {:else}
                        <Icon name="download" size={14} />
                      {/if}
                      Download Volume
                    </button>
                  {/if}
                  {#if grp.ownershipStatus !== 'owned'}
                    <button
                      class="btn pill"
                      aria-label="Find {grp.label} on Nyaa"
                      onclick={() => searchNyaa('volume', grp.volume)}
                      title="Search Nyaa for {grp.label}"
                    >
                      <Icon name="search" size={14} /> Find Nyaa
                    </button>
                  {/if}
                </div>
              {/if}
            </div>
          {/if}
          <ul class="rows">
            {#each grp.chapters as chapter}
              <li class:done={chapter.read}>
                <a class="main" href="#/read/{chapter.id}">
                  <strong>
                    {#if chapter.bookmark}<span class="kept"><Icon name="bookmark" size={14} fill /><span class="sr">Bookmarked.</span></span>{/if}
                    {#if (chapter.volume !== undefined && chapter.volume !== null ? chapter.volume : parseVolume(chapter.name)) !== null && !view.volumes}
                      <span class="tag vol">Vol. {chapter.volume ?? parseVolume(chapter.name)}</span>
                    {/if}
                    {chapter.name}
                  </strong>
                  <span>{about(chapter)}</span>
                </a>
                {#if isNew(chapter)}
                  <span class="tag spot">New</span>
                {:else if !chapter.read && chapter.lastPageRead > 0 && chapter.pageCount}
                  <span class="meter" aria-hidden="true"><span style:width="{Math.round(((chapter.lastPageRead + 1) / chapter.pageCount) * 100)}%"></span></span>
                {/if}
                {#if chapter.downloaded}
                  <button
                    class="btn icon"
                    aria-label="Delete the saved file of {chapter.name}"
                    disabled={busyChapter[chapter.id]}
                    onclick={() => deleteFile(chapter)}
                  >
                    {#if busyChapter[chapter.id]}
                      <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
                    {:else}
                      <Icon name="trash" />
                    {/if}
                  </button>
                {:else}
                  <button
                    class="btn icon"
                    aria-label="Download {chapter.name}"
                    disabled={busyChapter[chapter.id]}
                    onclick={() => download([chapter])}
                  >
                    {#if busyChapter[chapter.id]}
                      <span class="spin"><Icon name="spinner" size={18} stroke={3} /></span>
                    {:else}
                      <Icon name="download" />
                    {/if}
                  </button>
                {/if}
                <!-- What is seldom wanted waits behind one button, so a row keeps to two. -->
                <div class="menu own">
                  <button class="btn icon" aria-label="More for {chapter.name}" aria-expanded={opened === chapter.id} onclick={() => (opened = opened === chapter.id ? null : chapter.id)}>
                    <Icon name="more" fill />
                  </button>
                  {#if opened === chapter.id}
                    <div class="panel sheet">
                      {#if title.webUrl}
                        <a class="btn" href={title.webUrl} target="_blank" rel="noreferrer noopener">
                          <Icon name="globe" size={18} stroke={3} />Open on site
                        </a>
                      {/if}
                      <button class="btn" onclick={() => mark([chapter], !chapter.read)}>
                        <Icon name="check" size={18} stroke={3} />{chapter.read ? 'Mark unread' : 'Mark read'}
                      </button>
                      <button class="btn" onclick={() => bookmark(chapter)}>
                        <Icon name="bookmark" size={18} fill={chapter.bookmark} />{chapter.bookmark ? 'Take the bookmark off' : 'Bookmark'}
                      </button>
                    </div>
                  {/if}
                </div>
              </li>
            {/each}
          </ul>
        {/each}
        {#if !refreshing && listed.length === 0}
          <Notice>{nothing}</Notice>
        {/if}
      </div>
    </div>

    <div class="dock">
      {#if resume}
        <a class="btn big spot" href="#/read/{resume.id}">
          {started ? 'Resume' : 'Start'} {chapterShort(resume)}<Icon name="arrow" size={18} stroke={3} />
        </a>
      {:else if chapters.length}
        <a class="btn big" href="#/read/{chapters[chapters.length - 1].id}">Read again<Icon name="arrow" size={18} stroke={3} /></a>
      {/if}
      {#if title.webUrl}
        <a class="btn big icon" href={title.webUrl} target="_blank" rel="noreferrer noopener" aria-label="Open on the source's site">
          <Icon name="globe" />
        </a>
      {/if}
    </div>
  {/if}
</div>

{#if inspectingCollection}
  <div class="modal-backdrop" onclick={closeCollectionInspector} role="presentation">
    <div
      class="modal-card collection-modal"
      onclick={(e) => e.stopPropagation()}
      onkeydown={(e) => { if (e.key === 'Escape') closeCollectionInspector(); }}
      role="dialog"
      aria-modal="true"
      tabindex="-1"
    >
      <div class="modal-header">
        <div>
          <span class="tag spot">COLLECTION INSPECTOR</span>
          <h2 class="display modal-title">{inspectingCollection.release.title}</h2>
          <span class="modal-subtext">
            {inspectingCollection.totalUnits} total volumes · 
            <strong class="text-spot">{inspectingCollection.missingCount} missing</strong> · 
            {inspectingCollection.ownedCount} owned in Komga
          </span>
        </div>
        <button class="btn icon" onclick={closeCollectionInspector} aria-label="Close">
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body">
        <div class="mode-selector-card">
          <span class="selector-title">Acquisition Mode:</span>
          <div class="mode-options">
            <label class="mode-label" class:selected={collectionMode === 'missing_only'}>
              <input type="radio" name="col_mode" value="missing_only" bind:group={collectionMode} />
              <div>
                <strong>Missing Only ({inspectingCollection.missingCount} volumes)</strong>
                <span class="mode-desc">Only retrieves unowned volumes and skips {inspectingCollection.ownedCount} duplicates.</span>
              </div>
            </label>

            <label class="mode-label" class:selected={collectionMode === 'entire'}>
              <input type="radio" name="col_mode" value="entire" bind:group={collectionMode} />
              <div>
                <strong>Entire Collection ({inspectingCollection.totalUnits} volumes)</strong>
                <span class="mode-desc">Retrieves all volumes in collection.</span>
              </div>
            </label>

            <label class="mode-label" class:selected={collectionMode === 'manual'}>
              <input type="radio" name="col_mode" value="manual" bind:group={collectionMode} />
              <div>
                <strong>Select Manually</strong>
                <span class="mode-desc">Pick individual volumes below.</span>
              </div>
            </label>
          </div>
        </div>

        <div class="collection-units-section">
          <div class="units-header-row">
            <h4>Included Volumes ({inspectingCollection.units.length})</h4>
            {#if collectionMode === 'manual'}
              <div class="quick-select">
                <button class="btn-text" onclick={() => selectAllManual(true)}>Select All</button>
                <span>·</span>
                <button class="btn-text" onclick={() => selectAllManual(false)}>Deselect All</button>
              </div>
            {/if}
          </div>

          <div class="units-scroll-list">
            {#each inspectingCollection.units as u (u.volume)}
              <label class="unit-row" class:unit-owned={u.isOwned} class:unit-acquiring={u.isAcquiring}>
                {#if collectionMode === 'manual'}
                  <input
                    type="checkbox"
                    checked={manualSelectedVolumes[u.volume]}
                    onchange={() => toggleManualVolume(u.volume)}
                  />
                {:else if collectionMode === 'missing_only'}
                  <input type="checkbox" checked={!u.isOwned} disabled />
                {:else}
                  <input type="checkbox" checked={true} disabled />
                {/if}

                <div class="unit-row-info">
                  <strong>Volume {u.volume}</strong>
                  {#if u.filename}
                    <span class="unit-filename">{u.filename}</span>
                  {/if}
                </div>

                <div class="unit-row-status">
                  {#if u.isOwned}
                    <span class="tag spot"><Icon name="check" size={12} /> Owned</span>
                  {:else if u.isAcquiring}
                    <span class="tag"><span class="spin"><Icon name="spinner" size={12} stroke={3} /></span> Acquiring</span>
                  {:else}
                    <span class="tag warn">Missing</span>
                  {/if}
                </div>
              </label>
            {/each}
          </div>
        </div>
      </div>

      <div class="modal-footer">
        <button class="btn" onclick={closeCollectionInspector} disabled={acquiringCollectionLoading}>
          Cancel
        </button>
        <button
          class="btn spot"
          onclick={executeAcquireCollection}
          disabled={acquiringCollectionLoading}
        >
          {#if acquiringCollectionLoading}
            <span class="spin"><Icon name="spinner" size={16} stroke={3} /></span> Enqueuing Volumes…
          {:else}
            <Icon name="download" size={16} />
            Acquire {
              collectionMode === 'missing_only'
                ? inspectingCollection.missingCount
                : collectionMode === 'entire'
                ? inspectingCollection.totalUnits
                : Object.values(manualSelectedVolumes).filter(Boolean).length
            } Volumes via TorBox
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if planModalOpen}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="modal-backdrop" onclick={() => (planModalOpen = false)}>
    <div class="modal-content plan-modal" onclick={(e) => e.stopPropagation()}>
      <div class="modal-header">
        <div>
          <h3>Plan Missing Content: {title.title}</h3>
          <span class="muted">Profile: <strong>{currentPlan?.profileName ?? currentProfile?.name ?? 'Default Manga'}</strong></span>
        </div>
        <button class="btn icon close-btn" onclick={() => (planModalOpen = false)} aria-label="Close">
          <Icon name="close" size={18} />
        </button>
      </div>

      <div class="modal-body">
        {#if planLoading}
          <div class="nyaa-loading-box">
            <span class="spin"><Icon name="spinner" size={24} stroke={3} /></span>
            Evaluating candidates across providers & optimizing plan…
          </div>
        {:else if planError}
          <Notice error={planError} retry={() => openPlanModal(true)} />
        {:else if currentPlan}
          <!-- Plan Confidence & Status Banner -->
          <div class="plan-header-banner">
            <div class="plan-status-badges">
              <span class="tag {currentPlan.confidence === 'high' ? 'spot' : 'warn'}" style="font-weight: 700;">
                {currentPlan.confidence === 'high' ? 'HIGH CONFIDENCE' : 'REVIEW RECOMMENDED'}
              </span>
              <span class="tag">
                {currentPlan.missingUnitsCoveredCount} / {currentPlan.missingUnits.length} Missing Volumes Covered
              </span>
              {#if currentPlan.totalEstimatedSize > 0}
                <span class="tag">
                  Est. Retrieval: {(currentPlan.totalEstimatedSize / (1024 * 1024 * 1024)).toFixed(2)} GiB
                </span>
              {/if}
              <span class="tag spot">DRY RUN PREVIEW</span>
            </div>

            {#if currentPlan.staleReason}
              <div class="plan-stale-notice">
                <Icon name="info" size={16} />
                <span>{currentPlan.staleReason}</span>
              </div>
            {/if}
          </div>

          <!-- Recommended Acquisitions -->
          <h4 style="margin-top: 1rem; margin-bottom: 0.5rem;">Recommended Plan</h4>
          {#if currentPlan.selectedItems.length === 0}
            <p class="muted">No viable releases found for the missing units with current profile preferences.</p>
          {:else}
            <div class="plan-items-list">
              {#each currentPlan.selectedItems as item (item.candidate.providerReleaseId)}
                <div class="plan-candidate-card recommended">
                  <div class="plan-candidate-header">
                    <label class="candidate-checkbox-label">
                      <input
                        type="checkbox"
                        checked={selectedPlanItemIds[item.candidate.providerReleaseId]}
                        onchange={(e) => {
                          selectedPlanItemIds[item.candidate.providerReleaseId] = (e.target as HTMLInputElement).checked;
                        }}
                      />
                      <strong class="candidate-title">{item.candidate.title}</strong>
                    </label>
                    <span class="tag spot">{item.candidate.provider.toUpperCase()}</span>
                  </div>

                  <div class="candidate-meta-row">
                    {#if item.candidate.uploader}
                      <span>Uploader: <strong>{item.candidate.uploader}</strong></span>
                    {/if}
                    {#if item.candidate.format}
                      <span>Format: <strong>{item.candidate.format}</strong></span>
                    {/if}
                    {#if item.candidate.size}
                      <span>Size: <strong>{Math.round(item.candidate.size / (1024 * 1024))} MiB</strong></span>
                    {/if}
                    {#if item.candidate.health.seeders !== null && item.candidate.health.seeders !== undefined}
                      <span>Seeders: <strong>{item.candidate.health.seeders}</strong></span>
                    {/if}
                    <span>Smart Score: <strong>{item.candidate.score.toFixed(0)}</strong></span>
                  </div>

                  <!-- Explanations -->
                  <div class="candidate-explanations-box">
                    {#each item.explanation.whyRecommended as why}
                      <div class="why-rec-line">{why}</div>
                    {/each}
                    {#each item.explanation.whyLower as why}
                      <div class="why-low-line">{why}</div>
                    {/each}
                  </div>
                </div>
              {/each}
            </div>
          {/if}

          <!-- Alternatives (Collapsible) -->
          {#if currentPlan.alternatives.length > 0}
            <div class="alternatives-toggle-row">
              <button class="btn pill icon-text" onclick={() => (showAlternatives = !showAlternatives)}>
                <Icon name={showAlternatives ? 'chevron-up' : 'chevron-down'} size={14} />
                {showAlternatives ? 'Hide' : 'Show'} Alternative Candidates ({currentPlan.alternatives.length})
              </button>
            </div>

            {#if showAlternatives}
              <div class="alternatives-list">
                {#each currentPlan.alternatives as alt (alt.providerReleaseId)}
                  <div class="plan-candidate-card alternative">
                    <div class="plan-candidate-header">
                      <span class="candidate-title">{alt.title}</span>
                      <span class="tag">{alt.provider.toUpperCase()}</span>
                    </div>
                    <div class="candidate-meta-row">
                      {#if alt.uploader}<span>Uploader: <strong>{alt.uploader}</strong></span>{/if}
                      {#if alt.size}<span>Size: <strong>{Math.round(alt.size / (1024 * 1024))} MiB</strong></span>{/if}
                      {#if alt.health.seeders !== null && alt.health.seeders !== undefined}
                        <span>Seeders: <strong>{alt.health.seeders}</strong></span>
                      {/if}
                      <span>Smart Score: <strong>{alt.score.toFixed(0)}</strong></span>
                    </div>
                  </div>
                {/each}
              </div>
            {/if}
          {/if}

          <!-- Dry-run Safety Explainer -->
          <div class="dry-run-safety-box">
            <Icon name="check" size={16} />
            <div>
              <strong>Dry Run Mode:</strong>
              <span>No external jobs created. No TorBox submission. No files modified. Execution remains explicitly manual.</span>
            </div>
          </div>
        {/if}
      </div>

      <div class="modal-footer">
        <button class="btn" onclick={() => (planModalOpen = false)}>Close</button>
        {#if currentPlan && currentPlan.selectedItems.length > 0}
          <button class="btn" onclick={handleSavePlan}>
            <Icon name="bookmark" size={14} /> Save Plan
          </button>
          <button class="btn spot" onclick={handleAcquirePlanSelected} disabled={acquiringPlanLoading}>
            <Icon name="download" size={14} />
            {acquiringPlanLoading ? 'Submitting…' : 'Acquire Selected'}
          </button>
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .page {
    gap: 20px;
    padding-bottom: calc(120px + var(--safe-bottom));
  }

  .bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  /* With the menu along the bottom this screen stands alone, and its bar is the top of it: always there, for the way back. */
  :global(:root[data-nav='tabs']) .page {
    padding-top: 0;
  }

  :global(:root[data-nav='tabs']) .bar {
    position: sticky;
    top: var(--safe-top);
    z-index: 5;
    margin: 0 calc(-1 * var(--gutter));
    padding: 10px var(--gutter);
    border-bottom: 3px solid var(--ink);
    background: var(--paper);
  }

  .menu {
    position: relative;
  }

  .sheet {
    position: absolute;
    top: 52px;
    right: 0;
    z-index: 5;
    display: flex;
    flex-direction: column;
    min-width: 240px;
    max-height: 70vh;
    overflow-y: auto;
  }

  .sheet .btn {
    justify-content: flex-start;
    border-width: 0 0 2px;
  }

  .sheet .btn:last-child {
    border-bottom: 0;
  }

  .link {
    text-decoration: underline;
  }

  .cats {
    display: flex;
    flex-direction: column;
  }

  .cats span {
    padding: 10px 14px 6px;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  .columns {
    display: grid;
    grid-template-columns: minmax(0, 1fr);
    gap: 28px 48px;
  }

  @media (min-width: 960px) {
    .columns {
      grid-template-columns: minmax(0, 5fr) minmax(0, 7fr);
      align-items: start;
    }
  }

  .about {
    display: flex;
    flex-direction: column;
    gap: 18px;
  }

  .hero {
    display: flex;
    gap: 22px;
    padding: 6px 4px 0;
  }

  .art {
    flex: 0 0 clamp(110px, 30%, 170px);
  }

  .facts {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
  }

  .facts h1 {
    font-size: clamp(24px, 4.4vw, 40px);
    overflow-wrap: anywhere;
  }

  .facts strong,
  .facts > span {
    font-size: 13px;
    font-weight: 700;
  }

  .stats {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    border: 3px solid var(--ink);
    background: var(--surface);
  }

  .stats div {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 10px 4px;
    border-right: 3px solid var(--ink);
  }

  .stats div:last-child {
    border-right: 0;
  }

  .stats .hot {
    background: var(--spot);
    color: var(--on-spot);
  }

  .stats .display {
    font-size: 22px;
    line-height: 1.15;
  }

  .stats span:last-child {
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.1em;
    text-transform: uppercase;
  }

  .ownership-summary {
    border: 3px solid var(--ink);
    background: var(--surface);
    padding: 10px 14px;
    margin-top: 12px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    box-shadow: var(--sh-4);
  }

  .ownership-summary.unavailable {
    background: var(--paper-warm, var(--surface));
  }

  .ownership-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
  }

  .ownership-badge-title {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    font-weight: 900;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    color: var(--ink);
  }

  .ownership-actions {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .btn.mini {
    padding: 2px 8px;
    min-height: 26px;
    font-size: 11px;
    font-weight: 800;
    border-width: 2px;
  }

  .ownership-body {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 12px;
  }

  .ownership-metrics-row {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 16px;
  }

  .ownership-metric-col {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .ownership-metric {
    font-size: 14px;
    font-weight: 800;
    letter-spacing: 0.05em;
  }

  .ownership-metric strong {
    font-size: 18px;
    font-family: var(--display, inherit);
  }

  .ownership-expl {
    font-size: 12px;
    font-weight: 700;
    opacity: 0.75;
  }

  .ownership-details-box {
    margin-top: 6px;
    padding-top: 8px;
    border-top: 2px dashed var(--ink);
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
  }

  .detail-row {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .detail-k {
    font-weight: 800;
    min-width: 110px;
    opacity: 0.7;
  }

  .detail-v {
    font-weight: 700;
  }

  .detail-id {
    font-size: 11px;
    opacity: 0.75;
    background: var(--paper, #f5f5f5);
    padding: 1px 4px;
  }

  .capitalize {
    text-transform: capitalize;
  }

  .text {
    font-size: 14px;
    line-height: 1.5;
    white-space: pre-line;
    overflow-wrap: anywhere;
  }

  .clamp {
    display: -webkit-box;
    overflow: hidden;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 5;
    line-clamp: 5;
  }

  .more {
    align-self: flex-start;
  }

  .genres .tag {
    letter-spacing: 0.06em;
  }

  .chapters {
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 0;
  }

  .chapters h2 {
    font-size: 20px;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .done .main {
    color: var(--muted);
  }

  .done .main strong {
    font-weight: 700;
  }

  .meter {
    flex: 0 0 56px;
  }

  .sorter select {
    max-width: 220px;
  }

  .uploader-filter {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 10px;
    margin-top: 14px;
    padding-bottom: 2px;
  }

  .uploader-label {
    font-size: 13px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
  }

  .uploader-filter .pills {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
  }

  .own .sheet {
    min-width: 200px;
  }

  .kept {
    display: inline-flex;
    margin-right: 4px;
    vertical-align: -1px;
  }

  .dock {
    position: fixed;
    right: 0;
    bottom: var(--tabs);
    left: 0;
    z-index: 4;
    display: flex;
    justify-content: center;
    gap: 14px;
    padding: 14px max(var(--gutter), var(--safe-right)) calc(14px + var(--safe-bottom)) max(var(--gutter), var(--safe-left));
    border-top: 3px solid var(--ink);
    background: var(--paper);
  }

  .dock .btn:first-child {
    flex: 0 1 420px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .volume-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 8px 12px;
    margin-top: 16px;
    background: var(--surface);
    border: 3px solid var(--ink);
    box-shadow: 2px 2px 0 var(--ink);
  }

  .volume-info {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .volume-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .volume-header h3 {
    margin: 0;
    font-size: 14px;
    letter-spacing: 0.08em;
  }

  .tag.vol {
    display: inline-block;
    margin-right: 6px;
    padding: 1px 6px;
    font-size: 11px;
    font-weight: 900;
    background: var(--surface);
    border: 1.5px solid var(--ink);
  }

  .nyaa-discovery-card {
    border: 2px solid var(--ink);
    background: var(--paper);
    padding: 16px;
    margin: 16px 0;
    box-shadow: 3px 3px 0 var(--ink);
  }

  .nyaa-header-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    margin-bottom: 8px;
  }

  .nyaa-title {
    margin: 0;
    font-size: 15px;
    letter-spacing: 0.08em;
  }

  .nyaa-subtext {
    font-size: 12px;
    opacity: 0.75;
  }

  .nyaa-loading-box {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 16px;
    font-size: 14px;
    opacity: 0.85;
  }

  .nyaa-muted {
    font-size: 13px;
    opacity: 0.7;
    margin: 12px 0 4px;
  }

  .candidate-list {
    list-style: none;
    padding: 0;
    margin: 12px 0 0;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }

  .candidate-item {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 16px;
    border: 1.5px solid var(--ink);
    background: var(--surface);
    padding: 12px;
    box-shadow: 2px 2px 0 var(--ink);
  }

  .candidate-item.owned {
    opacity: 0.75;
  }

  .candidate-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .candidate-badges {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }

  .candidate-title {
    font-size: 14px;
    line-height: 1.35;
    word-break: break-word;
  }

  .candidate-meta {
    display: flex;
    gap: 8px;
    font-size: 12px;
    opacity: 0.8;
    flex-wrap: wrap;
  }

  .candidate-meta .seeders strong {
    color: var(--accent, #008800);
  }

  .candidate-action {
    flex-shrink: 0;
  }

  .nyaa-controls {
    display: flex;
    flex-direction: column;
    gap: 10px;
    margin-bottom: 14px;
    padding: 10px 12px;
    background: var(--shade);
    border: 2px solid var(--ink);
    border-radius: var(--radius);
  }

  .nyaa-filter-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
  }

  .filter-group {
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .filter-label {
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
    opacity: 0.7;
    letter-spacing: 0.5px;
  }

  .pill-group {
    display: flex;
    background: var(--paper);
    border: 1px solid var(--ink);
    border-radius: 999px;
    padding: 2px;
    gap: 2px;
  }

  .filter-pill {
    background: none;
    border: none;
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 999px;
    cursor: pointer;
    color: inherit;
    transition: all 0.15s ease;
  }

  .filter-pill.active {
    background: var(--ink);
    color: var(--paper);
  }

  .select-compact {
    padding: 3px 8px;
    font-size: 12px;
    height: auto;
    border: 1px solid var(--ink);
    border-radius: var(--radius);
    background: var(--paper);
  }

  .targeted-search-banner {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 6px 10px;
    background: var(--paper);
    border-left: 3px solid var(--accent);
    font-size: 12px;
  }

  .smart-badge {
    font-weight: 700;
    letter-spacing: 0.5px;
  }

  .coverage-pill {
    font-size: 11px;
  }

  .candidate-explanation {
    font-size: 12px;
    color: var(--ink);
    opacity: 0.9;
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
  }

  .btn-text-score {
    background: none;
    border: none;
    font-size: 11px;
    color: var(--accent);
    cursor: pointer;
    text-decoration: underline;
    padding: 0;
  }

  .score-breakdown-panel {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(110px, 1fr));
    gap: 6px;
    padding: 8px 10px;
    background: var(--shade);
    border: 1px dashed var(--ink);
    border-radius: var(--radius);
    font-size: 11px;
  }

  .score-col.penalty {
    color: var(--error, #cc0000);
  }

  /* Modal Backdrop & Card */
  .modal-backdrop {
    position: fixed;
    inset: 0;
    z-index: 1000;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
  }

  .modal-card.collection-modal {
    background: var(--paper);
    border: 3px solid var(--ink);
    border-radius: var(--radius);
    box-shadow: 4px 4px 0 var(--ink);
    width: 100%;
    max-width: 640px;
    max-height: 85vh;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .modal-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 2px solid var(--ink);
    background: var(--shade);
  }

  .modal-title {
    font-size: 18px;
    margin: 4px 0 2px;
    word-break: break-word;
  }

  .modal-subtext {
    font-size: 13px;
    opacity: 0.85;
  }

  .text-spot {
    color: var(--accent);
  }

  .modal-body {
    padding: 16px 20px;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .mode-selector-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .selector-title {
    font-size: 12px;
    font-weight: 700;
    text-transform: uppercase;
    opacity: 0.7;
  }

  .mode-options {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .mode-label {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 12px;
    border: 2px solid var(--ink);
    border-radius: var(--radius);
    background: var(--paper);
    cursor: pointer;
    transition: all 0.15s ease;
  }

  .mode-label.selected {
    background: var(--shade);
    border-color: var(--accent);
    box-shadow: 2px 2px 0 var(--ink);
  }

  .mode-desc {
    display: block;
    font-size: 12px;
    opacity: 0.75;
    margin-top: 2px;
  }

  .collection-units-section {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .units-header-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .units-header-row h4 {
    margin: 0;
    font-size: 13px;
    text-transform: uppercase;
    opacity: 0.8;
  }

  .quick-select {
    display: flex;
    gap: 6px;
    font-size: 11px;
  }

  .btn-text {
    background: none;
    border: none;
    padding: 0;
    font-size: 11px;
    color: var(--accent);
    cursor: pointer;
    text-decoration: underline;
  }

  .units-scroll-list {
    max-height: 240px;
    overflow-y: auto;
    border: 2px solid var(--ink);
    border-radius: var(--radius);
    display: flex;
    flex-direction: column;
  }

  .unit-row {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--ink);
    cursor: pointer;
    font-size: 13px;
  }

  .unit-row:last-child {
    border-bottom: none;
  }

  .unit-row.unit-owned {
    opacity: 0.7;
    background: var(--shade);
  }

  .unit-row-info {
    flex: 1;
    display: flex;
    flex-direction: column;
    min-width: 0;
  }

  .unit-filename {
    font-size: 11px;
    opacity: 0.7;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .modal-footer {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 12px;
    padding: 12px 20px;
    border-top: 2px solid var(--ink);
    background: var(--shade);
  }

  @media (max-width: 640px) {
    .candidate-item {
      flex-direction: column;
      align-items: flex-start;
    }
    .candidate-action {
      align-self: flex-end;
    }
  }

  @media (max-width: 640px) {
    .site-label {
      display: none;
    }
  }

  /* Missing content overview & profile row */
  .missing-content-overview-row {
    margin-top: 0.75rem;
    padding-top: 0.75rem;
    border-top: 1px dashed var(--border-color, rgba(255, 255, 255, 0.15));
    display: flex;
    flex-wrap: wrap;
    justify-content: space-between;
    align-items: center;
    gap: 0.75rem;
  }

  .missing-status-line {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .planner-actions-row {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .title-profile-row {
    margin-top: 0.5rem;
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: 0.85rem;
  }

  /* Plan Modal */
  .plan-modal {
    max-width: 680px;
  }

  .plan-header-banner {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding-bottom: 0.75rem;
    border-bottom: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
  }

  .plan-status-badges {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }

  .plan-stale-notice {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    color: var(--spot, #ff7a00);
    font-size: 0.82rem;
  }

  .plan-items-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }

  .plan-candidate-card {
    background: var(--card-bg, rgba(255, 255, 255, 0.03));
    border: 1px solid var(--border-color, rgba(255, 255, 255, 0.1));
    border-radius: 8px;
    padding: 0.85rem 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .plan-candidate-card.recommended {
    border-color: var(--spot, #ff7a00);
  }

  .plan-candidate-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  .candidate-checkbox-label {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    cursor: pointer;
  }

  .candidate-meta-row {
    display: flex;
    flex-wrap: wrap;
    gap: 1rem;
    font-size: 0.82rem;
    color: var(--muted, #888);
  }

  .candidate-explanations-box {
    margin-top: 0.35rem;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.8rem;
  }

  .why-rec-line {
    color: #4ade80;
  }

  .why-low-line {
    color: #f87171;
  }

  .alternatives-toggle-row {
    margin-top: 0.75rem;
  }

  .alternatives-list {
    margin-top: 0.5rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .dry-run-safety-box {
    margin-top: 1rem;
    display: flex;
    align-items: flex-start;
    gap: 0.75rem;
    padding: 0.75rem 1rem;
    background: rgba(255, 122, 0, 0.08);
    border: 1px solid rgba(255, 122, 0, 0.25);
    border-radius: 6px;
    font-size: 0.82rem;
  }
</style>
