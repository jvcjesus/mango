import type { ImportBook } from './importReview';
import type {
  PipelineEvent,
  Queued,
  VolumeOwnership,
  CanonicalLibraryReleasesSummary,
  ReconciliationSummary,
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
