// Calls to Mango's API, and the shapes it answers with.

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
  }
}

type Options = { method?: string; json?: unknown; signal?: AbortSignal };

/** Told when Mango answers that nobody is signed in. The session store listens. */
export const gate = { lost: (_code: string) => {} };

export async function api<T>(path: string, options: Options = {}): Promise<T> {
  let response: Response;
  try {
    response = await fetch('api' + path, {
      method: options.method ?? 'GET',
      signal: options.signal,
      headers: options.json === undefined ? undefined : { 'Content-Type': 'application/json' },
      body: options.json === undefined ? undefined : JSON.stringify(options.json),
    });
  } catch (error) {
    if (options.signal?.aborted) throw error;
    throw new ApiError(0, 'offline', 'Mango did not answer. Is it still running?');
  }
  let data: any = null;
  try {
    data = await response.json();
  } catch {
    // An answer without JSON: only the status says anything.
  }
  if (!response.ok) {
    const code = data?.error?.code;
    if (response.status === 401 && (code === 'signed_out' || code === 'setup_required')) gate.lost(code);
    throw new ApiError(response.status, data?.error?.code ?? 'error', data?.error?.message ?? `Mango answered ${response.status}`);
  }
  return data as T;
}

export const get = <T>(path: string, signal?: AbortSignal) => api<T>(path, { signal });
export const send = <T = unknown>(method: string, path: string, json?: unknown, signal?: AbortSignal) => api<T>(path, { method, json, signal });

/** A sentence for the screen, whatever was thrown. */
export function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export type Release = {
  version: string;
  name: string;
  notes: string;
  url: string;
  publishedAt: string | null;
  prerelease: boolean;
  /** The file for the platform Mango runs on, when the release has one. */
  download: { name: string; url: string; size: number } | null;
};

export type VersionInfo = {
  current: string;
  commit: string;
  platform: string;
  docker: boolean;
  /** Whether this Mango knows where its releases are listed. */
  canCheck: boolean;
  checkHours: number;
  checkedAt: number | null;
  error: string | null;
  updateAvailable: boolean;
  latest: Release | null;
  releases: Release[];
};

export type Card = { id: number; sourceId: string; title: string; cover: string | null; inLibrary: boolean };
export type Listing = { items: Card[]; hasNextPage: boolean };

export type Chapter = {
  id: number;
  name: string;
  number: number;
  scanlator: string | null;
  dateUpload: number;
  fetchedAt: number;
  read: boolean;
  lastPageRead: number;
  pageCount: number | null;
  downloaded: boolean;
  bookmark: boolean;
  volume?: number | null;
  releaseType?: 'chapter' | 'volume';
};

export type Title = {
  id: number;
  sourceId: string;
  sourceName: string | null;
  sourceLang: string | null;
  webUrl: string | null;
  title: string;
  artist: string | null;
  author: string | null;
  description: string | null;
  genres: string[];
  status: number;
  cover: string | null;
  inLibrary: boolean;
  addedAt: number | null;
  initialized: boolean;
  readerMode: string | null;
  readerFit: string | null;
  autoDownload: boolean;
  chaptersAt: number | null;
  /** How many days usually pass between two chapters, when the source's dates say. */
  releaseDays: number | null;
  categories: number[];
  chapters: Chapter[];
};

/** One of the two titles of a migration. */
export type MigrateSide = {
  id: number;
  title: string;
  cover: string | null;
  sourceName: string | null;
  sourceLang: string | null;
  inLibrary: boolean;
  chapters: number;
  read: number;
  /** The highest chapter number it has, and the highest that is read. -1 when there is none. */
  latest: number;
  lastRead: number;
  downloaded: number;
};

/** What moving `from` onto `to` would carry over. */
export type Comparison = { from: MigrateSide; to: MigrateSide; read: number; categories: number; tracks: number };

export type LibraryItem = {
  id: number;
  title: string;
  cover: string | null;
  sourceName: string | null;
  status: number;
  addedAt: number;
  total: number;
  unread: number;
  downloaded: number;
  lastReadAt: number;
  latestAt: number;
  categories: number[];
};

export type Category = { id: number; name: string };

export type Resume = {
  mangaId: number;
  title: string;
  cover: string | null;
  chapterId: number;
  chapterName: string;
  page: number;
  pageCount: number | null;
};

export type Library = { items: LibraryItem[]; categories: Category[]; resume: Resume | null };

export type HistoryEntry = {
  chapterId: number;
  chapterName: string;
  readAt: number;
  page: number;
  pageCount: number | null;
  read: boolean;
  mangaId: number;
  title: string;
  cover: string | null;
};

export type UpdateItem = {
  chapterId: number;
  chapterName: string;
  fetchedAt: number;
  read: boolean;
  downloaded: boolean;
  mangaId: number;
  title: string;
  cover: string | null;
  sourceName: string | null;
};

export type Updates = {
  items: UpdateItem[];
  errors: { mangaId: number; title: string; error: string; at: number }[];
  /** Unread chapters found in the last week. */
  fresh: number;
  /** `waiting` counts the titles a run left out because no chapter is expected of them yet. */
  status: { running: boolean; done: number; total: number; waiting: number; found: number; failed: number; startedAt: number | null; finishedAt: number | null };
};

export type Queued = {
  chapterId: number;
  chapterName: string;
  mangaId: number;
  title: string;
  state: string;
  stage: string;
  statusMessage?: string | null;
  destination?: string | null;
  source?: string | null;
  scanlator?: string | null;
  releaseType?: 'volume' | 'chapter' | null;
  volume?: number | null;
  done: number;
  total: number | null;
  attempts: number;
  retryAt: number | null;
  error: string | null;
  canRetryKomga?: boolean;
  metadataHandoff?: { state: string; message: string; jobId: string | null; komgaBookId: string | null } | null;
  failureStage?: string | null;
  publishedPath?: string | null;
  sourceType?: string | null;
  backendType?: string | null;
  externalJobId?: string | null;
  rawTitle?: string | null;
  uploader?: string | null;
  releaseSize?: number | null;
  collectionId?: string | null;
  collectionName?: string | null;
  bytesDownloaded?: number | null;
  totalBytes?: number | null;
  isStalled?: boolean;
  attempt?: number | null;
  maxAttempts?: number | null;
  transferStatus?: string | null;
};

export type Queue = { paused: boolean; queue: Queued[] };

export type ReaderInfo = {
  chapter: Chapter;
  mangaId: number;
  title: string;
  /** What sort of series the tags and the source make it; see lib/reader.ts. */
  kind: 'manga' | 'manhwa' | 'manhua' | 'comic';
  readerMode: string | null;
  readerFit: string | null;
  readerPrefs: Record<string, unknown> | null;
  prev: number | null;
  next: number | null;
  chapters: { id: number; name: string }[];
};

export type Tracker = {
  id: number;
  name: string;
  /** How one signs in: a token pasted in, a trip to the site and back, or a name and password. */
  login: 'token' | 'oauth' | 'password';
  signedIn: boolean;
  userName: string | null;
  statuses: { value: number; label: string }[];
  /** Scores show out of ten: what a link stores is the shown score times this. */
  scoreScale: number;
  scoreStep: number;
};

/** A title's link to a tracker. */
export type Track = {
  tracker: number;
  libraryId: number;
  mediaId: number;
  url: string;
  title: string;
  lastChapterRead: number;
  /** 0 when the tracker does not know. */
  totalChapters: number;
  score: number;
  status: number;
  /** Set while the last change has not reached the tracker. */
  error: string | null;
};

export type TrackingView = { trackers: Tracker[]; tracks: Track[] };

/** A title as a tracker's search lists it. */
export type TrackerMatch = { mediaId: number; title: string; summary: string; kind: string; year: number | null; totalChapters: number; url: string };

export type Source = {
  id: string;
  pkg: string;
  name: string;
  lang: string;
  baseUrl: string | null;
  supportsLatest: boolean;
  pinned: boolean;
  hidden: boolean;
};

/** One thing a source lets its user set. `value` is a boolean for a switch, text for a list or a text, a list of texts for a multi. */
export type SourcePref = {
  position: number;
  type: 'switch' | 'list' | 'multi' | 'text' | 'info';
  key: string | null;
  title: string | null;
  summary: string | null;
  enabled: boolean;
  visible: boolean;
  value?: boolean | string | string[] | null;
  /** What a list or a multi offers: the words shown, and the values kept, in the same order. */
  entries?: string[];
  entryValues?: string[];
  dialogMessage?: string | null;
};

/** One thing a source's search can be narrowed by. Its place in the list names it. */
export type SourceFilter =
  | { type: 'header' | 'separator'; name: string }
  | { type: 'select'; name: string; state: number; values: string[] }
  | { type: 'text'; name: string; state: string }
  | { type: 'checkbox'; name: string; state: boolean }
  /** 0 either way, 1 with it, 2 without it. */
  | { type: 'tristate'; name: string; state: number }
  | { type: 'sort'; name: string; state: { index: number; ascending: boolean } | null; values: string[] }
  | { type: 'group'; name: string; filters: SourceFilter[] };

/** A filter set to something else than the source starts it at. A group's state is the changes inside it. */
export type FilterChange = { position: number; state: unknown };

export type Store = {
  id: number;
  url: string;
  indexUrl: string;
  name: string;
  website: string | null;
  signingKey: string;
  keyChanged: boolean;
  offeredKey: string | null;
  extensions: number;
  checkedAt: number | null;
  error: string | null;
};

export type CatalogItem = {
  storeId: number;
  pkg: string;
  name: string;
  versionName: string;
  lib: string;
  contentWarning: number;
  languages: string[];
  sourceCount: number;
  installable: boolean;
  installed: boolean;
  installedVersion: string | null;
  updateAvailable: boolean;
};

export type Installed = {
  pkg: string;
  name: string;
  versionName: string;
  lib: string;
  contentWarning: number;
  storeId: number | null;
  updateAvailable: boolean;
  availableVersion: string | null;
  sources: Source[];
};

export interface PathValidationResult {
  path: string | null;
  code: 'valid' | 'not_configured' | 'missing' | 'not_a_directory' | 'not_writable';
  message: string;
  writable: boolean;
}

export interface OptimizerValidationResult {
  configured_path: string | null;
  resolved_path: string | null;
  source: 'custom' | 'bundled' | 'path' | 'none';
  code: 'valid' | 'not_configured' | 'missing' | 'not_a_file' | 'not_executable' | 'failed';
  message: string;
  version: string | null;
}

export interface ValidatePathsResponse {
  library: PathValidationResult;
  chinese_library: PathValidationResult;
  optimizer: OptimizerValidationResult;
  bundled_optimizer_present: boolean;
  resolved_optimizer_bin: string | null;
}

export interface PipelineConfig {
  enabled: boolean;
  library_dir: string | null;
  chinese_library_dir: string | null;
  mangabaka_enabled: boolean;
  mangabaka_url: string | null;
  mangabaka_token?: string | null;
  has_mangabaka_token: boolean;
  komf_enabled?: boolean;
  komf_url?: string | null;
  komga_enabled: boolean;
  komga_url: string | null;
  komga_api_key?: string | null;
  has_komga_api_key: boolean;
  komga_library_id: string | null;
  komga_chinese_library_id: string | null;
  komga_scan_enabled: boolean;
  optimize_enabled: boolean;
  optimizer_bin: string | null;
  target_height: number;
  image_quality: number;
  min_savings_percent: number;
  library_status?: PathValidationResult | null;
  chinese_library_status?: PathValidationResult | null;
  optimizer_status?: OptimizerValidationResult | null;
  bundled_optimizer_present?: boolean;
  resolved_optimizer_bin?: string | null;
  mangabaka_validated?: boolean | null;
}

export interface PipelineEvent {
  id: string;
  timestamp: number;
  chapter_id: number;
  series: string;
  release_name: string;
  is_volume: boolean;
  number: number;
  outcome: 'published' | 'already_published' | 'conflict' | 'skipped' | 'error';
  destination: string | null;
  mangabaka_status: string;
  optimizer_status: string;
  komga_status: string;
  error: string | null;
  mangabaka_id?: number | null;
  anilist_id?: number | null;
  requested_unit?: string | null;
}

export interface RepairCbzRequest {
  path?: string | null;
  series?: string | null;
  volume?: number | null;
  anilistId?: number | null;
  mangabakaId?: number | null;
}

export interface RepairOutcome {
  status: string;
  canonicalPath: string;
  series: string;
  volume: number;
  mangabakaId: number;
  fieldsPopulated: string[];
  pageCount: number;
}

export type OwnershipStatus = 'owned' | 'available' | 'acquiring' | 'failed' | 'unknown';

export interface VolumeOwnership {
  volume: number;
  status: OwnershipStatus;
  bookId?: string | null;
  bookName?: string | null;
  error?: string | null;
}

export interface ChapterOwnership {
  chapter: number;
  status: OwnershipStatus;
  bookId?: string | null;
  bookName?: string | null;
  error?: string | null;
}

export interface OwnershipSummary {
  ownedVolumesCount: number;
  totalSourceVolumes?: number | null;
  ownedChaptersCount: number;
  totalSourceChapters?: number | null;
  sourceReleaseType: 'volume' | 'chapter' | 'mixed' | 'empty';
  authoritativeTotalVolumes?: number | null;
  authoritativeTotalChapters?: number | null;
  explanation?: string | null;
}

export interface TitleOwnership {
  komgaConnected: boolean;
  error?: string | null;
  activeError?: string | null;
  seriesId?: string | null;
  seriesName?: string | null;
  libraryId?: string | null;
  libraryName?: string | null;
  volumes: VolumeOwnership[];
  chapters?: ChapterOwnership[];
  summary: OwnershipSummary;
  lastRefreshed: number;
}

export interface KomgaLibrary {
  id: string;
  name: string;
  root: string;
  unavailable: boolean;
}

export type SyncTarget = 'anilist' | 'mangabaka';

export type SyncResultCategory =
  | 'updated'
  | 'would_update'
  | 'dry_run_would_update'
  | 'no_change'
  | 'tracker_ahead'
  | 'skipped_untouched'
  | 'skipped_no_mapping'
  | 'ambiguous_mapping'
  | 'skipped_ambiguous_mapping'
  | 'skipped_unsafe_progress'
  | 'not_in_tracker_library'
  | 'skipped_not_in_tracker_library'
  | 'auth_error'
  | 'network_error'
  | 'unsupported'
  | 'verification_failed'
  | 'rate_limited'
  | 'remote_error';

export interface SyncRecord {
  id: string;
  timestamp: number;
  komgaSeriesId: string;
  komgaSeriesTitle: string;
  tracker: SyncTarget;
  trackerMediaId?: string | null;
  oldProgress?: number | null;
  newProgress?: number | null;
  dimension?: string | null;
  statusChange?: string | null;
  category: SyncResultCategory;
  message: string;
  isDryRun: boolean;
}

export interface SyncSummary {
  totalSeries: number;
  inspectedSeries?: number;
  untouchedSeries?: number;
  eligibleSeries?: number;
  anilistMapped?: number;
  mangabakaMapped?: number;
  unresolvedMappings?: number;
  ambiguousMappings?: number;
  updated: number;
  noChange: number;
  trackerAhead?: number;
  dryRunUpdates: number;
  skipped: number;
  errors: number;
  isDryRun: boolean;
  records: SyncRecord[];
}

export interface TrackerMapping {
  komgaSeriesId: string;
  tracker: SyncTarget;
  trackerId: string;
  trackerTitle?: string | null;
  editionId?: string | null;
  mappingSource?: string;
  confidence?: string;
  manualOverride: boolean;
  lastValidated?: number | null;
  updatedAt: number;
}

export interface UnresolvedTitleDiagnostic {
  komgaSeriesId: string;
  komgaTitle: string;
  tracker: SyncTarget;
  reason: string;
  candidates: string[];
  confidence: string;
  source: string;
}

export const runSyncDryRun = (target?: string, libraryId?: string) =>
  send<SyncSummary>('POST', '/sync/dry-run', { target, libraryId });

export const runSync = (target?: string, libraryId?: string) =>
  send<SyncSummary>('POST', '/sync/run', { target, libraryId });

export interface SyncStatus {
  running: boolean;
  komga_enabled: boolean;
  has_komga_api_key: boolean;
  mangabaka_enabled: boolean;
  has_mangabaka_token: boolean;
  mangabaka_validated: boolean | null;
}

export const getSyncStatus = () =>
  get<SyncStatus>('/sync/status');

export const getSyncHistory = (limit: number = 100) =>
  get<SyncRecord[]>(`/sync/history?limit=${limit}`);

export const getSyncMappings = (tracker?: string) =>
  get<TrackerMapping[]>(tracker ? `/sync/mappings?tracker=${tracker}` : '/sync/mappings');

export const saveSyncMapping = (mapping: TrackerMapping) =>
  send<boolean>('PUT', '/sync/mappings', mapping);

export const deleteSyncMapping = (seriesId: string, tracker: string) =>
  send<boolean>('DELETE', `/sync/mappings/${encodeURIComponent(seriesId)}/${encodeURIComponent(tracker)}`);

export const getUnresolvedDiagnostics = () =>
  get<UnresolvedTitleDiagnostic[]>('/sync/diagnostics/unresolved');

export const setManualSyncMapping = (payload: {
  komgaSeriesId: string;
  tracker: string;
  trackerId: string;
  editionId?: string | null;
  trackerTitle?: string | null;
}) =>
  send<TrackerMapping>('POST', '/sync/mappings/manual', payload);

export const resetSyncMapping = (komgaSeriesId: string, tracker: string) =>
  send<boolean>('POST', '/sync/mappings/reset', { komgaSeriesId, tracker });

export interface SyncScheduleConfig {
  enabled: boolean;
  intervalHours: number;
  lastRun?: number | null;
}

export const getSyncSchedule = () =>
  get<SyncScheduleConfig>('/sync/schedule');

export const saveSyncSchedule = (cfg: SyncScheduleConfig) =>
  send<boolean>('PUT', '/sync/schedule', cfg);

export interface TrackedCatalogEntry {
  id: number;
  mangaId?: number | null;
  komgaSeriesId?: string | null;
  title: string;
  origins: string[];
  autoAcquire: boolean;
  tracked: boolean;
  createdAt: number;
  updatedAt: number;
}

export const getTrackedCatalog = () =>
  get<TrackedCatalogEntry[]>('/catalog/tracked');

export const setTrackedAutoAcquire = (id: number, enabled: boolean) =>
  send<boolean>('PUT', `/catalog/tracked/${id}/auto-acquire`, { enabled });

export const setTrackedStatus = (id: number, enabled: boolean) =>
  send<boolean>('PUT', `/catalog/tracked/${id}/tracked`, { enabled });

export interface TrackerItemDetail {
  trackerId: string;
  title?: string | null;
  coverUrl?: string | null;
  status: string;
  normalizedStatus: string;
  progressChapter?: number | null;
  progressVolume?: number | null;
  totalChapters?: number | null;
  totalVolumes?: number | null;
  updatedAt: number;
}

export interface MangaBakaCatalogInfo {
  seriesId: number;
  title: string;
  englishTitle?: string | null;
  romajiTitle?: string | null;
  nativeTitle?: string | null;
  coverUrl?: string | null;
  authors: string[];
  artists: string[];
  publisher?: string | null;
  genres: string[];
  tags: string[];
  summary?: string | null;
  totalVolumes?: number | null;
  webUrl?: string | null;
}

export interface UnifiedTrackedItem {
  id: string;
  title: string;
  coverUrl?: string | null;
  aliases?: string[];
  normalizedStatus: string;
  format?: string | null;
  anilist?: TrackerItemDetail | null;
  mangabaka?: TrackerItemDetail | null;
  catalog?: MangaBakaCatalogInfo | null;
  ownershipStatus: string; // "Owned" | "Partially Owned" | "Not Owned" | "Unknown"
  ownedUnits?: number | null;
  totalUnits?: number | null;
  komgaSeriesId?: string | null;
  acquisitionStatus: string;
  autoAcquire: boolean;
  updatedAt: number;
}

export interface TrackerListRefreshStatus {
  tracker: string;
  lastRefreshedAt: number;
  itemCount: number;
  status: string;
  errorMessage?: string | null;
}

export interface UnifiedCatalogSummary {
  total: number;
  reading: number;
  planning: number;
  completed: number;
  paused: number;
  dropped: number;
  owned: number;
  partiallyOwned: number;
  notOwned: number;
}

export interface UnifiedCatalogResponse {
  items: UnifiedTrackedItem[];
  refreshStatuses: TrackerListRefreshStatus[];
  summary: UnifiedCatalogSummary;
}

export interface ListsQuery {
  status?: string;
  ownership?: string;
  tracker?: string;
  q?: string;
  sort?: string;
}

export interface ListsRefreshReport {
  refreshedTrackers: string[];
  errors: string[];
  statuses: TrackerListRefreshStatus[];
}

export const getLists = (query?: ListsQuery) => {
  const params = new URLSearchParams();
  if (query?.status) params.set('status', query.status);
  if (query?.ownership) params.set('ownership', query.ownership);
  if (query?.tracker) params.set('tracker', query.tracker);
  if (query?.q) params.set('q', query.q);
  if (query?.sort) params.set('sort', query.sort);
  const qs = params.toString();
  return get<UnifiedCatalogResponse>(`/lists${qs ? `?${qs}` : ''}`);
};

export const refreshLists = (tracker?: string | null) =>
  send<ListsRefreshReport>('POST', '/lists/refresh', { tracker: tracker ?? null });

// ---------- Acquisition (Nyaa & TorBox) ----------

export type AcquisitionReleaseUnit =
  | { type: 'volume'; value: number }
  | { type: 'chapter'; value: number }
  | { type: 'collection'; value: { start: number; end: number } }
  | { type: 'unknown' };

export interface AcquisitionRelease {
  source: string;
  sourceItemId: string;
  title: string;
  rawTitle: string;
  uploader?: string | null;
  sourceUrl: string;
  torrentUrl?: string | null;
  magnetUrl?: string | null;
  infoHash?: string | null;
  categoryId?: string | null;
  categoryName?: string | null;
  sizeBytes: number;
  sizeFormatted: string;
  seeders: number;
  leechers: number;
  publishedAt?: number | null;
  isTrusted: boolean;
  isRemake: boolean;
  isVariant?: boolean;
  variantName?: string | null;
  isMixed?: boolean;
  releaseUnit: AcquisitionReleaseUnit;
  releaseNumber?: number | null;
  releaseRange?: [number, number] | null;
  language?: string | null;
}

export interface SmartScore {
  identity: number;
  coverage: number;
  quality: number;
  health: number;
  freshness: number;
  efficiency: number;
  penalties: number;
  total: number;
  explanation: string;
  summaryTag: string;
}

export interface AcquisitionCandidate {
  release: AcquisitionRelease;
  score: number;
  smartScore?: SmartScore | null;
  matchType: string;
  ownershipStatus: string;
  unitsIncluded: number[];
  unitsOwned: number[];
  unitsMissing: number[];
  coverageSummary: string;
  usefulEfficiencyBytes?: number | null;
  isVariant?: boolean;
  variantName?: string | null;
}

export interface CollectionUnitEntry {
  volume: number;
  fileId?: number | null;
  filename?: string | null;
  fileSize?: number | null;
  isOwned: boolean;
  isAcquiring: boolean;
}

export interface CollectionInspection {
  release: AcquisitionRelease;
  units: CollectionUnitEntry[];
  totalUnits: number;
  ownedCount: number;
  missingCount: number;
  recommendedMode: 'missing_only' | 'entire';
}

export interface AcquireCollectionRequest {
  release: AcquisitionRelease;
  selectedVolumes: number[];
  mode: 'missing_only' | 'entire' | 'manual';
}

export interface AcquisitionConfig {
  nyaaEnabled: boolean;
  nyaaUrl?: string | null;
  nyaaCategory?: string | null;
  torboxEnabled: boolean;
  torboxUrl?: string | null;
  hasTorboxToken: boolean;
  torboxValidated?: boolean | null;
  torboxStatusMessage?: string | null;
}

export interface AcquisitionConfigPatch {
  nyaaEnabled?: boolean;
  nyaaUrl?: string;
  nyaaCategory?: string;
  torboxEnabled?: boolean;
  torboxUrl?: string;
  torboxToken?: string;
}

export interface AcquisitionHistoryRecord {
  id: number;
  chapterId?: number | null;
  mangaId: number;
  series: string;
  releaseName: string;
  sourceType: string;
  uploader?: string | null;
  externalJobId?: string | null;
  backendType: string;
  outcome: string;
  canonicalPath?: string | null;
  releaseSize?: number | null;
  completedAt: number;
  error?: string | null;
  metadataHandoff?: Queued["metadataHandoff"];
}

export const getTitleAcquisitionReleases = (
  id: number,
  optionsOrUnitType?:
    | string
    | {
        unitType?: string;
        unitNumber?: number;
        sort?: string;
        filterType?: string;
        filterOwnership?: string;
        filterUploader?: string;
        minSeeders?: number;
      },
  unitNumber?: number
) => {
  const params = new URLSearchParams();
  if (typeof optionsOrUnitType === 'string') {
    if (optionsOrUnitType) params.set('unit_type', optionsOrUnitType);
    if (unitNumber !== undefined) params.set('unit_number', String(unitNumber));
  } else if (optionsOrUnitType) {
    if (optionsOrUnitType.unitType) params.set('unit_type', optionsOrUnitType.unitType);
    if (optionsOrUnitType.unitNumber !== undefined) params.set('unit_number', String(optionsOrUnitType.unitNumber));
    if (optionsOrUnitType.sort) params.set('sort', optionsOrUnitType.sort);
    if (optionsOrUnitType.filterType) params.set('filter_type', optionsOrUnitType.filterType);
    if (optionsOrUnitType.filterOwnership) params.set('filter_ownership', optionsOrUnitType.filterOwnership);
    if (optionsOrUnitType.filterUploader) params.set('filter_uploader', optionsOrUnitType.filterUploader);
    if (optionsOrUnitType.minSeeders !== undefined) params.set('min_seeders', String(optionsOrUnitType.minSeeders));
  }
  const qs = params.toString();
  return get<AcquisitionCandidate[]>(`/titles/${id}/acquisition-releases${qs ? `?${qs}` : ''}`);
};

export const inspectCollection = (id: number, release: AcquisitionRelease) =>
  send<CollectionInspection>('POST', `/titles/${id}/acquisition-inspect-collection`, release);

export const acquireCollection = (id: number, request: AcquireCollectionRequest) =>
  send<{ success: boolean; queuedCount: number; chapterIds: number[] }>(
    'POST',
    `/titles/${id}/acquire-collection`,
    request
  );

export const acquireRelease = (id: number, release: AcquisitionRelease, targetUnit?: AcquisitionReleaseUnit) =>
  send<{ success: boolean; chapterId: number }>('POST', `/titles/${id}/acquire`, { release, targetUnit });

export const getAcquisitionSettings = () => get<AcquisitionConfig>('/settings/acquisition');

export const putAcquisitionSettings = (patch: AcquisitionConfigPatch) =>
  send<AcquisitionConfig>('PUT', '/settings/acquisition', patch);

export const testTorBoxConnection = (url?: string, token?: string) =>
  send<{ success: boolean; message: string }>('POST', '/settings/acquisition/torbox/test', { url, token });

export const getAcquisitionHistory = (mangaId?: number, limit?: number) => {
  const params = new URLSearchParams();
  if (mangaId !== undefined) params.set('manga_id', String(mangaId));
  if (limit !== undefined) params.set('limit', String(limit));
  const qs = params.toString();
  return get<AcquisitionHistoryRecord[]>(`/acquisition/history${qs ? `?${qs}` : ''}`);
};

// ---------- Acquisition Profiles, Intelligence & Missing Content Planner ----------

export interface AcquisitionProfile {
  id: string;
  name: string;
  isDefault: boolean;
  language: string;
  preferredReleaseType: string;
  sourceSelection: string;
  preferredSources: string[];
  preferredUploaders: string[];
  minSeeders: number;
  preferDigital: boolean;
  trustedOnly: boolean;
  collectionPolicy: string;
  maxUnnecessaryDuplicateCoverage: number;
  maxAcquisitionSizeBytes?: number | null;
  maxPlanSizeBytes?: number | null;
  qualityPreference: string;
  allowMixedCollections: boolean;
  allowZeroSeeders: string;
  allowUnsupportedEditions: boolean;
  autoAcquire: boolean;
  createdAt: number;
  updatedAt: number;
}

export interface UnitStatusEntry {
  unit: number;
  state: 'owned' | 'acquiring' | 'missing' | 'unavailable' | 'unknown';
  evidence: string;
  bookId?: string | null;
}

export interface MissingContentSummary {
  mangaId: number;
  title: string;
  totalKnownVolumes: number;
  ownedVolumes: number[];
  missingVolumes: number[];
  acquiringVolumes: number[];
  unavailableVolumes: number[];
  unknownVolumes: number[];
  volumeEntries: UnitStatusEntry[];
  totalKnownChapters: number;
  ownedChapters: number[];
  missingChapters: number[];
  acquiringChapters: number[];
  isFullyOwned: boolean;
  hasMissing: boolean;
  explanation: string;
}

export interface CandidateExplanation {
  candidateId: string;
  candidateTitle: string;
  provider: string;
  whyRecommended: string[];
  whyLower: string[];
}

export interface NormalizedAcquisitionCandidate {
  seriesIdentity: string;
  provider: string;
  providerReleaseId: string;
  title: string;
  releaseUnit: AcquisitionReleaseUnit;
  releaseNumber?: number | null;
  releaseRange?: [number, number] | null;
  language: string;
  edition?: string | null;
  format?: string | null;
  uploader?: string | null;
  size?: number | null;
  publishedAt?: number | null;
  availability: string;
  health: {
    seeders?: number | null;
    leechers?: number | null;
    sourceHealthy: boolean;
    uploaderReliabilityPercent?: number | null;
  };
  qualitySignals: {
    isDigital: boolean;
    isTrusted: boolean;
    isPreferredUploader: boolean;
    editionRecognized: boolean;
    editionPenalty: number;
  };
  ownedUnits: number[];
  missingUnits: number[];
  confidence: string;
  score: number;
  providerMetadata: any;
}

export interface AcquisitionPlanItem {
  candidate: NormalizedAcquisitionCandidate;
  unitsCovered: number[];
  duplicateUnits: number[];
  explanation: CandidateExplanation;
}

export interface AcquisitionPlan {
  id: string;
  mangaId: number;
  seriesTitle: string;
  profileId: string;
  profileName: string;
  missingUnits: number[];
  selectedItems: AcquisitionPlanItem[];
  alternatives: NormalizedAcquisitionCandidate[];
  confidence: string;
  state: string;
  staleReason?: string | null;
  totalEstimatedSize: number;
  totalAcquisitionsCount: number;
  missingUnitsCoveredCount: number;
  isDryRun: boolean;
  createdAt: number;
  refreshedAt: number;
}

export interface GlobalPlanSummary {
  totalSeriesAnalyzed: number;
  seriesPlanned: AcquisitionPlan[];
  totalVolumesToAcquire: number;
  totalCollectionsCount: number;
  totalIndividualCount: number;
  estimatedTotalBytes: number;
  unavailableVolumesCount: number;
  needsReviewCount: number;
  alreadySatisfiedCount: number;
}

export interface SourceHealthReport {
  sourceId: string;
  name: string;
  supportsVolumes: boolean;
  supportsChapters: boolean;
  supportsCollections: boolean;
  requiresExternalBackend: boolean;
  status: string;
  successCount: number;
  failureCount: number;
  lastResponseTimeMs?: number | null;
  lastError?: string | null;
  lastSuccessAt?: number | null;
  exactMatchCount: number;
  totalCandidatesFound: number;
  uploaderStats: Record<string, { validAcquisitions: number; attemptedAcquisitions: number; reliabilityPercent: number }>;
  updatedAt: number;
}

export const getAcquisitionProfiles = () => get<AcquisitionProfile[]>('/settings/profiles');
export const createAcquisitionProfile = (profile: Partial<AcquisitionProfile>) => send<AcquisitionProfile>('POST', '/settings/profiles', profile);
export const getAcquisitionProfile = (id: string) => get<AcquisitionProfile>(`/settings/profiles/${encodeURIComponent(id)}`);
export const updateAcquisitionProfile = (id: string, patch: Partial<AcquisitionProfile>) => send<AcquisitionProfile>('PUT', `/settings/profiles/${encodeURIComponent(id)}`, patch);
export const deleteAcquisitionProfile = (id: string) => send<boolean>('DELETE', `/settings/profiles/${encodeURIComponent(id)}`);
export const duplicateAcquisitionProfile = (id: string, name?: string) => send<AcquisitionProfile>('POST', `/settings/profiles/${encodeURIComponent(id)}/duplicate`, { name });

export const getTitleProfile = (id: number) => get<AcquisitionProfile>(`/titles/${id}/profile`);
export const setTitleProfile = (id: number, profileId: string | null) => send<AcquisitionProfile>('PUT', `/titles/${id}/profile`, { profileId });

export const getTitleMissingContent = (id: number) => get<MissingContentSummary>(`/titles/${id}/missing-content`);
export const planTitleMissingContent = (id: number, dryRun: boolean = true) => send<AcquisitionPlan>('POST', `/titles/${id}/plan`, { dryRun });
export const getTitlePlan = (id: number) => get<AcquisitionPlan | null>(`/titles/${id}/plan`);

export const getGlobalMissingContent = () => get<MissingContentSummary[]>('/acquisition/missing');
export const planAllMissingContent = () => send<GlobalPlanSummary>('POST', '/acquisition/plan-all', {});
export const getSourceHealthReports = () => get<SourceHealthReport[]>('/acquisition/sources/health');

export type ExecutionStatus =
  | 'APPROVED'
  | 'PREFLIGHT'
  | 'REVALIDATING'
  | 'QUEUED'
  | 'DOWNLOADING' | 'NEEDS_REVIEW' | 'IMPORTING'
  | 'VERIFYING'
  | 'PUBLISHING'
  | 'RECONCILING'
  | 'COMPLETED'
  | 'PARTIAL'
  | 'FAILED'
  | 'CANCELLED'
  | 'STALE';

export type ExecutionItemStatus =
  | 'PENDING'
  | 'QUEUED'
  | 'DOWNLOADING' | 'NEEDS_REVIEW' | 'IMPORTING'
  | 'VERIFYING'
  | 'PUBLISHING'
  | 'RECONCILING'
  | 'COMPLETED'
  | 'FAILED'
  | 'SKIPPED'
  | 'CANCELLED';

export interface AcquisitionExecutionItem {
  id: string;
  executionId: string;
  itemIndex: number;
  frozenCandidate: NormalizedAcquisitionCandidate;
  expectedUnits: number[];
  duplicateUnits: number[];
  status: ExecutionItemStatus;
  downloadJobId?: number | null;
  verificationState?: string | null;
  publicationState?: string | null;
  errorMessage?: string | null;
  completedAt?: number | null;
}

export interface AcquisitionExecution {
  id: string;
  planId: string;
  mangaId: number;
  title: string;
  profileId: string;
  profileSnapshot: AcquisitionProfile;
  planSnapshot: AcquisitionPlan;
  missingContentSnapshot: MissingContentSummary;
  items: AcquisitionExecutionItem[];
  status: ExecutionStatus;
  statusMessage?: string | null;
  expectedTotalBytes: number;
  cancellationReason?: string | null;
  failureReason?: string | null;
  approvedAt: number;
  startedAt?: number | null;
  completedAt?: number | null;
  updatedAt: number;
}

export interface ApprovePlanRequest {
  overrideConfirmed?: boolean;
  planId: string;
  approved: boolean;
}

export const createExecution = (id: number, req: ApprovePlanRequest) => send<AcquisitionExecution>('POST', `/titles/${id}/executions`, req);
export const getTitleExecutions = (id: number) => get<AcquisitionExecution[]>(`/titles/${id}/executions`);
export const getAcquisitionExecution = (id: string) => get<AcquisitionExecution>(`/acquisition/executions/${encodeURIComponent(id)}`);
export const startExecution = (id: string) => send<AcquisitionExecution>('POST', `/acquisition/executions/${encodeURIComponent(id)}/start`, {});
export const cancelExecution = (id: string, reason?: string) => send<AcquisitionExecution>('POST', `/acquisition/executions/${encodeURIComponent(id)}/cancel`, { reason });
export const retryExecution = (id: string) => send<AcquisitionExecution>('POST', `/acquisition/executions/${encodeURIComponent(id)}/retry`, {});

// ---------- Sources & Planning by Title ----------

export interface AcquisitionSourceOption {
  id: string;
  name: string;
  description: string;
  isDefault: boolean;
  available: boolean;
  supportsVolumes: boolean;
  supportsChapters: boolean;
}

export interface PlanByTitleRequest {
  title: string;
  komgaSeriesId?: string | null;
  sourceId?: string | null;
  contentMode?: string | null;
  requestedVolumes?: number[] | null;
  totalVolumes?: number | null;
  requestedEdition?: string | null;
  selectedReleaseId?: string | null;
  selectedArchive?: { path: string; size: number } | null;
  inspectPayload?: boolean;
  selectedArchives?: {path:string;size:number}[];
  useReleaseAnyway?: boolean;
}

export const getAcquisitionSources = (signal?: AbortSignal) => get<AcquisitionSourceOption[]>('/acquisition/sources', signal);
export interface SourceLocalUnit { unit: AcquisitionReleaseUnit; edition: string | null; archive: { path: string; size: number }; eligible: boolean; evidence: string; }
export interface InteractivePlan extends AcquisitionPlan { editions: string[]; candidates: NormalizedAcquisitionCandidate[]; diagnostics: string[]; sourceUnits: SourceLocalUnit[]; editionOwnershipKnown: boolean; }
export const planByTitle = (req: PlanByTitleRequest, signal?: AbortSignal) => send<InteractivePlan>('POST', '/acquisition/plan-by-title', req, signal);

// ---------- Library Normalization & Maintenance ----------

export type NormalizationStatus =
  | 'MATCHED'
  | 'ALREADY_NORMALIZED'
  | 'NEEDS_UPDATE'
  | 'AMBIGUOUS'
  | 'UNMATCHED'
  | 'SKIPPED_PROVIDER_NOT_APPLICABLE'
  | 'INVALID_ARCHIVE'
  | 'ERROR';

export interface BookMetadataDiff {
  field: string;
  current?: string | null;
  proposed?: string | null;
}

export interface BookNormalizationItem {
  komgaBookId: string;
  komgaSeriesId: string;
  seriesTitle: string;
  bookName: string;
  canonicalPath: string;
  volume?: number | null;
  status: NormalizationStatus;
  reason: string;
  mangabakaId?: number | null;
  mangabakaReleaseId?: string | null;
  anilistId?: number | null;
  diffs: BookMetadataDiff[];
  currentSha256?: string | null;
}

export interface SeriesNormalizationItem {
  komgaSeriesId: string;
  libraryId: string;
  seriesTitle: string;
  status: NormalizationStatus;
  reason: string;
  mangabakaId?: number | null;
  anilistId?: number | null;
  totalBooks: number;
  needsUpdateCount: number;
  alreadyNormalizedCount: number;
  books: BookNormalizationItem[];
}

export interface LibraryAnalysisSummary {
  previewId: string;
  libraryId: string;
  libraryName: string;
  libraryRoot: string;
  totalSeries: number;
  totalBooks: number;
  matchedCount: number;
  alreadyNormalizedCount: number;
  needsUpdateCount: number;
  ambiguousCount: number;
  unmatchedCount: number;
  skippedCount: number;
  errorCount: number;
  series: SeriesNormalizationItem[];
}

export interface NormalizationHistoryRecord {
  id: string;
  batchId: string;
  canonicalPath: string;
  seriesTitle: string;
  volume: number;
  komgaSeriesId?: string | null;
  komgaBookId?: string | null;
  mangabakaId?: number | null;
  mangabakaReleaseId?: string | null;
  anilistId?: number | null;
  originalSha256?: string | null;
  normalizedSha256?: string | null;
  status: string;
  errorMessage?: string | null;
  normalizedAt: number;
  revertedAt?: number | null;
  refreshPending: boolean;
  rollbackAvailable: boolean;
}

export interface NormalizationExecutionResult {
  batchId: string;
  totalProcessed: number;
  normalizedCount: number;
  alreadyNormalizedCount: number;
  failedCount: number;
  skippedCount: number;
  items: NormalizationHistoryRecord[];
}

export interface NormalizeItemRequest {
  komgaBookId: string;
}

export const runLibraryDryRun = (libraryId: string, seriesId?: string) =>
  send<LibraryAnalysisSummary>('POST', '/pipeline/maintenance/dry-run', { libraryId, seriesId: seriesId ?? null });

export const executeBatchNormalize = (previewId: string, items: NormalizeItemRequest[], dryRun: boolean = true) =>
  send<NormalizationExecutionResult>('POST', '/pipeline/maintenance/normalize', {
    previewId, bookIds: items.map(item => item.komgaBookId), dryRun, approved: !dryRun,
  });

export const retryNormalizationRefresh = (id: string) =>
  send<NormalizationHistoryRecord>('POST', `/pipeline/maintenance/refresh/${encodeURIComponent(id)}`, {});

export const getNormalizationHistory = (limit: number = 50) =>
  get<NormalizationHistoryRecord[]>(`/pipeline/maintenance/history?limit=${limit}`);

export const rollbackNormalizationRecord = (id: string) =>
  send<NormalizationHistoryRecord>('POST', `/pipeline/maintenance/rollback/${encodeURIComponent(id)}`, {});

export type ReconciliationStatus = 'up_to_date' | 'missing_volume' | 'review_required' | 'sync_error';

export interface ReconciliationSummary {
  seriesId: string;
  mangaId: number | null;
  komgaSeriesId: string | null;
  mangabakaId: number | null;
  collectionId: string | null;
  title: string;
  edition: string;
  status: ReconciliationStatus;
  isOngoing: boolean;
  totalOfficialVolumes: number | null;
  ownedVolumesCount: number;
  missingVolumesCount: number;
  ownedVolumes: number[];
  missingVolumes: number[];
  futureVolumes: number[];
  lastReconciledAt: number;
  lastError: string | null;
  reviewReason: string | null;
  displayStatus: string;
}

export const getMangaFollow = (id: number) =>
  get<{ followed: boolean }>(`/manga/${id}/follow`);

export const followManga = (id: number) =>
  send<{ followed: boolean }>('POST', `/manga/${id}/follow`, {});

export const unfollowManga = (id: number) =>
  send<{ followed: boolean }>('POST', `/manga/${id}/unfollow`, {});

export const getMangaReconciliation = (id: number) =>
  get<ReconciliationSummary | null>(`/manga/${id}/reconciliation`);

export const reconcileManga = (id: number) =>
  send<ReconciliationSummary>('POST', `/manga/${id}/reconcile`, {});

export const getFollowedReconciliations = () =>
  get<ReconciliationSummary[]>('/reconciliation/followed');

export interface DiscoveredCandidate {
  candidateId: string;
  provider: string;
  providerItemId: string;
  title: string;
  rawTitle: string;
  matchType: 'exact_volume' | 'volume_range_containing_target' | 'series_pack' | 'possible_match' | 'rejected';
  targetVolume?: number;
  sizeBytes?: number;
  uploader?: string;
  isTrusted: boolean;
  isDigital: boolean;
  score: number;
  scoreExplanation: string;
  downloadUrl?: string;
  magnetUrl?: string;
  infoHash?: string;
  seeders?: number;
  leechers?: number;
  publishedAt?: number;
}

export interface VolumeDiscoveryResult {
  seriesId: string;
  volumeNumber: number;
  status: string;
  candidateCount: number;
  topCandidate?: DiscoveredCandidate;
  candidates: DiscoveredCandidate[];
  providersChecked: string[];
  providerErrors: Record<string, string>;
  lastDiscoveredAt: number;
}

export interface SeriesDiscoverySummary {
  seriesId: string;
  missingVolumesCount: number;
  discoveredVolumesCount: number;
  results: VolumeDiscoveryResult[];
}

export const getVolumeCandidates = (id: number, volume: number) =>
  get<VolumeDiscoveryResult | null>(`/manga/${id}/volumes/${volume}/candidates`);

export const discoverVolumeCandidates = (id: number, volume: number) =>
  send<VolumeDiscoveryResult>('POST', `/manga/${id}/volumes/${volume}/discover`, {});

export const discoverAllMissingCandidates = (id: number) =>
  send<SeriesDiscoverySummary>('POST', `/manga/${id}/discover-missing`, {});

export const getSeriesDiscovery = (id: number) =>
  get<VolumeDiscoveryResult[]>(`/manga/${id}/discovery`);

export interface ApproveCandidateRequest {
  confirmRange?: boolean;
  confirmPack?: boolean;
  confirmReview?: boolean;
}

export interface ApproveCandidateResponse {
  status: string;
  message: string;
  executionId?: string;
  chapterIds: number[];
  alreadyOwned: boolean;
  alreadyAcquiring: boolean;
}

export const acquireMangaVolumeCandidate = (
  id: number,
  volume: number,
  candidateId: string,
  req?: ApproveCandidateRequest
) =>
  send<ApproveCandidateResponse>(
    'POST',
    `/manga/${id}/volumes/${volume}/candidates/${encodeURIComponent(candidateId)}/acquire`,
    req ?? {}
  );

export interface NtfyConfig {
  enabled: boolean;
  serverUrl: string;
  topic: string;
  hasToken: boolean;
  token?: string;
  prefNewVolume: boolean;
  prefReleaseAvailable: boolean;
  prefAcquisitionComplete: boolean;
  prefActionRequired: boolean;
  activatedAt?: number;
}

export interface OutboxEntry {
  id: number;
  eventId: number;
  eventType: string;
  transport: string;
  destination: string;
  payloadJson: string;
  status: 'pending' | 'sending' | 'delivered' | 'retry_wait' | 'permanently_failed' | 'suppressed';
  attempts: number;
  nextAttemptAt: number;
  lastAttemptAt?: number;
  deliveredAt?: number;
  lastError?: string;
  createdAt: number;
}

export interface TestNotificationResponse {
  success: boolean;
  message?: string;
  error?: string;
}

export const getNotificationConfig = () =>
  get<NtfyConfig>('/notifications/config');

export const saveNotificationConfig = (config: Partial<NtfyConfig>) =>
  send<NtfyConfig>('POST', '/notifications/config', config);

export const sendTestNotification = (params?: { serverUrl?: string; topic?: string; token?: string }) =>
  send<TestNotificationResponse>('POST', '/notifications/test', params ?? {});

export const getNotificationOutbox = () =>
  get<OutboxEntry[]>('/notifications/outbox');

export interface OpenTomeStatus {
  enabled: boolean;
  available: boolean;
  version?: string;
  fileSize?: number;
  lastUpdatedAt?: number;
  lastCheckedAt?: number;
  lastError?: string;
  cataloguePath: string;
}

export interface OpenTomeVolumeRelease {
  volumeNumber: number;
  title?: string;
  releaseDate?: string;
  releaseDateRaw?: string;
  precision: 'day' | 'month' | 'year' | 'unknown' | string;
  dateType: 'published' | 'projected' | 'unknown' | string;
  status: 'released' | 'releases_today' | 'future' | 'unknown' | string;
}

export interface OpenTomeMarketSchedule {
  market: 'ja' | 'en' | string;
  lineName: string;
  publisher?: string;
  tomeId: string;
  gcdSeriesId: number;
  latestVolume?: OpenTomeVolumeRelease;
  nextVolume?: OpenTomeVolumeRelease;
  totalVolumes: number;
}

export interface OpenTomeSeriesSchedule {
  workId: string;
  mappingConfidence: 'exact' | 'high' | 'ambiguous' | 'user_confirmed' | 'unmatched' | 'rejected' | string;
  japanese?: OpenTomeMarketSchedule;
  english?: OpenTomeMarketSchedule;
  lastEvaluatedAt: number;
}

export const getOpenTomeStatus = () =>
  get<OpenTomeStatus>('/opentome/status');

export const updateOpenTomeCatalogue = (force = false) =>
  send<{ updated: boolean }>('POST', `/opentome/update?force=${force}`);

export const setOpenTomeConfig = (params: { enabled?: boolean; manifestUrl?: string }) =>
  send<{ success: boolean }>('POST', '/opentome/config', params);

export const getMangaOpenTome = (id: number) =>
  get<OpenTomeSeriesSchedule | null>(`/manga/${id}/opentome`);

export const remapMangaOpenTome = (
  id: number,
  workId?: string | null,
  action: 'confirm' | 'reject' | 'auto' = 'confirm'
) =>
  send<{ success: boolean }>('POST', `/manga/${id}/opentome/remap`, { workId: workId ?? null, action });

export interface ReleaseDiscoveryTarget {
  id: number;
  seriesId: string;
  mangaId?: number | null;
  seriesTitle: string;
  volumeNumber: number;
  market: string;
  edition: string;
  releaseDate: string;
  releaseDatePrecision: string;
  dateType: string;
  status:
    | 'scheduled'
    | 'releases_today'
    | 'discovering'
    | 'candidate_available'
    | 'not_found_retry_wait'
    | 'already_owned'
    | 'mapping_review_required'
    | 'dormant'
    | string;
  attempts: number;
  maxAttempts: number;
  nextAttemptAt: number;
  lastAttemptAt?: number | null;
  candidatesCount: number;
  topCandidateId?: string | null;
  topCandidateTitle?: string | null;
  lastError?: string | null;
  createdAt: number;
  updatedAt: number;
}

export const getMangaReleaseDiscovery = (id: number) =>
  get<ReleaseDiscoveryTarget[]>(`/manga/${id}/release-discovery`);

export const checkMangaReleaseDiscovery = (id: number) =>
  send<ReleaseDiscoveryTarget[]>('POST', `/manga/${id}/release-discovery/check`, {});

export const retryReleaseDiscoveryTarget = (id: number, targetId: number) =>
  send<ReleaseDiscoveryTarget | null>(
    'POST',
    `/manga/${id}/release-discovery/targets/${targetId}/retry`,
    {}
  );





