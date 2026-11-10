import type { ImportBook } from './importReview';
import type {
  PipelineEvent,
  Queued,
  VolumeOwnership,
  CanonicalLibraryReleasesSummary,
  ReconciliationSummary,
  VolumeManagerEntry,
  ReviewQueueItem,
} from './api';

export type NormalizedStatus =
  | 'owned'
  | 'missing'
  | 'queued'
  | 'acquiring'
  | 'processing'
  | 'waiting_for_komga'
  | 'metadata'
  | 'complete'
  | 'attention';

export type PhysicalState =
  | 'in_library'
  | 'downloading'
  | 'retrieving'
  | 'processing'
  | 'queued'
  | 'staged'
  | 'failed'
  | 'missing'
  | 'unknown';

export type MetadataState =
  | 'complete'
  | 'updating'
  | 'waiting_for_komga'
  | 'failed'
  | 'pending'
  | 'not_applicable';

export interface VolumePresentation {
  volume: number;
  label: string;
  status: NormalizedStatus;
  statusLabel: string;
  statusDetail?: string | null;
  badgeVariant: 'complete' | 'active' | 'waiting' | 'attention' | 'neutral';

  physicalState: PhysicalState;
  physicalLabel: string;

  metadataState: MetadataState;
  metadataLabel: string;

  progressPercent: number | null;
  bytesDownloaded?: number | null;
  totalBytes?: number | null;

  contradictionWarning?: string | null;

  canRetryMetadata: boolean;
  canRetryKomga: boolean;
  canRetryDownload: boolean;
  canReview: boolean;
  canAcquire: boolean;

  bookId?: string | null;
  bookName?: string | null;
  publishedPath?: string | null;
  sourceType?: string | null;
  releaseName?: string | null;
  uploader?: string | null;
  chapterId?: number | null;
  payloadChapter?: number | null;
  jobId?: string | null;
  error?: string | null;
  lastUpdated?: number | null;
}

export interface VolumeDataInputs {
  volume: number;
  isOwnedInKomga?: boolean;
  bookId?: string | null;
  bookName?: string | null;
  ownershipError?: string | null;
  queuedItems?: Queued[];
  importCandidates?: ImportBook[];
  pipelineEvents?: PipelineEvent[];
  isMissingReported?: boolean;
}

export interface SeriesSummary {
  title: string;
  ownedCount: number;
  totalExpectedVolumes: number | null;
  missingCount: number;
  activeCount: number;
  attentionCount: number;
  progressPercent: number | null;
  isOngoing: boolean;
  isCompleted?: boolean;
  libraryStatus?: 'up_to_date' | 'library_complete' | 'missing_released' | 'identity_review_required' | string;
  statusExplanation: string;
  isIdentityAmbiguous: boolean;
  primaryActionType: 'resolve_identity' | 'review_release' | 'retry_metadata' | 'discover' | 'none';
  primaryActionLabel: string;
  libraryState?: CanonicalLibraryReleasesSummary | null;
}

export interface SeriesSummaryInputs {
  title: string;
  volumes: VolumePresentation[];
  totalExpectedVolumes?: number | null;
  isOngoing?: boolean;
  isCompleted?: boolean;
  isIdentityAmbiguous?: boolean;
  reconciliationStatus?: string | null;
  hasAvailableRelease?: boolean;
  libraryState?: CanonicalLibraryReleasesSummary | null;
  reconciliation?: ReconciliationSummary | null;
}

export interface StagingSummary {
  totalPayloads: number;
  completedPayloadChapters: number[];
  pendingPayloadChapters: number[];
  canClean: boolean;
}

/**
 * Derives the normalized presentation for a single logical volume by applying
 * strict state precedence over all available backend records.
 */
export function resolveVolumePresentation(input: VolumeDataInputs): VolumePresentation {
  const {
    volume,
    isOwnedInKomga = false,
    bookId = null,
    bookName = null,
    ownershipError = null,
    queuedItems = [],
    importCandidates = [],
    pipelineEvents = [],
    isMissingReported = false,
  } = input;

  const label = `Volume ${volume}`;

  // 1. Determine if canonical media is durably established
  const publishedEvent = pipelineEvents.find(
    (e) => e.outcome === 'published' || e.outcome === 'already_published'
  );
  const publishedQueueItem = queuedItems.find(
    (q) => q.publishedPath != null && q.publishedPath.length > 0
  );
  const completedCandidate = importCandidates.find(
    (c) => c.state === 'completed' || c.state === 'published'
  );

  const isPhysicallyOwned =
    isOwnedInKomga ||
    publishedEvent != null ||
    publishedQueueItem != null ||
    completedCandidate != null;

  // 2. Physical Ownership Precedence Branch
  if (isPhysicallyOwned) {
    const canonicalPath =
      publishedQueueItem?.publishedPath ?? publishedEvent?.destination ?? null;
    const resolvedBookName = bookName ?? (canonicalPath ? canonicalPath.split('/').pop() : null);

    // Inspect metadata handoff or verification states
    const activeQueueWithHandoff = queuedItems.find((q) => q.metadataHandoff != null);
    const handoff = activeQueueWithHandoff?.metadataHandoff;
    const hasMetadataError =
      handoff?.state === 'failed' ||
      handoff?.state === 'needs_review' ||
      handoff?.state === 'rejected' ||
      handoff?.state === 'unavailable' ||
      queuedItems.some(
        (q) => q.state === 'metadata_failed' || q.state === 'metadata_review'
      ) ||
      (publishedEvent?.komga_status != null &&
        publishedEvent.komga_status.toLowerCase().includes('error')) ||
      (publishedEvent?.mangabaka_status != null &&
        publishedEvent.mangabaka_status.toLowerCase().includes('error')) ||
      ownershipError != null;

    const isWaitingForKomga =
      queuedItems.some(
        (q) => q.state === 'verifying' || q.stage === 'verifying' || q.canRetryKomga
      ) || handoff?.state === 'verifying';

    const isMetadataUpdating =
      handoff?.state === 'submitting' ||
      handoff?.state === 'pending' ||
      handoff?.state === 'in_progress' ||
      queuedItems.some((q) => q.state === 'metadata' || q.state === 'metadata_wait');

    let status: NormalizedStatus;
    let statusLabel: string;
    let badgeVariant: VolumePresentation['badgeVariant'];
    let metadataState: MetadataState;
    let metadataLabel: string;
    let canRetryMetadata = false;
    let canRetryKomga = false;

    if (hasMetadataError) {
      status = 'attention';
      statusLabel = 'Media acquired · Metadata needs attention';
      badgeVariant = 'attention';
      metadataState = 'failed';
      metadataLabel =
        handoff?.message ??
        publishedEvent?.error ??
        ownershipError ??
        'Metadata enrichment failed';
      canRetryMetadata = true;
      canRetryKomga = queuedItems.some((q) => q.canRetryKomga);
    } else if (isWaitingForKomga) {
      status = 'waiting_for_komga';
      statusLabel = 'Waiting for Komga';
      badgeVariant = 'active';
      metadataState = 'waiting_for_komga';
      metadataLabel = 'Published · Waiting for Komga scan resolution';
      canRetryKomga = queuedItems.some((q) => q.canRetryKomga);
    } else if (isMetadataUpdating) {
      status = 'metadata';
      statusLabel = 'Updating metadata';
      badgeVariant = 'active';
      metadataState = 'updating';
      metadataLabel = handoff?.message ?? 'Enriching MangaBaka / KOMF metadata';
    } else {
      status = 'complete';
      statusLabel = 'Complete';
      badgeVariant = 'complete';
      metadataState = 'complete';
      metadataLabel = 'Metadata workflow successfully completed';
    }

    // Check for contradiction: is there an active physical acquisition job running for an already-owned volume?
    // Exclude completed or stale payload jobs.
    const activeReacquisition = queuedItems.find((q) => {
      const s = q.stage ?? q.state;
      return (
        s === 'downloading' ||
        s === 'torbox_downloading' ||
        s === 'torbox_retrieving' ||
        (s === 'queued' && q.publishedPath == null && q.state !== 'completed')
      );
    });

    let contradictionWarning: string | null = null;
    if (activeReacquisition) {
      contradictionWarning = `Volume is already owned in library, but another acquisition job (${activeReacquisition.rawTitle ?? activeReacquisition.chapterName}) is in queue/progress.`;
    }

    return {
      volume,
      label,
      status,
      statusLabel,
      badgeVariant,
      physicalState: 'in_library',
      physicalLabel: 'In canonical library',
      metadataState,
      metadataLabel,
      progressPercent: null,
      contradictionWarning,
      canRetryMetadata,
      canRetryKomga,
      canRetryDownload: false,
      canReview: false,
      canAcquire: false,
      bookId,
      bookName: resolvedBookName,
      publishedPath: canonicalPath,
      sourceType: publishedQueueItem?.sourceType ?? publishedEvent?.destination ?? null,
      releaseName: publishedEvent?.release_name ?? publishedQueueItem?.rawTitle ?? null,
      uploader: publishedQueueItem?.uploader ?? null,
      chapterId: activeQueueWithHandoff?.chapterId ?? publishedQueueItem?.chapterId ?? null,
      jobId: handoff?.jobId ?? null,
      error: hasMetadataError ? metadataLabel : null,
      lastUpdated: publishedEvent?.timestamp ?? null,
    };
  }

  // 3. Unowned Branch: Inspection, Processing, Acquiring, Queued, or Missing
  const reviewCandidate = importCandidates.find((c) => c.state === 'needs_review');
  if (reviewCandidate) {
    return {
      volume,
      label,
      status: 'attention',
      statusLabel: 'Needs review',
      badgeVariant: 'attention',
      physicalState: 'staged',
      physicalLabel: 'Payload inspected · Book assignment awaits review',
      metadataState: 'pending',
      metadataLabel: 'Metadata enrichment deferred until assignment approval',
      progressPercent: null,
      canRetryMetadata: false,
      canRetryKomga: false,
      canRetryDownload: false,
      canReview: true,
      canAcquire: false,
      chapterId: reviewCandidate.childChapter ?? reviewCandidate.id,
      payloadChapter: reviewCandidate.payloadChapter,
      releaseName: reviewCandidate.innerPath,
    };
  }

  const processingCandidate = importCandidates.find((c) => c.state === 'processing');
  const processingQueueItem = queuedItems.find((q) => {
    const s = q.stage ?? q.state;
    return (
      s === 'optimizing' ||
      s === 'validating' ||
      s === 'publishing' ||
      s === 'identifying' ||
      s === 'processing'
    );
  });

  if (processingCandidate || processingQueueItem) {
    const stageDesc =
      processingQueueItem?.statusMessage ??
      (processingQueueItem?.stage === 'optimizing'
        ? 'Optimizing image compression'
        : processingQueueItem?.stage === 'validating'
          ? 'Validating archive integrity'
          : processingQueueItem?.stage === 'publishing'
            ? 'Publishing to canonical library'
            : 'Processing archive');

    return {
      volume,
      label,
      status: 'processing',
      statusLabel: 'Processing',
      badgeVariant: 'active',
      physicalState: 'processing',
      physicalLabel: stageDesc,
      metadataState: 'pending',
      metadataLabel: 'Waiting for physical publishing',
      progressPercent: null,
      canRetryMetadata: false,
      canRetryKomga: false,
      canRetryDownload: false,
      canReview: false,
      canAcquire: false,
      chapterId: processingQueueItem?.chapterId ?? processingCandidate?.childChapter ?? null,
      releaseName: processingQueueItem?.rawTitle ?? processingCandidate?.innerPath ?? null,
    };
  }

  const activeDownload = queuedItems.find((q) => {
    const s = q.stage ?? q.state;
    return (
      s === 'downloading' ||
      s === 'torbox_downloading' ||
      s === 'torbox_retrieving' ||
      s === 'torbox_submitting' ||
      s === 'torbox_checking' ||
      s === 'torbox_selecting'
    );
  });

  if (activeDownload) {
    let pct: number | null = null;
    if (activeDownload.bytesDownloaded && activeDownload.totalBytes) {
      pct = Math.min(100, Math.round((activeDownload.bytesDownloaded / activeDownload.totalBytes) * 100));
    } else if (activeDownload.done && activeDownload.total) {
      pct = Math.round((activeDownload.done / activeDownload.total) * 100);
    }

    const downloadDesc =
      activeDownload.statusMessage ??
      (pct != null ? `Downloading · ${pct}%` : 'Downloading from TorBox');

    return {
      volume,
      label,
      status: 'acquiring',
      statusLabel: pct != null ? `Downloading · ${pct}%` : 'Acquiring',
      badgeVariant: 'active',
      physicalState: 'downloading',
      physicalLabel: downloadDesc,
      metadataState: 'pending',
      metadataLabel: 'Waiting for download completion',
      progressPercent: pct,
      bytesDownloaded: activeDownload.bytesDownloaded,
      totalBytes: activeDownload.totalBytes,
      canRetryMetadata: false,
      canRetryKomga: false,
      canRetryDownload: false,
      canReview: false,
      canAcquire: false,
      chapterId: activeDownload.chapterId,
      sourceType: activeDownload.sourceType ?? activeDownload.backendType,
      releaseName: activeDownload.rawTitle ?? activeDownload.chapterName,
      uploader: activeDownload.uploader,
    };
  }

  const queuedDownload = queuedItems.find(
    (q) => (q.stage === 'queued' || q.state === 'queued') && q.state !== 'completed'
  );

  if (queuedDownload) {
    return {
      volume,
      label,
      status: 'queued',
      statusLabel: 'Queued',
      badgeVariant: 'waiting',
      physicalState: 'queued',
      physicalLabel: queuedDownload.statusMessage ?? 'Waiting in queue',
      metadataState: 'pending',
      metadataLabel: 'Waiting for acquisition',
      progressPercent: null,
      canRetryMetadata: false,
      canRetryKomga: false,
      canRetryDownload: false,
      canReview: false,
      canAcquire: false,
      chapterId: queuedDownload.chapterId,
      sourceType: queuedDownload.sourceType,
      releaseName: queuedDownload.rawTitle ?? queuedDownload.chapterName,
      uploader: queuedDownload.uploader,
    };
  }

  const failedDownload = queuedItems.find((q) => q.state === 'failed' || q.stage === 'failed');
  if (failedDownload) {
    return {
      volume,
      label,
      status: 'attention',
      statusLabel: 'Acquisition failed',
      badgeVariant: 'attention',
      physicalState: 'failed',
      physicalLabel: failedDownload.error ?? 'Acquisition stopped with error',
      metadataState: 'pending',
      metadataLabel: 'Acquisition unfulfilled',
      progressPercent: null,
      canRetryMetadata: false,
      canRetryKomga: false,
      canRetryDownload: true,
      canReview: false,
      canAcquire: true,
      chapterId: failedDownload.chapterId,
      error: failedDownload.error,
      releaseName: failedDownload.rawTitle ?? failedDownload.chapterName,
    };
  }

  // 4. Default: Missing
  return {
    volume,
    label,
    status: 'missing',
    statusLabel: 'Missing',
    badgeVariant: 'neutral',
    physicalState: 'missing',
    physicalLabel: 'Not owned in library',
    metadataState: 'pending',
    metadataLabel: 'No metadata until acquired',
    progressPercent: null,
    canRetryMetadata: false,
    canRetryKomga: false,
    canRetryDownload: false,
    canReview: false,
    canAcquire: true,
  };
}

/**
 * Calculates the overall series summary metrics from volume presentations.
 */
export function resolveSeriesSummary(inputs: SeriesSummaryInputs): SeriesSummary {
  const {
    title,
    volumes,
    totalExpectedVolumes = null,
    isOngoing = false,
    isCompleted = false,
    isIdentityAmbiguous = false,
    reconciliationStatus = null,
    hasAvailableRelease = false,
    libraryState = null,
    reconciliation = null,
  } = inputs;

  // 1. If libraryState is provided, it is the canonical backend authority
  if (libraryState) {
    if (libraryState.isIdentityAmbiguous) {
      return {
        title: libraryState.title || title,
        ownedCount: libraryState.ownedVolumesCount,
        totalExpectedVolumes: null,
        missingCount: 0,
        activeCount: 0,
        attentionCount: 0,
        progressPercent: null,
        isOngoing: libraryState.isOngoing,
        isCompleted: libraryState.isCompleted,
        libraryStatus: 'identity_review_required',
        statusExplanation: 'Series identity needs review',
        isIdentityAmbiguous: true,
        primaryActionType: 'resolve_identity',
        primaryActionLabel: 'Resolve identity',
        libraryState,
      };
    }

    const ownedCount = libraryState.ownedVolumesCount;
    const releasedCount = libraryState.releasedVolumesCount;
    const missingCount = libraryState.missingReleasedCount;
    const progressPercent = libraryState.progressPercent ?? null;
    const isOngoingResolved = libraryState.isOngoing;
    const isCompletedResolved = libraryState.isCompleted;

    let statusExplanation = '';
    if (isCompletedResolved || libraryState.libraryStatus === 'library_complete') {
      statusExplanation = `${ownedCount} / ${releasedCount} released volumes owned · Complete`;
    } else if (libraryState.libraryStatus === 'up_to_date') {
      statusExplanation = `${ownedCount} / ${releasedCount} released volumes owned · Up to date`;
    } else if (missingCount > 0) {
      statusExplanation = `${ownedCount} / ${releasedCount} released volumes owned · ${missingCount} missing`;
    } else {
      statusExplanation = `${ownedCount} / ${releasedCount} released volumes owned`;
    }

    const actionType = (libraryState.primaryActionType ?? 'none') as SeriesSummary['primaryActionType'];
    const actionLabel = libraryState.primaryActionLabel || '';

    return {
      title: libraryState.title || title,
      ownedCount,
      totalExpectedVolumes: releasedCount,
      missingCount,
      activeCount: 0,
      attentionCount: 0,
      progressPercent,
      isOngoing: isOngoingResolved,
      isCompleted: isCompletedResolved,
      libraryStatus: libraryState.libraryStatus,
      statusExplanation,
      isIdentityAmbiguous: false,
      primaryActionType: actionType,
      primaryActionLabel: actionLabel,
      libraryState,
    };
  }

  // 2. Standalone / Fallback calculation (from volumes and reconciliation)
  const isAmbiguous =
    isIdentityAmbiguous ||
    reconciliationStatus === 'review_required' ||
    reconciliationStatus === 'sync_error' ||
    reconciliationStatus === 'unmatched';

  let ownedCount = 0;
  let missingCount = 0;
  let activeCount = 0;
  let attentionCount = 0;

  for (const v of volumes) {
    if (v.status === 'complete' || v.status === 'owned') {
      ownedCount++;
    } else if (v.status === 'missing') {
      missingCount++;
    } else if (
      v.status === 'acquiring' ||
      v.status === 'processing' ||
      v.status === 'waiting_for_komga' ||
      v.status === 'metadata' ||
      v.status === 'queued'
    ) {
      activeCount++;
    } else if (v.status === 'attention') {
      attentionCount++;
      // If it's already physically owned but metadata failed, it also counts as physically owned
      if (v.physicalState === 'in_library') {
        ownedCount++;
      }
    }
  }

  // If volumes is empty (e.g. flat chapters view where volumeGroups has presentation: null),
  // fallback to reconciliation so we never show false 0 owned / 8 missing!
  if (volumes.length === 0 && reconciliation) {
    ownedCount = reconciliation.ownedVolumesCount;
    missingCount = reconciliation.missingVolumesCount;
  }

  // If identity is ambiguous, do not present misleading 0/14 counts or 14 missing
  if (isAmbiguous) {
    return {
      title,
      ownedCount,
      totalExpectedVolumes: null,
      missingCount: 0,
      activeCount,
      attentionCount,
      progressPercent: null,
      isOngoing,
      isCompleted,
      libraryStatus: 'identity_review_required',
      statusExplanation: 'Series identity needs review',
      isIdentityAmbiguous: true,
      primaryActionType: 'resolve_identity',
      primaryActionLabel: 'Resolve identity',
    };
  }

  if (reconciliationStatus === 'collection_unavailable') {
    return {
      title,
      ownedCount,
      totalExpectedVolumes: null,
      missingCount: 0,
      activeCount,
      attentionCount,
      progressPercent: null,
      isOngoing,
      isCompleted,
      libraryStatus: 'collection_unavailable',
      statusExplanation: `${ownedCount} volume${ownedCount === 1 ? '' : 's'} owned · MangaBaka edition data unavailable`,
      isIdentityAmbiguous: false,
      primaryActionType: 'none',
      primaryActionLabel: '',
    };
  }

  const effectiveTotal =
    totalExpectedVolumes != null && totalExpectedVolumes > 0
      ? totalExpectedVolumes
      : reconciliation?.totalOfficialVolumes != null
        ? reconciliation.totalOfficialVolumes
        : Math.max(volumes.length, ownedCount + missingCount);

  if (volumes.length > 0 && effectiveTotal != null && effectiveTotal > ownedCount + activeCount + attentionCount) {
    missingCount = effectiveTotal - (ownedCount + activeCount + attentionCount);
  }

  const progressPercent = effectiveTotal > 0 ? Math.min(100, Math.round((ownedCount / effectiveTotal) * 100)) : null;

  const isCompletedResolved = isCompleted || (!isOngoing && ownedCount >= effectiveTotal && effectiveTotal > 0);
  const isUpToDateResolved = isOngoing && missingCount === 0 && ownedCount > 0;

  let libraryStatus = 'missing_released';
  let statusExplanation = '';
  if (isCompletedResolved) {
    libraryStatus = 'library_complete';
    statusExplanation = `${ownedCount} / ${effectiveTotal} released volumes owned · Complete`;
  } else if (isUpToDateResolved) {
    libraryStatus = 'up_to_date';
    statusExplanation = `${ownedCount} / ${effectiveTotal} released volumes owned · Up to date`;
  } else if (missingCount > 0) {
    libraryStatus = 'missing_released';
    statusExplanation = `${ownedCount} / ${effectiveTotal} released volumes owned · ${missingCount} missing`;
  } else {
    statusExplanation = `${ownedCount} of ${effectiveTotal} volumes owned`;
  }

  let primaryActionType: SeriesSummary['primaryActionType'] = 'none';
  let primaryActionLabel = '';

  if (volumes.some((v) => v.canRetryMetadata || v.metadataState === 'failed')) {
    primaryActionType = 'retry_metadata';
    primaryActionLabel = 'Retry metadata';
  } else if (hasAvailableRelease) {
    primaryActionType = 'review_release';
    primaryActionLabel = 'Review release';
  } else if (missingCount > 0) {
    primaryActionType = 'discover';
    primaryActionLabel = 'Discover releases';
  }

  return {
    title,
    ownedCount,
    totalExpectedVolumes: effectiveTotal,
    missingCount,
    activeCount,
    attentionCount,
    progressPercent,
    isOngoing,
    isCompleted: isCompletedResolved,
    libraryStatus,
    statusExplanation,
    isIdentityAmbiguous: false,
    primaryActionType,
    primaryActionLabel,
  };
}

/**
 * Analyzes import candidates to compute staging cleanup safety and summary.
 */
export function getStagingSummary(importBooks: ImportBook[]): StagingSummary {
  const allPayloads = Array.from(new Set(importBooks.map((b) => b.payloadChapter)));
  const pendingPayloads = new Set<number>();

  for (const b of importBooks) {
    if (b.state === 'needs_review' || b.state === 'processing') {
      pendingPayloads.add(b.payloadChapter);
    }
  }

  const completedPayloadChapters = allPayloads.filter((p) => !pendingPayloads.has(p));

  return {
    totalPayloads: allPayloads.length,
    completedPayloadChapters,
    pendingPayloadChapters: Array.from(pendingPayloads),
    canClean: completedPayloadChapters.length > 0,
  };
}

// ---------------------------------------------------------------------------
// M13: Unified Series Workspace & Acquisition Inbox Helpers
// ---------------------------------------------------------------------------

export type VolumeCollectionState =
  | 'owned'
  | 'missing'
  | 'available'
  | 'pending_review'
  | 'queued'
  | 'downloading'
  | 'validating'
  | 'staged'
  | 'publishing'
  | 'awaiting_verification'
  | 'failed'
  | 'unknown';

export interface VolumeCollectionStateInfo {
  state: VolumeCollectionState;
  label: string;
  badgeClass: string;
  isOwned: boolean;
  canInspect: boolean;
  canAcquire: boolean;
  canReview: boolean;
}

/**
 * Maps a volume record into one of the 12 explicit M13 collection states.
 * CRITICAL RULE: Komga-verified ownership remains authoritative; a volume is
 * NEVER marked 'owned' unless isOwned is explicitly true.
 */
export function resolveVolumeCollectionState(
  entry: VolumeManagerEntry,
  inReview = false
): VolumeCollectionStateInfo {
  // 1. Authoritative physical ownership in Komga
  if (entry.isOwned) {
    return {
      state: 'owned',
      label: 'Owned',
      badgeClass: 'state-owned',
      isOwned: true,
      canInspect: false,
      canAcquire: false,
      canReview: false,
    };
  }

  // 2. Failed state
  if (
    entry.activeStage === 'failed' ||
    entry.displayStatus === 'failed' ||
    entry.displayStatus === 'error'
  ) {
    return {
      state: 'failed',
      label: 'Failed',
      badgeClass: 'state-failed',
      isOwned: false,
      canInspect: true,
      canAcquire: true,
      canReview: false,
    };
  }

  // 3. Published / awaiting Komga verification
  if (
    entry.publicationReceiptId != null ||
    entry.activeStage === 'verifying' ||
    entry.displayStatus === 'published'
  ) {
    return {
      state: 'awaiting_verification',
      label: 'Published · Awaiting Verification',
      badgeClass: 'state-awaiting-verification',
      isOwned: false,
      canInspect: false,
      canAcquire: false,
      canReview: false,
    };
  }

  // 4. In active publication
  if (entry.activeStage === 'publishing') {
    return {
      state: 'publishing',
      label: 'Publishing',
      badgeClass: 'state-publishing',
      isOwned: false,
      canInspect: false,
      canAcquire: false,
      canReview: false,
    };
  }

  // 5. Staged for publication
  if (entry.activeStage === 'staged') {
    return {
      state: 'staged',
      label: 'Staged',
      badgeClass: 'state-staged',
      isOwned: false,
      canInspect: false,
      canAcquire: false,
      canReview: false,
    };
  }

  // 6. Validating archive
  if (entry.activeStage === 'validating') {
    return {
      state: 'validating',
      label: 'Validating Archive',
      badgeClass: 'state-validating',
      isOwned: false,
      canInspect: false,
      canAcquire: false,
      canReview: false,
    };
  }

  // 7. Actively downloading / retrieving
  if (
    entry.activeStage === 'downloading' ||
    entry.activeStage === 'retrieving' ||
    entry.activeStage === 'torbox_downloading' ||
    entry.displayStatus === 'downloading'
  ) {
    return {
      state: 'downloading',
      label: 'Downloading',
      badgeClass: 'state-downloading',
      isOwned: false,
      canInspect: false,
      canAcquire: false,
      canReview: false,
    };
  }

  // 8. Queued for download
  if (entry.activeStage === 'queued') {
    return {
      state: 'queued',
      label: 'Queued',
      badgeClass: 'state-queued',
      isOwned: false,
      canInspect: false,
      canAcquire: false,
      canReview: false,
    };
  }

  // 9. Pending review in acquisition inbox
  if (
    inReview ||
    entry.activeStage === 'pending_review' ||
    entry.activeStage === 'review'
  ) {
    return {
      state: 'pending_review',
      label: 'Pending Review',
      badgeClass: 'state-pending-review',
      isOwned: false,
      canInspect: true,
      canAcquire: false,
      canReview: true,
    };
  }

  // 10. Verified candidate available
  if (
    entry.candidateCount > 0 ||
    entry.displayStatus === 'candidate_available'
  ) {
    return {
      state: 'available',
      label: 'Available',
      badgeClass: 'state-available',
      isOwned: false,
      canInspect: true,
      canAcquire: true,
      canReview: false,
    };
  }

  // 11. Unknown / unannounced / uncertain
  if (
    entry.displayStatus === 'unreleased' ||
    entry.displayStatus === 'uncertain' ||
    entry.displayStatus === 'unknown' ||
    entry.opentomeStatus === 'unannounced' ||
    entry.opentomeStatus === 'unknown'
  ) {
    return {
      state: 'unknown',
      label: entry.displayStatus === 'unreleased' ? 'Unreleased' : 'Unknown',
      badgeClass: 'state-unknown',
      isOwned: false,
      canInspect: false,
      canAcquire: false,
      canReview: false,
    };
  }

  // 12. Missing from canonical library and no candidate currently discovered
  return {
    state: 'missing',
    label: 'Missing',
    badgeClass: 'state-missing',
    isOwned: false,
    canInspect: true,
    canAcquire: true,
    canReview: false,
  };
}

export interface NextMissingVolumeResult {
  nextMissingVolume: number | null;
  earlierGaps: number[];
  highestOwnedVolume: number;
  totalOwnedCount: number;
  totalExpectedCount: number | null;
  completionPercentage: number | null;
  isComplete: boolean;
  explanation: string;
}

/**
 * Calculates the series-level next missing volume sequence.
 * For example: if Volumes 1–5 are owned, Volume 6 is the next missing volume.
 * If Volumes 1, 3, 4 are owned, Volume 2 is identified as an interior gap.
 */
export function calculateNextMissingVolume(
  ownedVolumeNumbers: number[],
  totalExpectedVolumes: number | null = null,
  maxOfficialVolume: number | null = null
): NextMissingVolumeResult {
  const ownedSet = new Set(ownedVolumeNumbers.filter((v) => v > 0));
  const sortedOwned = Array.from(ownedSet).sort((a, b) => a - b);
  const highestOwned = sortedOwned.length > 0 ? sortedOwned[sortedOwned.length - 1] : 0;
  const totalOwnedCount = sortedOwned.length;

  const ceiling =
    totalExpectedVolumes != null && totalExpectedVolumes > 0
      ? totalExpectedVolumes
      : maxOfficialVolume != null && maxOfficialVolume > 0
      ? maxOfficialVolume
      : highestOwned > 0
      ? highestOwned + 1
      : 1;

  // Find interior gaps first
  const earlierGaps: number[] = [];
  for (let v = 1; v <= highestOwned; v++) {
    if (!ownedSet.has(v)) {
      earlierGaps.push(v);
    }
  }

  let nextMissing: number | null = null;
  if (earlierGaps.length > 0) {
    // An interior gap has top priority
    nextMissing = earlierGaps[0];
  } else if (totalExpectedVolumes != null && totalOwnedCount >= totalExpectedVolumes && highestOwned >= totalExpectedVolumes) {
    // Complete series!
    nextMissing = null;
  } else {
    // Next sequential volume after highest owned
    nextMissing = highestOwned + 1;
  }

  const isComplete =
    totalExpectedVolumes != null &&
    totalExpectedVolumes > 0 &&
    totalOwnedCount >= totalExpectedVolumes &&
    earlierGaps.length === 0;

  const completionPercentage =
    totalExpectedVolumes != null && totalExpectedVolumes > 0
      ? Math.min(100, Math.round((totalOwnedCount / totalExpectedVolumes) * 100))
      : null;

  let explanation = '';
  if (isComplete) {
    explanation = `All ${totalExpectedVolumes} official volumes verified and owned in Komga canonical library.`;
  } else if (earlierGaps.length > 0) {
    explanation = `Interior gap detected: Volume ${nextMissing} is missing prior to Volume ${highestOwned}.`;
  } else if (highestOwned > 0 && nextMissing != null) {
    explanation = `Volumes 1–${highestOwned} owned in Komga. Next sequential volume is Volume ${nextMissing}.`;
  } else if (nextMissing != null) {
    explanation = `No volumes owned yet. Volume 1 is the starting volume.`;
  } else {
    explanation = `Collection complete.`;
  }

  return {
    nextMissingVolume: nextMissing,
    earlierGaps,
    highestOwnedVolume: highestOwned,
    totalOwnedCount,
    totalExpectedCount: totalExpectedVolumes,
    completionPercentage,
    isComplete,
    explanation,
  };
}

export interface SeriesGroupedReviewQueue {
  seriesId: string;
  seriesTitle: string;
  mangaId: number | null;
  ownedCount: number;
  totalOfficialCount: number | null;
  nextMissingVolume: number | null;
  primaryProposals: ReviewQueueItem[];
  outOfOrderProposals: ReviewQueueItem[];
  otherProposals: ReviewQueueItem[];
  pendingCount: number;
  outOfOrderCount: number;
  warnings: string[];
}

/**
 * Transforms a flat review queue item list into a compact, series-grouped acquisition inbox.
 */
export function groupReviewQueueBySeries(
  items: ReviewQueueItem[],
  ownedVolumesBySeries: Record<string, number[]> = {},
  totalsBySeries: Record<string, number | null> = {}
): SeriesGroupedReviewQueue[] {
  const groupsMap = new Map<string, ReviewQueueItem[]>();

  for (const item of items) {
    const key = item.seriesId || item.seriesTitle || (item.mangaId ? `manga:${item.mangaId}` : 'unknown');
    if (!groupsMap.has(key)) {
      groupsMap.set(key, []);
    }
    groupsMap.get(key)!.push(item);
  }

  const result: SeriesGroupedReviewQueue[] = [];

  for (const [key, groupItems] of groupsMap.entries()) {
    const sample = groupItems[0];
    const seriesTitle = sample.seriesTitle || key;
    const mangaId = sample.mangaId;

    const ownedList = ownedVolumesBySeries[key] ?? ownedVolumesBySeries[seriesTitle] ?? [];
    const totalExpected = totalsBySeries[key] ?? totalsBySeries[seriesTitle] ?? null;

    const missingSeq = calculateNextMissingVolume(ownedList, totalExpected);

    const pendingItems = groupItems.filter((i) => i.status === 'pending');
    const otherProposals = groupItems.filter((i) => i.status !== 'pending');

    // Sort pending items by acquisition priority ascending, then volume number ascending
    pendingItems.sort((a, b) => {
      const pa = a.acquisitionPriority ?? 999;
      const pb = b.acquisitionPriority ?? 999;
      if (pa !== pb) return pa - pb;
      return a.volumeNumber - b.volumeNumber;
    });

    const primaryProposals: ReviewQueueItem[] = [];
    const outOfOrderProposals: ReviewQueueItem[] = [];
    const warnings: string[] = [];

    for (const item of pendingItems) {
      const hasGaps = item.earlierMissingGaps && item.earlierMissingGaps.length > 0;
      const isPriority1 = (item.acquisitionPriority ?? 1) === 1;
      const matchesNextMissing = missingSeq.nextMissingVolume != null && item.volumeNumber === missingSeq.nextMissingVolume;

      if (matchesNextMissing || (isPriority1 && !hasGaps)) {
        primaryProposals.push(item);
      } else {
        outOfOrderProposals.push(item);
      }

      if (hasGaps && !warnings.some((w) => w.includes(`Vol ${item.volumeNumber}`))) {
        warnings.push(`Volume ${item.volumeNumber} is out-of-order: earlier missing volumes (${item.earlierMissingGaps!.join(', ')})`);
      }
    }

    result.push({
      seriesId: key,
      seriesTitle,
      mangaId,
      ownedCount: missingSeq.totalOwnedCount,
      totalOfficialCount: totalExpected,
      nextMissingVolume: missingSeq.nextMissingVolume,
      primaryProposals,
      outOfOrderProposals,
      otherProposals,
      pendingCount: pendingItems.length,
      outOfOrderCount: outOfOrderProposals.length,
      warnings,
    });
  }

  // Sort groups: groups with pending actionable items first, then alphabetically
  result.sort((a, b) => {
    if (a.pendingCount > 0 && b.pendingCount === 0) return -1;
    if (b.pendingCount > 0 && a.pendingCount === 0) return 1;
    return a.seriesTitle.localeCompare(b.seriesTitle);
  });

  return result;
}

export type LifecycleStageName =
  | 'discovery'
  | 'review'
  | 'queued'
  | 'downloading'
  | 'validation'
  | 'staging'
  | 'publication'
  | 'verification';

export interface LifecycleStage {
  id: LifecycleStageName;
  label: string;
  status: 'completed' | 'current' | 'pending' | 'failed' | 'skipped';
  timestamp?: number | null;
  detail?: string | null;
}

/**
 * Derives the acquisition lifecycle timeline progression from persisted events.
 * Timeline: Discovery → Review → Queued → Downloading → Validation → Staging → Publication → Komga Verification
 */
export function deriveLifecycleTimeline(input: {
  discoveredAt?: number | null;
  reviewedAt?: number | null;
  reviewStatus?: string | null;
  queuedAt?: number | null;
  downloadStartedAt?: number | null;
  downloadCompletedAt?: number | null;
  validatedAt?: number | null;
  stagedAt?: number | null;
  publishedAt?: number | null;
  verifiedAt?: number | null;
  currentStage?: string | null;
  error?: string | null;
  provider?: string | null;
}): LifecycleStage[] {
  const {
    discoveredAt,
    reviewedAt,
    reviewStatus,
    queuedAt,
    downloadStartedAt,
    downloadCompletedAt,
    validatedAt,
    stagedAt,
    publishedAt,
    verifiedAt,
    currentStage = null,
    error = null,
    provider = null,
  } = input;

  const stages: LifecycleStage[] = [
    {
      id: 'discovery',
      label: 'Discovery',
      status: discoveredAt ? 'completed' : 'pending',
      timestamp: discoveredAt,
      detail: provider ? `Discovered via ${provider}` : 'Candidate identified',
    },
    {
      id: 'review',
      label: 'Review',
      status:
        reviewStatus === 'rejected' || reviewStatus === 'skipped'
          ? 'skipped'
          : reviewedAt || reviewStatus === 'approved'
          ? 'completed'
          : currentStage === 'review' || currentStage === 'pending_review'
          ? 'current'
          : 'pending',
      timestamp: reviewedAt,
      detail: reviewStatus ? `Status: ${reviewStatus}` : 'Review decision',
    },
    {
      id: 'queued',
      label: 'Queued',
      status:
        queuedAt || downloadStartedAt
          ? 'completed'
          : currentStage === 'queued'
          ? 'current'
          : 'pending',
      timestamp: queuedAt,
      detail: 'Added to download queue',
    },
    {
      id: 'downloading',
      label: 'Downloading',
      status:
        downloadCompletedAt || validatedAt
          ? 'completed'
          : currentStage === 'downloading' || currentStage === 'torbox_downloading'
          ? error
            ? 'failed'
            : 'current'
          : 'pending',
      timestamp: downloadStartedAt,
      detail: error && currentStage?.includes('download') ? error : 'Transfer in progress',
    },
    {
      id: 'validation',
      label: 'Validation',
      status:
        validatedAt || stagedAt
          ? 'completed'
          : currentStage === 'validating'
          ? error
            ? 'failed'
            : 'current'
          : 'pending',
      timestamp: validatedAt,
      detail: 'CBZ structure & image validation',
    },
    {
      id: 'staging',
      label: 'Staging',
      status:
        stagedAt || publishedAt
          ? 'completed'
          : currentStage === 'staged'
          ? 'current'
          : 'pending',
      timestamp: stagedAt,
      detail: 'Prepared in safe staging area',
    },
    {
      id: 'publication',
      label: 'Publication',
      status:
        publishedAt || verifiedAt
          ? 'completed'
          : currentStage === 'publishing'
          ? error
            ? 'failed'
            : 'current'
          : 'pending',
      timestamp: publishedAt,
      detail: 'Moved to canonical library folder',
    },
    {
      id: 'verification',
      label: 'Komga Verification',
      status:
        verifiedAt
          ? 'completed'
          : currentStage === 'verifying'
          ? 'current'
          : 'pending',
      timestamp: verifiedAt,
      detail: verifiedAt ? 'Verified in Komga library' : 'Awaiting Komga library scan',
    },
  ];

  return stages;
}
